use dragent_parser::{classify_command, EmptyRegistry, Token};

use crate::console::cmd_suggest::{is_known_path_command, specs};

use super::context::ClassifyContext;
use super::InputClassifier;
use crate::agent::detected_input_type::DetectedInputType;

pub struct HeuristicInputClassifier;

/// Words that strongly suggest the user is addressing the agent.
///
/// Every entry must be a word that is NOT also a real command, or a genuine
/// command typed here gets stolen by the agent. That rules out `make`, `find`,
/// `test`, `build`, `run`, `install`, `open`, `touch`, `watch`, `time`, `type`,
/// and `set` however natural they read as instructions — they live in
/// SHELL_KEYWORDS or AMBIGUOUS_COMMANDS instead.
const AGENT_LEAD_WORDS: &[&str] = &[
    "hello",
    "hi",
    "hey",
    "thanks",
    "explain",
    "what",
    "why",
    "how",
    "fix",
    "help",
    "describe",
    "show",
    "tell",
    "please",
    "summarize",
    "list",
    "generate",
    "write",
    "create",
    "give",
    "search",
    "analyze",
    "debug",
    "suggest",
    "recommend",
    "refactor",
    "optimize",
    "continue",
    "approve",
    // Coding-agent imperatives.
    "add",
    "implement",
    "update",
    "change",
    "rename",
    "improve",
    "document",
    "review",
    "convert",
    "migrate",
    "rewrite",
    "remove",
    "delete",
    "investigate",
    "compare",
    "extract",
    "simplify",
    "clarify",
    "design",
    "plan",
    "outline",
    "rework",
    "annotate",
    "translate",
    "propose",
    "draft",
    "teach",
    "walk",
    // Interrogatives beyond what/why/how.
    "when",
    "where",
    "who",
    "which",
    "whose",
    "should",
    "could",
    "would",
    "does",
];

/// Contractions never appear in shell syntax, so one is conclusive prose.
const CONTRACTIONS: &[&str] = &[
    "don't",
    "can't",
    "won't",
    "isn't",
    "aren't",
    "wasn't",
    "weren't",
    "doesn't",
    "didn't",
    "shouldn't",
    "wouldn't",
    "couldn't",
    "hasn't",
    "haven't",
    "hadn't",
    "it's",
    "that's",
    "there's",
    "here's",
    "what's",
    "let's",
    "i'm",
    "i've",
    "i'll",
    "i'd",
    "we're",
    "we've",
    "we'll",
    "you're",
    "you've",
    "they're",
];

/// First-person/second-person openers: "i need…", "we should…", "you can…".
/// A shell command never begins this way.
const PERSONAL_OPENERS: &[&str] = &[
    "i", "we", "my", "our", "you", "your", "me", "let's", "lets", "i'm", "i've",
];

/// Short replies after an agent turn.
const AGENT_FOLLOWUP_PHRASES: &[&str] = &[
    "yes",
    "no",
    "ok",
    "okay",
    "continue",
    "go on",
    "do it",
    "approve",
    "thanks",
    "thank you",
    "looks good",
    "perfect",
    "nice",
    "great",
];

/// External AI CLIs — always route as shell so users can launch them normally.
const EXTERNAL_AI_CLIS: &[&str] = &["claude", "codex", "gemini", "ollama"];

const SHELL_KEYWORDS: &[&str] = &[
    "git",
    "cd",
    "ls",
    "echo",
    "cargo",
    "npm",
    "yarn",
    "docker",
    "kubectl",
    "grep",
    "find",
    "cat",
    "head",
    "tail",
    "sed",
    "awk",
    "curl",
    "wget",
    "ssh",
    "scp",
    "cp",
    "mv",
    "rm",
    "mkdir",
    "chmod",
    "chown",
    "ps",
    "kill",
    "sudo",
    "apt",
    "brew",
    "pip",
    "python",
    "python3",
    "node",
    "ruby",
    "go",
    "make",
    "cmake",
    "gcc",
    "clang",
    "rustc",
    "java",
    "mvn",
    "gradle",
    "terraform",
    "ansible",
    "vim",
    "nano",
    "code",
    "open",
    "touch",
    "ln",
    "mount",
    "df",
    "du",
    "top",
    "htop",
    "env",
    "export",
    "source",
    "alias",
    "unset",
    "set",
    "which",
    "type",
    "man",
    "less",
    "more",
    "sort",
    "uniq",
    "wc",
    "xargs",
    "tee",
    "tar",
    "gzip",
    "gunzip",
    "zip",
    "unzip",
    "rsync",
    "ping",
    "netstat",
    "ifconfig",
    "ip",
    "systemctl",
    "service",
    "journalctl",
    "launchctl",
    "podman",
    "helm",
    "minikube",
    "az",
    "gcloud",
    "aws",
    "psql",
    "mysql",
    "mongo",
    "redis-cli",
    "sqlite3",
    "gh",
    "svn",
    "test",
    "true",
    "false",
    "read",
    "printf",
    "rustup",
    "rustfmt",
    "clippy",
    "pnpm",
    "npx",
    "bun",
    "deno",
    "k9s",
    "ffmpeg",
    "convert",
    "mogrify",
    "jq",
    "yq",
    "fx",
    "tmux",
    "screen",
    "bat",
    "rg",
    "fd",
    "fzf",
    "exa",
    "lsd",
    "stat",
    "file",
    "date",
    "time",
    "sleep",
    "watch",
    "sh",
    "bash",
    "zsh",
    "fish",
];

const ENGLISH_WORDS: &[&str] = &[
    "a",
    "an",
    "the",
    "is",
    "are",
    "was",
    "were",
    "be",
    "been",
    "being",
    "have",
    "has",
    "had",
    "do",
    "does",
    "did",
    "will",
    "would",
    "shall",
    "should",
    "may",
    "might",
    "must",
    "can",
    "could",
    "this",
    "that",
    "these",
    "those",
    "it",
    "its",
    "they",
    "them",
    "their",
    "we",
    "our",
    "you",
    "your",
    "i",
    "my",
    "me",
    "he",
    "she",
    "his",
    "her",
    "to",
    "of",
    "in",
    "on",
    "at",
    "by",
    "for",
    "with",
    "about",
    "as",
    "into",
    "through",
    "during",
    "before",
    "after",
    "above",
    "below",
    "from",
    "up",
    "down",
    "out",
    "off",
    "over",
    "under",
    "again",
    "and",
    "but",
    "or",
    "nor",
    "so",
    "yet",
    "both",
    "either",
    "neither",
    "not",
    "no",
    "all",
    "any",
    "few",
    "more",
    "most",
    "other",
    "some",
    "such",
    "than",
    "too",
    "very",
    "just",
    "here",
    "there",
    "when",
    "where",
    "who",
    "which",
    "then",
    "also",
    "only",
    "even",
    "if",
    "because",
    "while",
    "although",
    "though",
    "unless",
    "until",
    "since",
    "now",
    "still",
    "already",
    "always",
    "never",
    "did",
    "does",
    "done",
    "got",
    "get",
    "go",
    "going",
    "making",
    "use",
    "using",
    "see",
    "like",
    "know",
    "think",
    "want",
    "need",
    "last",
    "error",
    "fail",
    "failed",
    "output",
    "code",
    "result",
    "command",
    "file",
    "directory",
    "function",
    "variable",
    "type",
    "value",
    "return",
    "message",
    "problem",
    "issue",
    "question",
    "time",
    "version",
    "number",
    "string",
    "list",
    "item",
    "me",
    "my",
    "please",
    "work",
    "plan",
    "solution",
    "available",
];

const AMBIGUOUS_COMMANDS: &[&str] = &["make", "find"];

/// Function words that can never realistically appear as standalone shell arguments.
/// Their presence in the "rest" of a command-like input means the user is writing prose.
const FUNCTION_WORDS: &[&str] = &[
    // Copulas / auxiliary verbs
    "is", "are", "was", "were", "be", "been", // Articles
    "a", "an", "the", // Pronouns (as standalone tokens)
    "it", "its", // Coordinating conjunctions
    "and", "but", "or", // Prepositions that never appear as bare shell args
    "of", "in", "on", "at", "by", "with", "about", "as",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum SignalStrength {
    Weak = 0,
    Moderate = 1,
    Strong = 2,
}

#[derive(Clone, Copy, Debug)]
struct RawClassification {
    input_type: DetectedInputType,
    strength: SignalStrength,
}

impl HeuristicInputClassifier {
    pub fn new() -> Self {
        Self
    }
}

impl InputClassifier for HeuristicInputClassifier {
    fn classify(&self, text: &str, ctx: &ClassifyContext) -> Option<DetectedInputType> {
        let text = text.trim();
        if text.is_empty() {
            return None;
        }

        let primary = classify_buffer(text, ctx);
        let secondary = if should_retry_without_last_token(text) {
            let shortened = trim_incomplete_last_token(text);
            if shortened.is_empty() {
                None
            } else {
                Some(classify_buffer(shortened, ctx))
            }
        } else {
            None
        };

        let merged = merge_classifications(primary, secondary);
        Some(apply_hysteresis(merged, ctx.current_type))
    }
}

fn classify_buffer(text: &str, ctx: &ClassifyContext) -> RawClassification {
    if text.starts_with('#') || text.starts_with('!') {
        return RawClassification {
            input_type: DetectedInputType::Shell,
            strength: SignalStrength::Strong,
        };
    }

    let tokens: Vec<&str> = text.split_whitespace().collect();
    let first_raw = tokens.first().copied().unwrap_or("");
    // Parse past any leading KEY=VALUE env-var prefix to find the real command name.
    // e.g. "FOO=bar git push" → first = "git", not "FOO=bar".
    let first_cmd: Option<String> = if first_raw.contains('=') {
        first_command_name(text)
    } else {
        None
    };
    let first: &str = first_cmd.as_deref().unwrap_or(first_raw);
    let first_lower = first.to_lowercase();
    let lowered = text.to_lowercase();

    if ctx.after_agent_submit && AGENT_FOLLOWUP_PHRASES.contains(&lowered.as_str()) {
        return RawClassification {
            input_type: DetectedInputType::Agent,
            strength: SignalStrength::Strong,
        };
    }

    if EXTERNAL_AI_CLIS.contains(&first_lower.as_str()) {
        return RawClassification {
            input_type: DetectedInputType::Shell,
            strength: SignalStrength::Strong,
        };
    }

    if matches_command_history(text, &ctx.recent_commands) {
        return RawClassification {
            input_type: DetectedInputType::Shell,
            strength: SignalStrength::Strong,
        };
    }

    if tokens.len() == 1 {
        if is_agent_lead_prefix(&first_lower) {
            return RawClassification {
                input_type: DetectedInputType::Agent,
                strength: SignalStrength::Moderate,
            };
        }
        if AGENT_LEAD_WORDS.contains(&first_lower.as_str()) {
            return RawClassification {
                input_type: DetectedInputType::Agent,
                strength: SignalStrength::Strong,
            };
        }
    } else if AGENT_LEAD_WORDS.contains(&first_lower.as_str()) {
        return RawClassification {
            input_type: DetectedInputType::Agent,
            strength: SignalStrength::Strong,
        };
    }

    if ctx.last_block_failed
        && matches!(
            first_lower.as_str(),
            "fix" | "why" | "explain" | "help" | "debug" | "what" | "how"
        )
    {
        return RawClassification {
            input_type: DetectedInputType::Agent,
            strength: SignalStrength::Moderate,
        };
    }

    // Function words (articles, copulas, prepositions) cannot appear as bare shell
    // arguments, so "ls is the name of it all" or "git is broken" are always prose.
    if tokens.len() > 1 && rest_has_function_word(&tokens) {
        return RawClassification {
            input_type: DetectedInputType::Agent,
            strength: SignalStrength::Strong,
        };
    }

    // Contractions have no shell meaning at all. Checked before the shell
    // keywords so "git isn't pushing" reaches the agent rather than being
    // claimed by its first word.
    if tokens.iter().any(|t| {
        CONTRACTIONS.contains(
            &t.trim_matches(|c: char| !c.is_alphanumeric() && c != '\'')
                .to_lowercase()
                .as_str(),
        )
    }) {
        return RawClassification {
            input_type: DetectedInputType::Agent,
            strength: SignalStrength::Strong,
        };
    }

    // "i need…", "we should…", "you can…" — no command starts this way.
    if tokens.len() > 1 && PERSONAL_OPENERS.contains(&first_lower.as_str()) {
        return RawClassification {
            input_type: DetectedInputType::Agent,
            strength: SignalStrength::Strong,
        };
    }

    if !is_ambiguous_command_used_as_nl(&first_lower, &tokens) {
        if SHELL_KEYWORDS.contains(&first_lower.as_str())
            || specs().any(|s| s.name.eq_ignore_ascii_case(first))
            || is_known_path_command(&first)
        {
            return RawClassification {
                input_type: DetectedInputType::Shell,
                strength: SignalStrength::Strong,
            };
        }
    }

    if has_shell_syntax(text) {
        return RawClassification {
            input_type: DetectedInputType::Shell,
            strength: SignalStrength::Strong,
        };
    }

    if tokens.len() == 1 && is_path_like(&first) {
        return RawClassification {
            input_type: DetectedInputType::Shell,
            strength: SignalStrength::Moderate,
        };
    }

    if text.ends_with('?') {
        return RawClassification {
            input_type: DetectedInputType::Agent,
            strength: SignalStrength::Strong,
        };
    }

    if let Some(strength) = english_prose_strength(&tokens, &first) {
        return RawClassification {
            input_type: DetectedInputType::Agent,
            strength,
        };
    }

    // Below here the shell-keyword and shell-syntax checks have already had
    // their say, so anything reaching this point has no command-like feature.

    // A sentence's worth of words with no flag, operator, or path in sight.
    // Real commands stay terse; by eight bare words this is a request.
    if tokens.len() >= 8
        && !tokens
            .iter()
            .any(|t| t.starts_with('-') || is_path_like(t) && t.contains('/'))
    {
        return RawClassification {
            input_type: DetectedInputType::Agent,
            strength: SignalStrength::Strong,
        };
    }

    // Terminal punctuation on a multi-word input. Deliberately not a lone '.'
    // or a single token — "cd .." and "./x" must not trip this.
    if tokens.len() >= 3 && text.ends_with(['.', '!']) && !text.ends_with("..") {
        return RawClassification {
            input_type: DetectedInputType::Agent,
            strength: SignalStrength::Moderate,
        };
    }

    if is_ambiguous_command_used_as_nl(&first_lower, &tokens) {
        return RawClassification {
            input_type: DetectedInputType::Agent,
            strength: SignalStrength::Moderate,
        };
    }

    RawClassification {
        input_type: DetectedInputType::Shell,
        strength: SignalStrength::Weak,
    }
}

fn merge_classifications(
    primary: RawClassification,
    secondary: Option<RawClassification>,
) -> RawClassification {
    let Some(secondary) = secondary else {
        return primary;
    };

    if primary.input_type == DetectedInputType::Agent
        || secondary.input_type == DetectedInputType::Agent
    {
        let strength = primary.strength.max(secondary.strength);
        return RawClassification {
            input_type: DetectedInputType::Agent,
            strength,
        };
    }

    primary
}

fn apply_hysteresis(raw: RawClassification, current: DetectedInputType) -> DetectedInputType {
    if raw.input_type == current {
        return current;
    }

    match (current, raw.input_type) {
        (DetectedInputType::Agent, DetectedInputType::Shell) => {
            if raw.strength >= SignalStrength::Strong {
                DetectedInputType::Shell
            } else {
                DetectedInputType::Agent
            }
        }
        (DetectedInputType::Shell, DetectedInputType::Agent) => {
            if raw.strength >= SignalStrength::Moderate {
                DetectedInputType::Agent
            } else {
                DetectedInputType::Shell
            }
        }
        _ => raw.input_type,
    }
}

/// True when `partial` looks like the user re-typing a recent shell command.
fn matches_command_history(partial: &str, history: &[String]) -> bool {
    let partial = partial.trim();
    if partial.len() < 2 {
        return false;
    }
    let partial_lower = partial.to_ascii_lowercase();
    history.iter().rev().take(30).any(|cmd| {
        let cmd = cmd.trim();
        if cmd.is_empty() {
            return false;
        }
        let cmd_lower = cmd.to_ascii_lowercase();
        cmd_lower.starts_with(&partial_lower)
            || partial_lower == cmd_lower
            || (partial_lower.starts_with(&cmd_lower) && partial.len() <= cmd.len() + 2)
    })
}

/// Progressive typing: `expl` / `wha` bias toward agent before the word is complete.
fn is_agent_lead_prefix(token: &str) -> bool {
    if token.len() < 2 {
        return false;
    }
    AGENT_LEAD_WORDS
        .iter()
        .any(|word| word.starts_with(token) && token.len() < word.len())
}

fn should_retry_without_last_token(text: &str) -> bool {
    if text.ends_with(' ') {
        return false;
    }
    if text.ends_with(['?', '!', '.', ',', ';']) {
        return false;
    }
    text.split_whitespace().count() > 2
}

fn trim_incomplete_last_token(text: &str) -> &str {
    match text.rfind(char::is_whitespace) {
        Some(idx) => text[..idx].trim_end(),
        None => "",
    }
}

fn has_shell_syntax(text: &str) -> bool {
    // Use the lexer so operators inside quoted strings don't false-positive
    // (e.g. "echo 'a > b'" has no shell redirect; the `>` is inside a quote).
    let has_operator = dragent_parser::lex(text).iter().any(|t| {
        matches!(
            t.item,
            Token::Pipe
                | Token::LogicalAnd
                | Token::LogicalOr
                | Token::Semicolon
                | Token::Ampersand
                | Token::RedirectOut
                | Token::RedirectAppend
                | Token::RedirectIn
                | Token::RedirectErr
                | Token::RedirectErrAppend
                | Token::RedirectBoth
        )
    });
    has_operator
        || text.contains(" --")
        || text.starts_with("./")
        || text.starts_with("../")
        || (text.starts_with('/') && !text.starts_with("/ "))
}

/// Parses `text` and returns the real command name, stripping leading
/// `KEY=VALUE` env-var assignments. Returns `None` when no command name exists.
fn first_command_name(text: &str) -> Option<String> {
    let (group, _) = dragent_parser::lite_parse(text);
    let cmd = group
        .pipelines
        .into_iter()
        .next()?
        .commands
        .into_iter()
        .next()?;
    let classified = classify_command(cmd, &EmptyRegistry);
    if classified.name.item.is_empty() {
        None
    } else {
        Some(classified.name.item)
    }
}

fn is_path_like(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '/' | '.' | '-' | '_' | '~'))
}

/// True when the tokens after position 0 contain a function word with no flags present.
/// Flags (`-x`, `--flag`) cannot appear alongside function words in real NL, so their
/// presence cancels the check and lets the shell-keyword path decide.
fn rest_has_function_word(tokens: &[&str]) -> bool {
    let rest = &tokens[1..];
    if rest.is_empty() {
        return false;
    }
    if rest.iter().any(|t| t.starts_with('-')) {
        return false;
    }
    rest.iter().any(|t| {
        FUNCTION_WORDS.contains(
            &t.trim_matches(|c: char| !c.is_alphanumeric())
                .to_lowercase()
                .as_str(),
        )
    })
}

fn is_ambiguous_command_used_as_nl(first: &str, tokens: &[&str]) -> bool {
    if !AMBIGUOUS_COMMANDS.contains(&first) || tokens.len() < 3 {
        return false;
    }
    let rest = &tokens[1..];
    let word_like = rest
        .iter()
        .filter(|&&t| !t.is_empty() && t.chars().all(|c| c.is_ascii_alphabetic()))
        .count();
    (word_like as f32 / rest.len() as f32) >= 0.6
}

fn english_prose_strength(tokens: &[&str], first: &str) -> Option<SignalStrength> {
    if tokens.len() < 2 {
        return None;
    }

    let skip_first = SHELL_KEYWORDS.contains(&first.to_lowercase().as_str())
        || specs().any(|s| s.name.eq_ignore_ascii_case(first))
        || is_known_path_command(first);

    let scored: Vec<&str> = if skip_first && tokens.len() > 1 {
        tokens[1..].to_vec()
    } else {
        tokens.to_vec()
    };

    if scored.is_empty() {
        return None;
    }

    let english_count = scored
        .iter()
        .filter(|&&t| {
            let lower = t
                .trim_matches(|c: char| !c.is_alphanumeric())
                .to_lowercase();
            ENGLISH_WORDS.contains(&lower.as_str())
        })
        .count();

    let ratio = english_count as f32 / scored.len() as f32;
    let threshold = if scored.len() <= 2 {
        1.0
    } else if scored.len() <= 4 {
        0.75
    } else {
        0.55
    };

    if ratio >= threshold {
        Some(if ratio >= 0.85 || scored.len() >= 5 {
            SignalStrength::Strong
        } else {
            SignalStrength::Moderate
        })
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn classify(text: &str) -> Option<DetectedInputType> {
        HeuristicInputClassifier::new().classify(text, &ClassifyContext::default())
    }

    fn classify_with(text: &str, ctx: &ClassifyContext) -> Option<DetectedInputType> {
        HeuristicInputClassifier::new().classify(text, ctx)
    }

    #[test]
    fn empty_returns_none() {
        assert_eq!(classify(""), None);
        assert_eq!(classify("   "), None);
    }

    #[test]
    fn shell_git() {
        assert_eq!(classify("git status"), Some(DetectedInputType::Shell));
        assert_eq!(
            classify("git commit -m 'fix'"),
            Some(DetectedInputType::Shell)
        );
    }

    #[test]
    fn shell_ls() {
        assert_eq!(classify("ls -la"), Some(DetectedInputType::Shell));
        assert_eq!(classify("ls"), Some(DetectedInputType::Shell));
    }

    #[test]
    fn shell_cargo() {
        assert_eq!(
            classify("cargo test -p dragent"),
            Some(DetectedInputType::Shell)
        );
    }

    #[test]
    fn agent_explain() {
        assert_eq!(
            classify("explain this error"),
            Some(DetectedInputType::Agent)
        );
    }

    #[test]
    fn agent_prefix_while_typing() {
        assert_eq!(classify("expl"), Some(DetectedInputType::Agent));
        assert_eq!(classify("wha"), Some(DetectedInputType::Agent));
    }

    #[test]
    fn agent_hello() {
        assert_eq!(classify("hello"), Some(DetectedInputType::Agent));
    }

    #[test]
    fn agent_why() {
        assert_eq!(
            classify("why did that fail"),
            Some(DetectedInputType::Agent)
        );
    }

    #[test]
    fn agent_what() {
        assert_eq!(
            classify("what is this function doing"),
            Some(DetectedInputType::Agent)
        );
    }

    #[test]
    fn agent_fix() {
        assert_eq!(classify("fix the bug"), Some(DetectedInputType::Agent));
    }

    #[test]
    fn shell_pipe() {
        assert_eq!(
            classify("cat foo | grep bar"),
            Some(DetectedInputType::Shell)
        );
    }

    #[test]
    fn external_ai_cli_is_shell() {
        assert_eq!(classify("claude"), Some(DetectedInputType::Shell));
        assert_eq!(classify("codex run"), Some(DetectedInputType::Shell));
        assert_eq!(classify("gemini chat"), Some(DetectedInputType::Shell));
        assert_eq!(classify("ollama run llama"), Some(DetectedInputType::Shell));
    }

    #[test]
    fn history_prefix_is_shell() {
        let ctx = ClassifyContext {
            recent_commands: vec!["cargo test -p dragent".into()],
            ..ClassifyContext::default()
        };
        assert_eq!(
            classify_with("cargo test", &ctx),
            Some(DetectedInputType::Shell)
        );
        assert_eq!(
            classify_with("cargo t", &ctx),
            Some(DetectedInputType::Shell)
        );
    }

    #[test]
    fn agent_followup_after_agent_submit() {
        let ctx = ClassifyContext {
            after_agent_submit: true,
            ..ClassifyContext::default()
        };
        assert_eq!(classify_with("yes", &ctx), Some(DetectedInputType::Agent));
        assert_eq!(
            classify_with("continue", &ctx),
            Some(DetectedInputType::Agent)
        );
        assert_eq!(classify_with("do it", &ctx), Some(DetectedInputType::Agent));
    }

    #[test]
    fn hysteresis_stays_agent_on_weak_shell_signal() {
        let ctx = ClassifyContext {
            current_type: DetectedInputType::Agent,
            ..ClassifyContext::default()
        };
        // "run" alone is weak — should not flip away from Agent immediately
        assert_eq!(classify_with("run", &ctx), Some(DetectedInputType::Agent));
    }

    #[test]
    fn hysteresis_flips_to_shell_on_strong_command() {
        let ctx = ClassifyContext {
            current_type: DetectedInputType::Agent,
            ..ClassifyContext::default()
        };
        assert_eq!(
            classify_with("git status", &ctx),
            Some(DetectedInputType::Shell)
        );
    }

    #[test]
    fn failed_block_biases_fix_toward_agent() {
        let ctx = ClassifyContext {
            last_block_failed: true,
            ..ClassifyContext::default()
        };
        assert_eq!(
            classify_with("fix this", &ctx),
            Some(DetectedInputType::Agent)
        );
    }

    #[test]
    fn ambiguous_make_as_nl() {
        assert_eq!(
            classify("make yourself available"),
            Some(DetectedInputType::Agent)
        );
        assert_eq!(
            classify("make it work please"),
            Some(DetectedInputType::Agent)
        );
    }

    #[test]
    fn ambiguous_make_as_shell() {
        assert_eq!(classify("make build"), Some(DetectedInputType::Shell));
        assert_eq!(classify("make -j4 build"), Some(DetectedInputType::Shell));
    }

    #[test]
    fn find_command_is_shell() {
        assert_eq!(
            classify("find . -name '*.rs'"),
            Some(DetectedInputType::Shell)
        );
    }

    #[test]
    fn ambiguous_find_as_nl() {
        assert_eq!(
            classify("find me a solution"),
            Some(DetectedInputType::Agent)
        );
    }

    #[test]
    fn specs_only_commands_are_shell() {
        assert_eq!(classify("nx build"), Some(DetectedInputType::Shell));
        assert_eq!(classify("vitest run"), Some(DetectedInputType::Shell));
    }

    #[test]
    fn question_mark_is_agent() {
        assert_eq!(
            classify("what does this do?"),
            Some(DetectedInputType::Agent)
        );
    }

    #[test]
    fn shell_command_used_as_prose_is_agent() {
        // Known commands where the rest is clearly natural language prose.
        assert_eq!(
            classify("ls is the name of it all"),
            Some(DetectedInputType::Agent)
        );
        assert_eq!(classify("git is broken"), Some(DetectedInputType::Agent));
        assert_eq!(classify("cat the output"), Some(DetectedInputType::Agent));
        assert_eq!(
            classify("grep all the files"),
            Some(DetectedInputType::Agent)
        );
        assert_eq!(
            classify("ssh to the server"),
            Some(DetectedInputType::Agent)
        );
        assert_eq!(
            classify("echo is a command"),
            Some(DetectedInputType::Agent)
        );
    }

    #[test]
    fn shell_command_with_flags_stays_shell() {
        // Flags cancel the prose check — these are real commands.
        assert_eq!(classify("ls -la"), Some(DetectedInputType::Shell));
        assert_eq!(classify("grep -r error ."), Some(DetectedInputType::Shell));
        assert_eq!(classify("git --version"), Some(DetectedInputType::Shell));
    }

    #[test]
    fn ambiguous_args_without_function_words_stay_shell() {
        // No function words in these — normal shell commands.
        assert_eq!(classify("grep error logs"), Some(DetectedInputType::Shell));
        assert_eq!(
            classify("git push origin main"),
            Some(DetectedInputType::Shell)
        );
    }

    // ── New-signal coverage ──────────────────────────────────────────────

    #[test]
    fn contractions_are_prose_even_after_a_command_word() {
        assert_eq!(
            classify("git isn't pushing"),
            Some(DetectedInputType::Agent)
        );
        assert_eq!(
            classify("the build doesn't work"),
            Some(DetectedInputType::Agent)
        );
        assert_eq!(
            classify("i can't run the tests"),
            Some(DetectedInputType::Agent)
        );
        assert_eq!(
            classify("let's refactor this"),
            Some(DetectedInputType::Agent)
        );
    }

    #[test]
    fn personal_openers_are_prose() {
        assert_eq!(
            classify("i need a login page"),
            Some(DetectedInputType::Agent)
        );
        assert_eq!(
            classify("we should cache that"),
            Some(DetectedInputType::Agent)
        );
        assert_eq!(
            classify("you can skip the retry"),
            Some(DetectedInputType::Agent)
        );
        assert_eq!(
            classify("my tests keep timing out"),
            Some(DetectedInputType::Agent)
        );
    }

    #[test]
    fn coding_imperatives_route_to_agent() {
        assert_eq!(
            classify("add a retry to the fetch"),
            Some(DetectedInputType::Agent)
        );
        assert_eq!(
            classify("implement pagination"),
            Some(DetectedInputType::Agent)
        );
        assert_eq!(
            classify("rename this function"),
            Some(DetectedInputType::Agent)
        );
        assert_eq!(
            classify("review the auth module"),
            Some(DetectedInputType::Agent)
        );
        assert_eq!(
            classify("migrate the schema"),
            Some(DetectedInputType::Agent)
        );
    }

    #[test]
    fn extra_interrogatives_route_to_agent() {
        assert_eq!(
            classify("when does the token expire"),
            Some(DetectedInputType::Agent)
        );
        assert_eq!(
            classify("where is the config loaded"),
            Some(DetectedInputType::Agent)
        );
        assert_eq!(
            classify("should we retry on 500"),
            Some(DetectedInputType::Agent)
        );
    }

    #[test]
    fn long_bare_sentences_are_prose() {
        assert_eq!(
            classify("the deploy keeps failing every time we push a new release tag"),
            Some(DetectedInputType::Agent)
        );
    }

    #[test]
    fn terminal_punctuation_on_a_sentence_is_prose() {
        assert_eq!(
            classify("that broke the build."),
            Some(DetectedInputType::Agent)
        );
        assert_eq!(classify("ship it now!"), Some(DetectedInputType::Agent));
    }

    // ── Regression guards for the new signals ────────────────────────────
    // Each of these is a command the additions above could plausibly steal.

    #[test]
    fn commands_that_look_like_imperatives_stay_shell() {
        // These words were deliberately excluded from AGENT_LEAD_WORDS
        // because they are real executables.
        assert_eq!(classify("make build"), Some(DetectedInputType::Shell));
        assert_eq!(
            classify("find . -name '*.rs'"),
            Some(DetectedInputType::Shell)
        );
        assert_eq!(
            classify("test -f Cargo.toml"),
            Some(DetectedInputType::Shell)
        );
        assert_eq!(classify("open ."), Some(DetectedInputType::Shell));
        assert_eq!(
            classify("touch new_file.rs"),
            Some(DetectedInputType::Shell)
        );
    }

    #[test]
    fn dot_paths_are_not_sentence_punctuation() {
        // The terminal-punctuation rule must not fire on relative paths.
        assert_eq!(classify("cd .."), Some(DetectedInputType::Shell));
        assert_eq!(
            classify("cp -r src ../backup"),
            Some(DetectedInputType::Shell)
        );
        assert_eq!(classify("git add ."), Some(DetectedInputType::Shell));
    }

    #[test]
    fn long_commands_with_flags_stay_shell() {
        // The long-input rule must not fire when flags/paths are present.
        assert_eq!(
            classify("docker run --rm -it -v /tmp:/tmp -p 8080:80 nginx bash"),
            Some(DetectedInputType::Shell)
        );
        assert_eq!(
            classify("cargo test -p dragent --bin dragent input_classifier -- --nocapture"),
            Some(DetectedInputType::Shell)
        );
    }
}
