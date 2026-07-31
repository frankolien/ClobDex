//! A market's lot/tick geometry, and the conversions between price, size, and value.
//!
//! # The exactness invariant
//!
//! The quote value of a fill is
//!
//! ```text
//! quote_lots = price_in_ticks * tick_size_in_quote_lots_per_base_unit * base_lots
//!              / base_lots_per_base_unit
//! ```
//!
//! If that division truncates, the venue keeps dust on every fill and conservation of
//! funds fails. [`LotConfig::new`] therefore rejects any market whose tick size is not a
//! multiple of `base_lots_per_base_unit`, which folds the division into a constant
//! ([`quote_lots_per_base_lot_per_tick`](LotConfig::quote_lots_per_base_lot_per_tick))
//! and makes every fill exact for any size.
//!
//! The cost is a restricted set of admissible tick sizes. Phoenix takes the other
//! branch with an `AdjustedQuoteLots` intermediate that defers the division — more
//! flexible, harder to audit.

use bytemuck::{Pod, Zeroable};

use super::units::{BaseAtoms, BaseLots, QuoteAtoms, QuoteLots, Ticks};

/// Why a [`LotConfig`] was rejected.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum LotConfigError {
    /// `base_lots_per_base_unit` was zero.
    ZeroBaseLotsPerBaseUnit,
    /// `tick_size_in_quote_lots_per_base_unit` was zero.
    ZeroTickSize,
    /// `base_atoms_per_base_lot` was zero.
    ZeroBaseAtomsPerBaseLot,
    /// `quote_atoms_per_quote_lot` was zero.
    ZeroQuoteAtomsPerQuoteLot,
    /// Tick size is not a multiple of `base_lots_per_base_unit`, so some fills would
    /// truncate. See the [module docs](self#the-exactness-invariant).
    TickSizeNotDivisibleByBaseLotsPerBaseUnit,
}

impl core::fmt::Display for LotConfigError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            Self::ZeroBaseLotsPerBaseUnit => "base_lots_per_base_unit must be non-zero",
            Self::ZeroTickSize => "tick_size_in_quote_lots_per_base_unit must be non-zero",
            Self::ZeroBaseAtomsPerBaseLot => "base_atoms_per_base_lot must be non-zero",
            Self::ZeroQuoteAtomsPerQuoteLot => "quote_atoms_per_quote_lot must be non-zero",
            Self::TickSizeNotDivisibleByBaseLotsPerBaseUnit => {
                "tick size must be a multiple of base_lots_per_base_unit"
            }
        })
    }
}

#[cfg(feature = "std")]
impl std::error::Error for LotConfigError {}

/// The immutable lot/tick geometry of one market.
///
/// `Pod`, so it can live directly in the market account header. A raw cast bypasses
/// [`LotConfig::new`], so code that reads a config out of account bytes must call
/// [`LotConfig::validate`] once rather than trusting the bytes.
#[repr(C)]
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct LotConfig {
    /// Base lots per whole base unit (`10^base_decimals` atoms). For a SOL market with
    /// a 0.001 SOL minimum size, this is `1_000`.
    pub base_lots_per_base_unit: u64,
    /// One tick, in quote lots per base unit. Must be a multiple of
    /// `base_lots_per_base_unit`.
    pub tick_size_in_quote_lots_per_base_unit: u64,
    /// Base-token atoms per base lot.
    pub base_atoms_per_base_lot: u64,
    /// Quote-token atoms per quote lot.
    pub quote_atoms_per_quote_lot: u64,
}

// SAFETY: `repr(C)` over four `u64`s. Size 32, align 8, no padding.
unsafe impl Zeroable for LotConfig {}
unsafe impl Pod for LotConfig {}

impl LotConfig {
    /// Builds and validates a market's geometry.
    ///
    /// # Errors
    ///
    /// [`LotConfigError`] if any parameter is zero, or if the tick size violates the
    /// [exactness invariant](self#the-exactness-invariant).
    pub const fn new(
        base_lots_per_base_unit: u64,
        tick_size_in_quote_lots_per_base_unit: u64,
        base_atoms_per_base_lot: u64,
        quote_atoms_per_quote_lot: u64,
    ) -> Result<Self, LotConfigError> {
        let config = Self {
            base_lots_per_base_unit,
            tick_size_in_quote_lots_per_base_unit,
            base_atoms_per_base_lot,
            quote_atoms_per_quote_lot,
        };
        match config.validate() {
            Ok(()) => Ok(config),
            Err(error) => Err(error),
        }
    }

    /// Re-checks every invariant. Required after casting a config out of account bytes.
    ///
    /// # Errors
    ///
    /// The first [`LotConfigError`] found.
    pub const fn validate(&self) -> Result<(), LotConfigError> {
        if self.base_lots_per_base_unit == 0 {
            return Err(LotConfigError::ZeroBaseLotsPerBaseUnit);
        }
        if self.tick_size_in_quote_lots_per_base_unit == 0 {
            return Err(LotConfigError::ZeroTickSize);
        }
        if self.base_atoms_per_base_lot == 0 {
            return Err(LotConfigError::ZeroBaseAtomsPerBaseLot);
        }
        if self.quote_atoms_per_quote_lot == 0 {
            return Err(LotConfigError::ZeroQuoteAtomsPerQuoteLot);
        }
        if self.tick_size_in_quote_lots_per_base_unit % self.base_lots_per_base_unit != 0 {
            return Err(LotConfigError::TickSizeNotDivisibleByBaseLotsPerBaseUnit);
        }
        Ok(())
    }

    /// The value of one base lot at a price of one tick.
    ///
    /// Exact by the [config invariant](self#the-exactness-invariant), and the only
    /// conversion constant the matching engine needs on the hot path.
    #[inline(always)]
    pub const fn quote_lots_per_base_lot_per_tick(&self) -> u64 {
        self.tick_size_in_quote_lots_per_base_unit / self.base_lots_per_base_unit
    }

    /// The quote value of filling `size` at `price`.
    ///
    /// Returns `None` on overflow rather than panicking: aggregators legitimately probe
    /// absurd sizes, and that should reject the order rather than revert the caller's
    /// whole transaction.
    #[inline]
    pub const fn quote_lots_for(&self, price: Ticks, size: BaseLots) -> Option<QuoteLots> {
        let per_lot = self.quote_lots_per_base_lot_per_tick() as u128;
        let Some(unit_value) = (price.0 as u128).checked_mul(per_lot) else {
            return None;
        };
        let Some(total) = unit_value.checked_mul(size.0 as u128) else {
            return None;
        };
        if total > u64::MAX as u128 {
            return None;
        }
        Some(QuoteLots(total as u64))
    }

    /// The largest whole size buyable at `price` with `budget`.
    ///
    /// Rounds down, so the result always costs at most `budget`. `None` if `price` is
    /// zero (size would be unbounded) or on overflow.
    #[inline]
    pub const fn base_lots_for(&self, price: Ticks, budget: QuoteLots) -> Option<BaseLots> {
        let per_lot = self.quote_lots_per_base_lot_per_tick() as u128;
        let Some(cost_per_base_lot) = (price.0 as u128).checked_mul(per_lot) else {
            return None;
        };
        if cost_per_base_lot == 0 {
            return None;
        }
        let size = (budget.0 as u128) / cost_per_base_lot;
        if size > u64::MAX as u128 {
            return None;
        }
        Some(BaseLots(size as u64))
    }

    /// Base lots to raw atoms. `None` on overflow.
    #[inline]
    pub const fn base_atoms(&self, lots: BaseLots) -> Option<BaseAtoms> {
        match lots.0.checked_mul(self.base_atoms_per_base_lot) {
            Some(value) => Some(BaseAtoms(value)),
            None => None,
        }
    }

    /// Quote lots to raw atoms. `None` on overflow.
    #[inline]
    pub const fn quote_atoms(&self, lots: QuoteLots) -> Option<QuoteAtoms> {
        match lots.0.checked_mul(self.quote_atoms_per_quote_lot) {
            Some(value) => Some(QuoteAtoms(value)),
            None => None,
        }
    }

    /// Raw atoms to base lots, rounding down. Any remainder is dust that stays with the
    /// depositor.
    #[inline]
    pub const fn base_lots_from_atoms(&self, atoms: BaseAtoms) -> BaseLots {
        BaseLots(atoms.0 / self.base_atoms_per_base_lot)
    }

    /// Raw atoms to quote lots, rounding down.
    #[inline]
    pub const fn quote_lots_from_atoms(&self, atoms: QuoteAtoms) -> QuoteLots {
        QuoteLots(atoms.0 / self.quote_atoms_per_quote_lot)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// SOL/USDC: SOL has 9 decimals, USDC has 6. Minimum size 0.001 SOL, price
    /// granularity $0.001.
    fn sol_usdc() -> LotConfig {
        LotConfig::new(1_000, 1_000, 1_000_000, 1).unwrap()
    }

    #[test]
    fn rejects_indivisible_tick_size() {
        assert_eq!(
            LotConfig::new(1_000, 1_500, 1_000_000, 1),
            Err(LotConfigError::TickSizeNotDivisibleByBaseLotsPerBaseUnit)
        );
    }

    #[test]
    fn rejects_zero_parameters() {
        assert_eq!(
            LotConfig::new(0, 1_000, 1, 1),
            Err(LotConfigError::ZeroBaseLotsPerBaseUnit)
        );
        assert_eq!(LotConfig::new(1_000, 0, 1, 1), Err(LotConfigError::ZeroTickSize));
        assert_eq!(
            LotConfig::new(1_000, 1_000, 0, 1),
            Err(LotConfigError::ZeroBaseAtomsPerBaseLot)
        );
        assert_eq!(
            LotConfig::new(1_000, 1_000, 1, 0),
            Err(LotConfigError::ZeroQuoteAtomsPerQuoteLot)
        );
    }

    #[test]
    fn folded_constant_matches_the_unfolded_formula() {
        let config = sol_usdc();
        let (price, size) = (Ticks(187_500), BaseLots(2_500));

        let folded = config.quote_lots_for(price, size).unwrap();
        let unfolded = (price.0 as u128)
            * (config.tick_size_in_quote_lots_per_base_unit as u128)
            * (size.0 as u128)
            / (config.base_lots_per_base_unit as u128);

        assert_eq!(folded.0 as u128, unfolded);
        // 2.5 SOL at $187.50 = $468.75.
        assert_eq!(folded, QuoteLots(468_750_000));
    }

    #[test]
    fn budget_conversion_never_overspends() {
        let config = sol_usdc();
        let price = Ticks(187_500);
        let budget = QuoteLots(187_500 * 2 - 1);

        let size = config.base_lots_for(price, budget).unwrap();
        assert_eq!(size, BaseLots(1));
        assert!(config.quote_lots_for(price, size).unwrap() <= budget);
    }

    #[test]
    fn overflow_and_zero_price_return_none() {
        let config = sol_usdc();
        assert_eq!(config.quote_lots_for(Ticks::MAX, BaseLots::MAX), None);
        assert_eq!(config.base_atoms(BaseLots::MAX), None);
        assert_eq!(config.base_lots_for(Ticks::ZERO, QuoteLots(1)), None);
    }

    #[test]
    fn atom_conversion_rounds_down_to_whole_lots() {
        let config = sol_usdc();
        // The trailing 500 atoms are below one base lot.
        assert_eq!(config.base_lots_from_atoms(BaseAtoms(1_000_000_500)), BaseLots(1_000));
        assert_eq!(config.base_atoms(BaseLots(1_000)), Some(BaseAtoms(1_000_000_000)));
    }
}
