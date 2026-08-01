//! The knobs, and the ones that are not free.
//!
//! Most of these are taste: how wide to quote, how deep, how much per level. Two are
//! not, and [`Params::validate`] enforces them:
//!
//! - **The ladder must not be able to cross.** Inventory shifts the whole ladder, so if
//!   it could shift further than the half-spread, a long enough position would push the
//!   bid above the ask. Requiring `max_skew_in_ticks < half_spread_in_ticks` makes that
//!   unreachable rather than unlikely. It is the precondition every no-cross claim in
//!   [`ladder`](crate::ladder) rests on.
//! - **The ladder must fit in a transaction.** A refresh is one `BatchUpdate`, and a
//!   configuration whose refresh cannot be sent is one that fails at the worst moment
//!   rather than at startup.
//!
//! Checking both here means the rest of the crate can do arithmetic without asking
//! whether its inputs make sense.

/// Levels per side the ladder may have.
///
/// A refresh cancels and replaces both sides in one `BatchUpdate`, which costs roughly
/// 72 bytes per level: two 16-byte order IDs to cancel and two 20-byte post-only packets
/// to place. A Solana transaction is 1,232 bytes and the signature, header, accounts and
/// blockhash take about 205 of them, leaving room for around fourteen. Twelve is that
/// with the margin a wire format needs when it changes.
pub const MAX_LEVELS: u8 = 12;

/// What the bot is allowed to quote.
///
/// Prices are in ticks and sizes in base lots — the market's own units, so nothing here
/// needs the lot geometry to be interpreted. A tick is whatever the market was created
/// with; see [`LotConfig`](clob_book::LotConfig).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Params {
    /// What the bot believes the asset is worth, in ticks.
    ///
    /// The anchor for a book that cannot supply a price of its own. On a market with
    /// other participants it is a starting point that the book overrides; on an empty
    /// one it is the whole opinion, and the bot's quotes become the market's price.
    pub reference_in_ticks: u64,

    /// Distance from fair to the first quote on each side, in ticks.
    ///
    /// The quoted spread is twice this. It is the bot's compensation for standing
    /// ready to trade, and the reason it can be adversely selected and still profit.
    pub half_spread_in_ticks: u64,

    /// Ticks between consecutive levels on the same side.
    pub level_step_in_ticks: u64,

    /// Levels per side. At most [`MAX_LEVELS`].
    pub levels: u8,

    /// Size of each level, in base lots.
    pub size_in_base_lots: u64,

    /// Base lots the bot would like to be holding.
    ///
    /// A maker's inventory is a position it did not choose — it accumulates whatever the
    /// market sold it. Naming a target is what makes that position something to steer
    /// back to rather than something to watch grow.
    pub target_base_lots: u64,

    /// How far from [`target_base_lots`](Self::target_base_lots) the position may drift
    /// before the side that would widen it stops quoting.
    ///
    /// Also the scale over which inventory skew reaches its maximum, so at the limit the
    /// ladder is fully leaned and the offending side is switched off. One number, because
    /// two would let the skew saturate somewhere the position cap does not care about.
    pub inventory_limit_lots: u64,

    /// The most inventory may shift the ladder, in ticks.
    ///
    /// Must be strictly less than [`half_spread_in_ticks`](Self::half_spread_in_ticks).
    pub max_skew_in_ticks: u64,

    /// How far a resting quote may sit from where it now belongs before the bot pays for
    /// a refresh, in ticks.
    ///
    /// Zero means re-quote on any movement at all, which on a live market means a
    /// transaction per block. The point of the tolerance is that a maker's edge is the
    /// spread, and fees spent chasing sub-tick precision come out of it.
    pub drift_tolerance_in_ticks: u64,
}

/// A configuration the bot refuses to run with.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ParamsError {
    /// No levels, so nothing would ever be quoted.
    NoLevels,
    /// More levels than a single refresh transaction can carry.
    TooManyLevels {
        /// Levels asked for.
        asked: u8,
        /// Levels allowed.
        allowed: u8,
    },
    /// A level of zero size, which is not an order.
    ZeroSize,
    /// A zero half-spread, which quotes both sides at the same price.
    ZeroSpread,
    /// Inventory could shift the ladder far enough to cross it.
    SkewExceedsSpread {
        /// The skew asked for.
        max_skew_in_ticks: u64,
        /// The half-spread it must stay under.
        half_spread_in_ticks: u64,
    },
    /// A zero inventory limit, which no position can be measured against.
    ZeroInventoryLimit,
    /// A reference price of zero, which is not a price.
    ZeroReference,
}

impl core::fmt::Display for ParamsError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::NoLevels => write!(f, "levels must be at least 1"),
            Self::TooManyLevels { asked, allowed } => write!(
                f,
                "{asked} levels per side will not fit in one refresh transaction; the most is {allowed}"
            ),
            Self::ZeroSize => write!(f, "size must be at least 1 base lot"),
            Self::ZeroSpread => write!(f, "half-spread must be at least 1 tick, or the bid and the ask are the same price"),
            Self::SkewExceedsSpread { max_skew_in_ticks, half_spread_in_ticks } => write!(
                f,
                "a skew of {max_skew_in_ticks} ticks can shift the ladder past a half-spread of \
                 {half_spread_in_ticks}, which would cross the bid above the ask"
            ),
            Self::ZeroInventoryLimit => write!(f, "inventory limit must be at least 1 base lot"),
            Self::ZeroReference => write!(f, "reference price must be at least 1 tick"),
        }
    }
}

impl std::error::Error for ParamsError {}

impl Params {
    /// Rejects any configuration the rest of the crate would have to defend against.
    ///
    /// # Errors
    ///
    /// The first [`ParamsError`] found.
    pub fn validate(&self) -> Result<(), ParamsError> {
        if self.levels == 0 {
            return Err(ParamsError::NoLevels);
        }
        if self.levels > MAX_LEVELS {
            return Err(ParamsError::TooManyLevels {
                asked: self.levels,
                allowed: MAX_LEVELS,
            });
        }
        if self.size_in_base_lots == 0 {
            return Err(ParamsError::ZeroSize);
        }
        if self.half_spread_in_ticks == 0 {
            return Err(ParamsError::ZeroSpread);
        }
        // The invariant the whole no-cross argument rests on. With the ladder centred at
        // `fair + skew`, the best bid is `centre - half_spread` and the best ask is
        // `centre + half_spread`; both stay on their own side of fair exactly as long as
        // the skew cannot reach the half-spread.
        if self.max_skew_in_ticks >= self.half_spread_in_ticks {
            return Err(ParamsError::SkewExceedsSpread {
                max_skew_in_ticks: self.max_skew_in_ticks,
                half_spread_in_ticks: self.half_spread_in_ticks,
            });
        }
        if self.inventory_limit_lots == 0 {
            return Err(ParamsError::ZeroInventoryLimit);
        }
        if self.reference_in_ticks == 0 {
            return Err(ParamsError::ZeroReference);
        }
        Ok(())
    }

    /// Quotes the bot wants resting when nothing is in its way: both sides, every level.
    pub fn full_ladder_size(&self) -> usize {
        self.levels as usize * 2
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A configuration that passes, for tests to break one field at a time.
    fn valid() -> Params {
        Params {
            reference_in_ticks: 150_000,
            half_spread_in_ticks: 50,
            level_step_in_ticks: 25,
            levels: 3,
            size_in_base_lots: 100,
            target_base_lots: 10_000,
            inventory_limit_lots: 5_000,
            max_skew_in_ticks: 20,
            drift_tolerance_in_ticks: 10,
        }
    }

    #[test]
    fn the_baseline_configuration_is_accepted() {
        assert_eq!(valid().validate(), Ok(()));
    }

    #[test]
    fn a_skew_that_could_cross_the_ladder_is_refused() {
        // Equal, not greater: at exactly a half-spread the bid lands on fair and the ask
        // lands on fair, which is a locked book rather than a quote.
        let params = Params {
            half_spread_in_ticks: 20,
            max_skew_in_ticks: 20,
            ..valid()
        };
        assert!(matches!(
            params.validate(),
            Err(ParamsError::SkewExceedsSpread { .. })
        ));
    }

    #[test]
    fn a_skew_just_under_the_half_spread_is_allowed() {
        let params = Params {
            half_spread_in_ticks: 20,
            max_skew_in_ticks: 19,
            ..valid()
        };
        assert_eq!(params.validate(), Ok(()));
    }

    #[test]
    fn a_ladder_too_big_to_send_is_refused() {
        let params = Params {
            levels: MAX_LEVELS + 1,
            ..valid()
        };
        assert_eq!(
            params.validate(),
            Err(ParamsError::TooManyLevels {
                asked: MAX_LEVELS + 1,
                allowed: MAX_LEVELS,
            })
        );
    }

    #[test]
    fn the_largest_allowed_ladder_is_allowed() {
        let params = Params {
            levels: MAX_LEVELS,
            ..valid()
        };
        assert_eq!(params.validate(), Ok(()));
        assert_eq!(params.full_ladder_size(), MAX_LEVELS as usize * 2);
    }

    #[test]
    fn every_degenerate_field_is_named_in_its_own_error() {
        // Each of these would otherwise surface as a divide by zero, an empty ladder, or
        // a pair of orders at one price — all of them harder to read than a message.
        let cases = [
            (Params { levels: 0, ..valid() }, ParamsError::NoLevels),
            (Params { size_in_base_lots: 0, ..valid() }, ParamsError::ZeroSize),
            (
                Params { half_spread_in_ticks: 0, max_skew_in_ticks: 0, ..valid() },
                ParamsError::ZeroSpread,
            ),
            (
                Params { inventory_limit_lots: 0, ..valid() },
                ParamsError::ZeroInventoryLimit,
            ),
            (Params { reference_in_ticks: 0, ..valid() }, ParamsError::ZeroReference),
        ];
        for (params, expected) in cases {
            assert_eq!(params.validate(), Err(expected), "{params:?}");
        }
    }

    #[test]
    fn a_zero_half_spread_is_reported_as_a_spread_problem_not_a_skew_one() {
        // Both are zero, so both invariants are technically violated. The spread is the
        // one the operator has to change, so it is the one named.
        let params = Params {
            half_spread_in_ticks: 0,
            max_skew_in_ticks: 0,
            ..valid()
        };
        assert_eq!(params.validate(), Err(ParamsError::ZeroSpread));
    }
}
