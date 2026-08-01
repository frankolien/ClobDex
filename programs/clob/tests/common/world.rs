//! A market that persists across instructions, with real vaults and real tokens.
//!
//! The rest of the harness builds a market account by hand and runs one instruction
//! against it. That is right for testing an instruction. It is wrong for testing a
//! *sequence*, because the state a hand-built fixture starts from is a state somebody
//! decided was reachable — and the interesting bugs live in states nobody thought of.
//!
//! So this starts from nothing. Empty market, empty vaults, traders holding tokens in
//! their own wallets, and every subsequent byte of state produced by an instruction the
//! program actually executed.
//!
//! # What it can check that the engine's own tests cannot
//!
//! The engine has property tests for conservation, and they are thorough. They also run
//! against a `Market` in memory, where a deposit is a number going up. Here a deposit is
//! a CPI to the SPL Token program moving atoms into an account the market does not own,
//! which makes a third invariant expressible and it is the one that matters:
//!
//! **The vault holds exactly what the market says it owes.** Internal consistency is not
//! solvency. A market whose books balance perfectly while its vault is short is a market
//! that is already insolvent and has not noticed.
//!
//! # Reverts
//!
//! A failed instruction changes nothing — that is what a revert means on Solana — so the
//! result is discarded rather than merged. Getting this wrong would let the fuzzer build
//! states no validator would ever produce, and every bug it then found would be fiction.

use std::collections::BTreeMap;

use clob_book::{BaseLots, FIFOOrderId, QuoteLots, Side};
use clob_client::instruction::{self as sdk, MarketAddresses};
use clob_engine::{NO_SEAT, TraderKey, TraderState};
use mollusk_svm::Mollusk;
use solana_account::Account;
use solana_instruction::Instruction;
use solana_pubkey::Pubkey;

use super::{
    Fixture, PROGRAM_ID, TestMarket, lot_config, market_of, mint_account, mollusk_with_token,
    token_account, token_balance, wallet,
};

/// Base atoms each trader starts with. Large enough that the campaign is bounded by the
/// program's rules rather than by running out of money.
const START_BASE_ATOMS: u64 = 100_000_000_000_000;

/// Quote atoms each trader starts with.
const START_QUOTE_ATOMS: u64 = 100_000_000_000_000;

/// A wallet with a seat to claim and tokens to trade.
pub struct Trader {
    /// Signs.
    pub wallet: Pubkey,
    /// Its base token account.
    pub base: Pubkey,
    /// Its quote token account.
    pub quote: Pubkey,
}

/// Everything the market touches, and the rules that must hold over all of it.
pub struct World {
    fixture: Fixture,
    mollusk: Mollusk,
    accounts: BTreeMap<Pubkey, Account>,
    traders: Vec<Trader>,
    /// Atoms in existence when the campaign started. Nothing may change these.
    base_atoms_at_start: u64,
    quote_atoms_at_start: u64,
}

impl World {
    /// An empty market, `traders` funded wallets, and vaults holding nothing.
    pub fn new(traders: usize, taker_fee_bps: u64) -> Self {
        let fixture = Fixture::new();
        let mut accounts = BTreeMap::new();

        accounts.insert(fixture.market, fixture.market_account(taker_fee_bps));
        accounts.insert(fixture.base_mint, mint_account(9));
        accounts.insert(fixture.quote_mint, mint_account(6));
        // The vaults start empty. Everything they ever hold arrives through a deposit or
        // a swap that the program itself executed.
        accounts.insert(
            fixture.base_vault,
            token_account(fixture.base_mint, fixture.vault_signer, 0),
        );
        accounts.insert(
            fixture.quote_vault,
            token_account(fixture.quote_mint, fixture.vault_signer, 0),
        );
        accounts.insert(fixture.vault_signer, wallet());
        // Fees are swept to a token account like any other. Counting it in the totals is
        // what keeps a sweep from looking like tokens leaving the world.
        accounts.insert(
            fixture.fee_recipient,
            token_account(fixture.quote_mint, fixture.authority, 0),
        );
        accounts.insert(fixture.authority, wallet());

        let (token_program, token_account_data) = mollusk_svm_programs_token::token::keyed_account();
        accounts.insert(token_program, token_account_data);

        let traders: Vec<Trader> = (0..traders)
            .map(|i| {
                let index = i as u8;
                let trader = Trader {
                    wallet: Pubkey::new_from_array([100 + index; 32]),
                    base: Pubkey::new_from_array([140 + index; 32]),
                    quote: Pubkey::new_from_array([180 + index; 32]),
                };
                accounts.insert(trader.wallet, wallet());
                accounts.insert(
                    trader.base,
                    token_account(fixture.base_mint, trader.wallet, START_BASE_ATOMS),
                );
                accounts.insert(
                    trader.quote,
                    token_account(fixture.quote_mint, trader.wallet, START_QUOTE_ATOMS),
                );
                trader
            })
            .collect();

        let mut world = Self {
            fixture,
            // One instance, reused. Building it loads and verifies the SBF binary, which
            // is far more expensive than executing an instruction with it.
            mollusk: mollusk_with_token(),
            accounts,
            traders,
            base_atoms_at_start: 0,
            quote_atoms_at_start: 0,
        };
        let (base, quote) = world.token_totals();
        world.base_atoms_at_start = base;
        world.quote_atoms_at_start = quote;
        world
    }

    /// Addresses for the SDK builders.
    pub fn addresses(&self) -> MarketAddresses {
        MarketAddresses::new(
            PROGRAM_ID,
            self.fixture.market,
            self.fixture.base_vault,
            self.fixture.quote_vault,
        )
    }

    /// The wallets in this world.
    pub fn traders(&self) -> &[Trader] {
        &self.traders
    }

    /// Where swept fees land.
    pub fn fee_recipient(&self) -> Pubkey {
        self.fixture.fee_recipient
    }

    /// The market as the program left it.
    pub fn market(&self) -> &TestMarket {
        market_of(&self.accounts[&self.fixture.market])
    }

    /// Executes one instruction, keeping the result only if it succeeded.
    ///
    /// Returns whether it landed. A rejection is an ordinary outcome — most randomly
    /// generated instructions are nonsense — and is not distinguished by kind, because
    /// the campaign's claim is about what survives, not about why anything failed.
    pub fn execute(&mut self, instruction: &Instruction) -> bool {
        let keyed: Vec<(Pubkey, Account)> = instruction
            .accounts
            .iter()
            .map(|meta| {
                // An address the world has never heard of is passed as an empty account,
                // which is what a validator would hand the program for one that does not
                // exist. Refusing it here would hide whatever the program does with it.
                let account = self
                    .accounts
                    .get(&meta.pubkey)
                    .cloned()
                    .unwrap_or_else(wallet);
                (meta.pubkey, account)
            })
            .collect();

        let result = self.mollusk.process_instruction(instruction, &keyed);
        if result.raw_result.is_err() {
            return false;
        }
        for (key, account) in result.resulting_accounts {
            self.accounts.insert(key, account);
        }
        true
    }

    /// Every invariant that must hold after every instruction, landed or rejected.
    ///
    /// # Errors
    ///
    /// A description of the first one broken.
    pub fn check(&self) -> Result<(), String> {
        let market = self.market();

        // 1. The market's own account of itself: balances sum to the deposited totals,
        //    locked funds correspond to resting orders, every order has a live owner.
        market
            .check_conservation()
            .map_err(|error| format!("the market is internally inconsistent: {error:?}"))?;

        // 2. Solvency. The books balancing says nothing about whether the money is there.
        let config = lot_config();
        let header = market.header();
        let (base_vault, quote_vault) = self.vault_atoms();
        let owed_base = header.base_lots_deposited.as_u64() * config.base_atoms_per_base_lot;
        let owed_quote = header.quote_lots_deposited.as_u64() * config.quote_atoms_per_quote_lot;

        if base_vault != owed_base {
            return Err(format!(
                "the base vault holds {base_vault} atoms against {owed_base} owed"
            ));
        }
        if quote_vault != owed_quote {
            return Err(format!(
                "the quote vault holds {quote_vault} atoms against {owed_quote} owed"
            ));
        }

        // 3. Nothing was minted and nothing was burned. The market moves tokens between
        //    accounts; it has no authority to create them, and a total that changed would
        //    mean it had found one.
        let (base_total, quote_total) = self.token_totals();
        if base_total != self.base_atoms_at_start {
            return Err(format!(
                "base atoms went from {} to {base_total}",
                self.base_atoms_at_start
            ));
        }
        if quote_total != self.quote_atoms_at_start {
            return Err(format!(
                "quote atoms went from {} to {quote_total}",
                self.quote_atoms_at_start
            ));
        }
        Ok(())
    }

    /// Winds the market down the way an operator would: cancel everything, withdraw
    /// everything, sweep the fees.
    ///
    /// Cancels are bounded and repeated rather than unbounded. An unbounded cancel-all on
    /// a deep book can exceed the compute budget and revert, and a wind-down that can
    /// fail on a full book is not a wind-down.
    pub fn drain(&mut self) {
        for index in 0..self.traders.len() {
            for side in [Side::Bid, Side::Ask] {
                // Bounded so a stuck cancel cannot spin: each landed call clears at least
                // one order, so the loop is bounded by the book's capacity either way.
                for _ in 0..64 {
                    if self.resting(index, side).is_empty() {
                        break;
                    }
                    let instruction = sdk::cancel_all_orders(
                        &self.addresses(),
                        &self.traders[index].wallet,
                        side,
                        16,
                    );
                    if !self.execute(&instruction) {
                        break;
                    }
                }
            }
        }

        for index in 0..self.traders.len() {
            let (base, quote) = self.free_balances(index);
            if base == 0 && quote == 0 {
                continue;
            }
            let trader = &self.traders[index];
            let instruction = sdk::withdraw(
                &self.addresses(),
                &trader.wallet,
                &trader.base,
                &trader.quote,
                BaseLots(base),
                QuoteLots(quote),
            );
            assert!(
                self.execute(&instruction),
                "a trader with no resting orders must be able to withdraw everything it holds"
            );
        }

        let instruction = sdk::collect_fees(&self.addresses(), &self.fixture.fee_recipient);
        assert!(self.execute(&instruction), "fees must always be sweepable");
    }

    /// One trader's resting orders on a side, best first.
    pub fn resting(&self, trader: usize, side: Side) -> Vec<FIFOOrderId> {
        let seat = self.seat(trader);
        if seat == NO_SEAT {
            return Vec::new();
        }
        self.market()
            .book()
            .iter_side(side)
            .filter(|entry| entry.value.trader_index == u64::from(seat))
            .map(|entry| entry.key)
            .collect()
    }

    /// All of one trader's resting orders.
    pub fn all_resting(&self, trader: usize) -> Vec<FIFOOrderId> {
        let mut orders = self.resting(trader, Side::Bid);
        orders.extend(self.resting(trader, Side::Ask));
        orders
    }

    /// How many orders are on the book, both sides.
    pub fn book_len(&self) -> usize {
        let book = self.market().book();
        book.iter_side(Side::Bid).count() + book.iter_side(Side::Ask).count()
    }

    /// A trader's seat, or [`NO_SEAT`].
    pub fn seat(&self, trader: usize) -> u32 {
        self.market()
            .seat_index(&TraderKey(self.traders[trader].wallet.to_bytes()))
    }

    /// A trader's balances, or zeroes if it holds no seat.
    pub fn balances(&self, trader: usize) -> TraderState {
        let seat = self.seat(trader);
        if seat == NO_SEAT {
            return TraderState::default();
        }
        self.market()
            .traders()
            .state(seat)
            .copied()
            .unwrap_or_default()
    }

    /// A trader's withdrawable balances.
    pub fn free_balances(&self, trader: usize) -> (u64, u64) {
        let state = self.balances(trader);
        (
            state.base_lots_free.as_u64(),
            state.quote_lots_free.as_u64(),
        )
    }

    /// Atoms in the two vaults.
    pub fn vault_atoms(&self) -> (u64, u64) {
        (
            token_balance(&self.accounts[&self.fixture.base_vault]),
            token_balance(&self.accounts[&self.fixture.quote_vault]),
        )
    }

    /// Every base and quote atom this world contains, wherever it is sitting.
    pub fn token_totals(&self) -> (u64, u64) {
        let mut base = token_balance(&self.accounts[&self.fixture.base_vault]);
        let mut quote = token_balance(&self.accounts[&self.fixture.quote_vault]);
        quote += token_balance(&self.accounts[&self.fixture.fee_recipient]);
        for trader in &self.traders {
            base += token_balance(&self.accounts[&trader.base]);
            quote += token_balance(&self.accounts[&trader.quote]);
        }
        (base, quote)
    }

    /// What each trader is holding in its own wallet.
    pub fn wallet_atoms(&self, trader: usize) -> (u64, u64) {
        (
            token_balance(&self.accounts[&self.traders[trader].base]),
            token_balance(&self.accounts[&self.traders[trader].quote]),
        )
    }

    /// Atoms swept to the fee recipient.
    pub fn collected_fee_atoms(&self) -> u64 {
        token_balance(&self.accounts[&self.fixture.fee_recipient])
    }

    /// Atoms the world began with.
    pub fn starting_totals(&self) -> (u64, u64) {
        (self.base_atoms_at_start, self.quote_atoms_at_start)
    }

    // -----------------------------------------------------------------------------
    // Faults, for proving the checks are live
    //
    // An invariant that has never been observed failing is indistinguishable from one
    // that is not being evaluated, and this whole harness is worth nothing if its checks
    // are vacuous. These break one rule each, without going through the program, so each
    // check can be shown rejecting exactly what it claims to catch.
    // -----------------------------------------------------------------------------

    /// Moves base atoms out of the vault into a trader's wallet behind the market's back.
    ///
    /// Breaks solvency alone: no token was created, so the totals still balance and only
    /// the vault check can see it.
    pub fn embezzle_base(&mut self, trader: usize, atoms: u64) {
        let vault = self.fixture.base_vault;
        let wallet = self.traders[trader].base;
        let (vault_held, wallet_held) = (
            token_balance(&self.accounts[&vault]),
            token_balance(&self.accounts[&wallet]),
        );
        set_token_balance(self.accounts.get_mut(&vault).expect("the vault"), vault_held - atoms);
        set_token_balance(
            self.accounts.get_mut(&wallet).expect("the wallet"),
            wallet_held + atoms,
        );
    }

    /// Creates base atoms in a trader's wallet from nothing.
    ///
    /// Breaks the supply alone: the vault still holds what the market owes, so only the
    /// totals check can see it.
    pub fn counterfeit_base(&mut self, trader: usize, atoms: u64) {
        let wallet = self.traders[trader].base;
        let held = token_balance(&self.accounts[&wallet]);
        set_token_balance(self.accounts.get_mut(&wallet).expect("the wallet"), held + atoms);
    }
}

/// Writes the amount field of an SPL token account, at the offset `token_balance` reads.
fn set_token_balance(account: &mut Account, amount: u64) {
    account.data[64..72].copy_from_slice(&amount.to_le_bytes());
}
