//! `--model` for agents started from the command line: the option each
//! agent takes a model name with, placed right after its executable so it
//! can't be read as the value of an option that takes the prompt.

use crate::workspace::PromptAgent;

/// The option `agent` takes a model with, for agents known to have one.
pub fn flag(agent: PromptAgent) -> Option<&'static str> {
    match agent {
        PromptAgent::Claude
        | PromptAgent::Codex
        | PromptAgent::Gemini
        | PromptAgent::OpenCode
        | PromptAgent::Copilot
        | PromptAgent::Cursor
        | PromptAgent::Qwen => Some("--model"),
        _ => None,
    }
}

/// Model names are plain identifiers (`opus`, `gpt-5.1-codex`,
/// `anthropic/claude-sonnet-4`), so they need no quoting in any shell.
fn plain(model: &str) -> bool {
    !model.is_empty()
        && model
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-._:/@[]".contains(c))
}

/// Whether `--model` can be used with the agent `key`.
pub fn check(key: &str, model: &str) -> Result<(), String> {
    let agent = crate::automations::executor::prompt_agent(key)
        .ok_or_else(|| format!("unknown agent “{key}”"))?;
    if flag(agent).is_none() {
        return Err(format!("{} doesn't take a model from the command line", agent.label()));
    }
    if !plain(model) {
        return Err(format!("“{model}” isn't a model name"));
    }
    Ok(())
}

/// `command` with the model option after its first word.
pub fn apply(command: &str, agent: PromptAgent, model: &str) -> Result<String, String> {
    let flag = flag(agent)
        .ok_or_else(|| format!("{} doesn't take a model from the command line", agent.label()))?;
    if !plain(model) {
        return Err(format!("“{model}” isn't a model name"));
    }
    let command = command.trim_start();
    Ok(match command.split_once(char::is_whitespace) {
        Some((program, rest)) => format!("{program} {flag} {model} {}", rest.trim_start()),
        None => format!("{command} {flag} {model}"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_model_goes_right_after_the_program() {
        assert_eq!(
            apply("gemini -i", PromptAgent::Gemini, "gemini-2.5-pro").unwrap(),
            "gemini --model gemini-2.5-pro -i"
        );
        assert_eq!(apply("codex", PromptAgent::Codex, "o3").unwrap(), "codex --model o3");
        assert!(apply("amp", PromptAgent::Amp, "x").is_err());
        assert!(apply("claude", PromptAgent::Claude, "opus; rm -rf /").is_err());
        assert!(check("claude", "anthropic/claude-sonnet-4").is_ok());
    }
}
