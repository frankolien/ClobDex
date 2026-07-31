//! Mollusk harness: builds market accounts and instruction data for the tests.
//!
//! Mollusk executes the *compiled SBF binary*, not the Rust library, so these tests
//! measure what actually runs on a validator — including the compute a real
//! transaction would be charged.

#![allow(dead_code)]

use clob_book::{BaseLots, FIFOOrderId, LotConfig, QuoteLots, Side, Ticks};
use clob_engine::{FeeSchedule, Market};
use mollusk_svm::Mollusk;
use solana_account::Account;
use solana_instruction::{AccountMeta, Instruction};
use solana_pubkey::Pubkey;

use clob_program::state::{
    HEADER_LEN, MARKET_DISCRIMINATOR, MARKET_VERSION, MarketAccountHeader, SizeClass,
};

/// Must match the id the binary is deployed under; Mollusk takes it as given.
pub const PROGRAM_ID: Pubkey = Pubkey::new_from_array([9u8; 32]);

/// The size class the tests exercise. Small keeps account fixtures manageable while
/// still going through exactly the same code path as the larger classes.
pub type TestMarket = Market<128, 128, 32>;

pub fn mollusk() -> Mollusk {
    // Resolved relative to the workspace target directory by `cargo build-sbf`.
    Mollusk::new(&PROGRAM_ID, "clob_program")
}

/// SOL/USDC-shaped geometry: one base lot is 0.001 SOL, one tick is $0.001.
pub fn lot_config() -> LotConfig {
    LotConfig::new(1_000, 1_000, 1_000_000, 1).unwrap()
}

/// Fixed addresses so tests read deterministically.
pub struct Fixture {
    pub market: Pubkey,
    pub base_mint: Pubkey,
    pub quote_mint: Pubkey,
    pub base_vault: Pubkey,
    pub quote_vault: Pubkey,
    pub vault_signer: Pubkey,
    pub authority: Pubkey,
    pub fee_recipient: Pubkey,
}

impl Fixture {
    pub fn new() -> Self {
        let market = Pubkey::new_from_array([1u8; 32]);
        let (vault_signer, _) =
            Pubkey::find_program_address(&[b"vault", market.as_ref()], &PROGRAM_ID);
        Self {
            market,
            base_mint: Pubkey::new_from_array([2u8; 32]),
            quote_mint: Pubkey::new_from_array([3u8; 32]),
            base_vault: Pubkey::new_from_array([4u8; 32]),
            quote_vault: Pubkey::new_from_array([5u8; 32]),
            vault_signer,
            authority: Pubkey::new_from_array([7u8; 32]),
            fee_recipient: Pubkey::new_from_array([8u8; 32]),
        }
    }

    /// An initialized market account, built directly rather than through the
    /// instruction, so order-entry tests measure order entry and nothing else.
    pub fn market_account(&self, taker_fee_bps: u64) -> Account {
        let mut data = vec![0u8; SizeClass::Small.account_len()];
        let (header_bytes, market_bytes) = data.split_at_mut(HEADER_LEN);

        *bytemuck::from_bytes_mut::<MarketAccountHeader>(header_bytes) = MarketAccountHeader {
            discriminator: MARKET_DISCRIMINATOR,
            version: MARKET_VERSION,
            size_class: SizeClass::Small as u64,
            vault_signer_bump: Pubkey::find_program_address(
                &[b"vault", self.market.as_ref()],
                &PROGRAM_ID,
            )
            .1 as u64,
            base_mint: self.base_mint.to_bytes(),
            quote_mint: self.quote_mint.to_bytes(),
            base_vault: self.base_vault.to_bytes(),
            quote_vault: self.quote_vault.to_bytes(),
            authority: self.authority.to_bytes(),
            fee_recipient: self.fee_recipient.to_bytes(),
        };

        let market = bytemuck::from_bytes_mut::<TestMarket>(
            &mut market_bytes[..TestMarket::SIZE_IN_BYTES],
        );
        market
            .initialize(lot_config(), FeeSchedule::new(taker_fee_bps).unwrap())
            .unwrap();

        Account {
            lamports: 1_000_000_000,
            data,
            owner: PROGRAM_ID,
            executable: false,
            rent_epoch: 0,
        }
    }

    /// A funded seat, written straight into the account so tests do not have to run
    /// deposit first.
    pub fn seat(&self, account: &mut Account, trader: Pubkey, base: u64, quote: u64) {
        let market = market_mut(account);
        let seat = market
            .claim_seat(clob_engine::TraderKey(trader.to_bytes()))
            .unwrap();
        market
            .deposit(seat, BaseLots(base), QuoteLots(quote))
            .unwrap();
    }
}

/// Borrows the engine market out of an account's bytes.
pub fn market_mut(account: &mut Account) -> &mut TestMarket {
    bytemuck::from_bytes_mut(&mut account.data[HEADER_LEN..HEADER_LEN + TestMarket::SIZE_IN_BYTES])
}

/// Reads the engine market out of an account's bytes.
pub fn market_of(account: &Account) -> &TestMarket {
    bytemuck::from_bytes(&account.data[HEADER_LEN..HEADER_LEN + TestMarket::SIZE_IN_BYTES])
}

/// A signer account with no data.
pub fn wallet() -> Account {
    Account {
        lamports: 1_000_000_000,
        data: vec![],
        owner: solana_pubkey::Pubkey::default(),
        executable: false,
        rent_epoch: 0,
    }
}

// ---------------------------------------------------------------------------------
// Instruction builders
// ---------------------------------------------------------------------------------

fn trade_ix(market: Pubkey, trader: Pubkey, data: Vec<u8>) -> Instruction {
    Instruction {
        program_id: PROGRAM_ID,
        accounts: vec![
            AccountMeta::new(market, false),
            AccountMeta::new_readonly(trader, true),
        ],
        data,
    }
}

/// The program-wide event signer.
pub fn log_authority() -> (Pubkey, u8) {
    Pubkey::find_program_address(&[b"log"], &PROGRAM_ID)
}

/// Rewrites a trade instruction into its receipt form: two more accounts and a trailing
/// bump byte.
pub fn with_receipt(mut instruction: Instruction) -> Instruction {
    let (authority, bump) = log_authority();
    instruction.data.push(bump);
    instruction
        .accounts
        .push(AccountMeta::new_readonly(authority, false));
    instruction
        .accounts
        .push(AccountMeta::new_readonly(PROGRAM_ID, false));
    instruction
}

/// Encodes a `PlaceOrder` for a limit order.
pub fn limit_order_ix(
    market: Pubkey,
    trader: Pubkey,
    side: Side,
    price: u64,
    size: u64,
    match_limit: u32,
) -> Instruction {
    let mut data = vec![4u8, 0, side as u8, 1];
    data.extend_from_slice(&price.to_le_bytes());
    data.extend_from_slice(&size.to_le_bytes());
    data.push(0); // DecrementTake
    data.extend_from_slice(&match_limit.to_le_bytes());
    trade_ix(market, trader, data)
}

/// Encodes a `PlaceOrder` for a post-only order that rejects on cross.
pub fn post_only_ix(
    market: Pubkey,
    trader: Pubkey,
    side: Side,
    price: u64,
    size: u64,
) -> Instruction {
    let mut data = vec![4u8, 1, side as u8, 1];
    data.extend_from_slice(&price.to_le_bytes());
    data.extend_from_slice(&size.to_le_bytes());
    data.push(0); // Reject
    trade_ix(market, trader, data)
}

/// Encodes a `PlaceOrder` for an unpriced market order.
pub fn market_order_ix(
    market: Pubkey,
    trader: Pubkey,
    side: Side,
    size: u64,
    match_limit: u32,
) -> Instruction {
    let mut data = vec![4u8, 2, side as u8, 0];
    data.extend_from_slice(&0u64.to_le_bytes());
    data.extend_from_slice(&size.to_le_bytes());
    data.extend_from_slice(&0u64.to_le_bytes()); // no minimum fill
    data.push(0); // DecrementTake
    data.extend_from_slice(&match_limit.to_le_bytes());
    trade_ix(market, trader, data)
}

/// Encodes a `CancelOrder`.
pub fn cancel_ix(market: Pubkey, trader: Pubkey, id: FIFOOrderId) -> Instruction {
    let mut data = vec![5u8];
    data.extend_from_slice(&id.price_in_ticks.as_u64().to_le_bytes());
    data.extend_from_slice(&id.order_sequence_number.to_le_bytes());
    trade_ix(market, trader, data)
}

/// Encodes a `CancelAllOrders`.
pub fn cancel_all_ix(market: Pubkey, trader: Pubkey, side: Side, limit: u32) -> Instruction {
    let mut data = vec![7u8, side as u8];
    data.extend_from_slice(&limit.to_le_bytes());
    trade_ix(market, trader, data)
}

/// Encodes a `ClaimSeat`.
pub fn claim_seat_ix(market: Pubkey, trader: Pubkey) -> Instruction {
    trade_ix(market, trader, vec![1u8])
}

/// Convenience: the id of the caller's most recently placed order on `side`.
pub fn newest_order(account: &Account, side: Side) -> FIFOOrderId {
    market_of(account)
        .book()
        .iter_side(side)
        .map(|e| e.key)
        .max_by_key(|id| id.sequence_number())
        .expect("expected a resting order")
}

/// Reads a seat's free balances.
pub fn free_balances(account: &Account, trader: Pubkey) -> (u64, u64) {
    let market = market_of(account);
    let seat = market.seat_index(&clob_engine::TraderKey(trader.to_bytes()));
    let state = market.traders().state(seat).unwrap();
    (
        state.base_lots_free.as_u64(),
        state.quote_lots_free.as_u64(),
    )
}

/// Rests `count` orders at descending prices from `start_price`, straight into the
/// account. Used to build book depth for the compute benchmarks.
pub fn seed_depth(
    account: &mut Account,
    trader: Pubkey,
    side: Side,
    start_price: u64,
    count: u64,
    size: u64,
) {
    let market = market_mut(account);
    let seat = market.seat_index(&clob_engine::TraderKey(trader.to_bytes()));
    for i in 0..count {
        let price = match side {
            Side::Bid => start_price - i,
            Side::Ask => start_price + i,
        };
        market
            .place_order(
                seat,
                clob_engine::OrderPacket::post_only(side, Ticks(price), BaseLots(size)),
                &mut (),
            )
            .expect("depth should rest");
    }
}

// ---------------------------------------------------------------------------------
// SPL token fixtures
//
// The swap, deposit and withdraw instructions do real token CPI, so testing them means
// running the actual SPL Token program rather than writing balances into the market by
// hand.
// ---------------------------------------------------------------------------------

use mollusk_svm_programs_token::token;
use spl_token::state::{Account as TokenAccount, AccountState, Mint};

/// A Mollusk instance with the SPL Token program loaded.
pub fn mollusk_with_token() -> Mollusk {
    let mut mollusk = mollusk();
    token::add_program(&mut mollusk);
    mollusk
}

/// An initialised mint with no authority.
pub fn mint_account(decimals: u8) -> Account {
    token::create_account_for_mint(Mint {
        mint_authority: solana_pubkey::Pubkey::default().into(),
        supply: u64::MAX / 2,
        decimals,
        is_initialized: true,
        freeze_authority: None.into(),
    })
}

/// A token account holding `amount` of `mint`, owned by `owner`.
pub fn token_account(mint: Pubkey, owner: Pubkey, amount: u64) -> Account {
    token::create_account_for_token_account(TokenAccount {
        mint,
        owner,
        amount,
        delegate: None.into(),
        state: AccountState::Initialized,
        is_native: None.into(),
        delegated_amount: 0,
        close_authority: None.into(),
    })
}

/// The token balance recorded in an account's data.
pub fn token_balance(account: &Account) -> u64 {
    u64::from_le_bytes(account.data[64..72].try_into().unwrap())
}

/// Encodes a `Swap`.
#[allow(clippy::too_many_arguments)]
pub fn swap_ix(
    fixture: &Fixture,
    trader: Pubkey,
    trader_base: Pubkey,
    trader_quote: Pubkey,
    side: Side,
    price: u64,
    size: u64,
    min_fill: u64,
    match_limit: u32,
) -> Instruction {
    let mut data = vec![10u8, side as u8];
    data.extend_from_slice(&price.to_le_bytes());
    data.extend_from_slice(&size.to_le_bytes());
    data.extend_from_slice(&min_fill.to_le_bytes());
    data.extend_from_slice(&match_limit.to_le_bytes());

    Instruction {
        program_id: PROGRAM_ID,
        accounts: vec![
            AccountMeta::new(fixture.market, false),
            AccountMeta::new_readonly(trader, true),
            AccountMeta::new(trader_base, false),
            AccountMeta::new(trader_quote, false),
            AccountMeta::new(fixture.base_vault, false),
            AccountMeta::new(fixture.quote_vault, false),
            AccountMeta::new_readonly(fixture.vault_signer, false),
            AccountMeta::new_readonly(token::ID, false),
        ],
        data,
    }
}

/// Encodes a `Deposit` or `Withdraw`. Amounts are in lots.
pub fn funds_ix(
    fixture: &Fixture,
    trader: Pubkey,
    trader_base: Pubkey,
    trader_quote: Pubkey,
    withdraw: bool,
    base_lots: u64,
    quote_lots: u64,
) -> Instruction {
    let mut data = vec![if withdraw { 3u8 } else { 2u8 }];
    data.extend_from_slice(&base_lots.to_le_bytes());
    data.extend_from_slice(&quote_lots.to_le_bytes());

    let mut accounts = vec![
        AccountMeta::new(fixture.market, false),
        AccountMeta::new_readonly(trader, true),
        AccountMeta::new(trader_base, false),
        AccountMeta::new(trader_quote, false),
        AccountMeta::new(fixture.base_vault, false),
        AccountMeta::new(fixture.quote_vault, false),
    ];
    if withdraw {
        accounts.push(AccountMeta::new_readonly(fixture.vault_signer, false));
    }
    accounts.push(AccountMeta::new_readonly(
        mollusk_svm_programs_token::token::ID,
        false,
    ));

    Instruction {
        program_id: PROGRAM_ID,
        accounts,
        data,
    }
}
