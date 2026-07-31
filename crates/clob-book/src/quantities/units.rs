//! The five quantity newtypes.
//!
//! Each is a `#[repr(transparent)]` wrapper over `u64`. Wrapping them costs nothing at
//! runtime and makes the one bug class that matters here — passing base lots where
//! quote lots were expected — a compile error instead of a silent accounting hole.

use bytemuck::{Pod, Zeroable};

/// One cold panic site shared by every checked operator, so the SBF binary carries a
/// single panic string rather than one per call site.
#[cold]
#[inline(never)]
fn overflow() -> ! {
    panic!("clob-book: quantity arithmetic overflow")
}

/// Declares a `u64` newtype with checked arithmetic and `Pod` support.
macro_rules! quantity {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        ///
        /// Operators are checked and panic on overflow, which on-chain is a transaction
        /// revert — the correct failure mode for a venue holding user funds. Use the
        /// `checked_*` methods to handle saturation explicitly.
        #[repr(transparent)]
        #[derive(Copy, Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(pub u64);

        // SAFETY: `repr(transparent)` over `u64`. No padding, no invalid bit patterns,
        // no interior mutability, no `Drop`.
        unsafe impl Zeroable for $name {}
        unsafe impl Pod for $name {}

        impl $name {
            #[doc = concat!("Zero ", stringify!($name), ".")]
            pub const ZERO: Self = Self(0);
            #[doc = concat!("The largest representable ", stringify!($name), ".")]
            pub const MAX: Self = Self(u64::MAX);

            /// Wraps a raw `u64`.
            #[inline(always)]
            pub const fn new(value: u64) -> Self {
                Self(value)
            }

            /// Unwraps to the raw `u64`.
            #[inline(always)]
            pub const fn as_u64(self) -> u64 {
                self.0
            }

            /// Whether this is [`Self::ZERO`].
            #[inline(always)]
            pub const fn is_zero(self) -> bool {
                self.0 == 0
            }

            /// Addition, `None` on overflow.
            #[inline(always)]
            pub const fn checked_add(self, rhs: Self) -> Option<Self> {
                match self.0.checked_add(rhs.0) {
                    Some(v) => Some(Self(v)),
                    None => None,
                }
            }

            /// Subtraction, `None` on underflow.
            #[inline(always)]
            pub const fn checked_sub(self, rhs: Self) -> Option<Self> {
                match self.0.checked_sub(rhs.0) {
                    Some(v) => Some(Self(v)),
                    None => None,
                }
            }

            /// Subtraction clamped at [`Self::ZERO`].
            #[inline(always)]
            pub const fn saturating_sub(self, rhs: Self) -> Self {
                Self(self.0.saturating_sub(rhs.0))
            }

            /// Addition clamped at [`Self::MAX`].
            #[inline(always)]
            pub const fn saturating_add(self, rhs: Self) -> Self {
                Self(self.0.saturating_add(rhs.0))
            }

            /// The smaller of two values.
            #[inline(always)]
            pub const fn min(self, rhs: Self) -> Self {
                if self.0 < rhs.0 { self } else { rhs }
            }

            /// The larger of two values.
            #[inline(always)]
            pub const fn max(self, rhs: Self) -> Self {
                if self.0 > rhs.0 { self } else { rhs }
            }
        }

        impl core::ops::Add for $name {
            type Output = Self;
            #[inline(always)]
            fn add(self, rhs: Self) -> Self {
                match self.0.checked_add(rhs.0) {
                    Some(v) => Self(v),
                    None => overflow(),
                }
            }
        }

        impl core::ops::Sub for $name {
            type Output = Self;
            #[inline(always)]
            fn sub(self, rhs: Self) -> Self {
                match self.0.checked_sub(rhs.0) {
                    Some(v) => Self(v),
                    None => overflow(),
                }
            }
        }

        impl core::ops::AddAssign for $name {
            #[inline(always)]
            fn add_assign(&mut self, rhs: Self) {
                *self = *self + rhs;
            }
        }

        impl core::ops::SubAssign for $name {
            #[inline(always)]
            fn sub_assign(&mut self, rhs: Self) {
                *self = *self - rhs;
            }
        }

        impl core::ops::Mul<u64> for $name {
            type Output = Self;
            #[inline(always)]
            fn mul(self, rhs: u64) -> Self {
                match self.0.checked_mul(rhs) {
                    Some(v) => Self(v),
                    None => overflow(),
                }
            }
        }

        impl core::fmt::Display for $name {
            fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                core::fmt::Display::fmt(&self.0, f)
            }
        }

        impl From<u64> for $name {
            #[inline(always)]
            fn from(value: u64) -> Self {
                Self(value)
            }
        }

        impl From<$name> for u64 {
            #[inline(always)]
            fn from(value: $name) -> u64 {
                value.0
            }
        }
    };
}

quantity! {
    /// A price, in ticks.
    Ticks
}

quantity! {
    /// A size, in base lots.
    BaseLots
}

quantity! {
    /// A size or value, in quote lots.
    QuoteLots
}

quantity! {
    /// A raw base-token amount, in the mint's smallest unit.
    BaseAtoms
}

quantity! {
    /// A raw quote-token amount, in the mint's smallest unit.
    QuoteAtoms
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn operators_panic_rather_than_wrap() {
        assert!(std::panic::catch_unwind(|| BaseLots::MAX + BaseLots(1)).is_err());
        assert!(std::panic::catch_unwind(|| BaseLots::ZERO - BaseLots(1)).is_err());
    }

    #[test]
    fn checked_and_saturating_variants_do_not_panic() {
        assert_eq!(BaseLots::MAX.checked_add(BaseLots(1)), None);
        assert_eq!(BaseLots::ZERO.checked_sub(BaseLots(1)), None);
        assert_eq!(BaseLots::ZERO.saturating_sub(BaseLots(1)), BaseLots::ZERO);
        assert_eq!(BaseLots::MAX.saturating_add(BaseLots(1)), BaseLots::MAX);
    }

    #[test]
    fn newtypes_are_byte_identical_to_u64() {
        assert_eq!(core::mem::size_of::<Ticks>(), 8);
        assert_eq!(bytemuck::bytes_of(&Ticks(1)), &1u64.to_ne_bytes());
    }
}
