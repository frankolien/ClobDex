//! Decoding a market account.
//!
//! On-chain the market is cast in place and never copied, because compute is scarce.
//! Off-chain the opposite is true: an indexer wants owned, size-agnostic data it can
//! hold, diff and serialise. So this decodes once into plain structures rather than
//! handing back a reference into a byte buffer whose capacities are const generics.

use clob_book::{BaseLots, FIFOOrderId, LotConfig, QuoteLots, Side, Ticks};
use clob_engine::{FeeSchedule, Market, MarketHeader, TraderKey, TraderState};
use clob_program::state::{HEADER_LEN, MARKET_DISCRIMINATOR, MARKET_VERSION, MarketAccountHeader, SizeClass};

/// Why a market account could not be decoded.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum DecodeError {
    /// The buffer is shorter than the header.
    TooShort,
    /// The first eight bytes are not the market discriminator.
    NotAMarket,
    /// The account was written by an incompatible program version.
    VersionMismatch {
        /// Version found in the account.
        found: u64,
        /// Version this client understands.
        expected: u64,
    },
    /// The size class byte is not one this client knows.
    UnknownSizeClass(u64),
    /// The account is shorter than its declared size class requires.
    Truncated {
        /// Bytes the declared size class needs.
        expected: usize,
        /// Bytes the buffer actually holds.
        found: usize,
    },
}

impl core::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::TooShort => write!(f, "buffer shorter than the market header"),
            Self::NotAMarket => write!(f, "not a market account"),
            Self::VersionMismatch { found, expected } => {
                write!(f, "market version {found}, expected {expected}")
            }
            Self::UnknownSizeClass(value) => write!(f, "unknown size class {value}"),
            Self::Truncated { expected, found } => {
                write!(f, "account is {found} bytes, expected {expected}")
            }
        }
    }
}

impl std::error::Error for DecodeError {}

/// One resting order.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct BookOrder {
    /// Identity, from which price and side decode.
    pub id: FIFOOrderId,
    /// Seat index of the owner.
    pub trader_index: u32,
    /// Remaining size.
    pub num_base_lots: BaseLots,
}

impl BookOrder {
    /// Limit price.
    pub fn price_in_ticks(&self) -> Ticks {
        self.id.price_in_ticks
    }

    /// Arrival order across the whole market.
    pub fn sequence_number(&self) -> u64 {
        self.id.sequence_number()
    }
}

/// A decoded market, independent of its on-chain capacities.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MarketState {
    /// The account preamble: mints, vaults, authority.
    pub account: MarketAccountHeader,
    /// Which capacities this account was created at.
    pub size_class: SizeClass,
    /// Configuration and running totals.
    pub header: MarketHeader,
    /// Bids, best first — highest price, oldest within a price.
    pub bids: Vec<BookOrder>,
    /// Asks, best first — lowest price, oldest within a price.
    pub asks: Vec<BookOrder>,
    /// Claimed seats and their balances.
    pub traders: Vec<(TraderKey, TraderState)>,
}

/// Pulls the orders and seats out of a concrete market into owned vectors.
macro_rules! decode_sized {
    ($bytes:expr, $bids:literal, $asks:literal, $seats:literal) => {{
        let needed = Market::<$bids, $asks, $seats>::SIZE_IN_BYTES;
        if $bytes.len() < needed {
            return Err(DecodeError::Truncated {
                expected: needed + HEADER_LEN,
                found: $bytes.len() + HEADER_LEN,
            });
        }
        let market: &Market<$bids, $asks, $seats> = bytemuck::from_bytes(&$bytes[..needed]);
        (
            *market.header(),
            collect_side(market.book().iter_side(Side::Bid)),
            collect_side(market.book().iter_side(Side::Ask)),
            market.traders().iter().collect::<Vec<_>>(),
        )
    }};
}

fn collect_side<I>(entries: I) -> Vec<BookOrder>
where
    I: Iterator<Item = clob_book::Entry<FIFOOrderId, clob_book::RestingOrder>>,
{
    entries
        .map(|entry| BookOrder {
            id: entry.key,
            trader_index: entry.value.trader_index as u32,
            num_base_lots: entry.value.num_base_lots,
        })
        .collect()
}

impl MarketState {
    /// Decodes a market account's raw data.
    ///
    /// # Errors
    ///
    /// [`DecodeError`] describing what about the buffer was wrong.
    pub fn decode(data: &[u8]) -> Result<Self, DecodeError> {
        if data.len() < HEADER_LEN {
            return Err(DecodeError::TooShort);
        }
        let account: MarketAccountHeader = *bytemuck::from_bytes(&data[..HEADER_LEN]);

        if account.discriminator != MARKET_DISCRIMINATOR {
            return Err(DecodeError::NotAMarket);
        }
        if account.version != MARKET_VERSION {
            return Err(DecodeError::VersionMismatch {
                found: account.version,
                expected: MARKET_VERSION,
            });
        }

        let size_class = SizeClass::from_u64(account.size_class)
            .map_err(|_| DecodeError::UnknownSizeClass(account.size_class))?;

        let body = &data[HEADER_LEN..];
        let (header, bids, asks, traders) = match size_class {
            SizeClass::Small => decode_sized!(body, 128, 128, 32),
            SizeClass::Medium => decode_sized!(body, 512, 512, 128),
            SizeClass::Large => decode_sized!(body, 2048, 2048, 512),
        };

        Ok(Self {
            account,
            size_class,
            header,
            bids,
            asks,
            traders,
        })
    }

    /// Lot and tick geometry.
    pub fn lot_config(&self) -> &LotConfig {
        &self.header.lot_config
    }

    /// Fee rate.
    pub fn fees(&self) -> &FeeSchedule {
        &self.header.fees
    }

    /// Orders on `side`, best first.
    pub fn side(&self, side: Side) -> &[BookOrder] {
        match side {
            Side::Bid => &self.bids,
            Side::Ask => &self.asks,
        }
    }

    /// Highest-priority bid.
    pub fn best_bid(&self) -> Option<&BookOrder> {
        self.bids.first()
    }

    /// Highest-priority ask.
    pub fn best_ask(&self) -> Option<&BookOrder> {
        self.asks.first()
    }

    /// Best ask minus best bid, in ticks. `None` if either side is empty or the book is
    /// crossed — a crossed book is a transient state, not a negative spread.
    pub fn spread_in_ticks(&self) -> Option<u64> {
        let bid = self.best_bid()?.price_in_ticks().as_u64();
        let ask = self.best_ask()?.price_in_ticks().as_u64();
        ask.checked_sub(bid)
    }

    /// Midpoint of the touch, in ticks, rounded down.
    ///
    /// Defined even when the book is crossed, unlike
    /// [`spread_in_ticks`](Self::spread_in_ticks) — a crossed midpoint is still the
    /// average of the two prices, whereas a negative spread is not a spread.
    pub fn mid_price_in_ticks(&self) -> Option<u64> {
        let bid = self.best_bid()?.price_in_ticks().as_u64();
        let ask = self.best_ask()?.price_in_ticks().as_u64();
        bid.checked_add(ask).map(|sum| sum / 2)
    }

    /// Balances for a trader, if they hold a seat.
    pub fn trader(&self, key: &TraderKey) -> Option<&TraderState> {
        self.traders.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    /// Aggregates `side` into price levels, best first, up to `depth` levels.
    ///
    /// This is the view a UI renders and an indexer publishes. Individual orders are the
    /// venue's business; a price and a size is what a trader reads.
    pub fn level_two(&self, side: Side, depth: usize) -> Vec<Level> {
        let mut levels: Vec<Level> = Vec::new();
        for order in self.side(side) {
            match levels.last_mut() {
                Some(last) if last.price_in_ticks == order.price_in_ticks() => {
                    last.base_lots = last.base_lots.saturating_add(order.num_base_lots);
                    last.order_count += 1;
                }
                _ => {
                    if levels.len() == depth {
                        break;
                    }
                    levels.push(Level {
                        price_in_ticks: order.price_in_ticks(),
                        base_lots: order.num_base_lots,
                        order_count: 1,
                    });
                }
            }
        }
        levels
    }

    /// Total resting size on `side` at prices at or better than `limit`.
    pub fn depth_at_or_better(&self, side: Side, limit: Ticks) -> BaseLots {
        self.side(side)
            .iter()
            .take_while(|order| match side {
                Side::Bid => order.price_in_ticks() >= limit,
                Side::Ask => order.price_in_ticks() <= limit,
            })
            .fold(BaseLots::ZERO, |sum, order| {
                sum.saturating_add(order.num_base_lots)
            })
    }

    /// What sweeping `size` from `side` would cost, and the worst price it reaches.
    ///
    /// `None` if the book cannot fill that much. Ignores fees, which the caller applies
    /// from [`MarketState::fees`].
    pub fn quote_sweep(&self, side: Side, size: BaseLots) -> Option<Sweep> {
        let mut remaining = size;
        let mut quote_lots = QuoteLots::ZERO;

        for order in self.side(side) {
            let taken = remaining.min(order.num_base_lots);
            let value = self.lot_config().quote_lots_for(order.price_in_ticks(), taken)?;
            quote_lots = quote_lots.checked_add(value)?;
            remaining -= taken;
            if remaining.is_zero() {
                return Some(Sweep {
                    base_lots: size,
                    quote_lots,
                    // The last level touched is the worst price reached.
                    worst_price_in_ticks: order.price_in_ticks(),
                });
            }
        }
        None
    }
}

/// One aggregated price level.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Level {
    /// Price.
    pub price_in_ticks: Ticks,
    /// Total resting size at this price.
    pub base_lots: BaseLots,
    /// How many individual orders make it up. Depth of one large order and depth of
    /// twenty small ones behave very differently, and the count is the only way to tell.
    pub order_count: u32,
}

/// The result of pricing a hypothetical sweep.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Sweep {
    /// Size that would fill.
    pub base_lots: BaseLots,
    /// Gross quote value, before fees.
    pub quote_lots: QuoteLots,
    /// Worst price reached.
    pub worst_price_in_ticks: Ticks,
}
