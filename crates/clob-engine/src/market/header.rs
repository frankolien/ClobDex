//! The market's fixed configuration and running totals.

use bytemuck::{Pod, Zeroable};
use clob_book::{BaseLots, LotConfig, QuoteLots};

use crate::error::Result;
use crate::fees::FeeSchedule;

/// Market configuration plus the totals that make conservation checkable in O(1).
///
/// `base_lots_deposited` and `quote_lots_deposited` are derivable by summing every seat,
/// but keeping them as running totals means an instruction can reconcile the market
/// against its vault balances without walking the trader table — which is what makes the
/// check affordable to run on-chain rather than only in tests.
#[repr(C)]
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct MarketHeader {
    /// Tick and lot geometry. Immutable after creation.
    pub lot_config: LotConfig,
    /// Taker fee rate.
    pub fees: FeeSchedule,
    /// Base lots currently held by the market on behalf of all seats.
    pub base_lots_deposited: BaseLots,
    /// Quote lots currently held by the market, including unclaimed fees.
    pub quote_lots_deposited: QuoteLots,
    /// Lifetime fees earned. Never decreases, so it survives a sweep.
    pub collected_quote_lot_fees: QuoteLots,
    /// Fees earned but not yet swept. Part of `quote_lots_deposited` until collected.
    pub unclaimed_quote_lot_fees: QuoteLots,
}

// SAFETY: repr(C) over LotConfig (4 x u64), FeeSchedule (u64) and four u64 quantities.
// Size 72, align 8, no padding.
unsafe impl Zeroable for MarketHeader {}
unsafe impl Pod for MarketHeader {}

impl MarketHeader {
    /// Builds a header from validated configuration.
    ///
    /// # Errors
    ///
    /// [`EngineError::InvalidLotConfig`](crate::EngineError::InvalidLotConfig) or
    /// [`EngineError::InvalidFeeRate`](crate::EngineError::InvalidFeeRate).
    pub fn new(lot_config: LotConfig, fees: FeeSchedule) -> Result<Self> {
        lot_config.validate()?;
        fees.validate()?;
        Ok(Self {
            lot_config,
            fees,
            ..Default::default()
        })
    }

    /// Re-checks configuration after a raw cast out of account bytes.
    ///
    /// # Errors
    ///
    /// The first invalid field found.
    pub fn validate(&self) -> Result<()> {
        self.lot_config.validate()?;
        self.fees.validate()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::EngineError;

    #[test]
    fn a_header_has_no_padding() {
        assert_eq!(core::mem::size_of::<MarketHeader>(), 72);
        assert_eq!(core::mem::align_of::<MarketHeader>(), 8);
    }

    #[test]
    fn bad_configuration_is_rejected_at_construction() {
        // 1_500 does not divide evenly into 1_000 base lots per base unit.
        let bad_lots = LotConfig {
            base_lots_per_base_unit: 1_000,
            tick_size_in_quote_lots_per_base_unit: 1_500,
            base_atoms_per_base_lot: 1,
            quote_atoms_per_quote_lot: 1,
        };
        assert!(MarketHeader::new(bad_lots, FeeSchedule::FREE).is_err());

        let good_lots = LotConfig::new(1_000, 1_000, 1_000_000, 1).unwrap();
        assert_eq!(
            MarketHeader::new(good_lots, FeeSchedule { taker_fee_bps: 20_000 }),
            Err(EngineError::InvalidFeeRate)
        );
    }
}
