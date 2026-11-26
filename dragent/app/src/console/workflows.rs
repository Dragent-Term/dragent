//! Saved command workflows with `{{argument}}` placeholders.

use std::collections::HashMap;
use std::fs;
use std::ops::Range;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Deserialize a string field that some specs write as YAML null (`~`) — treat
/// null (or missing) as an empty string instead of a parse error.
fn de_nullable_string<'de, D>(d: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(Option::<String>::deserialize(d)?.unwrap_or_default())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowArg {
    pub name: String,
    #[serde(default)]
    pub description: String,
    // `default_value` is commands.dev / Warp Workflows' field name; accept both
    // so their specs load unchanged. Their `default_value: ~` (YAML null) is
    // coerced to an empty string rather than failing to parse.
    #[serde(
        default,
        alias = "default_value",
        deserialize_with = "de_nullable_string"
    )]
    pub default: String,
    #[serde(default)]
    pub values: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Workflow {
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub command: String,
    #[serde(default)]
    pub arguments: Vec<WorkflowArg>,
}

#[derive(Debug, Clone)]
pub struct WorkflowMatch {
    pub workflow: Workflow,
    pub score: f32,
}

#[derive(Debug, Clone)]
pub struct WorkflowSession {
    pub workflow: Workflow,
    pub rendered: String,
    pub arg_ranges: Vec<(String, Range<usize>)>,
    pub active_arg: usize,
}

impl Workflow {
    pub fn render(&self, values: &HashMap<String, String>) -> String {
        let mut out = self.command.clone();
        for arg in &self.arguments {
            let placeholder = format!("{{{{{}}}}}", arg.name);
            let value = values
                .get(&arg.name)
                .cloned()
                .filter(|v| !v.is_empty())
                .unwrap_or_else(|| arg.default.clone());
            out = out.replace(&placeholder, &value);
        }
        out
    }

    pub fn placeholder_values(&self) -> HashMap<String, String> {
        self.arguments
            .iter()
            .map(|a| (a.name.clone(), a.default.clone()))
            .collect()
    }
}

impl WorkflowSession {
    pub fn new(workflow: Workflow) -> Self {
        let values = workflow.placeholder_values();
        let rendered = workflow.render(&values);
        let arg_ranges = argument_ranges(&rendered, &workflow.arguments);
        Self {
            workflow,
            rendered,
            arg_ranges,
            active_arg: 0,
        }
    }

    pub fn active_range(&self) -> Option<Range<usize>> {
        self.arg_ranges.get(self.active_arg).map(|(_, r)| r.clone())
    }

    pub fn cycle_prev(&mut self) {
        if self.arg_ranges.is_empty() {
            return;
        }
        if self.active_arg == 0 {
            self.active_arg = self.arg_ranges.len() - 1;
        } else {
            self.active_arg -= 1;
        }
    }

    pub fn cycle_next(&mut self) {
        if self.arg_ranges.is_empty() {
            return;
        }
        self.active_arg = (self.active_arg + 1) % self.arg_ranges.len();
    }

    pub fn finalize(&self) -> String {
        self.rendered.clone()
    }
}

pub fn argument_ranges(text: &str, args: &[WorkflowArg]) -> Vec<(String, Range<usize>)> {
    let mut ranges = Vec::new();
    for arg in args {
        let needle = if arg.default.is_empty() {
            format!("{{{{{}}}}}", arg.name)
        } else {
            arg.default.clone()
        };
        if let Some(start) = text.find(&needle) {
            ranges.push((arg.name.clone(), start..start + needle.len()));
        }
    }
    ranges.sort_by_key(|(_, r)| r.start);
    ranges
}

#[derive(Debug, Default)]
pub struct WorkflowStore {
    workflows: Vec<Workflow>,
}

/// The commands.dev / warpdotdev Workflows catalog (Apache-2.0), embedded so it
/// ships built-in. See `app/assets/workflows/{LICENSE,ATTRIBUTION.txt}`.
#[derive(rust_embed::RustEmbed)]
#[folder = "assets/workflows"]
#[include = "**/*.yaml"]
struct BundledWorkflows;

impl WorkflowStore {
    /// User-editable workflow directory shown by Settings → Agent → Commands.
    /// Keeping this in one place makes the discovery path and the settings UI
    /// agree on where custom commands live.
    pub fn user_workflow_dir() -> PathBuf {
        dragent_paths::home().join("workflows")
    }

    pub fn load() -> Self {
        // 1. Built-in catalog (commands.dev). User dirs below can add/override.
        let mut workflows = bundled_workflows().to_vec();
        // 2. User workflows: ~/.dragent/workflows.
        for dir in workflow_dirs() {
            load_dir(&dir, &mut workflows);
        }
        if workflows.is_empty() {
            workflows.extend(builtin_workflows());
        }
        Self { workflows }
    }

    /// Parse the bundled catalog ahead of the first terminal session.
    pub fn warm() {
        bundled_workflows();
    }

    pub fn all(&self) -> &[Workflow] {
        &self.workflows
    }

    pub fn search(&self, query: &str) -> Vec<WorkflowMatch> {
        let q = query.trim().to_lowercase();
        // Dev-only command-palette entry for the perf overlay (the workflow
        // palette is labelled "Workflow / command palette"). Only matches
        // perf/overlay/fps queries so release behaviour is untouched, and the
        // accept path below toggles the overlay instead of running a command.
        let mut dev_extra: Vec<WorkflowMatch> = Vec::new();
        #[cfg(not(target_family = "wasm"))]
        if (dragent_paths::is_dev() || cfg!(debug_assertions))
            && !q.is_empty()
            && (q.contains("perf") || q.contains("overlay") || q.contains("fps"))
        {
            dev_extra.push(WorkflowMatch {
                workflow: Workflow {
                    name: "Toggle perf overlay (dev)".to_string(),
                    description: "Show/hide the frame-time + memory + input-latency overlay"
                        .to_string(),
                    command: "__dragent_toggle_perf_overlay__".to_string(),
                    arguments: vec![],
                },
                score: 200.0,
            });
        }
        if self.workflows.is_empty() {
            return dev_extra;
        }
        let mut matches: Vec<WorkflowMatch> = self
            .workflows
            .iter()
            .filter_map(|wf| {
                if q.is_empty() {
                    return Some(WorkflowMatch {
                        workflow: wf.clone(),
                        score: wf.name.len() as f32,
                    });
                }
                let name = wf.name.to_lowercase();
                let desc = wf.description.to_lowercase();
                let cmd = wf.command.to_lowercase();
                let mut score = 0.0;
                if name.starts_with(&q) {
                    score += 100.0;
                } else if name.contains(&q) {
                    score += 50.0;
                } else if desc.contains(&q) || cmd.contains(&q) {
                    score += 20.0;
                } else {
                    return None;
                }
                Some(WorkflowMatch {
                    workflow: wf.clone(),
                    score,
                })
            })
            .collect();
        matches.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        // Dev palette entry sorts first (score 200 beats any prefix match).
        matches.splice(0..0, dev_extra);
        matches.truncate(12);
        matches
    }
}

fn workflow_dirs() -> Vec<PathBuf> {
    vec![WorkflowStore::user_workflow_dir()]
}

/// The embedded catalog, parsed once per process: it never changes, and every
/// terminal session used to re-parse all of its YAML on the UI thread.
fn bundled_workflows() -> &'static [Workflow] {
    static BUNDLED: std::sync::OnceLock<Vec<Workflow>> = std::sync::OnceLock::new();
    BUNDLED.get_or_init(|| {
        let mut workflows = Vec::new();
        for path in BundledWorkflows::iter() {
            if let Some(file) = BundledWorkflows::get(&path) {
                if let Ok(raw) = std::str::from_utf8(&file.data) {
                    parse_yaml_file(raw, &mut workflows);
                }
            }
        }
        workflows
    })
}

fn load_dir(dir: &Path, out: &mut Vec<Workflow>) {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("yaml") {
            continue;
        }
        if let Ok(raw) = fs::read_to_string(&path) {
            parse_yaml_file(&raw, out);
        }
    }
}

fn parse_yaml_file(raw: &str, out: &mut Vec<Workflow>) {
    if let Ok(list) = serde_yaml::from_str::<Vec<Workflow>>(raw) {
        out.extend(list);
        return;
    }
    if let Ok(one) = serde_yaml::from_str::<Workflow>(raw) {
        out.push(one);
    }
}

fn builtin_workflows() -> Vec<Workflow> {
    vec![
        Workflow {
            name: "Git status".into(),
            description: "Show working tree status".into(),
            command: "git status".into(),
            arguments: vec![],
        },
        Workflow {
            name: "Cargo build release".into(),
            description: "Build optimized binary".into(),
            command: "cargo build --release".into(),
            arguments: vec![],
        },
        Workflow {
            name: "Deploy".into(),
            description: "Build and deploy to an environment".into(),
            command: "cargo build --release && ./scripts/deploy.sh {{environment}} {{version}}"
                .into(),
            arguments: vec![
                WorkflowArg {
                    name: "environment".into(),
                    description: "Target environment".into(),
                    default: "staging".into(),
                    values: vec!["staging".into(), "production".into()],
                },
                WorkflowArg {
                    name: "version".into(),
                    description: "Release tag".into(),
                    default: "latest".into(),
                    values: vec![],
                },
            ],
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_commands_dev_catalog_parses() {
        let mut parsed = 0usize;
        for path in BundledWorkflows::iter() {
            let file = BundledWorkflows::get(&path).expect("embedded file");
            let raw = std::str::from_utf8(&file.data).expect("utf8");
            let before = parsed;
            let mut out = Vec::new();
            parse_yaml_file(raw, &mut out);
            parsed += out.len();
            assert!(parsed > before, "spec did not parse: {path}");
        }
        // The full catalog is a couple hundred specs; assert it's substantial.
        assert!(
            parsed > 200,
            "expected 200+ bundled workflows, got {parsed}"
        );
    }

    #[test]
    fn renders_placeholders() {
        let wf = Workflow {
            name: "Deploy".into(),
            description: String::new(),
            command: "deploy {{environment}} {{version}}".into(),
            arguments: vec![
                WorkflowArg {
                    name: "environment".into(),
                    description: String::new(),
                    default: "staging".into(),
                    values: vec![],
                },
                WorkflowArg {
                    name: "version".into(),
                    description: String::new(),
                    default: "v1".into(),
                    values: vec![],
                },
            ],
        };
        let rendered = wf.render(&wf.placeholder_values());
        assert_eq!(rendered, "deploy staging v1");
    }

    #[test]
    fn finds_argument_ranges() {
        let wf = Workflow {
            name: "Deploy".into(),
            description: String::new(),
            command: "deploy staging latest".into(),
            arguments: vec![
                WorkflowArg {
                    name: "environment".into(),
                    description: String::new(),
                    default: "staging".into(),
                    values: vec![],
                },
                WorkflowArg {
                    name: "version".into(),
                    description: String::new(),
                    default: "latest".into(),
                    values: vec![],
                },
            ],
        };
        let session = WorkflowSession::new(wf);
        assert_eq!(session.arg_ranges.len(), 2);
    }
}
