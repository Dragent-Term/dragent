//! The prompt screen's Coordinate option: the request goes to coordinated
//! work, led by the agent picked on the screen (Dragent's own agent too),
//! which plans it and gives each part to a coding agent (see `cli::team`).

use super::action::PromptAgent;

/// Why the request can't go to a team as things are.
pub(crate) fn team_problem(lead: PromptAgent, request: &str, remote: bool) -> Option<String> {
    if remote && lead == PromptAgent::Dragent {
        return Some("Choose a terminal agent as lead for coordination on an SSH machine.".into());
    }
    if request.trim().is_empty() {
        return Some("Say what the agents should do.".into());
    }
    None
}

/// Agents that can work on a team's tasks: those that run in a terminal.
/// Dragent's own agent leads, but doesn't take tasks yet.
pub(crate) fn can_work(agent: PromptAgent) -> bool {
    agent != PromptAgent::Dragent
}

/// The team's agents, as keys: the lead's own kind first when it can work
/// (a team of one agent gives its tasks to more of it), then the helpers
/// chosen.
pub(crate) fn roster(lead: PromptAgent, helpers: &[PromptAgent]) -> Vec<String> {
    let mut roster: Vec<String> = Some(lead)
        .filter(|lead| can_work(*lead))
        .map(|lead| lead.command_key().to_string())
        .into_iter()
        .collect();
    for helper in helpers.iter().copied().filter(|helper| can_work(*helper)) {
        let key = helper.command_key().to_string();
        if !roster.contains(&key) {
            roster.push(key);
        }
    }
    roster
}

/// The chip's label: what the team is when it's on.
pub(crate) fn chip_label(on: bool, lead: PromptAgent, helpers: &[PromptAgent]) -> String {
    if !on {
        return "Coordinate".into();
    }
    let others: Vec<&str> = helpers
        .iter()
        .copied()
        .filter(|helper| *helper != lead && can_work(*helper))
        .map(PromptAgent::label)
        .collect();
    if others.is_empty() {
        "Agents".into()
    } else {
        others.join(", ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_lead_comes_first_and_only_terminal_agents_work() {
        assert_eq!(
            roster(
                PromptAgent::Claude,
                &[
                    PromptAgent::Codex,
                    PromptAgent::Claude,
                    PromptAgent::Dragent
                ]
            ),
            ["claude", "codex"]
        );
        assert_eq!(roster(PromptAgent::Claude, &[]), ["claude"]);
        assert_eq!(
            chip_label(false, PromptAgent::Claude, &[PromptAgent::Codex]),
            "Coordinate"
        );
        assert_eq!(
            chip_label(true, PromptAgent::Claude, &[]),
            "Agents"
        );
        assert_eq!(chip_label(true, PromptAgent::Claude, &[PromptAgent::Codex]), "Codex");
        // Dragent's own agent leads; the coding agents chosen do the work.
        assert!(team_problem(PromptAgent::Dragent, "Build it", false).is_none());
        assert_eq!(
            roster(
                PromptAgent::Dragent,
                &[PromptAgent::Codex, PromptAgent::Claude]
            ),
            ["codex", "claude"]
        );
        assert_eq!(
            chip_label(true, PromptAgent::Dragent, &[PromptAgent::Codex]),
            "Codex"
        );
        assert!(team_problem(PromptAgent::Claude, "Build it", true).is_none());
        assert!(team_problem(PromptAgent::Dragent, "Build it", true).is_some());
        assert!(team_problem(PromptAgent::Claude, "  ", false).is_some());
        assert!(team_problem(PromptAgent::Claude, "Build it", false).is_none());
    }
}
