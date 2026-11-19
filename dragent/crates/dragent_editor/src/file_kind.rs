use std::path::Path;

/// File/icon classification shared by project trees and future editor tabs.
/// This is intentionally broader than [`super::Language`]: a file can have a
/// precise ecosystem icon even when we do not yet ship a full syntax grammar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FileKind(&'static str);

impl FileKind {
    pub const fn icon_asset(self) -> &'static str {
        self.0
    }

    pub fn from_path(path: &Path) -> Self {
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        let ext = path
            .extension()
            .and_then(|ext| ext.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();

        let asset = match name.as_str() {
            "dockerfile" | ".dockerignore" => "icons/docker.svg",
            "makefile" | "gnumakefile" => "icons/makefile.svg",
            "jenkinsfile" => "icons/jenkins.svg",
            "azure-pipelines.yml" | "azure-pipelines.yaml" => "icons/azure-pipelines.svg",
            "github-actions.yml" | "github-actions.yaml" => "icons/github-actions-workflow.svg",
            "cargo.toml" | "cargo.lock" => "icons/rust.svg",
            "package.json" | "package-lock.json" | "npm-shrinkwrap.json" => "icons/json.svg",
            ".gitignore" | ".gitattributes" => "icons/gitlab.svg",
            _ => match ext.as_str() {
                "rs" => "icons/rust.svg",
                "tsx" => "icons/react_ts.svg",
                "jsx" => "icons/react.svg",
                "ts" => "icons/typescript.svg",
                "js" | "mjs" | "cjs" => "icons/javascript.svg",
                "json" | "jsonc" | "json5" => "icons/json.svg",
                "py" | "pyi" => "icons/python.svg",
                "go" => "icons/go.svg",
                "mod" => "icons/go-mod.svg",
                "html" | "htm" => "icons/html.svg",
                "css" | "scss" | "sass" | "less" => "icons/css.svg",
                "md" | "mdx" => "icons/markdown.svg",
                "sh" | "bash" | "zsh" | "fish" => "icons/shellcheck.svg",
                "yaml" | "yml" => "icons/yaml.svg",
                "toml" => "icons/toml.svg",
                "c" | "h" => "icons/c.svg",
                "cc" | "cpp" | "cxx" | "hpp" => "icons/cpp.svg",
                "cs" => "icons/csharp.svg",
                "java" => "icons/java.svg",
                "kt" | "kts" => "icons/kotlin.svg",
                "swift" => "icons/swift.svg",
                "dart" => "icons/dart.svg",
                "rb" | "rake" => "icons/ruby.svg",
                "php" => "icons/php.svg",
                "lua" => "icons/lua.svg",
                "r" => "icons/r.svg",
                "scala" | "sc" => "icons/scala.svg",
                "ex" | "exs" => "icons/elixir.svg",
                "hs" | "lhs" => "icons/haskell.svg",
                "clj" | "cljs" => "icons/clojure.svg",
                "zig" => "icons/zig.svg",
                "nim" => "icons/nim.svg",
                "sol" => "icons/solidity.svg",
                "graphql" | "gql" => "icons/graphql.svg",
                "sql" => "icons/database.svg",
                "proto" => "icons/proto.svg",
                "xml" | "xsl" | "xslt" => "icons/xml.svg",
                "tf" | "tfvars" => "icons/terraform.svg",
                "hcl" => "icons/hcl.svg",
                "vue" => "icons/vue.svg",
                "svelte" => "icons/svelte.svg",
                "astro" => "icons/astro.svg",
                "wasm" => "icons/webassembly.svg",
                "ps1" | "psm1" => "icons/powershell.svg",
                "gradle" => "icons/gradle.svg",
                "groovy" => "icons/groovy.svg",
                "cmake" => "icons/cmake.svg",
                "helm" => "icons/helm.svg",
                "nginx" => "icons/nginx.svg",
                "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "ico" | "avif" | "tiff"
                | "tif" | "svg" | "heic" | "heif" => "icons/image.svg",
                _ => "icons/file.svg",
            },
        };
        Self(asset)
    }
}

#[cfg(test)]
mod tests {
    use super::FileKind;
    use std::path::Path;

    #[test]
    fn recognizes_cloud_native_and_language_files() {
        assert_eq!(
            FileKind::from_path(Path::new("main.tf")).icon_asset(),
            "icons/terraform.svg"
        );
        assert_eq!(
            FileKind::from_path(Path::new("Dockerfile")).icon_asset(),
            "icons/docker.svg"
        );
        assert_eq!(
            FileKind::from_path(Path::new("resolver.go")).icon_asset(),
            "icons/go.svg"
        );
        assert_eq!(
            FileKind::from_path(Path::new("schema.graphql")).icon_asset(),
            "icons/graphql.svg"
        );
        assert_eq!(
            FileKind::from_path(Path::new("components/App.tsx")).icon_asset(),
            "icons/react_ts.svg"
        );
        assert_eq!(
            FileKind::from_path(Path::new("components/App.jsx")).icon_asset(),
            "icons/react.svg"
        );
        assert_eq!(
            FileKind::from_path(Path::new("client.ts")).icon_asset(),
            "icons/typescript.svg"
        );
        assert_eq!(
            FileKind::from_path(Path::new("client.js")).icon_asset(),
            "icons/javascript.svg"
        );
    }

    #[test]
    fn image_files_get_the_image_icon_not_the_generic_file_icon() {
        for ext in ["png", "jpg", "jpeg", "gif", "webp", "svg", "ico", "avif"] {
            assert_eq!(
                FileKind::from_path(Path::new(&format!("asset.{ext}"))).icon_asset(),
                "icons/image.svg",
                "extension {ext} should resolve to the image icon"
            );
        }
    }
}
