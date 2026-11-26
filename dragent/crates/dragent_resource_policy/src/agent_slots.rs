//! The optional cap on how many agents run at once. There is none unless the
//! person sets one; without it only critically low memory holds new agents.

/// Sanity ceiling for a cap.
pub const MAX_AGENTS: usize = 1024;

/// Clamp a configured cap into sense. No cap stays no cap.
pub fn clamp_max_agents(configured: Option<usize>) -> Option<usize> {
    configured.map(|n| n.clamp(1, MAX_AGENTS))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caps_clamp_into_sense_and_none_stays_none() {
        assert_eq!(clamp_max_agents(None), None);
        assert_eq!(clamp_max_agents(Some(0)), Some(1));
        assert_eq!(clamp_max_agents(Some(12)), Some(12));
        assert_eq!(clamp_max_agents(Some(usize::MAX)), Some(MAX_AGENTS));
    }
}
