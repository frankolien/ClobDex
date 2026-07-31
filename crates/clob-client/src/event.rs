//! Decoding trade events out of a transaction.
//!
//! Events arrive as *inner instruction data*, not as logs. An indexer walking a
//! transaction looks at each inner instruction addressed to this program, checks the
//! leading [`Discriminant::LogEvent`] byte, and hands the rest here.
//!
//! The layout is fixed little-endian and versioned. A decoder that does not recognise
//! the version must stop rather than guess, which is why [`decode`] rejects instead of
//! reading what it can.

use clob_book::{BaseLots, FIFOOrderId, QuoteLots, Ticks};
use clob_engine::MatchStop;
use clob_program::event::{EVENT_ORDER_PLACED, EVENT_VERSION};
use clob_program::instruction::Discriminant;

/// Bytes in the fixed part of the payload.
const SUMMARY_LEN: usize = 64;
/// Bytes per serialised fill.
const FILL_LEN: usize = 48;

/// Why an event payload could not be decoded.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum EventError {
    /// The data did not begin with the `LogEvent` discriminant.
    NotAnEvent,
    /// The payload is shorter than the fixed summary.
    TooShort,
    /// Written by a program version this decoder does not understand.
    UnknownVersion(u8),
    /// The event kind byte is not one this decoder knows.
    UnknownKind(u8),
    /// The stop code is not a known [`MatchStop`].
    UnknownStop(u8),
    /// The declared fill count does not fit in the remaining bytes.
    FillCountMismatch {
        /// Fill count the event claims to carry.
        declared: u32,
        /// Fill records the payload has room for.
        available: usize,
    },
}

impl core::fmt::Display for EventError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::NotAnEvent => write!(f, "not a LogEvent instruction"),
            Self::TooShort => write!(f, "payload shorter than the event summary"),
            Self::UnknownVersion(v) => write!(f, "unknown event version {v}"),
            Self::UnknownKind(k) => write!(f, "unknown event kind {k}"),
            Self::UnknownStop(s) => write!(f, "unknown match stop code {s}"),
            Self::FillCountMismatch { declared, available } => {
                write!(f, "event declares {declared} fills but carries {available}")
            }
        }
    }
}

impl std::error::Error for EventError {}

/// One maker order consumed.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct FillRecord {
    /// Seat that owned the resting order.
    pub maker_seat: u32,
    /// Size traded.
    pub base_lots_filled: BaseLots,
    /// The resting order that was hit.
    pub maker_order_id: FIFOOrderId,
    /// Execution price. Always the maker's price.
    pub price_in_ticks: Ticks,
    /// Gross quote value.
    pub quote_lots_filled: QuoteLots,
    /// Fee charged to the taker on this fill.
    pub fee_in_quote_lots: QuoteLots,
    /// Size still resting on the maker order; zero if it was consumed.
    pub maker_base_lots_remaining: BaseLots,
}

/// A decoded order-placed event.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OrderPlaced {
    /// Seat that submitted the order.
    pub taker_seat: u32,
    /// Id of the resting remainder, if any part of the order posted.
    pub order_id: Option<FIFOOrderId>,
    /// Total size taken.
    pub base_lots_filled: BaseLots,
    /// Total gross quote value taken.
    pub quote_lots_filled: QuoteLots,
    /// Total fee charged.
    pub fee_in_quote_lots: QuoteLots,
    /// Size left resting.
    pub base_lots_posted: BaseLots,
    /// Why matching ended.
    pub stop: MatchStop,
    /// How many fills occurred, including any the event could not carry.
    pub fills_seen: u32,
    /// The fills the event does carry.
    ///
    /// Shorter than `fills_seen` when [`OrderPlaced::truncated`] is set. The aggregate
    /// totals above stay exact regardless, so a consumer that needs the missing tail
    /// knows to reconstruct it from account diffs rather than silently recording a
    /// short trade.
    pub fills: Vec<FillRecord>,
}

impl OrderPlaced {
    /// Whether the per-fill detail is incomplete.
    pub fn truncated(&self) -> bool {
        self.fills_seen as usize > self.fills.len()
    }
}

fn u32_at(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

fn u64_at(bytes: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap())
}

fn stop_from_u8(value: u8) -> Result<MatchStop, EventError> {
    Ok(match value {
        0 => MatchStop::FullyFilled,
        1 => MatchStop::PriceLimit,
        2 => MatchStop::MatchLimit,
        3 => MatchStop::BookEmpty,
        4 => MatchStop::InsufficientFunds,
        5 => MatchStop::DidNotCross,
        other => return Err(EventError::UnknownStop(other)),
    })
}

/// Decodes an inner instruction's data into an event.
///
/// `data` is the whole instruction payload, discriminant included, exactly as it appears
/// in a transaction record.
///
/// # Errors
///
/// [`EventError`] if the data is not an event, is truncated, or was written by a version
/// this decoder does not understand.
pub fn decode(data: &[u8]) -> Result<OrderPlaced, EventError> {
    // [discriminant][log authority bump][payload]
    let Some((&tag, rest)) = data.split_first() else {
        return Err(EventError::NotAnEvent);
    };
    if tag != Discriminant::LogEvent as u8 {
        return Err(EventError::NotAnEvent);
    }
    let Some((_bump, payload)) = rest.split_first() else {
        return Err(EventError::TooShort);
    };
    if payload.len() < SUMMARY_LEN {
        return Err(EventError::TooShort);
    }

    if payload[0] != EVENT_VERSION {
        return Err(EventError::UnknownVersion(payload[0]));
    }
    if payload[1] != EVENT_ORDER_PLACED {
        return Err(EventError::UnknownKind(payload[1]));
    }

    let stop = stop_from_u8(payload[2])?;
    let base_lots_posted = BaseLots(u64_at(payload, 48));
    let fills_seen = u32_at(payload, 56);
    let fills_recorded = u32_at(payload, 60);

    let available = (payload.len() - SUMMARY_LEN) / FILL_LEN;
    if fills_recorded as usize > available {
        return Err(EventError::FillCountMismatch {
            declared: fills_recorded,
            available,
        });
    }

    let fills = (0..fills_recorded as usize)
        .map(|i| {
            let o = SUMMARY_LEN + i * FILL_LEN;
            let price = Ticks(u64_at(payload, o + 8));
            FillRecord {
                maker_seat: u32_at(payload, o),
                base_lots_filled: BaseLots(u32_at(payload, o + 4) as u64),
                maker_order_id: FIFOOrderId::from_encoded(price, u64_at(payload, o + 16)),
                price_in_ticks: price,
                quote_lots_filled: QuoteLots(u64_at(payload, o + 24)),
                fee_in_quote_lots: QuoteLots(u64_at(payload, o + 32)),
                maker_base_lots_remaining: BaseLots(u64_at(payload, o + 40)),
            }
        })
        .collect();

    Ok(OrderPlaced {
        taker_seat: u32_at(payload, 4),
        // Nothing posted means no id; the posted size is the flag, since a zero
        // sequence number is a perfectly real order id.
        order_id: (!base_lots_posted.is_zero())
            .then(|| FIFOOrderId::from_encoded(Ticks(u64_at(payload, 8)), u64_at(payload, 16))),
        base_lots_filled: BaseLots(u64_at(payload, 24)),
        quote_lots_filled: QuoteLots(u64_at(payload, 32)),
        fee_in_quote_lots: QuoteLots(u64_at(payload, 40)),
        base_lots_posted,
        stop,
        fills_seen,
        fills,
    })
}

/// Decodes every event in a transaction's inner instructions, skipping anything else.
///
/// The intended entry point for an indexer: hand it each inner instruction's data and
/// take what comes back.
pub fn decode_all<'a, I>(instructions: I) -> Result<Vec<OrderPlaced>, EventError>
where
    I: IntoIterator<Item = &'a [u8]>,
{
    instructions
        .into_iter()
        .filter(|data| data.first() == Some(&(Discriminant::LogEvent as u8)))
        .map(decode)
        .collect()
}
