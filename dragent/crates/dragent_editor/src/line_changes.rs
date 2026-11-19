/// Unsaved source-line state rendered in the editor gutter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineChangeKind {
    Added,
    Modified,
    Deleted,
}
