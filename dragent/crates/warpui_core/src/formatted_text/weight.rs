//! Font weight overrides for formatted text.

/// Explicit font weights that are not the default "normal" weight.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum CustomWeight {
    Thin,
    ExtraLight,
    Light,
    Medium,
    Semibold,
    Bold,
    ExtraBold,
    Black,
}

impl CustomWeight {
    pub fn is_at_least_bold(&self) -> bool {
        matches!(
            self,
            CustomWeight::Bold | CustomWeight::ExtraBold | CustomWeight::Black
        )
    }

    pub fn merge_weights(
        first: Option<CustomWeight>,
        second: Option<CustomWeight>,
    ) -> Option<CustomWeight> {
        first.or(second)
    }
}
