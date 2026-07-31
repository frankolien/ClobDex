//! Every way an engine operation can fail.
//!
//! One flat enum rather than per-module error types: on-chain these become numeric
//! program error codes, and a flat mapping is far easier to keep stable across versions
//! than a nested one.

use clob_book::LotConfigError;

/// A rejected engine operation.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum EngineError {
    /// The market's trader table is at capacity.
    SeatTableFull,
    /// The trader has no seat on this market.
    SeatNotFound,
    /// The seat still has resting orders or a non-zero balance.
    SeatNotEmpty,
    /// That side of the book is at capacity.
    BookSideFull,
    /// The trader's free base balance does not cover the operation.
    InsufficientBaseFunds,
    /// The trader's free quote balance does not cover the operation.
    InsufficientQuoteFunds,
    /// No resting order with that id.
    OrderNotFound,
    /// The order exists but belongs to a different seat.
    NotOrderOwner,
    /// The order would have matched against its owner's own resting order, and the
    /// requested [`SelfTradeBehavior`](crate::SelfTradeBehavior) was `Abort`.
    SelfTradeAborted,
    /// A post-only order would have crossed the book and the caller asked to reject
    /// rather than slide.
    PostOnlyWouldCross,
    /// A post-only slide had no valid price to slide to.
    PostOnlyNoRoom,
    /// An immediate-or-cancel order filled less than its `min_base_lots_to_fill`.
    MinimumFillNotMet,
    /// The order had zero size.
    ZeroSize,
    /// A price or value computation exceeded `u64`.
    Overflow,
    /// The fee rate exceeded 100%.
    InvalidFeeRate,
    /// The market's lot geometry is invalid.
    InvalidLotConfig(LotConfigError),
    /// `initialize` was called on a market that is not blank.
    MarketAlreadyInitialized,
}

impl core::fmt::Display for EngineError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::SeatTableFull => f.write_str("trader table is at capacity"),
            Self::SeatNotFound => f.write_str("trader has no seat on this market"),
            Self::SeatNotEmpty => f.write_str("seat still has orders or a balance"),
            Self::BookSideFull => f.write_str("book side is at capacity"),
            Self::InsufficientBaseFunds => f.write_str("insufficient free base funds"),
            Self::InsufficientQuoteFunds => f.write_str("insufficient free quote funds"),
            Self::OrderNotFound => f.write_str("no resting order with that id"),
            Self::NotOrderOwner => f.write_str("order belongs to a different seat"),
            Self::SelfTradeAborted => f.write_str("order would have self-traded"),
            Self::PostOnlyWouldCross => f.write_str("post-only order would cross"),
            Self::PostOnlyNoRoom => f.write_str("post-only order had no price to slide to"),
            Self::MinimumFillNotMet => f.write_str("filled less than the requested minimum"),
            Self::ZeroSize => f.write_str("order size was zero"),
            Self::Overflow => f.write_str("arithmetic overflow"),
            Self::InvalidFeeRate => f.write_str("fee rate exceeds 100%"),
            Self::InvalidLotConfig(inner) => write!(f, "invalid lot config: {inner}"),
            Self::MarketAlreadyInitialized => f.write_str("market is already initialized"),
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for EngineError {}

impl From<LotConfigError> for EngineError {
    fn from(error: LotConfigError) -> Self {
        Self::InvalidLotConfig(error)
    }
}

/// Shorthand for engine operations.
pub type Result<T> = core::result::Result<T, EngineError>;
