//! Prefix-filtered command history navigation (type `git ` + ↑).

/// Indices into `history` (oldest → newest) for entries matching `prefix`.
/// Most-recent match is last in the returned vec.
pub fn matching_history_indices(history: &[String], prefix: &str) -> Vec<usize> {
    if history.is_empty() {
        return Vec::new();
    }
    if prefix.is_empty() {
        return (0..history.len()).collect();
    }
    history
        .iter()
        .enumerate()
        .filter(|(_, cmd)| cmd.starts_with(prefix))
        .map(|(i, _)| i)
        .collect()
}

/// Navigate prefix-filtered history.
///
/// `current` is the index into `history` last shown, or `None` when starting from
/// the live buffer. Returns `(history_index, command_text)`.
pub fn step_prefixed_history(
    history: &[String],
    prefix: &str,
    current: Option<usize>,
    previous: bool,
) -> Option<(usize, String)> {
    let matches = matching_history_indices(history, prefix);
    if matches.is_empty() {
        return None;
    }

    let pos = match current {
        None => {
            if previous {
                matches.len() - 1
            } else {
                return None;
            }
        }
        Some(idx) => {
            let match_pos = matches.iter().position(|&i| i == idx)?;
            if previous {
                match_pos.checked_sub(1)?
            } else if match_pos + 1 >= matches.len() {
                return None;
            } else {
                match_pos + 1
            }
        }
    };

    let history_idx = matches[pos];
    Some((history_idx, history[history_idx].clone()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hist() -> Vec<String> {
        vec![
            "ls".into(),
            "git status".into(),
            "git commit".into(),
            "cargo build".into(),
        ]
    }

    #[test]
    fn filters_by_prefix() {
        let h = hist();
        let m = matching_history_indices(&h, "git ");
        assert_eq!(m, vec![1, 2]);
    }

    #[test]
    fn steps_backward_through_prefix_matches() {
        let h = hist();
        let (i1, c1) = step_prefixed_history(&h, "git ", None, true).unwrap();
        assert_eq!(c1, "git commit");
        let (i2, c2) = step_prefixed_history(&h, "git ", Some(i1), true).unwrap();
        assert_eq!(c2, "git status");
        assert!(step_prefixed_history(&h, "git ", Some(i2), true).is_none());
    }

    #[test]
    fn steps_forward_through_prefix_matches() {
        let h = hist();
        let (i1, c1) = step_prefixed_history(&h, "git ", None, true).unwrap();
        assert_eq!(c1, "git commit");
        let (i2, c2) = step_prefixed_history(&h, "git ", Some(i1), true).unwrap();
        assert_eq!(c2, "git status");
        let (_, c3) = step_prefixed_history(&h, "git ", Some(i2), false).unwrap();
        assert_eq!(c3, "git commit");
        assert!(step_prefixed_history(&h, "git ", Some(i1), false).is_none());
    }
}
