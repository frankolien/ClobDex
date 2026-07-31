//! From an empty ledger to a settled trade.
//!
//! Every other test in this repo starts from a market account built by hand. This one
//! starts from nothing: it creates the market through the SDK, funds two wallets, and
//! runs a full maker/taker cycle against the real program and the real SPL Token
//! program under LiteSVM.
//!
//! That makes it the only test that would catch a mistake in how the pieces fit
//! together — a wrong account size, a vault owned by the wrong authority, a signer that
//! was never required — as opposed to a mistake inside any one of them.

use clob_book::{BaseLots, LotConfig, QuoteLots, Side, Ticks};
use clob_client::instruction::{self as sdk, MarketAddresses, Receipt};
use clob_client::setup::{CreateMarketParams, TOKEN_ACCOUNT_LEN, create_market};
use clob_client::state::MarketState;
use clob_engine::TraderKey;
use clob_program::state::SizeClass;
use litesvm::LiteSVM;
use solana_account::Account;
use solana_instruction::Instruction;
use solana_keypair::Keypair;
use solana_message::Message;
use solana_pubkey::Pubkey;
use solana_signer::Signer;
use solana_transaction::Transaction;

const PROGRAM_ID: Pubkey = Pubkey::new_from_array([9u8; 32]);

/// SOL/USDC-shaped: one base lot is 0.001 SOL, one tick is $0.001.
fn lot_config() -> LotConfig {
    LotConfig::new(1_000, 1_000, 1_000_000, 1).unwrap()
}

fn program_path() -> std::path::PathBuf {
    let dir = std::env::var("SBF_OUT_DIR").unwrap_or_else(|_| "target/deploy".to_string());
    std::path::PathBuf::from(dir).join("clob_program.so")
}

/// A live cluster with the program and SPL Token loaded, and a funded payer.
struct World {
    svm: LiteSVM,
    payer: Keypair,
    base_mint: Pubkey,
    quote_mint: Pubkey,
}

impl World {
    fn new() -> Self {
        let mut svm = LiteSVM::new();
        svm.add_program_from_file(PROGRAM_ID, program_path())
            .expect("run `cargo build-sbf --manifest-path programs/clob/Cargo.toml` first");
        svm.add_program(
            mollusk_svm_programs_token::token::ID,
            mollusk_svm_programs_token::token::ELF,
        )
        .expect("token program should load");

        let payer = Keypair::new();
        svm.airdrop(&payer.pubkey(), 1_000_000_000_000).unwrap();

        let base_mint = Pubkey::new_unique();
        let quote_mint = Pubkey::new_unique();
        svm.set_account(base_mint, mint_account(9)).unwrap();
        svm.set_account(quote_mint, mint_account(6)).unwrap();

        Self {
            svm,
            payer,
            base_mint,
            quote_mint,
        }
    }

    /// Signs and sends, failing loudly with the program logs when it does not land.
    fn send(&mut self, instructions: &[Instruction], signers: &[&Keypair]) {
        let message = Message::new(instructions, Some(&self.payer.pubkey()));
        let tx = Transaction::new(signers, message, self.svm.latest_blockhash());
        if let Err(failure) = self.svm.send_transaction(tx) {
            panic!("transaction failed: {:?}\n{}", failure.err, failure.meta.pretty_logs());
        }
    }

    fn account(&self, key: &Pubkey) -> Account {
        self.svm.get_account(key).expect("account should exist")
    }

    /// A funded wallet with token accounts for both mints.
    fn wallet(&mut self, base: u64, quote: u64) -> (Keypair, Pubkey, Pubkey) {
        let owner = Keypair::new();
        self.svm.airdrop(&owner.pubkey(), 1_000_000_000).unwrap();

        let base_account = Pubkey::new_unique();
        let quote_account = Pubkey::new_unique();
        self.svm
            .set_account(base_account, token_account(self.base_mint, owner.pubkey(), base))
            .unwrap();
        self.svm
            .set_account(quote_account, token_account(self.quote_mint, owner.pubkey(), quote))
            .unwrap();

        (owner, base_account, quote_account)
    }
}

fn mint_account(decimals: u8) -> Account {
    mollusk_svm_programs_token::token::create_account_for_mint(spl_token::state::Mint {
        mint_authority: Pubkey::default().into(),
        supply: u64::MAX / 2,
        decimals,
        is_initialized: true,
        freeze_authority: None.into(),
    })
}

fn token_account(mint: Pubkey, owner: Pubkey, amount: u64) -> Account {
    mollusk_svm_programs_token::token::create_account_for_token_account(
        spl_token::state::Account {
            mint,
            owner,
            amount,
            delegate: None.into(),
            state: spl_token::state::AccountState::Initialized,
            is_native: None.into(),
            delegated_amount: 0,
            close_authority: None.into(),
        },
    )
}

fn token_balance(account: &Account) -> u64 {
    u64::from_le_bytes(account.data[64..72].try_into().unwrap())
}

/// Creates a market through the SDK, sweeping fees to `fee_recipient`.
fn create_with_recipient(
    world: &mut World,
    taker_fee_bps: u64,
    fee_recipient: Pubkey,
) -> MarketAddresses {
    let market = Keypair::new();
    let base_vault = Keypair::new();
    let quote_vault = Keypair::new();
    let authority = world.payer.insecure_clone();

    let params = CreateMarketParams {
        program_id: PROGRAM_ID,
        payer: world.payer.pubkey(),
        market: market.pubkey(),
        base_vault: base_vault.pubkey(),
        quote_vault: quote_vault.pubkey(),
        base_mint: world.base_mint,
        quote_mint: world.quote_mint,
        authority: authority.pubkey(),
        fee_recipient,
        size_class: SizeClass::Small,
        lot_config: lot_config(),
        taker_fee_bps,
        market_rent_lamports: world
            .svm
            .minimum_balance_for_rent_exemption(SizeClass::Small.account_len()),
        vault_rent_lamports: world
            .svm
            .minimum_balance_for_rent_exemption(TOKEN_ACCOUNT_LEN as usize),
    };

    let setup = create_market(&params);
    world.send(
        &setup.instructions,
        &[&world.payer.insecure_clone(), &market, &base_vault, &quote_vault],
    );
    setup.addresses
}

/// A market whose fees go nowhere useful; fine for tests that never sweep.
fn create(world: &mut World, taker_fee_bps: u64) -> MarketAddresses {
    let recipient = world.payer.pubkey();
    create_with_recipient(world, taker_fee_bps, recipient)
}

#[test]
fn a_market_can_be_created_from_nothing() {
    let mut world = World::new();
    let addresses = create(&mut world, 2);

    let state = MarketState::decode(&world.account(&addresses.market).data)
        .expect("the created account should decode as a market");

    assert_eq!(state.size_class, SizeClass::Small);
    assert_eq!(state.lot_config(), &lot_config());
    assert_eq!(state.fees().taker_fee_bps, 2);
    assert_eq!(state.account.base_mint, world.base_mint.to_bytes());
    assert_eq!(state.account.base_vault, addresses.base_vault.to_bytes());
    assert!(state.bids.is_empty() && state.asks.is_empty());
    assert!(state.traders.is_empty());

    // Both vaults exist, are empty, and answer to the market's PDA — which is what
    // makes them unspendable by anyone but the program.
    for vault in [addresses.base_vault, addresses.quote_vault] {
        let account = world.account(&vault);
        assert_eq!(account.owner, mollusk_svm_programs_token::token::ID);
        assert_eq!(token_balance(&account), 0);
        assert_eq!(&account.data[32..64], addresses.vault_signer.as_ref());
    }
}

#[test]
fn a_market_cannot_be_created_twice_in_the_same_account() {
    let mut world = World::new();
    let addresses = create(&mut world, 0);

    // Re-running just the initialise step against a live market must be refused, or an
    // authority could rewrite the lot geometry and revalue every resting order.
    let reinit = sdk::initialize_market(
        &addresses,
        &world.base_mint,
        &world.quote_mint,
        &world.payer.pubkey(),
        &world.payer.pubkey(),
        SizeClass::Small,
        &lot_config(),
        9_999,
    );
    let message = Message::new(&[reinit], Some(&world.payer.pubkey()));
    let tx = Transaction::new(&[&world.payer], message, world.svm.latest_blockhash());

    assert!(
        world.svm.send_transaction(tx).is_err(),
        "re-initialising a live market must fail"
    );
}

#[test]
fn a_full_maker_taker_cycle_settles_correctly() {
    let mut world = World::new();
    let addresses = create(&mut world, 10);
    let config = lot_config();

    // The maker brings 100 base lots; the taker brings quote.
    let (maker, maker_base, maker_quote) =
        world.wallet(100 * config.base_atoms_per_base_lot, 0);
    let (taker, taker_base, taker_quote) = world.wallet(0, 1_000_000);

    // Deposit, then rest an offer at 100 ticks.
    world.send(
        &[
            sdk::deposit(&addresses, &maker.pubkey(), &maker_base, &maker_quote, BaseLots(100), QuoteLots::ZERO),
            sdk::place_order(
                &addresses,
                &maker.pubkey(),
                &sdk::post_only(Side::Ask, Ticks(100), BaseLots(40), clob_engine::PostOnlyRejection::Reject),
                Receipt::Off,
            ),
        ],
        &[&world.payer.insecure_clone(), &maker],
    );

    let state = MarketState::decode(&world.account(&addresses.market).data).unwrap();
    assert_eq!(state.best_ask().unwrap().price_in_ticks(), Ticks(100));
    assert_eq!(state.level_two(Side::Ask, 5)[0].base_lots, BaseLots(40));
    // Deposited base really moved into the vault.
    assert_eq!(
        token_balance(&world.account(&addresses.base_vault)),
        100 * config.base_atoms_per_base_lot
    );

    // The taker holds no seat and no balance: it swaps.
    world.send(
        &[sdk::swap(
            &addresses,
            &taker.pubkey(),
            &taker_base,
            &taker_quote,
            Side::Bid,
            Ticks(105),
            BaseLots(25),
            BaseLots(25),
            16,
            Receipt::On,
        )],
        &[&world.payer.insecure_clone(), &taker],
    );

    // 25 base lots landed in the taker's wallet, paid for at the maker's price plus fee.
    assert_eq!(
        token_balance(&world.account(&taker_base)),
        25 * config.base_atoms_per_base_lot
    );
    let spent = 1_000_000 - token_balance(&world.account(&taker_quote));
    assert_eq!(spent, 2_500 + 3, "2500 quote lots at 10 bps, rounded up");

    let state = MarketState::decode(&world.account(&addresses.market).data).unwrap();
    assert_eq!(state.best_ask().unwrap().num_base_lots, BaseLots(15));
    assert_eq!(state.header.unclaimed_quote_lot_fees, QuoteLots(3));
    // The swapper left no seat behind; only the maker holds one.
    assert_eq!(state.traders.len(), 1);
    assert_eq!(
        state.trader(&TraderKey(maker.pubkey().to_bytes())).unwrap().quote_lots_free,
        QuoteLots(2_500)
    );

    // The maker cancels the rest and withdraws everything.
    world.send(
        &[
            sdk::cancel_all_orders(&addresses, &maker.pubkey(), Side::Ask, 16),
            sdk::withdraw(
                &addresses,
                &maker.pubkey(),
                &maker_base,
                &maker_quote,
                BaseLots(75),
                QuoteLots(2_500),
            ),
        ],
        &[&world.payer.insecure_clone(), &maker],
    );

    // Everything the maker put in came back, less what it sold, plus what it earned.
    assert_eq!(
        token_balance(&world.account(&maker_base)),
        75 * config.base_atoms_per_base_lot
    );
    assert_eq!(token_balance(&world.account(&maker_quote)), 2_500);

    // The vault now holds only the venue's unswept fee.
    let state = MarketState::decode(&world.account(&addresses.market).data).unwrap();
    assert!(state.asks.is_empty());
    assert_eq!(state.header.quote_lots_deposited, QuoteLots(3));
    assert_eq!(token_balance(&world.account(&addresses.quote_vault)), 3);
    assert_eq!(token_balance(&world.account(&addresses.base_vault)), 0);
}

#[test]
fn fees_reach_the_recipient() {
    let mut world = World::new();
    let config = lot_config();

    // A real quote token account for the venue to be paid into.
    let (_treasury_owner, _, treasury) = world.wallet(0, 0);
    let addresses = create_with_recipient(&mut world, 10, treasury);

    let (maker, maker_base, maker_quote) =
        world.wallet(100 * config.base_atoms_per_base_lot, 0);
    let (taker, taker_base, taker_quote) = world.wallet(0, 1_000_000);

    world.send(
        &[
            sdk::deposit(&addresses, &maker.pubkey(), &maker_base, &maker_quote, BaseLots(100), QuoteLots::ZERO),
            sdk::place_order(
                &addresses,
                &maker.pubkey(),
                &sdk::post_only(Side::Ask, Ticks(100), BaseLots(50), clob_engine::PostOnlyRejection::Reject),
                Receipt::Off,
            ),
        ],
        &[&world.payer.insecure_clone(), &maker],
    );
    world.send(
        &[sdk::swap(
            &addresses,
            &taker.pubkey(),
            &taker_base,
            &taker_quote,
            Side::Bid,
            Ticks(100),
            BaseLots(50),
            BaseLots(0),
            16,
            Receipt::Off,
        )],
        &[&world.payer.insecure_clone(), &taker],
    );

    // 5000 quote lots at 10 bps.
    let before = MarketState::decode(&world.account(&addresses.market).data).unwrap();
    assert_eq!(before.header.unclaimed_quote_lot_fees, QuoteLots(5));
    assert_eq!(token_balance(&world.account(&treasury)), 0);

    // Collecting is permissionless -- anyone may pay the compute to move fees to the
    // recipient the market already names, so a random wallet sends it.
    let stranger = Keypair::new();
    world.svm.airdrop(&stranger.pubkey(), 1_000_000_000).unwrap();
    let collect = sdk::collect_fees(&addresses, &treasury);
    let message = Message::new(&[collect], Some(&stranger.pubkey()));
    let tx = Transaction::new(&[&stranger], message, world.svm.latest_blockhash());
    world.svm.send_transaction(tx).expect("anyone may sweep fees");

    // The tokens actually moved, and the market no longer counts them as its own.
    assert_eq!(token_balance(&world.account(&treasury)), 5);
    let after = MarketState::decode(&world.account(&addresses.market).data).unwrap();
    assert_eq!(after.header.unclaimed_quote_lot_fees, QuoteLots::ZERO);
    // Lifetime earnings survive the sweep; the running balance does not.
    assert_eq!(after.header.collected_quote_lot_fees, QuoteLots(5));
    assert_eq!(after.header.quote_lots_deposited, QuoteLots(5_000));
}

#[test]
fn fees_cannot_be_swept_to_an_account_the_market_does_not_name() {
    // Otherwise anyone could redirect the venue's revenue by passing their own token
    // account to a permissionless instruction.
    let mut world = World::new();
    let (_owner, _, treasury) = world.wallet(0, 0);
    let addresses = create_with_recipient(&mut world, 10, treasury);
    let (_thief_owner, _, thief) = world.wallet(0, 0);

    let collect = sdk::collect_fees(&addresses, &thief);
    let message = Message::new(&[collect], Some(&world.payer.pubkey()));
    let tx = Transaction::new(&[&world.payer], message, world.svm.latest_blockhash());

    assert!(
        world.svm.send_transaction(tx).is_err(),
        "sweeping to an unnamed recipient must fail"
    );
}
