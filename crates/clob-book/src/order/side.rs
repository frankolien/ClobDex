//! Which half of the book an order rests on.

/// The side of the book an order rests on.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum Side {
    /// A resting buy. The best bid is the *maximum* of the bid tree.
    Bid = 0,
    /// A resting sell. The best ask is the *minimum* of the ask tree.
    Ask = 1,
}

impl Side {
    /// The side an incoming order must take to cross against this one.
    #[inline(always)]
    pub const fn opposite(self) -> Self {
        match self {
            Self::Bid => Self::Ask,
            Self::Ask => Self::Bid,
        }
    }

    /// Whether this is [`Side::Bid`].
    #[inline(always)]
    pub const fn is_bid(self) -> bool {
        matches!(self, Self::Bid)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opposite_is_an_involution() {
        assert_eq!(Side::Bid.opposite(), Side::Ask);
        assert_eq!(Side::Ask.opposite().opposite(), Side::Ask);
    }
}
