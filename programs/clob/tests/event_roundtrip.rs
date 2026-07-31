//! The event round trip, end to end.
//!
//! Everything else about events is checked one layer at a time: the encoder is
//! unit-tested byte by byte, and Mollusk proves the self-CPI is accepted with a valid
//! signature. Neither shows that a consumer can actually *read* the payload back out of
//! a transaction, which is the only thing an indexer cares about.
//!
//! These tests run a real signed transaction through LiteSVM and decode the event from
//! the transaction's inner instructions — the same place a Geyser stream or an RPC
//! `getTransaction` response would surface it. If the wire format drifts from what the
//! encoder writes, this is what fails.

use clob_book::{BaseLots, QuoteLots, Side, Ticks};
use clob_engine::{FeeSchedule, Market, MatchStop, OrderPacket, TraderKey};
use clob_program::state::{
    HEADER_LEN, MARKET_DISCRIMINATOR, MARKET_VERSION, MarketAccountHeader, SizeClass,
};
use litesvm::LiteSVM;
use solana_account::Account;
use solana_instruction::{AccountMeta, Instruction};
use solana_keypair::Keypair;
use solana_message::Message;
use solana_pubkey::Pubkey;
use solana_signer::Signer;
use solana_transaction::Transaction;

const PROGRAM_ID: Pubkey = Pubkey::new_from_array([9u8; 32]);
type TestMarket = Market<128, 128, 32>;

/// Where `cargo build-sbf` leaves the program. `SBF_OUT_DIR` is set for the workspace
/// in `.cargo/config.toml`.
fn program_path() -> std::path::PathBuf {
    let dir = std::env::var("SBF_OUT_DIR").unwrap_or_else(|_| "target/deploy".to_string());
    std::path::PathBuf::from(dir).join("clob_program.so")
}

/// The decoded event, as an indexer would reconstruct it.
#[derive(Debug, PartialEq, Eq)]
struct Event {
    version: u8,
    kind: u8,
    stop: u8,
    truncated: bool,
    taker_seat: u32,
    order_price: u64,
    order_sequence: u64,
    base_lots_filled: u64,
    quote_lots_filled: u64,
    fee_in_quote_lots: u64,
    base_lots_posted: u64,
    fills_seen: u32,
    fills_recorded: u32,
    fills: Vec<FillRecord>,
}

#[derive(Debug, PartialEq, Eq)]
struct FillRecord {
    maker_seat: u32,
    base_lots_filled: u32,
    price_in_ticks: u64,
    sequence: u64,
    quote_lots_filled: u64,
    fee_in_quote_lots: u64,
    maker_remaining: u64,
}

fn u32_at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}

fn u64_at(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(b[o..o + 8].try_into().unwrap())
}

/// Parses the payload exactly as documented, with no access to the encoder's internals.
///
/// Written against the field layout rather than by reusing `EventBuffer`, so a change
/// to the encoder that a client would not survive shows up here as a failure rather
/// than being silently mirrored.
fn parse_event(payload: &[u8]) -> Event {
    const SUMMARY: usize = 64;
    const FILL: usize = 48;

    let fills_recorded = u32_at(payload, 60);
    let fills = (0..fills_recorded as usize)
        .map(|i| {
            let o = SUMMARY + i * FILL;
            FillRecord {
                maker_seat: u32_at(payload, o),
                base_lots_filled: u32_at(payload, o + 4),
                price_in_ticks: u64_at(payload, o + 8),
                sequence: u64_at(payload, o + 16),
                quote_lots_filled: u64_at(payload, o + 24),
                fee_in_quote_lots: u64_at(payload, o + 32),
                maker_remaining: u64_at(payload, o + 40),
            }
        })
        .collect();

    Event {
        version: payload[0],
        kind: payload[1],
        stop: payload[2],
        truncated: payload[3] != 0,
        taker_seat: u32_at(payload, 4),
        order_price: u64_at(payload, 8),
        order_sequence: u64_at(payload, 16),
        base_lots_filled: u64_at(payload, 24),
        quote_lots_filled: u64_at(payload, 32),
        fee_in_quote_lots: u64_at(payload, 40),
        base_lots_posted: u64_at(payload, 48),
        fills_seen: u32_at(payload, 56),
        fills_recorded,
        fills,
    }
}

/// A market account with `depth` resting asks and two funded seats.
fn market_account(taker: Pubkey, maker: Pubkey, depth: u64, taker_fee_bps: u64) -> Account {
    let mut data = vec![0u8; SizeClass::Small.account_len()];
    let (header_bytes, market_bytes) = data.split_at_mut(HEADER_LEN);

    *bytemuck::from_bytes_mut::<MarketAccountHeader>(header_bytes) = MarketAccountHeader {
        discriminator: MARKET_DISCRIMINATOR,
        version: MARKET_VERSION,
        size_class: SizeClass::Small as u64,
        vault_signer_bump: 0,
        ..Default::default()
    };

    let market =
        bytemuck::from_bytes_mut::<TestMarket>(&mut market_bytes[..TestMarket::SIZE_IN_BYTES]);
    market
        .initialize(
            clob_book::LotConfig::new(1_000, 1_000, 1_000_000, 1).unwrap(),
            FeeSchedule::new(taker_fee_bps).unwrap(),
        )
        .unwrap();

    let maker_seat = market.claim_seat(TraderKey(maker.to_bytes())).unwrap();
    market
        .deposit(maker_seat, BaseLots(1_000_000), QuoteLots(1_000_000_000))
        .unwrap();
    let taker_seat = market.claim_seat(TraderKey(taker.to_bytes())).unwrap();
    market
        .deposit(taker_seat, BaseLots(1_000_000), QuoteLots(1_000_000_000))
        .unwrap();

    for i in 0..depth {
        market
            .place_order(
                maker_seat,
                OrderPacket::post_only(Side::Ask, Ticks(100 + i), BaseLots(10)),
                &mut (),
            )
            .unwrap();
    }

    Account {
        lamports: 10_000_000_000,
        data,
        owner: PROGRAM_ID,
        executable: false,
        rent_epoch: 0,
    }
}

struct Harness {
    svm: LiteSVM,
    payer: Keypair,
    market: Pubkey,
    taker: Keypair,
    log_authority: Pubkey,
    log_bump: u8,
}

fn harness(depth: u64, taker_fee_bps: u64) -> Harness {
    let mut svm = LiteSVM::new();
    // Loaded at run time from the same place Mollusk looks, rather than with
    // include_bytes!: the binary is a build artifact, and baking it in at compile time
    // would make `cargo check` on a clean checkout fail until `cargo build-sbf` had run.
    svm.add_program_from_file(PROGRAM_ID, program_path())
        .expect("run `cargo build-sbf --manifest-path programs/clob/Cargo.toml` first");

    let payer = Keypair::new();
    let taker = Keypair::new();
    let maker = Keypair::new();
    svm.airdrop(&payer.pubkey(), 10_000_000_000).unwrap();
    svm.airdrop(&taker.pubkey(), 10_000_000_000).unwrap();

    let market = Pubkey::new_unique();
    svm.set_account(
        market,
        market_account(taker.pubkey(), maker.pubkey(), depth, taker_fee_bps),
    )
    .unwrap();

    let (log_authority, log_bump) = Pubkey::find_program_address(&[b"log"], &PROGRAM_ID);

    Harness {
        svm,
        payer,
        market,
        taker,
        log_authority,
        log_bump,
    }
}

/// A receipt-form market order.
fn order_ix(h: &Harness, size: u64, match_limit: u32) -> Instruction {
    let mut data = vec![4u8, 2, Side::Bid as u8, 0];
    data.extend_from_slice(&0u64.to_le_bytes());
    data.extend_from_slice(&size.to_le_bytes());
    data.extend_from_slice(&0u64.to_le_bytes());
    data.push(0);
    data.extend_from_slice(&match_limit.to_le_bytes());
    data.push(h.log_bump);

    Instruction {
        program_id: PROGRAM_ID,
        accounts: vec![
            AccountMeta::new(h.market, false),
            AccountMeta::new_readonly(h.taker.pubkey(), true),
            AccountMeta::new_readonly(h.log_authority, false),
            AccountMeta::new_readonly(PROGRAM_ID, false),
        ],
        data,
    }
}

/// Sends an instruction and pulls the event out of the resulting transaction record.
fn send_and_decode(h: &mut Harness, instruction: Instruction) -> Event {
    let message = Message::new(&[instruction], Some(&h.payer.pubkey()));
    let tx = Transaction::new(
        &[&h.payer, &h.taker],
        message,
        h.svm.latest_blockhash(),
    );
    let meta = h.svm.send_transaction(tx).expect("transaction should succeed");

    // Exactly what an indexer walks: inner instructions of the outer call, filtered to
    // this program's LogEvent discriminant.
    let inner = meta
        .inner_instructions
        .iter()
        .flatten()
        .map(|i| &i.instruction.data)
        .find(|data| data.first() == Some(&9))
        .expect("expected a LogEvent inner instruction");

    // [discriminant][log bump][payload]
    parse_event(&inner[2..])
}

#[test]
fn a_fill_is_readable_from_the_transaction_record() {
    let mut h = harness(3, 0);
    let event = { let ix = order_ix(&h, 25, 16); send_and_decode(&mut h, ix) };

    assert_eq!(event.version, 1);
    assert_eq!(event.kind, 0, "order-placed");
    assert_eq!(event.stop, MatchStop::FullyFilled as u8);
    assert!(!event.truncated);
    assert_eq!(event.base_lots_filled, 25);
    // 10 at 100, 10 at 101, 5 at 102.
    assert_eq!(event.quote_lots_filled, 1_000 + 1_010 + 510);
    assert_eq!(event.base_lots_posted, 0);
    assert_eq!(event.fills_seen, 3);
    assert_eq!(event.fills_recorded, 3);
}

#[test]
fn every_individual_fill_survives_the_round_trip() {
    // An indexer reconstructs the trade tape from these, so each field has to arrive
    // intact — not just the aggregate.
    let mut h = harness(3, 0);
    let event = { let ix = order_ix(&h, 25, 16); send_and_decode(&mut h, ix) };

    let prices: Vec<u64> = event.fills.iter().map(|f| f.price_in_ticks).collect();
    let sizes: Vec<u32> = event.fills.iter().map(|f| f.base_lots_filled).collect();
    let values: Vec<u64> = event.fills.iter().map(|f| f.quote_lots_filled).collect();

    assert_eq!(prices, vec![100, 101, 102]);
    assert_eq!(sizes, vec![10, 10, 5]);
    assert_eq!(values, vec![1_000, 1_010, 510]);
    // The last maker order was only partly consumed.
    assert_eq!(event.fills[2].maker_remaining, 5);
    assert_eq!(event.fills[0].maker_remaining, 0);
    // Every fill names a real maker seat.
    assert!(event.fills.iter().all(|f| f.maker_seat != 0));
}

#[test]
fn fees_appear_in_the_event_the_taker_actually_paid() {
    let mut h = harness(2, 10);
    let event = { let ix = order_ix(&h, 20, 8); send_and_decode(&mut h, ix) };

    // 10 bps of 1_000 and of 1_010, each rounded up.
    assert_eq!(event.fee_in_quote_lots, 1 + 2);
    let per_fill: u64 = event.fills.iter().map(|f| f.fee_in_quote_lots).sum();
    assert_eq!(per_fill, event.fee_in_quote_lots, "fills must sum to the total");
}

#[test]
fn a_deep_sweep_flags_truncation_and_keeps_the_totals() {
    // Past the buffer the tape is incomplete, and the event has to say so — otherwise
    // an indexer would record a wrong trade rather than an incomplete one.
    let mut h = harness(40, 0);
    let event = { let ix = order_ix(&h, 400, 64); send_and_decode(&mut h, ix) };

    assert!(event.truncated);
    assert_eq!(event.fills_seen, 40);
    assert_eq!(
        event.fills_recorded,
        clob_program::event::MAX_LOGGED_FILLS as u32
    );
    assert_eq!(event.fills.len(), clob_program::event::MAX_LOGGED_FILLS);
    // Lossy in detail, exact in aggregate.
    assert_eq!(event.base_lots_filled, 400);
    let recorded: u32 = event.fills.iter().map(|f| f.base_lots_filled).sum();
    assert!(recorded < 400, "recorded detail should be a strict subset");
}

#[test]
fn a_resting_remainder_is_reported_with_its_order_id() {
    // A limit order that partly fills and partly posts: the client needs the id back to
    // cancel it later, and this is the only place it appears.
    let mut h = harness(1, 0);

    let mut data = vec![4u8, 0, Side::Bid as u8, 1];
    data.extend_from_slice(&200u64.to_le_bytes());
    data.extend_from_slice(&30u64.to_le_bytes());
    data.push(0);
    data.extend_from_slice(&8u32.to_le_bytes());
    data.push(h.log_bump);

    let instruction = Instruction {
        program_id: PROGRAM_ID,
        accounts: vec![
            AccountMeta::new(h.market, false),
            AccountMeta::new_readonly(h.taker.pubkey(), true),
            AccountMeta::new_readonly(h.log_authority, false),
            AccountMeta::new_readonly(PROGRAM_ID, false),
        ],
        data,
    };

    let event = send_and_decode(&mut h, instruction);

    assert_eq!(event.base_lots_filled, 10);
    assert_eq!(event.base_lots_posted, 20);
    assert_eq!(event.order_price, 200);
    // Bid sequence numbers are stored inverted, so the high bit is set.
    assert_eq!(event.order_sequence >> 63, 1, "posted id should decode as a bid");
}

#[test]
fn the_plain_form_produces_no_event_at_all() {
    let mut h = harness(1, 0);

    let mut data = vec![4u8, 2, Side::Bid as u8, 0];
    data.extend_from_slice(&0u64.to_le_bytes());
    data.extend_from_slice(&10u64.to_le_bytes());
    data.extend_from_slice(&0u64.to_le_bytes());
    data.push(0);
    data.extend_from_slice(&8u32.to_le_bytes());

    let instruction = Instruction {
        program_id: PROGRAM_ID,
        accounts: vec![
            AccountMeta::new(h.market, false),
            AccountMeta::new_readonly(h.taker.pubkey(), true),
        ],
        data,
    };

    let message = Message::new(&[instruction], Some(&h.payer.pubkey()));
    let tx = Transaction::new(&[&h.payer, &h.taker], message, h.svm.latest_blockhash());
    let meta = h.svm.send_transaction(tx).expect("transaction should succeed");

    let events = meta
        .inner_instructions
        .iter()
        .flatten()
        .filter(|i| i.instruction.data.first() == Some(&9))
        .count();
    assert_eq!(events, 0, "the cheap path must not emit");
}
