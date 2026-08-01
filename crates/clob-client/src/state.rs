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
    /// The lot geometry is not one any market could have been created with.
    ///
    /// A `LotConfig` is `Pod`, so casting it out of account bytes bypasses
    /// [`LotConfig::new`] and its checks entirely. A zero in the wrong field divides by
    /// zero the first time anything is priced, which is a panic in whatever decoded it —
    /// so the config is re-checked here rather than trusted.
    InvalidLotConfig(clob_book::LotConfigError),
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
            Self::InvalidLotConfig(error) => {
                write!(f, "the market's lot configuration is invalid: {error:?}")
            }
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
    /// Claimed seats, with the index resting orders refer to them by.
    pub traders: Vec<Seat>,
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
            market
                .traders()
                .iter_indexed()
                .map(|(index, key, state)| Seat {
                    index: index as u32,
                    key,
                    state,
                })
                .collect::<Vec<_>>(),
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

        // A Pod cast bypasses LotConfig::new, so nothing has checked these numbers.
        // Refusing here means anything that survives decoding is safe to price; the
        // alternative is a divide by zero in whatever reads it next.
        header
            .lot_config
            .validate()
            .map_err(DecodeError::InvalidLotConfig)?;

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

    /// Where a fee sweep sends the quote it collects.
    ///
    /// Needed by callers rather than only by the venue:
    /// [`collect_fees`](crate::instruction::collect_fees) is permissionless, and the
    /// recipient it must be given is fixed in the account — which is exactly why anyone
    /// may call it and nobody gains by doing so.
    pub fn fee_recipient(&self) -> solana_pubkey::Pubkey {
        solana_pubkey::Pubkey::new_from_array(self.account.fee_recipient)
    }

    /// The market's authority.
    pub fn authority(&self) -> solana_pubkey::Pubkey {
        solana_pubkey::Pubkey::new_from_array(self.account.authority)
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

    /// Seats holding nothing, which anyone may evict to make room.
    ///
    /// Worth checking before giving up on a full market: a table can fill with seats
    /// belonging to traders who claimed one and never came back, and those are exactly
    /// the seats [`evict_seat`](crate::instruction::evict_seat) exists to reclaim.
    ///
    /// A seat with a resting order is never listed — posting locks funds, so a live
    /// quote means a non-zero balance.
    pub fn evictable_seats(&self) -> Vec<TraderKey> {
        self.traders
            .iter()
            .filter(|seat| seat.state.is_empty())
            .map(|seat| seat.key)
            .collect()
    }

    /// Whether the trader table is full, so a new trader needs a seat freed first.
    pub fn seats_are_full(&self) -> bool {
        self.traders.len() >= self.seat_capacity()
    }

    /// How many seats this market was created with.
    pub fn seat_capacity(&self) -> usize {
        match self.size_class {
            SizeClass::Small => 32,
            SizeClass::Medium => 128,
            SizeClass::Large => 512,
        }
    }

    /// The seat index a wallet's orders carry, if it holds a seat.
    ///
    /// This is how an observer resolves the trader that submitted a transaction into
    /// something comparable with the orders on the book. Without it, a self-trade and a
    /// fill are indistinguishable — both remove liquidity from the side opposite the
    /// taker — and reported volume becomes something anyone can inflate for free.
    pub fn seat_of(&self, key: &TraderKey) -> Option<u32> {
        self.traders
            .iter()
            .find(|seat| &seat.key == key)
            .map(|seat| seat.index)
    }

    /// Balances for a trader, if they hold a seat.
    pub fn trader(&self, key: &TraderKey) -> Option<&TraderState> {
        self.traders.iter().find(|seat| &seat.key == key).map(|seat| &seat.state)
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

/// A claimed seat: who holds it, what they hold, and the index it is known by.
///
/// Resting orders carry the index rather than the wallet, so this is the join between
/// an order on the book and the wallet that owns it. An observer needs that join to tell
/// a fill from a self-trade — a distinction only ownership can make, and one that
/// decides whether a trade is reported as volume at all.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Seat {
    /// What resting orders refer to this seat by.
    pub index: u32,
    /// The wallet holding it.
    pub key: TraderKey,
    /// Its balances.
    pub state: TraderState,
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
