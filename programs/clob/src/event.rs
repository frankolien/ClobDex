//! Trade events, emitted by calling back into this program.
//!
//! # Why not just log
//!
//! Program logs are capped per transaction and silently truncated when they overflow.
//! A taker sweeping twenty price levels is exactly the case an indexer most needs to
//! read, and exactly the case most likely to be cut off — so logs are unreliable
//! precisely where reliability matters.
//!
//! Calling back into this program instead puts the payload in the transaction's
//! *inner instruction data*, which is returned in full in the transaction meta and is
//! not subject to the log budget. An indexer reads the inner instructions of a
//! [`Discriminant::LogEvent`](crate::instruction::Discriminant::LogEvent) call and gets
//! the whole event.
//!
//! # Why the log authority
//!
//! [`LogEvent`](crate::processor::log_event) requires a program-derived signer. Only
//! this program can sign for its own PDA, so nobody else can forge an event carrying
//! this program's id. The handler itself does nothing: the call exists so the data
//! lands in the transaction record.
//!
//! # Bounded, and honest about it
//!
//! There is no allocator here, so fills are buffered in a fixed-size array on the
//! stack. Beyond [`MAX_LOGGED_FILLS`] the per-fill detail is dropped and
//! `fills_truncated` is set — the aggregate totals stay exact either way. An indexer
//! that needs every fill of a very deep sweep must reconstruct the tail from account
//! diffs; the summary tells it when that is necessary rather than leaving it to guess.

use clob_engine::{Fill, FillObserver, OrderOutcome};

/// Event format version, so an indexer can reject payloads it does not understand.
pub const EVENT_VERSION: u8 = 1;

/// Marks the payload as an order-placed event.
pub const EVENT_ORDER_PLACED: u8 = 0;

/// How many individual fills fit in one event.
///
/// Sized against the 4 KiB SBF stack frame: 24 fills is 1,152 bytes of buffer, which
/// leaves ample room for the rest of the frame. Deeper sweeps keep exact totals and set
/// the truncation flag.
pub const MAX_LOGGED_FILLS: usize = 24;

/// Bytes per serialised fill.
const FILL_LEN: usize = 48;

/// Bytes in the fixed part of the payload: four flag bytes, the taker seat, the posted
/// order id, four 64-bit totals, and the two fill counts.
///
/// Hand-counted constants like this are exactly what silently corrupts a wire format,
/// so `encode` asserts it did not overflow and a test asserts an empty event is
/// precisely this long.
const SUMMARY_LEN: usize = 4 + 4 + 16 + 4 * 8 + 4 + 4;

/// Total payload capacity.
pub const MAX_EVENT_LEN: usize = SUMMARY_LEN + MAX_LOGGED_FILLS * FILL_LEN;

/// Collects fills during matching and serialises them afterwards.
///
/// Serialisation is deliberately deferred: the market account's data is mutably
/// borrowed for the whole of matching, and a CPI while that borrow is held would be
/// rejected by the runtime. Buffering first and emitting after the borrow is dropped is
/// what makes event emission possible at all.
pub struct EventBuffer {
    fills: [Fill; MAX_LOGGED_FILLS],
    recorded: usize,
    seen: usize,
}

impl Default for EventBuffer {
    fn default() -> Self {
        Self::new()
    }
}

impl EventBuffer {
    /// An empty buffer.
    pub fn new() -> Self {
        Self {
            fills: [EMPTY_FILL; MAX_LOGGED_FILLS],
            recorded: 0,
            seen: 0,
        }
    }

    /// Whether any fill had to be dropped.
    pub const fn truncated(&self) -> bool {
        self.seen > self.recorded
    }

    /// How many fills occurred, including any that were dropped.
    pub const fn seen(&self) -> usize {
        self.seen
    }

    /// Writes the event payload into `out`, returning the number of bytes used.
    ///
    /// The layout is fixed little-endian, matching the instruction wire format, so an
    /// indexer decodes it with the same primitives it already needs.
    pub fn encode(&self, outcome: &OrderOutcome, taker_seat: u32, out: &mut [u8]) -> usize {
        let mut cursor = Cursor::new(out);

        cursor.u8(EVENT_VERSION);
        cursor.u8(EVENT_ORDER_PLACED);
        cursor.u8(outcome.stop as u8);
        cursor.u8(u8::from(self.truncated()));
        cursor.u32(taker_seat);

        let (price, sequence) = match outcome.order_id {
            Some(id) => (id.price_in_ticks.as_u64(), id.order_sequence_number),
            // A zero sequence number is a real bid id, so the price field alone cannot
            // signal "nothing posted". `base_lots_posted` is the flag; this is data.
            None => (0, 0),
        };
        cursor.u64(price);
        cursor.u64(sequence);
        cursor.u64(outcome.base_lots_filled.as_u64());
        cursor.u64(outcome.quote_lots_filled.as_u64());
        cursor.u64(outcome.fee_in_quote_lots.as_u64());
        cursor.u64(outcome.base_lots_posted.as_u64());
        cursor.u32(self.seen as u32);
        cursor.u32(self.recorded as u32);

        for fill in &self.fills[..self.recorded] {
            cursor.u32(fill.maker_seat);
            cursor.u32(fill.base_lots_filled.as_u64() as u32);
            cursor.u64(fill.maker_order_id.price_in_ticks.as_u64());
            cursor.u64(fill.maker_order_id.order_sequence_number);
            cursor.u64(fill.quote_lots_filled.as_u64());
            cursor.u64(fill.fee_in_quote_lots.as_u64());
            cursor.u64(fill.maker_base_lots_remaining.as_u64());
        }

        debug_assert!(
            !cursor.overflowed,
            "event buffer too small: SUMMARY_LEN or FILL_LEN is wrong"
        );
        cursor.written
    }
}

impl FillObserver for EventBuffer {
    fn on_fill(&mut self, fill: &Fill) {
        self.seen += 1;
        if self.recorded < MAX_LOGGED_FILLS {
            self.fills[self.recorded] = *fill;
            self.recorded += 1;
        }
    }
}

/// A zeroed fill, used only to give the buffer array an initial value.
const EMPTY_FILL: Fill = Fill {
    maker_order_id: clob_book::FIFOOrderId {
        price_in_ticks: clob_book::Ticks(0),
        order_sequence_number: 0,
    },
    maker_seat: 0,
    taker_seat: 0,
    price_in_ticks: clob_book::Ticks(0),
    base_lots_filled: clob_book::BaseLots(0),
    quote_lots_filled: clob_book::QuoteLots(0),
    fee_in_quote_lots: clob_book::QuoteLots(0),
    maker_base_lots_remaining: clob_book::BaseLots(0),
};

/// Little-endian writer over a fixed buffer.
///
/// Writes past the end are dropped rather than panicking, because a truncated event is
/// a far better outcome than a reverted trade. But silence would let a wrong size
/// constant corrupt every event undetected, so overflow is recorded and asserted
/// against in tests.
struct Cursor<'a> {
    out: &'a mut [u8],
    written: usize,
    overflowed: bool,
}

impl<'a> Cursor<'a> {
    fn new(out: &'a mut [u8]) -> Self {
        Self {
            out,
            written: 0,
            overflowed: false,
        }
    }

    fn bytes(&mut self, value: &[u8]) {
        let end = self.written + value.len();
        if end <= self.out.len() {
            self.out[self.written..end].copy_from_slice(value);
            self.written = end;
        } else {
            self.overflowed = true;
        }
    }

    fn u8(&mut self, value: u8) {
        self.bytes(&[value]);
    }

    fn u32(&mut self, value: u32) {
        self.bytes(&value.to_le_bytes());
    }

    fn u64(&mut self, value: u64) {
        self.bytes(&value.to_le_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clob_book::{BaseLots, FIFOOrderId, QuoteLots, Side, Ticks};
    use clob_engine::MatchStop;

    fn fill(seat: u32, size: u64) -> Fill {
        Fill {
            maker_order_id: FIFOOrderId::new(Side::Ask, Ticks(100), seat as u64),
            maker_seat: seat,
            taker_seat: 99,
            price_in_ticks: Ticks(100),
            base_lots_filled: BaseLots(size),
            quote_lots_filled: QuoteLots(size * 100),
            fee_in_quote_lots: QuoteLots(1),
            maker_base_lots_remaining: BaseLots(0),
        }
    }

    fn outcome() -> OrderOutcome {
        OrderOutcome {
            base_lots_filled: BaseLots(10),
            quote_lots_filled: QuoteLots(1_000),
            ..OrderOutcome::empty(MatchStop::FullyFilled)
        }
    }

    #[test]
    fn an_event_fits_its_declared_capacity() {
        let mut buffer = EventBuffer::new();
        for i in 0..MAX_LOGGED_FILLS {
            buffer.on_fill(&fill(i as u32, 1));
        }

        let mut out = [0u8; MAX_EVENT_LEN];
        let written = buffer.encode(&outcome(), 7, &mut out);

        assert_eq!(written, MAX_EVENT_LEN);
        assert!(!buffer.truncated());
    }

    #[test]
    fn overflowing_fills_are_flagged_but_totals_stay_exact() {
        let mut buffer = EventBuffer::new();
        for i in 0..MAX_LOGGED_FILLS + 10 {
            buffer.on_fill(&fill(i as u32, 1));
        }

        assert!(buffer.truncated());
        assert_eq!(buffer.seen(), MAX_LOGGED_FILLS + 10);

        let mut out = [0u8; MAX_EVENT_LEN];
        buffer.encode(&outcome(), 7, &mut out);

        // Byte 3 is the truncation flag; the aggregate totals are unaffected by it.
        assert_eq!(out[3], 1);
        assert_eq!(
            u32::from_le_bytes(out[56..60].try_into().unwrap()),
            (MAX_LOGGED_FILLS + 10) as u32,
            "fills seen"
        );
        assert_eq!(
            u32::from_le_bytes(out[60..64].try_into().unwrap()),
            MAX_LOGGED_FILLS as u32,
            "fills recorded"
        );
    }

    #[test]
    fn the_summary_carries_the_outcome_verbatim() {
        let buffer = EventBuffer::new();
        let mut out = [0u8; MAX_EVENT_LEN];
        let written = buffer.encode(&outcome(), 7, &mut out);

        assert_eq!(written, SUMMARY_LEN);
        assert_eq!(out[0], EVENT_VERSION);
        assert_eq!(out[1], EVENT_ORDER_PLACED);
        assert_eq!(out[2], MatchStop::FullyFilled as u8);
        assert_eq!(u32::from_le_bytes([out[4], out[5], out[6], out[7]]), 7);
        // base_lots_filled sits after version/type/stop/truncated, seat and order id.
        assert_eq!(
            u64::from_le_bytes(out[24..32].try_into().unwrap()),
            10
        );
    }
}
