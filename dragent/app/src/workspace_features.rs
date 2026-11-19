//! Release gates for the Agentic Workspace.

/// Whether experimental Shape, Coordinate, and Review layouts may be used.
///
/// The persisted preference is the product-facing flag. The Cargo feature and
/// environment override are intentionally available for development and
/// automated testing without rewriting a user's settings file.
pub fn agentic_workspace_enabled(persisted_opt_in: bool) -> bool {
    cfg!(feature = "agentic-workspace")
        || persisted_opt_in
        || std::env::var("DRAGENT_AGENTIC_WORKSPACE")
            .ok()
            .is_some_and(|value| matches!(value.as_str(), "1" | "true" | "TRUE" | "yes" | "YES"))
}

/// Resolve the presentation at the release boundary. This keeps recovery and
/// live event handling consistent: a completed Coordinate run reopens in
/// Review, while disabling the experiment always returns to Develop.
pub fn effective_workspace_layout(
    persisted: dragent_workspace_model::BuiltInLayout,
    work_status: Option<dragent_workspace_model::WorkStatus>,
    enabled: bool,
) -> dragent_workspace_model::BuiltInLayout {
    use dragent_workspace_model::{BuiltInLayout, WorkStatus};
    if !enabled {
        BuiltInLayout::Develop
    } else if persisted == BuiltInLayout::Coordinate && work_status == Some(WorkStatus::Completed) {
        BuiltInLayout::Review
    } else {
        persisted
    }
}

#[cfg(test)]
mod tests {
    use super::{agentic_workspace_enabled, effective_workspace_layout};
    use dragent_workspace_model::{BuiltInLayout, WorkStatus};

    #[test]
    fn persisted_opt_in_enables_workspace() {
        assert!(agentic_workspace_enabled(true));
    }

    #[test]
    fn disabled_experiment_always_returns_to_develop() {
        assert_eq!(
            effective_workspace_layout(
                BuiltInLayout::Coordinate,
                Some(WorkStatus::Executing),
                false
            ),
            BuiltInLayout::Develop
        );
    }

    #[test]
    fn completed_coordinate_run_recovers_into_review() {
        assert_eq!(
            effective_workspace_layout(
                BuiltInLayout::Coordinate,
                Some(WorkStatus::Completed),
                true
            ),
            BuiltInLayout::Review
        );
    }
}
