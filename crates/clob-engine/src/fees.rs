//! Taker fees.
//!
//! Makers pay nothing. That is not generosity: on a venue with no flow, depth is the
//! scarce side of the market, and a maker rebate or even a maker fee changes quoting
//! behaviour far more than a few basis points of taker cost changes routing. Phoenix
//! settled on the same split.
//!
//! Fees accrue in quote lots on the market and are swept separately, so a fill never
//! needs to touch a vault.

use bytemuck::{Pod, Zeroable};
use clob_book::QuoteLots;

use crate::error::{EngineError, Result};

/// One hundred percent, in basis points.
pub const BPS_DENOMINATOR: u64 = 10_000;

/// A market's fee rate.
///
/// Stored as `u64` rather than the `u16` the range needs, so the market header packs
/// without padding and stays castable.
#[repr(C)]
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct FeeSchedule {
    /// Taker fee in basis points of the quote value of each fill.
    pub taker_fee_bps: u64,
}

// SAFETY: repr(C) over a single u64.
unsafe impl Zeroable for FeeSchedule {}
unsafe impl Pod for FeeSchedule {}

impl FeeSchedule {
    /// A market that charges nothing.
    pub const FREE: Self = Self { taker_fee_bps: 0 };

    /// Builds a schedule.
    ///
    /// # Errors
    ///
    /// [`EngineError::InvalidFeeRate`] if the rate exceeds 100%.
    pub const fn new(taker_fee_bps: u64) -> Result<Self> {
        if taker_fee_bps > BPS_DENOMINATOR {
            return Err(EngineError::InvalidFeeRate);
        }
        Ok(Self { taker_fee_bps })
    }

    /// Re-checks the rate after a raw cast out of account bytes.
    ///
    /// # Errors
    ///
    /// [`EngineError::InvalidFeeRate`] if the rate exceeds 100%.
    pub const fn validate(&self) -> Result<()> {
        if self.taker_fee_bps > BPS_DENOMINATOR {
            return Err(EngineError::InvalidFeeRate);
        }
        Ok(())
    }

    /// The fee on a fill worth `quote_lots`, rounded up.
    ///
    /// Rounding up favours the venue, which is the conventional direction and — more
    /// importantly — keeps the fee non-zero on small fills. Rounding down would let a
    /// taker split an order into dust and pay nothing.
    ///
    /// The result never exceeds `quote_lots`, since the rate is capped at 100%, so a
    /// taker selling always nets a non-negative amount.
    ///
    /// # Errors
    ///
    /// [`EngineError::Overflow`] if the intermediate product exceeds `u128`, which
    /// cannot happen for validated rates but is checked rather than assumed.
    pub fn fee_on(&self, quote_lots: QuoteLots) -> Result<QuoteLots> {
        if self.taker_fee_bps == 0 {
            return Ok(QuoteLots::ZERO);
        }
        let numerator = (quote_lots.as_u64() as u128)
            .checked_mul(self.taker_fee_bps as u128)
            .ok_or(EngineError::Overflow)?;
        // Ceiling division.
        let fee = numerator.div_ceil(BPS_DENOMINATOR as u128);
        u64::try_from(fee)
            .map(QuoteLots)
            .map_err(|_| EngineError::Overflow)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rates_above_one_hundred_percent_are_rejected() {
        assert!(FeeSchedule::new(BPS_DENOMINATOR).is_ok());
        assert_eq!(
            FeeSchedule::new(BPS_DENOMINATOR + 1),
            Err(EngineError::InvalidFeeRate)
        );
    }

    #[test]
    fn a_free_market_charges_nothing() {
        assert_eq!(
            FeeSchedule::FREE.fee_on(QuoteLots(1_000_000)),
            Ok(QuoteLots::ZERO)
        );
    }

    #[test]
    fn two_basis_points_of_a_round_number_is_exact() {
        let schedule = FeeSchedule::new(2).unwrap();
        assert_eq!(schedule.fee_on(QuoteLots(1_000_000)), Ok(QuoteLots(200)));
    }

    #[test]
    fn dust_fills_still_pay_something() {
        // Rounding down would make order-splitting a fee-avoidance strategy.
        let schedule = FeeSchedule::new(2).unwrap();
        assert_eq!(schedule.fee_on(QuoteLots(1)), Ok(QuoteLots(1)));
        assert_eq!(schedule.fee_on(QuoteLots(5_001)), Ok(QuoteLots(2)));
    }

    #[test]
    fn the_fee_never_exceeds_the_fill() {
        // Otherwise a taker selling would net a negative amount.
        let schedule = FeeSchedule::new(BPS_DENOMINATOR).unwrap();
        for quote in [0u64, 1, 7, 1_000_000, u64::MAX] {
            assert_eq!(schedule.fee_on(QuoteLots(quote)), Ok(QuoteLots(quote)));
        }
    }

    #[test]
    fn the_largest_fill_at_the_largest_rate_does_not_overflow() {
        let schedule = FeeSchedule::new(BPS_DENOMINATOR).unwrap();
        assert!(schedule.fee_on(QuoteLots::MAX).is_ok());
    }
}
