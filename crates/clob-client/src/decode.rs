//! Reading instructions back off the chain.
//!
//! The builders in [`instruction`](crate::instruction) write; this reads. An indexer
//! needs the reverse direction to know what a transaction asked for, and it closes the
//! loop on the builders: anything the SDK can construct, it can also recognise.
//!
//! Parsing goes through [`clob_program::instruction::Reader`] — the program's own
//! parser, not a reimplementation — so a decoded instruction is by construction the same
//! one the program saw.

use clob_book::{BaseLots, FIFOOrderId, LotConfig, QuoteLots, Side, Ticks};
use clob_engine::OrderPacket;
use clob_program::instruction::{Discriminant, Reader};
use clob_program::state::SizeClass;

/// An instruction as it appeared on chain.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClobInstruction {
    /// Market creation.
    InitializeMarket {
        /// Book and seat capacities.
        size_class: SizeClass,
        /// Tick and lot geometry.
        lot_config: LotConfig,
        /// Taker fee in basis points.
        taker_fee_bps: u64,
    },
    /// Seat claim.
    ClaimSeat,
    /// Tokens in.
    Deposit {
        /// Base lots credited.
        base_lots: BaseLots,
        /// Quote lots credited.
        quote_lots: QuoteLots,
    },
    /// Tokens out.
    Withdraw {
        /// Base lots debited.
        base_lots: BaseLots,
        /// Quote lots debited.
        quote_lots: QuoteLots,
    },
    /// An order against funds already on the market.
    PlaceOrder {
        /// What was submitted.
        packet: OrderPacket,
        /// Whether an event receipt was requested.
        receipt: bool,
    },
    /// A cancel.
    CancelOrder {
        /// The order removed.
        order_id: FIFOOrderId,
    },
    /// A partial cancel.
    ReduceOrder {
        /// The order shrunk.
        order_id: FIFOOrderId,
        /// Size removed.
        base_lots: BaseLots,
    },
    /// A bounded cancel-all on one side.
    CancelAllOrders {
        /// Side cleared.
        side: Side,
        /// Maximum orders cancelled.
        limit: u32,
    },
    /// A fee sweep.
    CollectFees,
    /// An emitted event. Only ever appears as an inner instruction.
    LogEvent,
    /// Deposit, match and withdraw in one.
    Swap {
        /// Side the taker was on.
        side: Side,
        /// Limit price.
        price_in_ticks: Ticks,
        /// Size requested.
        num_base_lots: BaseLots,
        /// Minimum fill, below which the whole thing reverts.
        min_base_lots_to_fill: BaseLots,
        /// Maximum resting orders consumed.
        match_limit: u32,
        /// Whether an event receipt was requested.
        receipt: bool,
    },
    /// A permissionless seat eviction.
    EvictSeat,
    /// Cancels and placements in one instruction.
    BatchUpdate {
        /// Orders cancelled, in order.
        cancels: Vec<FIFOOrderId>,
        /// Orders submitted, in order.
        orders: Vec<OrderPacket>,
    },
}

impl ClobInstruction {
    /// The side a taker was on, for the instructions that take liquidity.
    ///
    /// This is what tells an observer which half of the book any fills came from: a
    /// taker on the bid consumes asks.
    pub fn taker_side(&self) -> Option<Side> {
        match self {
            Self::PlaceOrder { packet, .. } if packet.can_take() => Some(packet.side()),
            Self::Swap { side, .. } => Some(*side),
            // A batch is a taker if any order in it is. Reporting the first such side is
            // enough for attribution: a batch crossing both sides at once would be a
            // maker trading against itself, which the engine rejects before this matters.
            Self::BatchUpdate { orders, .. } => orders
                .iter()
                .find(|packet| packet.can_take())
                .map(|packet| packet.side()),
            _ => None,
        }
    }

    /// Whether this instruction can add liquidity to the book.
    pub fn can_post(&self) -> bool {
        match self {
            Self::PlaceOrder { packet, .. } => packet.can_post(),
            Self::BatchUpdate { orders, .. } => orders.iter().any(OrderPacket::can_post),
            _ => false,
        }
    }
}

/// Why an instruction could not be decoded.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum InstructionDecodeError {
    /// The data was empty.
    Empty,
    /// The leading byte is not an instruction this program defines.
    UnknownDiscriminant(u8),
    /// The data ended before the instruction's fields did.
    Truncated,
    /// A field held a value outside its defined range.
    Malformed,
}

impl core::fmt::Display for InstructionDecodeError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Empty => write!(f, "empty instruction data"),
            Self::UnknownDiscriminant(tag) => write!(f, "unknown discriminant {tag}"),
            Self::Truncated => write!(f, "instruction data ended early"),
            Self::Malformed => write!(f, "instruction field out of range"),
        }
    }
}

impl std::error::Error for InstructionDecodeError {}

/// The program's `Reader` reports every failure as a `ProgramError`; an observer only
/// needs to know that the bytes did not parse.
fn read<T>(result: Result<T, clob_program::pinocchio_error::ProgramError>) -> Result<T, InstructionDecodeError> {
    result.map_err(|_| InstructionDecodeError::Truncated)
}

/// Decodes one instruction's data.
///
/// # Errors
///
/// [`InstructionDecodeError`] if the data is empty, carries an unknown discriminant, or
/// ends before its fields do.
pub fn decode(data: &[u8]) -> Result<ClobInstruction, InstructionDecodeError> {
    let (&tag, rest) = data.split_first().ok_or(InstructionDecodeError::Empty)?;
    let discriminant =
        Discriminant::parse(tag).map_err(|_| InstructionDecodeError::UnknownDiscriminant(tag))?;
    let mut reader = Reader::new(rest);

    Ok(match discriminant {
        Discriminant::InitializeMarket => {
            let size_class = SizeClass::from_u64(read(reader.u64())?)
                .map_err(|_| InstructionDecodeError::Malformed)?;
            ClobInstruction::InitializeMarket {
                size_class,
                lot_config: LotConfig {
                    base_lots_per_base_unit: read(reader.u64())?,
                    tick_size_in_quote_lots_per_base_unit: read(reader.u64())?,
                    base_atoms_per_base_lot: read(reader.u64())?,
                    quote_atoms_per_quote_lot: read(reader.u64())?,
                },
                taker_fee_bps: read(reader.u64())?,
            }
        }
        Discriminant::ClaimSeat => ClobInstruction::ClaimSeat,
        Discriminant::Deposit => ClobInstruction::Deposit {
            base_lots: read(reader.base_lots())?,
            quote_lots: read(reader.quote_lots())?,
        },
        Discriminant::Withdraw => ClobInstruction::Withdraw {
            base_lots: read(reader.base_lots())?,
            quote_lots: read(reader.quote_lots())?,
        },
        Discriminant::PlaceOrder => {
            let packet = read(reader.order_packet())?;
            // The receipt form appends the log authority bump; nothing else follows a
            // packet, so a trailing byte means exactly that.
            ClobInstruction::PlaceOrder {
                packet,
                receipt: reader.optional_u8().is_some(),
            }
        }
        Discriminant::CancelOrder => ClobInstruction::CancelOrder {
            order_id: read(reader.order_id())?,
        },
        Discriminant::ReduceOrder => ClobInstruction::ReduceOrder {
            order_id: read(reader.order_id())?,
            base_lots: read(reader.base_lots())?,
        },
        Discriminant::CancelAllOrders => ClobInstruction::CancelAllOrders {
            side: read(reader.side())?,
            limit: read(reader.u32())?,
        },
        Discriminant::CollectFees => ClobInstruction::CollectFees,
        Discriminant::LogEvent => ClobInstruction::LogEvent,
        Discriminant::Swap => ClobInstruction::Swap {
            side: read(reader.side())?,
            price_in_ticks: Ticks(read(reader.u64())?),
            num_base_lots: BaseLots(read(reader.u64())?),
            min_base_lots_to_fill: BaseLots(read(reader.u64())?),
            match_limit: read(reader.u32())?,
            receipt: reader.optional_u8().is_some(),
        },
        Discriminant::EvictSeat => ClobInstruction::EvictSeat,
        Discriminant::BatchUpdate => {
            let cancel_count = read(reader.u8())?;
            let mut cancels = Vec::with_capacity(cancel_count as usize);
            for _ in 0..cancel_count {
                cancels.push(read(reader.order_id())?);
            }

            let order_count = read(reader.u8())?;
            let mut orders = Vec::with_capacity(order_count as usize);
            for _ in 0..order_count {
                orders.push(read(reader.order_packet())?);
            }
            ClobInstruction::BatchUpdate { cancels, orders }
        }
    })
}
