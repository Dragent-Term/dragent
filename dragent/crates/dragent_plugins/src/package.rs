//! Reading a package and copying it into Dragent's folder. Neither runs
//! anything from the package.

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::manifest::{resolve_command, Manifest, Transport};

/// Most files and bytes a package may hold.
const MAX_FILES: usize = 2_000;
const MAX_BYTES: u64 = 50 * 1024 * 1024;

/// What installing a package would bring, read without running any of it.
#[derive(Clone, Debug, Serialize)]
pub struct Inspection {
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: String,
    pub publisher: Option<String>,
    pub files: usize,
    pub bytes: u64,
    pub skills: Vec<SkillInfo>,
    pub servers: Vec<ServerInfo>,
    /// Programs the servers need, and whether each is on PATH here.
    pub requires: Vec<(String, bool)>,
}

#[derive(Clone, Debug, Serialize)]
pub struct SkillInfo {
    pub name: String,
    pub description: String,
    pub files: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ServerInfo {
    pub name: String,
    pub transport: &'static str,
    /// Exactly what will run (stdio) or be reached (http) once the person
    /// turns it on.
    pub runs: String,
    /// The program comes with the package, rather than from PATH.
    pub runs_packaged_program: bool,
    /// The systems it has a program for (all three when it names one).
    pub systems: Vec<&'static str>,
    /// It runs on this system; one that doesn't isn't added here.
    pub runs_here: bool,
    pub settings: Vec<String>,
    pub secrets: Vec<String>,
}

/// Every file in the package, relative, refusing links (a link could
/// point anywhere on the machine) and oversized packages.
pub fn files(dir: &Path) -> Result<(Vec<String>, u64), String> {
    let mut found = Vec::new();
    let mut bytes = 0u64;
    let mut stack = vec![PathBuf::new()];
    while let Some(relative) = stack.pop() {
        let entries = std::fs::read_dir(dir.join(&relative)).map_err(|error| error.to_string())?;
        for entry in entries {
            let entry = entry.map_err(|error| error.to_string())?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if relative.as_os_str().is_empty() && name == ".git" {
                continue;
            }
            let path = relative.join(name.as_ref());
            let kind = entry.file_type().map_err(|error| error.to_string())?;
            if kind.is_symlink() {
                return Err(format!("{} is a link: packages can't contain links.", path.display()));
            } else if kind.is_dir() {
                stack.push(path);
            } else if kind.is_file() {
                bytes += entry.metadata().map(|meta| meta.len()).unwrap_or(0);
                found.push(path.to_string_lossy().replace('\\', "/"));
                if found.len() > MAX_FILES || bytes > MAX_BYTES {
                    return Err("The package is larger than 2,000 files or 50 MB.".into());
                }
            }
        }
    }
    found.sort();
    Ok((found, bytes))
}

/// A skill's description, from its SKILL.md frontmatter.
fn description(skill_md: &Path) -> String {
    let text = std::fs::read_to_string(skill_md).unwrap_or_default();
    text.strip_prefix("---")
        .and_then(|rest| rest.split("\n---").next())
        .and_then(|front| {
            front
                .lines()
                .find_map(|line| line.trim().strip_prefix("description:"))
        })
        .map(|value| value.trim().trim_matches('"').to_string())
        .unwrap_or_default()
}

fn on_path(program: &str) -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    // Windows runs `npx` as npx.cmd: try the endings it would.
    let endings: Vec<String> = if cfg!(windows) {
        std::env::var("PATHEXT")
            .unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into())
            .split(';')
            .filter(|ending| !ending.is_empty())
            .map(str::to_lowercase)
            .collect()
    } else {
        Vec::new()
    };
    std::env::split_paths(&path).any(|dir| {
        dir.join(program).is_file()
            || endings
                .iter()
                .any(|ending| dir.join(format!("{program}{ending}")).is_file())
    })
}

/// What the package at `dir` brings; refuses a package that isn't sound.
pub fn inspect(dir: &Path) -> Result<Inspection, String> {
    let manifest = Manifest::read(dir)?;
    let (files, bytes) = files(dir)?;
    let mut skills = Vec::new();
    for skill in &manifest.skills {
        let skill_md = format!("{}/SKILL.md", skill.trim_end_matches('/'));
        if !files.contains(&skill_md.trim_start_matches("./").to_string()) {
            return Err(format!("skill “{skill}” has no SKILL.md."));
        }
        let prefix = format!("{}/", skill.trim_start_matches("./").trim_end_matches('/'));
        skills.push(SkillInfo {
            name: Path::new(skill)
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default(),
            description: description(&dir.join(&skill_md)),
            files: files
                .iter()
                .filter_map(|file| file.strip_prefix(&prefix).map(str::to_string))
                .collect(),
        });
    }
    let servers = manifest
        .servers
        .iter()
        .map(|(name, server)| {
            for command in server.programs() {
                if command.starts_with("./") {
                    let local = resolve_command(command, Path::new(""));
                    if !files.contains(&local) {
                        return Err(format!("server “{name}” runs {local}, which isn't in the package."));
                    }
                }
            }
            let here = std::env::consts::OS;
            let packaged = server
                .program_on(here)
                .is_some_and(|(command, _)| command.starts_with("./"));
            Ok(ServerInfo {
                name: name.clone(),
                transport: match server.transport {
                    Transport::Stdio => "stdio",
                    Transport::Http => "http",
                },
                runs: server.display(Path::new("<plugin folder>"), here),
                runs_packaged_program: packaged,
                systems: server.systems(),
                runs_here: server.transport == Transport::Http || server.program_on(here).is_some(),
                settings: server
                    .env
                    .iter()
                    .filter(|(_, spec)| !spec.secret)
                    .map(|(key, _)| key.clone())
                    .collect(),
                secrets: server
                    .env
                    .iter()
                    .filter(|(_, spec)| spec.secret)
                    .map(|(key, _)| key.clone())
                    .collect(),
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(Inspection {
        id: manifest.id.clone(),
        name: manifest.name.clone(),
        version: manifest.version.clone(),
        description: manifest.description.clone(),
        publisher: manifest.publisher.clone(),
        files: files.len(),
        bytes,
        skills,
        servers,
        requires: manifest
            .requires
            .commands
            .iter()
            .map(|program| (program.clone(), on_path(program)))
            .collect(),
    })
}

/// Copy the package at `source` to `<store>/packages/<id>/<version>/`
/// (replacing a copy of the same version), whole or not at all.
pub fn copy_into(store: &Path, source: &Path, manifest: &Manifest) -> Result<PathBuf, String> {
    let (files, _) = files(source)?;
    let parent = store.join("packages").join(&manifest.id);
    std::fs::create_dir_all(&parent).map_err(|error| error.to_string())?;
    let target = parent.join(&manifest.version);
    let staging = parent.join(format!(".{}-incoming", manifest.version));
    let _ = std::fs::remove_dir_all(&staging);
    for file in &files {
        let to = staging.join(file);
        if let Some(folder) = to.parent() {
            std::fs::create_dir_all(folder).map_err(|error| error.to_string())?;
        }
        std::fs::copy(source.join(file), &to).map_err(|error| format!("{file}: {error}"))?;
    }
    let _ = std::fs::remove_dir_all(&target);
    std::fs::rename(&staging, &target).map_err(|error| error.to_string())?;
    Ok(target)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A package with one skill and one stdio server.
    pub(crate) fn sample(dir: &Path, version: &str, skill_text: &str) {
        std::fs::create_dir_all(dir.join("skills/acme-tickets/references")).unwrap();
        std::fs::create_dir_all(dir.join("bin")).unwrap();
        std::fs::write(
            dir.join("skills/acme-tickets/SKILL.md"),
            format!("---\nname: acme-tickets\ndescription: \"Work with Acme tickets.\"\n---\n{skill_text}\n"),
        )
        .unwrap();
        std::fs::write(dir.join("skills/acme-tickets/references/fields.md"), "Fields\n").unwrap();
        std::fs::write(dir.join("bin/server"), "#!/bin/sh\n").unwrap();
        std::fs::write(
            dir.join(crate::MANIFEST_FILE),
            serde_json::json!({
                "schema": 1, "id": "acme-tickets", "name": "Acme Tickets", "version": version,
                "skills": ["skills/acme-tickets"],
                "servers": {"tickets": {"transport": "stdio", "command": "./bin/server", "args": ["--stdio"],
                    "env": {"ACME_TOKEN": {"secret": true}, "ACME_SITE": {"default": "acme.example"}}}}
            })
            .to_string(),
        )
        .unwrap();
    }

    #[test]
    fn inspecting_reads_without_running_and_names_everything() {
        let dir = tempfile::tempdir().unwrap();
        sample(dir.path(), "1.0.0", "Use the tickets tools.");
        let seen = inspect(dir.path()).unwrap();
        assert_eq!((seen.id.as_str(), seen.files), ("acme-tickets", 4));
        assert_eq!(seen.skills[0].description, "Work with Acme tickets.");
        assert_eq!(seen.skills[0].files, vec!["SKILL.md", "references/fields.md"]);
        let server = &seen.servers[0];
        assert_eq!(server.runs, "<plugin folder>/bin/server --stdio");
        assert!(server.runs_packaged_program);
        assert_eq!((server.secrets.clone(), server.settings.clone()), (vec!["ACME_TOKEN".to_string()], vec!["ACME_SITE".to_string()]));
    }

    #[test]
    fn links_and_missing_pieces_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        sample(dir.path(), "1.0.0", "x");
        std::fs::remove_file(dir.path().join("bin/server")).unwrap();
        assert!(inspect(dir.path()).unwrap_err().contains("isn't in the package"));
        sample(dir.path(), "1.0.0", "x");
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("/etc/passwd", dir.path().join("skills/acme-tickets/secrets")).unwrap();
            assert!(inspect(dir.path()).unwrap_err().contains("is a link"));
        }
    }

    #[test]
    fn a_copy_is_whole_and_versioned() {
        let source = tempfile::tempdir().unwrap();
        let store = tempfile::tempdir().unwrap();
        sample(source.path(), "1.0.0", "x");
        let manifest = Manifest::read(source.path()).unwrap();
        let copied = copy_into(store.path(), source.path(), &manifest).unwrap();
        assert!(copied.ends_with("packages/acme-tickets/1.0.0"));
        assert!(copied.join("skills/acme-tickets/references/fields.md").is_file());
        assert!(!store.path().join("packages/acme-tickets/.1.0.0-incoming").exists());
    }
}
