//! Byte and character offset newtypes for text layout.
//!
//! MIT-licensed original implementation for `warpui_core`.

use std::ops::{Add, AddAssign, Range, Sub, SubAssign};

/// An offset within a piece of text, in terms of Unicode scalar values (`char`).
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct CharOffset(usize);

/// An offset within a piece of text, in terms of bytes.
#[derive(Clone, Copy, Debug, Default, Hash, Eq, Ord, PartialEq, PartialOrd)]
pub struct ByteOffset(usize);

macro_rules! impl_offset {
    ($typ:ty) => {
        impl $typ {
            pub const fn zero() -> Self {
                Self(0)
            }

            pub fn as_usize(self) -> usize {
                self.0
            }

            pub fn empty_range(self) -> Range<Self> {
                self..self
            }

            pub fn add_signed(self, rhs: isize) -> Self {
                let (inner, overflow) = self.0.overflowing_add_signed(rhs);
                debug_assert!(!overflow, "arithmetic overflow");
                Self(inner)
            }

            pub fn range(range: Range<usize>) -> Range<Self> {
                range.start.into()..range.end.into()
            }
        }

        impl From<usize> for $typ {
            fn from(value: usize) -> Self {
                Self(value)
            }
        }

        impl AddAssign for $typ {
            fn add_assign(&mut self, rhs: Self) {
                self.0 += rhs.0;
            }
        }

        impl AddAssign<usize> for $typ {
            fn add_assign(&mut self, rhs: usize) {
                self.0 += rhs;
            }
        }

        impl SubAssign for $typ {
            fn sub_assign(&mut self, rhs: Self) {
                self.0 -= rhs.0;
            }
        }

        impl SubAssign<usize> for $typ {
            fn sub_assign(&mut self, rhs: usize) {
                self.0 -= rhs;
            }
        }

        impl Add for $typ {
            type Output = Self;

            fn add(self, rhs: Self) -> Self::Output {
                Self(self.0 + rhs.0)
            }
        }

        impl Add<usize> for $typ {
            type Output = Self;

            fn add(self, rhs: usize) -> Self::Output {
                Self(self.0 + rhs)
            }
        }

        impl Sub for $typ {
            type Output = Self;

            fn sub(self, rhs: Self) -> Self::Output {
                Self(self.0 - rhs.0)
            }
        }

        impl Sub<usize> for $typ {
            type Output = Self;

            fn sub(self, rhs: usize) -> Self::Output {
                Self(self.0 - rhs)
            }
        }

        impl num_traits::SaturatingSub for $typ {
            #[inline]
            fn saturating_sub(&self, rhs: &Self) -> Self {
                Self(self.0.saturating_sub(rhs.0))
            }
        }

        impl std::fmt::Display for $typ {
            fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                self.0.fmt(f)
            }
        }
    };
}

impl_offset!(CharOffset);
impl_offset!(ByteOffset);

impl AddAssign<i32> for CharOffset {
    fn add_assign(&mut self, rhs: i32) {
        if rhs.is_positive() {
            self.0 += rhs as usize;
        } else {
            self.0 -= rhs.unsigned_abs() as usize;
        }
    }
}

/// Counts characters while scanning a string left-to-right.
pub struct CharCounter<'a> {
    current_offset: CharOffset,
    char_indices: std::str::CharIndices<'a>,
}

impl<'a> CharCounter<'a> {
    pub fn new(str: &'a str) -> Self {
        Self {
            char_indices: str.char_indices(),
            current_offset: CharOffset::zero(),
        }
    }

    pub fn char_offset(&mut self, byte_offset: impl Into<ByteOffset>) -> Option<CharOffset> {
        let byte_offset = byte_offset.into();
        if self.char_indices.offset() > byte_offset.as_usize() {
            None
        } else {
            for (next_byte_offset, _) in self.char_indices.by_ref() {
                self.current_offset += 1;
                if next_byte_offset == byte_offset.as_usize() {
                    return Some(self.current_offset - 1);
                }
            }
            None
        }
    }
}
