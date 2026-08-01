//! Typed instruction builders.
//!
//! Every instruction the program accepts, built from typed arguments rather than
//! hand-assembled bytes. The encoding here is the mirror of
//! [`clob_program::instruction::Reader`], and the two are checked against each other in
//! `tests/wire_format.rs` — a builder that writes bytes the program cannot read is a
//! test failure rather than a runtime one.

use clob_book::{BaseLots, FIFOOrderId, LotConfig, QuoteLots, Side, Ticks};
use clob_engine::{OrderPacket, PostOnlyRejection, SelfTradeBehavior};
use clob_program::instruction::Discriminant;
use clob_program::state::SizeClass;
use solana_instruction::{AccountMeta, Instruction};
use solana_pubkey::Pubkey;

use crate::address::log_authority;

/// Whether an order should emit an event receipt.
///
/// Emission costs about 1,500 compute units and two extra accounts, so it is a choice
/// rather than a default. A market maker cancel-replacing continuously already knows
/// what it submitted; a taker or an aggregator wants the receipt.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum Receipt {
    /// No event. Two accounts.
    #[default]
    Off,
    /// Emit an event. Adds the log authority and the program to the account list.
    On,
}

/// Everything needed to address a market.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct MarketAddresses {
    /// The program.
    pub program_id: Pubkey,
    /// The market account.
    pub market: Pubkey,
    /// Vault holding base deposits.
    pub base_vault: Pubkey,
    /// Vault holding quote deposits.
    pub quote_vault: Pubkey,
    /// Authority owning both vaults.
    pub vault_signer: Pubkey,
    /// SPL Token program.
    pub token_program: Pubkey,
}

/// The canonical SPL Token program id.
pub const TOKEN_PROGRAM_ID: Pubkey = Pubkey::new_from_array([
    6, 221, 246, 225, 215, 101, 161, 147, 217, 203, 225, 70, 206, 235, 121, 172, 28, 180, 133,
    237, 95, 91, 55, 145, 58, 140, 245, 133, 126, 255, 0, 169,
]);

impl MarketAddresses {
    /// Fills in the vault signer and token program from the market address.
    pub fn new(program_id: Pubkey, market: Pubkey, base_vault: Pubkey, quote_vault: Pubkey) -> Self {
        Self {
            vault_signer: crate::address::vault_signer(&program_id, &market).0,
            program_id,
            market,
            base_vault,
            quote_vault,
            token_program: TOKEN_PROGRAM_ID,
        }
    }
}

/// Encoder for one instruction's data.
struct Data(Vec<u8>);

impl Data {
    fn new(discriminant: Discriminant) -> Self {
        Self(vec![discriminant as u8])
    }

    fn u8(mut self, value: u8) -> Self {
        self.0.push(value);
        self
    }

    fn u32(mut self, value: u32) -> Self {
        self.0.extend_from_slice(&value.to_le_bytes());
        self
    }

    fn u64(mut self, value: u64) -> Self {
        self.0.extend_from_slice(&value.to_le_bytes());
        self
    }

    fn side(self, side: Side) -> Self {
        self.u8(side as u8)
    }

    fn order_id(self, id: &FIFOOrderId) -> Self {
        self.u64(id.price_in_ticks.as_u64())
            .u64(id.order_sequence_number)
    }

    /// Order packets are fixed-width up to their kind-specific tail, so an unpriced
    /// market order is a priced packet with the price flag cleared.
    fn packet(self, packet: &OrderPacket) -> Self {
        let (kind, has_price, price) = match packet {
            OrderPacket::Limit { price_in_ticks, .. } => (0u8, true, *price_in_ticks),
            OrderPacket::PostOnly { price_in_ticks, .. } => (1, true, *price_in_ticks),
            OrderPacket::ImmediateOrCancel { price_in_ticks, .. } => (
                2,
                price_in_ticks.is_some(),
                price_in_ticks.unwrap_or(Ticks::ZERO),
            ),
        };

        let base = self
            .u8(kind)
            .side(packet.side())
            .u8(u8::from(has_price))
            .u64(price.as_u64())
            .u64(packet.num_base_lots().as_u64());

        match packet {
            OrderPacket::Limit {
                self_trade_behavior,
                match_limit,
                ..
            } => base.u8(*self_trade_behavior as u8).u32(*match_limit),
            OrderPacket::PostOnly { rejection, .. } => base.u8(*rejection as u8),
            OrderPacket::ImmediateOrCancel {
                min_base_lots_to_fill,
                self_trade_behavior,
                match_limit,
                ..
            } => base
                .u64(min_base_lots_to_fill.as_u64())
                .u8(*self_trade_behavior as u8)
                .u32(*match_limit),
        }
    }

    /// Appends the log-authority bump when a receipt was asked for.
    fn receipt(self, receipt: Receipt, program_id: &Pubkey) -> Self {
        match receipt {
            Receipt::Off => self,
            Receipt::On => self.u8(log_authority(program_id).1),
        }
    }

    fn into_vec(self) -> Vec<u8> {
        self.0
    }
}

/// Appends the two accounts a receipt needs.
fn receipt_accounts(accounts: &mut Vec<AccountMeta>, receipt: Receipt, program_id: &Pubkey) {
    if receipt == Receipt::On {
        accounts.push(AccountMeta::new_readonly(log_authority(program_id).0, false));
        accounts.push(AccountMeta::new_readonly(*program_id, false));
    }
}

/// Creates a market in an account the caller has already allocated.
///
/// Allocation stays with the caller because it needs the payer's signature on a
/// `CreateAccount` from the system program; this instruction validates the size it was
/// given rather than choosing it.
#[allow(clippy::too_many_arguments)]
pub fn initialize_market(
    addresses: &MarketAddresses,
    base_mint: &Pubkey,
    quote_mint: &Pubkey,
    authority: &Pubkey,
    fee_recipient: &Pubkey,
    size_class: SizeClass,
    lot_config: &LotConfig,
    taker_fee_bps: u64,
) -> Instruction {
    let bump = crate::address::vault_signer(&addresses.program_id, &addresses.market).1;
    let data = Data::new(Discriminant::InitializeMarket)
        .u64(size_class as u64)
        .u64(lot_config.base_lots_per_base_unit)
        .u64(lot_config.tick_size_in_quote_lots_per_base_unit)
        .u64(lot_config.base_atoms_per_base_lot)
        .u64(lot_config.quote_atoms_per_quote_lot)
        .u64(taker_fee_bps)
        .u8(bump)
        .into_vec();

    Instruction {
        program_id: addresses.program_id,
        accounts: vec![
            AccountMeta::new(addresses.market, false),
            AccountMeta::new_readonly(*base_mint, false),
            AccountMeta::new_readonly(*quote_mint, false),
            AccountMeta::new_readonly(addresses.base_vault, false),
            AccountMeta::new_readonly(addresses.quote_vault, false),
            AccountMeta::new_readonly(addresses.vault_signer, false),
            AccountMeta::new_readonly(*authority, true),
            AccountMeta::new_readonly(*fee_recipient, false),
        ],
        data,
    }
}

/// Claims a seat, or returns the existing one. Idempotent, so it is safe to send before
/// a first order without reading the market first.
pub fn claim_seat(addresses: &MarketAddresses, trader: &Pubkey) -> Instruction {
    Instruction {
        program_id: addresses.program_id,
        accounts: vec![
            AccountMeta::new(addresses.market, false),
            AccountMeta::new_readonly(*trader, true),
        ],
        data: Data::new(Discriminant::ClaimSeat).into_vec(),
    }
}

/// Returns an empty seat to the market, making room for a new trader.
///
/// Permissionless: no signature from the evicted trader, and none needed. The
/// instruction only succeeds on a seat holding nothing at all, and losing an empty seat
/// costs its owner nothing — claiming is free and idempotent. Requiring a signature
/// would put the market's liveness in the hands of the people who have already stopped
/// participating.
///
/// Use [`MarketState::evictable_seats`](crate::MarketState::evictable_seats) to find
/// candidates without guessing.
pub fn evict_seat(addresses: &MarketAddresses, trader: &Pubkey) -> Instruction {
    Instruction {
        program_id: addresses.program_id,
        accounts: vec![
            AccountMeta::new(addresses.market, false),
            AccountMeta::new_readonly(*trader, false),
        ],
        data: Data::new(Discriminant::EvictSeat).into_vec(),
    }
}

/// Moves tokens in and credits a seat. Amounts are in lots, which convert to atoms
/// exactly — atoms would round down and strand the remainder in the vault.
pub fn deposit(
    addresses: &MarketAddresses,
    trader: &Pubkey,
    trader_base: &Pubkey,
    trader_quote: &Pubkey,
    base_lots: BaseLots,
    quote_lots: QuoteLots,
) -> Instruction {
    Instruction {
        program_id: addresses.program_id,
        accounts: vec![
            AccountMeta::new(addresses.market, false),
            AccountMeta::new_readonly(*trader, true),
            AccountMeta::new(*trader_base, false),
            AccountMeta::new(*trader_quote, false),
            AccountMeta::new(addresses.base_vault, false),
            AccountMeta::new(addresses.quote_vault, false),
            AccountMeta::new_readonly(addresses.token_program, false),
        ],
        data: Data::new(Discriminant::Deposit)
            .u64(base_lots.as_u64())
            .u64(quote_lots.as_u64())
            .into_vec(),
    }
}

/// Debits a seat and moves tokens out. Only free balances can leave; funds locked
/// behind resting orders have to be cancelled first.
pub fn withdraw(
    addresses: &MarketAddresses,
    trader: &Pubkey,
    trader_base: &Pubkey,
    trader_quote: &Pubkey,
    base_lots: BaseLots,
    quote_lots: QuoteLots,
) -> Instruction {
    Instruction {
        program_id: addresses.program_id,
        accounts: vec![
            AccountMeta::new(addresses.market, false),
            AccountMeta::new_readonly(*trader, true),
            AccountMeta::new(*trader_base, false),
            AccountMeta::new(*trader_quote, false),
            AccountMeta::new(addresses.base_vault, false),
            AccountMeta::new(addresses.quote_vault, false),
            AccountMeta::new_readonly(addresses.vault_signer, false),
            AccountMeta::new_readonly(addresses.token_program, false),
        ],
        data: Data::new(Discriminant::Withdraw)
            .u64(base_lots.as_u64())
            .u64(quote_lots.as_u64())
            .into_vec(),
    }
}

/// Submits an order against funds already on the market.
pub fn place_order(
    addresses: &MarketAddresses,
    trader: &Pubkey,
    packet: &OrderPacket,
    receipt: Receipt,
) -> Instruction {
    let mut accounts = vec![
        AccountMeta::new(addresses.market, false),
        AccountMeta::new_readonly(*trader, true),
    ];
    receipt_accounts(&mut accounts, receipt, &addresses.program_id);

    Instruction {
        program_id: addresses.program_id,
        accounts,
        data: Data::new(Discriminant::PlaceOrder)
            .packet(packet)
            .receipt(receipt, &addresses.program_id)
            .into_vec(),
    }
}

/// Cancels a resting order and releases its backing funds.
pub fn cancel_order(
    addresses: &MarketAddresses,
    trader: &Pubkey,
    order_id: &FIFOOrderId,
) -> Instruction {
    Instruction {
        program_id: addresses.program_id,
        accounts: vec![
            AccountMeta::new(addresses.market, false),
            AccountMeta::new_readonly(*trader, true),
        ],
        data: Data::new(Discriminant::CancelOrder)
            .order_id(order_id)
            .into_vec(),
    }
}

/// Shrinks a resting order without giving up its queue position.
pub fn reduce_order(
    addresses: &MarketAddresses,
    trader: &Pubkey,
    order_id: &FIFOOrderId,
    base_lots: BaseLots,
) -> Instruction {
    Instruction {
        program_id: addresses.program_id,
        accounts: vec![
            AccountMeta::new(addresses.market, false),
            AccountMeta::new_readonly(*trader, true),
        ],
        data: Data::new(Discriminant::ReduceOrder)
            .order_id(order_id)
            .u64(base_lots.as_u64())
            .into_vec(),
    }
}

/// Cancels a set of orders and places a set of orders, in one instruction.
///
/// The market-maker cycle. Doing it as separate instructions means a transaction per
/// quote update, and a maker refreshing a two-sided ladder does that continuously.
///
/// Cancels run first, because releasing the capital behind stale quotes is what funds
/// the new ones. A cancel that finds nothing is not an error — the order was filled,
/// which is the ordinary case rather than a failure. Placement is all-or-nothing.
///
/// There is no receipt form: the receipt exists for takers and aggregators, and a maker
/// already knows what it submitted.
///
/// # Panics
///
/// Never. Counts above 255 are rejected by the caller's transaction size long before
/// they reach this, and both lists are truncated to what a `u8` can express.
pub fn batch_update(
    addresses: &MarketAddresses,
    trader: &Pubkey,
    cancels: &[FIFOOrderId],
    orders: &[OrderPacket],
) -> Instruction {
    let mut data = Data::new(Discriminant::BatchUpdate).u8(cancels.len().min(255) as u8);
    for id in cancels.iter().take(255) {
        data = data.order_id(id);
    }

    data = data.u8(orders.len().min(255) as u8);
    for packet in orders.iter().take(255) {
        data = data.packet(packet);
    }

    Instruction {
        program_id: addresses.program_id,
        accounts: vec![
            AccountMeta::new(addresses.market, false),
            AccountMeta::new_readonly(*trader, true),
        ],
        data: data.into_vec(),
    }
}

/// Cancels up to `limit` of the caller's orders on one side.
///
/// The bound is the caller's to set. An unbounded cancel-all on a deep book can exceed
/// the compute budget and revert, which is the worst possible moment to fail.
pub fn cancel_all_orders(
    addresses: &MarketAddresses,
    trader: &Pubkey,
    side: Side,
    limit: u32,
) -> Instruction {
    Instruction {
        program_id: addresses.program_id,
        accounts: vec![
            AccountMeta::new(addresses.market, false),
            AccountMeta::new_readonly(*trader, true),
        ],
        data: Data::new(Discriminant::CancelAllOrders)
            .side(side)
            .u32(limit)
            .into_vec(),
    }
}

/// Sweeps accrued fees to the market's fee recipient. Permissionless — the recipient is
/// fixed in the market header, so there is nothing to gain by calling it.
pub fn collect_fees(addresses: &MarketAddresses, fee_recipient: &Pubkey) -> Instruction {
    Instruction {
        program_id: addresses.program_id,
        accounts: vec![
            AccountMeta::new(addresses.market, false),
            AccountMeta::new(addresses.quote_vault, false),
            AccountMeta::new(*fee_recipient, false),
            AccountMeta::new_readonly(addresses.vault_signer, false),
            AccountMeta::new_readonly(addresses.token_program, false),
        ],
        data: Data::new(Discriminant::CollectFees).into_vec(),
    }
}

/// Deposits, matches and withdraws in one instruction, for a caller holding no balance
/// on the market.
///
/// A swap must be priced: the program moves in the most the order could cost, and an
/// unpriced market buy has no bounded cost.
#[allow(clippy::too_many_arguments)]
pub fn swap(
    addresses: &MarketAddresses,
    trader: &Pubkey,
    trader_base: &Pubkey,
    trader_quote: &Pubkey,
    side: Side,
    price_in_ticks: Ticks,
    num_base_lots: BaseLots,
    min_base_lots_to_fill: BaseLots,
    match_limit: u32,
    receipt: Receipt,
) -> Instruction {
    let mut accounts = vec![
        AccountMeta::new(addresses.market, false),
        AccountMeta::new_readonly(*trader, true),
        AccountMeta::new(*trader_base, false),
        AccountMeta::new(*trader_quote, false),
        AccountMeta::new(addresses.base_vault, false),
        AccountMeta::new(addresses.quote_vault, false),
        AccountMeta::new_readonly(addresses.vault_signer, false),
        AccountMeta::new_readonly(addresses.token_program, false),
    ];
    receipt_accounts(&mut accounts, receipt, &addresses.program_id);

    Instruction {
        program_id: addresses.program_id,
        accounts,
        data: Data::new(Discriminant::Swap)
            .side(side)
            .u64(price_in_ticks.as_u64())
            .u64(num_base_lots.as_u64())
            .u64(min_base_lots_to_fill.as_u64())
            .u32(match_limit)
            .receipt(receipt, &addresses.program_id)
            .into_vec(),
    }
}

// ---------------------------------------------------------------------------------
// Convenience constructors for the common order shapes
// ---------------------------------------------------------------------------------

/// A good-till-cancelled limit order that crosses first and rests the remainder.
pub fn limit(side: Side, price_in_ticks: Ticks, num_base_lots: BaseLots) -> OrderPacket {
    OrderPacket::Limit {
        side,
        price_in_ticks,
        num_base_lots,
        self_trade_behavior: SelfTradeBehavior::DecrementTake,
        match_limit: u32::MAX,
    }
}

/// A quote that rests or is rejected, never taking liquidity.
pub fn post_only(
    side: Side,
    price_in_ticks: Ticks,
    num_base_lots: BaseLots,
    rejection: PostOnlyRejection,
) -> OrderPacket {
    OrderPacket::PostOnly {
        side,
        price_in_ticks,
        num_base_lots,
        rejection,
    }
}

/// An unpriced sweep that keeps whatever it fills.
pub fn market_order(side: Side, num_base_lots: BaseLots, match_limit: u32) -> OrderPacket {
    OrderPacket::ImmediateOrCancel {
        side,
        price_in_ticks: None,
        num_base_lots,
        min_base_lots_to_fill: BaseLots::ZERO,
        self_trade_behavior: SelfTradeBehavior::DecrementTake,
        match_limit,
    }
}

/// An all-or-nothing immediate order.
pub fn fill_or_kill(side: Side, price_in_ticks: Ticks, num_base_lots: BaseLots) -> OrderPacket {
    OrderPacket::ImmediateOrCancel {
        side,
        price_in_ticks: Some(price_in_ticks),
        num_base_lots,
        min_base_lots_to_fill: num_base_lots,
        self_trade_behavior: SelfTradeBehavior::DecrementTake,
        match_limit: u32::MAX,
    }
}
