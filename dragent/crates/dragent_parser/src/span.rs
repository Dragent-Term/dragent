//! Source location types: `Span`, `Spanned<T>`, `HasSpan`, `SpannedItem`.
//!
//! Architecture inspired by nushell's nu-protocol crate (MIT license):
//! https://github.com/nushell/nushell/tree/main/crates/nu-protocol/src/span.rs

use std::ops::Deref;

/// A half-open byte-offset range `[start, end)` into a source string.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

impl Span {
    #[inline]
    pub fn new(start: usize, end: usize) -> Self {
        debug_assert!(start <= end, "Span::new: start ({start}) > end ({end})");
        Self { start, end }
    }

    /// A span that carries no source location (synthesized nodes, default args).
    #[inline]
    pub fn unknown() -> Self {
        Self { start: 0, end: 0 }
    }

    #[inline]
    pub fn is_unknown(self) -> bool {
        self.start == 0 && self.end == 0
    }

    /// Smallest span that covers both `self` and `other`.
    #[inline]
    pub fn until(self, other: Span) -> Span {
        Span {
            start: self.start.min(other.start),
            end: self.end.max(other.end),
        }
    }

    /// Extract the source text covered by this span.
    ///
    /// Clamps to `source.len()` so a span that was built from one string can be
    /// used safely with a substring.
    pub fn slice<'a>(&self, source: &'a str) -> &'a str {
        let end = self.end.min(source.len());
        let start = self.start.min(end);
        &source[start..end]
    }

    /// Number of bytes covered.
    #[inline]
    pub fn len(self) -> usize {
        self.end.saturating_sub(self.start)
    }

    #[inline]
    pub fn is_empty(self) -> bool {
        self.end <= self.start
    }

    /// Smallest span covering all spans in `spans`.  Returns `unknown()` for an
    /// empty slice.
    pub fn from_list(spans: &[Span]) -> Span {
        spans.iter().copied().fold(Span::unknown(), |acc, s| {
            if acc.is_unknown() {
                s
            } else {
                acc.until(s)
            }
        })
    }
}

impl From<(usize, usize)> for Span {
    fn from((start, end): (usize, usize)) -> Self {
        Self::new(start, end)
    }
}

impl From<std::ops::Range<usize>> for Span {
    fn from(r: std::ops::Range<usize>) -> Self {
        Self::new(r.start, r.end)
    }
}

impl From<Span> for std::ops::Range<usize> {
    fn from(s: Span) -> Self {
        s.start..s.end
    }
}

// ── Spanned<T> ────────────────────────────────────────────────────────────────

/// A value of type `T` tagged with its source location.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Spanned<T> {
    pub item: T,
    pub span: Span,
}

impl<T> Spanned<T> {
    pub fn new(item: T, span: Span) -> Self {
        Self { item, span }
    }

    pub fn map<U>(self, f: impl FnOnce(T) -> U) -> Spanned<U> {
        Spanned {
            item: f(self.item),
            span: self.span,
        }
    }
}

impl<T> Deref for Spanned<T> {
    type Target = T;
    fn deref(&self) -> &Self::Target {
        &self.item
    }
}

// ── HasSpan ───────────────────────────────────────────────────────────────────

/// Implemented by all parse-tree nodes that carry a source location.
pub trait HasSpan {
    fn span(&self) -> Span;
}

impl HasSpan for Span {
    fn span(&self) -> Span {
        *self
    }
}

impl<T> HasSpan for Spanned<T> {
    fn span(&self) -> Span {
        self.span
    }
}

// ── SpannedItem ───────────────────────────────────────────────────────────────

/// Extension trait: lets any `Sized` value call `.spanned(span)` to wrap itself.
pub trait SpannedItem: Sized {
    fn spanned(self, span: impl Into<Span>) -> Spanned<Self> {
        Spanned::new(self, span.into())
    }

    fn spanned_unknown(self) -> Spanned<Self> {
        Spanned::new(self, Span::unknown())
    }
}

impl<T: Sized> SpannedItem for T {}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn until_covers_both() {
        let a = Span::new(1, 5);
        let b = Span::new(3, 8);
        assert_eq!(a.until(b), Span::new(1, 8));
    }

    #[test]
    fn until_is_commutative() {
        let a = Span::new(2, 4);
        let b = Span::new(1, 10);
        assert_eq!(a.until(b), b.until(a));
    }

    #[test]
    fn slice_extracts_text() {
        let src = "hello world";
        assert_eq!(Span::new(6, 11).slice(src), "world");
    }

    #[test]
    fn slice_clamps_to_source_len() {
        let src = "hi";
        assert_eq!(Span::new(0, 100).slice(src), "hi");
    }

    #[test]
    fn from_list_covers_all() {
        let spans = vec![Span::new(2, 4), Span::new(6, 10)];
        assert_eq!(Span::from_list(&spans), Span::new(2, 10));
    }

    #[test]
    fn from_list_empty_is_unknown() {
        assert_eq!(Span::from_list(&[]), Span::unknown());
    }

    #[test]
    fn spanned_item_trait() {
        let s = "hello".spanned(Span::new(0, 5));
        assert_eq!(*s, "hello");
        assert_eq!(s.span, Span::new(0, 5));
    }

    #[test]
    fn spanned_map_preserves_span() {
        let s = 42u32.spanned(Span::new(1, 3));
        let mapped = s.map(|n| n as u64 * 2);
        assert_eq!(mapped.item, 84u64);
        assert_eq!(mapped.span, Span::new(1, 3));
    }

    #[test]
    fn has_span_for_spanned() {
        let s = "x".spanned(Span::new(5, 6));
        assert_eq!(s.span(), Span::new(5, 6));
    }
}
