//! Language-aware symbol extraction using regex patterns.
//! Extracts function, class, struct, enum, trait, interface, and type definitions
//! from source files without requiring an external parser.

use std::path::Path;

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum SymbolKind {
    Function,
    Method,
    Class,
    Struct,
    Enum,
    Trait,
    Interface,
    Type,
    Constant,
    Variable,
    Module,
}

impl SymbolKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Function => "fn",
            Self::Method => "method",
            Self::Class => "class",
            Self::Struct => "struct",
            Self::Enum => "enum",
            Self::Trait => "trait",
            Self::Interface => "interface",
            Self::Type => "type",
            Self::Constant => "const",
            Self::Variable => "var",
            Self::Module => "mod",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Symbol {
    pub name: String,
    pub kind: SymbolKind,
    /// 0-based line number.
    pub line: usize,
    /// Byte offset of the line start.
    pub byte_offset: usize,
}

/// Extract symbols from a source file's text content.
pub fn extract_symbols(path: &Path, content: &str) -> Vec<Symbol> {
    let lang = detect_language(path);
    match lang {
        Language::Rust => extract_rust(content),
        Language::Go => extract_go(content),
        Language::TypeScript | Language::JavaScript => extract_js_ts(content),
        Language::Python => extract_python(content),
        Language::Java | Language::Kotlin => extract_java_kotlin(content),
        Language::C | Language::Cpp => extract_c_cpp(content),
        Language::Swift => extract_swift(content),
        Language::Ruby => extract_ruby(content),
        Language::Unknown => vec![],
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Language {
    Rust,
    Go,
    TypeScript,
    JavaScript,
    Python,
    Java,
    Kotlin,
    C,
    Cpp,
    Swift,
    Ruby,
    Unknown,
}

fn detect_language(path: &Path) -> Language {
    match path.extension().and_then(|e| e.to_str()) {
        Some("rs") => Language::Rust,
        Some("go") => Language::Go,
        Some("ts") | Some("tsx") => Language::TypeScript,
        Some("js") | Some("jsx") | Some("mjs") | Some("cjs") => Language::JavaScript,
        Some("py") => Language::Python,
        Some("java") => Language::Java,
        Some("kt") | Some("kts") => Language::Kotlin,
        Some("c") | Some("h") => Language::C,
        Some("cpp") | Some("cc") | Some("cxx") | Some("hpp") => Language::Cpp,
        Some("swift") => Language::Swift,
        Some("rb") => Language::Ruby,
        _ => Language::Unknown,
    }
}

// ── Line helpers ──────────────────────────────────────────────────────────────

struct LineIter<'a> {
    content: &'a str,
    pos: usize,
    line: usize,
}

impl<'a> LineIter<'a> {
    fn new(content: &'a str) -> Self {
        Self {
            content,
            pos: 0,
            line: 0,
        }
    }
}

impl<'a> Iterator for LineIter<'a> {
    type Item = (usize, usize, &'a str); // (line_idx, byte_offset, line_text)

    fn next(&mut self) -> Option<Self::Item> {
        if self.pos >= self.content.len() {
            return None;
        }
        let start = self.pos;
        let line_idx = self.line;
        let end = self.content[start..]
            .find('\n')
            .map(|i| start + i + 1)
            .unwrap_or(self.content.len());
        self.pos = end;
        self.line += 1;
        Some((
            line_idx,
            start,
            &self.content[start..end.min(self.content.len())],
        ))
    }
}

// ── Rust ──────────────────────────────────────────────────────────────────────

fn extract_rust(content: &str) -> Vec<Symbol> {
    let mut symbols = Vec::new();
    for (line_idx, byte_offset, line) in LineIter::new(content) {
        let t = line.trim();
        // fn / pub fn / pub(crate) fn / async fn / pub async fn
        if let Some(name) = extract_after_keyword(t, &["fn "], &['(', '<', ' ', '\n', '\r']) {
            let kind = if t.contains("self") {
                SymbolKind::Method
            } else {
                SymbolKind::Function
            };
            symbols.push(Symbol {
                name,
                kind,
                line: line_idx,
                byte_offset,
            });
        } else if let Some(name) =
            extract_after_keyword(t, &["struct "], &[' ', '<', '(', '{', ';', '\n'])
        {
            symbols.push(Symbol {
                name,
                kind: SymbolKind::Struct,
                line: line_idx,
                byte_offset,
            });
        } else if let Some(name) = extract_after_keyword(t, &["enum "], &[' ', '<', '{', ';', '\n'])
        {
            symbols.push(Symbol {
                name,
                kind: SymbolKind::Enum,
                line: line_idx,
                byte_offset,
            });
        } else if let Some(name) = extract_after_keyword(t, &["trait "], &[' ', '<', '{', '\n']) {
            symbols.push(Symbol {
                name,
                kind: SymbolKind::Trait,
                line: line_idx,
                byte_offset,
            });
        } else if let Some(name) = extract_after_keyword(t, &["impl "], &[' ', '<', '\n', '{']) {
            symbols.push(Symbol {
                name,
                kind: SymbolKind::Struct,
                line: line_idx,
                byte_offset,
            });
        } else if let Some(name) = extract_after_keyword(t, &["type "], &[' ', '<', '=', '\n']) {
            symbols.push(Symbol {
                name,
                kind: SymbolKind::Type,
                line: line_idx,
                byte_offset,
            });
        } else if let Some(name) =
            extract_after_keyword(t, &["const ", "pub const "], &[' ', ':', '\n'])
        {
            symbols.push(Symbol {
                name,
                kind: SymbolKind::Constant,
                line: line_idx,
                byte_offset,
            });
        } else if let Some(name) = extract_after_keyword(t, &["mod "], &[' ', ';', '{', '\n']) {
            symbols.push(Symbol {
                name,
                kind: SymbolKind::Module,
                line: line_idx,
                byte_offset,
            });
        }
    }
    symbols
}

// ── Go ────────────────────────────────────────────────────────────────────────

fn extract_go(content: &str) -> Vec<Symbol> {
    let mut symbols = Vec::new();
    for (line_idx, byte_offset, line) in LineIter::new(content) {
        let t = line.trim();
        if t.starts_with("func ") {
            // func name(, func (recv) name(
            let after = &t["func ".len()..];
            if after.starts_with('(') {
                // Method with receiver: func (r Recv) MethodName(
                if let Some(close) = after.find(") ") {
                    let rest = &after[close + 2..];
                    if let Some(name) = extract_ident(rest) {
                        symbols.push(Symbol {
                            name,
                            kind: SymbolKind::Method,
                            line: line_idx,
                            byte_offset,
                        });
                    }
                }
            } else if let Some(name) = extract_ident(after) {
                symbols.push(Symbol {
                    name,
                    kind: SymbolKind::Function,
                    line: line_idx,
                    byte_offset,
                });
            }
        } else if t.starts_with("type ") {
            let after = &t["type ".len()..];
            if let Some(name) = extract_ident(after) {
                let kind = if after.contains(" struct") || after.ends_with("struct {") {
                    SymbolKind::Struct
                } else if after.contains(" interface") {
                    SymbolKind::Interface
                } else {
                    SymbolKind::Type
                };
                symbols.push(Symbol {
                    name,
                    kind,
                    line: line_idx,
                    byte_offset,
                });
            }
        } else if t.starts_with("const ") || t.starts_with("var ") {
            let is_const = t.starts_with("const ");
            let after = if is_const {
                &t["const ".len()..]
            } else {
                &t["var ".len()..]
            };
            if !after.starts_with('(') {
                if let Some(name) = extract_ident(after) {
                    let kind = if is_const {
                        SymbolKind::Constant
                    } else {
                        SymbolKind::Variable
                    };
                    symbols.push(Symbol {
                        name,
                        kind,
                        line: line_idx,
                        byte_offset,
                    });
                }
            }
        }
    }
    symbols
}

// ── JavaScript / TypeScript ───────────────────────────────────────────────────

fn extract_js_ts(content: &str) -> Vec<Symbol> {
    let mut symbols = Vec::new();
    for (line_idx, byte_offset, line) in LineIter::new(content) {
        let t = line.trim();
        // function name(
        if let Some(name) = extract_after_keyword(t, &["function "], &['(', ' ', '\n']) {
            symbols.push(Symbol {
                name,
                kind: SymbolKind::Function,
                line: line_idx,
                byte_offset,
            });
        }
        // class Name
        else if let Some(name) = extract_after_keyword(
            t,
            &["class ", "export class ", "export default class "],
            &[' ', '{', '<', '\n'],
        ) {
            symbols.push(Symbol {
                name,
                kind: SymbolKind::Class,
                line: line_idx,
                byte_offset,
            });
        }
        // interface Name (TypeScript)
        else if let Some(name) = extract_after_keyword(
            t,
            &["interface ", "export interface "],
            &[' ', '{', '<', '\n'],
        ) {
            symbols.push(Symbol {
                name,
                kind: SymbolKind::Interface,
                line: line_idx,
                byte_offset,
            });
        }
        // type Name = (TypeScript)
        else if let Some(name) =
            extract_after_keyword(t, &["type ", "export type "], &[' ', '=', '<', '\n'])
        {
            symbols.push(Symbol {
                name,
                kind: SymbolKind::Type,
                line: line_idx,
                byte_offset,
            });
        }
        // const/let/var name = function or arrow
        else {
            let prefixes = [
                "export const ",
                "const ",
                "export let ",
                "let ",
                "export var ",
                "var ",
            ];
            for prefix in &prefixes {
                if t.starts_with(prefix) {
                    let after = &t[prefix.len()..];
                    if let Some(name) = extract_ident(after) {
                        let rest = after[name.len()..].trim_start();
                        if rest.starts_with("= function")
                            || rest.starts_with("= async function")
                            || rest.starts_with("= (")
                            || rest.starts_with("= async (")
                            || rest.starts_with("= () =>")
                        {
                            symbols.push(Symbol {
                                name,
                                kind: SymbolKind::Function,
                                line: line_idx,
                                byte_offset,
                            });
                        } else if rest.starts_with(":") || rest.starts_with("=") {
                            symbols.push(Symbol {
                                name,
                                kind: SymbolKind::Constant,
                                line: line_idx,
                                byte_offset,
                            });
                        }
                        break;
                    }
                }
            }
        }
    }
    symbols
}

// ── Python ────────────────────────────────────────────────────────────────────

fn extract_python(content: &str) -> Vec<Symbol> {
    let mut symbols = Vec::new();
    for (line_idx, byte_offset, line) in LineIter::new(content) {
        let t = line.trim();
        if t.starts_with("def ") || t.starts_with("async def ") {
            let after = if t.starts_with("async ") {
                &t["async def ".len()..]
            } else {
                &t["def ".len()..]
            };
            if let Some(name) = extract_ident(after) {
                let indent = line.len() - line.trim_start().len();
                let kind = if indent > 0 {
                    SymbolKind::Method
                } else {
                    SymbolKind::Function
                };
                symbols.push(Symbol {
                    name,
                    kind,
                    line: line_idx,
                    byte_offset,
                });
            }
        } else if t.starts_with("class ") {
            let after = &t["class ".len()..];
            if let Some(name) = extract_ident(after) {
                symbols.push(Symbol {
                    name,
                    kind: SymbolKind::Class,
                    line: line_idx,
                    byte_offset,
                });
            }
        }
    }
    symbols
}

// ── Java / Kotlin ─────────────────────────────────────────────────────────────

fn extract_java_kotlin(content: &str) -> Vec<Symbol> {
    let mut symbols = Vec::new();
    for (line_idx, byte_offset, line) in LineIter::new(content) {
        let t = line.trim();
        if let Some(name) = extract_after_keyword(
            t,
            &[
                "class ",
                "public class ",
                "private class ",
                "protected class ",
                "abstract class ",
                "data class ",
                "sealed class ",
                "open class ",
            ],
            &[' ', '(', '{', '<', '\n'],
        ) {
            symbols.push(Symbol {
                name,
                kind: SymbolKind::Class,
                line: line_idx,
                byte_offset,
            });
        } else if let Some(name) = extract_after_keyword(
            t,
            &["interface ", "public interface "],
            &[' ', '{', '<', '\n'],
        ) {
            symbols.push(Symbol {
                name,
                kind: SymbolKind::Interface,
                line: line_idx,
                byte_offset,
            });
        } else if let Some(name) =
            extract_after_keyword(t, &["enum ", "public enum "], &[' ', '{', '\n'])
        {
            symbols.push(Symbol {
                name,
                kind: SymbolKind::Enum,
                line: line_idx,
                byte_offset,
            });
        } else if let Some(name) = extract_after_keyword(t, &["fun "], &['(', '<', '\n']) {
            symbols.push(Symbol {
                name,
                kind: SymbolKind::Function,
                line: line_idx,
                byte_offset,
            });
        }
    }
    symbols
}

// ── C / C++ ───────────────────────────────────────────────────────────────────

fn extract_c_cpp(content: &str) -> Vec<Symbol> {
    let mut symbols = Vec::new();
    for (line_idx, byte_offset, line) in LineIter::new(content) {
        let t = line.trim();
        if let Some(name) =
            extract_after_keyword(t, &["struct ", "typedef struct "], &[' ', '{', '\n'])
        {
            if !name.is_empty() {
                symbols.push(Symbol {
                    name,
                    kind: SymbolKind::Struct,
                    line: line_idx,
                    byte_offset,
                });
            }
        } else if let Some(name) =
            extract_after_keyword(t, &["class "], &[' ', ':', '{', '<', '\n'])
        {
            symbols.push(Symbol {
                name,
                kind: SymbolKind::Class,
                line: line_idx,
                byte_offset,
            });
        } else if let Some(name) = extract_after_keyword(t, &["enum "], &[' ', '{', '\n']) {
            symbols.push(Symbol {
                name,
                kind: SymbolKind::Enum,
                line: line_idx,
                byte_offset,
            });
        }
        // C function definitions: return_type name(
        else if !t.starts_with("//") && !t.starts_with("*") && !t.starts_with("#") {
            if t.contains('(')
                && !t.contains(';')
                && !t.contains("if ")
                && !t.contains("while ")
                && !t.contains("for ")
                && !t.contains("switch ")
            {
                let before_paren = t.split('(').next().unwrap_or("").trim();
                let words: Vec<&str> = before_paren.split_whitespace().collect();
                if words.len() >= 2 {
                    let name = words[words.len() - 1].to_string();
                    if is_valid_ident(&name) {
                        symbols.push(Symbol {
                            name,
                            kind: SymbolKind::Function,
                            line: line_idx,
                            byte_offset,
                        });
                    }
                }
            }
        }
    }
    symbols
}

// ── Swift ─────────────────────────────────────────────────────────────────────

fn extract_swift(content: &str) -> Vec<Symbol> {
    let mut symbols = Vec::new();
    for (line_idx, byte_offset, line) in LineIter::new(content) {
        let t = line.trim();
        if let Some(name) = extract_after_keyword(
            t,
            &[
                "func ",
                "public func ",
                "private func ",
                "internal func ",
                "open func ",
                "override func ",
            ],
            &['(', '<', '\n'],
        ) {
            symbols.push(Symbol {
                name,
                kind: SymbolKind::Function,
                line: line_idx,
                byte_offset,
            });
        } else if let Some(name) = extract_after_keyword(
            t,
            &["class ", "public class ", "final class "],
            &[' ', ':', '{', '<', '\n'],
        ) {
            symbols.push(Symbol {
                name,
                kind: SymbolKind::Class,
                line: line_idx,
                byte_offset,
            });
        } else if let Some(name) =
            extract_after_keyword(t, &["struct "], &[' ', ':', '{', '<', '\n'])
        {
            symbols.push(Symbol {
                name,
                kind: SymbolKind::Struct,
                line: line_idx,
                byte_offset,
            });
        } else if let Some(name) = extract_after_keyword(t, &["enum "], &[' ', ':', '{', '\n']) {
            symbols.push(Symbol {
                name,
                kind: SymbolKind::Enum,
                line: line_idx,
                byte_offset,
            });
        } else if let Some(name) = extract_after_keyword(t, &["protocol "], &[' ', ':', '{', '\n'])
        {
            symbols.push(Symbol {
                name,
                kind: SymbolKind::Interface,
                line: line_idx,
                byte_offset,
            });
        }
    }
    symbols
}

// ── Ruby ──────────────────────────────────────────────────────────────────────

fn extract_ruby(content: &str) -> Vec<Symbol> {
    let mut symbols = Vec::new();
    for (line_idx, byte_offset, line) in LineIter::new(content) {
        let t = line.trim();
        if t.starts_with("def ") || t.starts_with("def self.") {
            let after = if t.starts_with("def self.") {
                &t["def self.".len()..]
            } else {
                &t["def ".len()..]
            };
            if let Some(name) = extract_ident(after) {
                symbols.push(Symbol {
                    name,
                    kind: SymbolKind::Method,
                    line: line_idx,
                    byte_offset,
                });
            }
        } else if t.starts_with("class ") || t.starts_with("module ") {
            let (prefix, kind) = if t.starts_with("class ") {
                ("class ", SymbolKind::Class)
            } else {
                ("module ", SymbolKind::Module)
            };
            let after = &t[prefix.len()..];
            if let Some(name) = extract_ident(after) {
                symbols.push(Symbol {
                    name,
                    kind,
                    line: line_idx,
                    byte_offset,
                });
            }
        }
    }
    symbols
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Extract an identifier starting at the beginning of `s`.
fn extract_ident(s: &str) -> Option<String> {
    let s = s.trim_start();
    let end = s
        .find(|c: char| !c.is_alphanumeric() && c != '_')
        .unwrap_or(s.len());
    if end == 0 {
        return None;
    }
    let name = &s[..end];
    if is_valid_ident(name) {
        Some(name.to_string())
    } else {
        None
    }
}

/// Try each prefix in order; if the trimmed line starts with the prefix (after stripping
/// common visibility/async modifiers), extract the identifier that follows.
fn extract_after_keyword(line: &str, keywords: &[&str], stop_chars: &[char]) -> Option<String> {
    let stripped = strip_modifiers(line);
    for kw in keywords {
        if stripped.starts_with(kw) {
            let after = stripped[kw.len()..].trim_start();
            let end = after.find(stop_chars).unwrap_or(after.len());
            if end == 0 {
                continue;
            }
            let name = after[..end].trim().to_string();
            if is_valid_ident(&name) {
                return Some(name);
            }
        }
    }
    None
}

/// Strip common leading visibility/async/static modifiers so keyword matching works
/// regardless of `pub async fn` vs `fn` etc.
fn strip_modifiers(s: &str) -> &str {
    let prefixes = [
        "pub(crate) ",
        "pub(super) ",
        "pub(in ",
        "pub ",
        "private ",
        "protected ",
        "public ",
        "static ",
        "async ",
        "override ",
        "abstract ",
        "final ",
        "open ",
        "internal ",
        "external ",
        "inline ",
        "export default ",
        "export async ",
        "export ",
        "default ",
        "declare ",
    ];
    let mut current = s;
    loop {
        let mut changed = false;
        for prefix in &prefixes {
            if current.starts_with(prefix) {
                current = &current[prefix.len()..];
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    current
}

fn is_valid_ident(s: &str) -> bool {
    if s.is_empty() || s.len() > 128 {
        return false;
    }
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_alphanumeric() || c == '_')
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn rust_extracts_fn_and_struct() {
        let src = "pub fn hello() {}\nstruct Foo;\npub async fn bar() {}";
        let symbols = extract_rust(src);
        let names: Vec<&str> = symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"hello"), "missing hello: {names:?}");
        assert!(names.contains(&"Foo"), "missing Foo: {names:?}");
        assert!(names.contains(&"bar"), "missing bar: {names:?}");
    }

    #[test]
    fn go_extracts_func_and_type() {
        let src = "func Hello() {}\nfunc (s *Server) Start() {}\ntype Config struct {}";
        let symbols = extract_go(src);
        let names: Vec<&str> = symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"Hello"), "{names:?}");
        assert!(names.contains(&"Start"), "{names:?}");
        assert!(names.contains(&"Config"), "{names:?}");
    }

    #[test]
    fn python_extracts_def_and_class() {
        let src = "def greet():\n    pass\n\nclass Animal:\n    def eat(self):\n        pass";
        let symbols = extract_python(src);
        let names: Vec<&str> = symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"greet"), "{names:?}");
        assert!(names.contains(&"Animal"), "{names:?}");
        assert!(names.contains(&"eat"), "{names:?}");
    }

    #[test]
    fn typescript_extracts_class_and_function() {
        let src = "export class Foo {}\nexport function bar() {}\nconst baz = () => {};";
        let symbols = extract_js_ts(src);
        let names: Vec<&str> = symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"Foo"), "{names:?}");
        assert!(names.contains(&"bar"), "{names:?}");
        assert!(names.contains(&"baz"), "{names:?}");
    }

    #[test]
    fn extract_symbols_dispatches_by_extension() {
        let src = "fn main() {}";
        let symbols = extract_symbols(Path::new("main.rs"), src);
        assert_eq!(symbols.len(), 1);
        assert_eq!(symbols[0].name, "main");
        assert!(matches!(symbols[0].kind, SymbolKind::Function));
    }

    #[test]
    fn unknown_extension_returns_empty() {
        let symbols = extract_symbols(Path::new("data.csv"), "a,b,c");
        assert!(symbols.is_empty());
    }
}
