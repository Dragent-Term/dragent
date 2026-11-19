pub mod context;
pub mod heuristic;

use crate::agent::detected_input_type::DetectedInputType;
pub use context::ClassifyContext;

pub trait InputClassifier {
    /// Classify buffer text using session context.
    ///
    /// Returns `None` when the buffer is empty — callers should leave the current
    /// input type unchanged (do not reset to Shell on clear).
    fn classify(&self, text: &str, ctx: &ClassifyContext) -> Option<DetectedInputType>;
}
