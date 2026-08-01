//! The wire format, written down so a second implementation can be held to it.
//!
//! `wire_format.rs` checks this SDK's builders against the program's own reader, which
//! works because both are Rust and both use one definition of the layout. A TypeScript
//! SDK cannot do that. It has to re-implement the encoding, which is exactly the failure
//! the builders' own documentation warns about:
//!
//! > Two copies of a byte layout is how a client and a program drift apart; one copy
//! > cannot.
//!
//! Since the second copy is unavoidable, the answer is to make drift fail a test rather
//! than fail in production. This emits every instruction the SDK can build, with its
//! arguments and its exact bytes, to `spec/wire-vectors.json`. Rust asserts the file
//! still matches what it produces; TypeScript asserts it can reproduce the same bytes
//! from the same arguments. Neither can move without the other noticing.
//!
//! # Regenerating
//!
//! ```text
//! UPDATE_VECTORS=1 cargo test -p clob-client --test wire_vectors
//! ```
//!
//! A deliberate change to the format is a change to the checked-in file, reviewed as
//! part of the diff. An accidental one is a failing test.

use clob_book::{BaseLots, FIFOOrderId, LotConfig, QuoteLots, Side, Ticks};
use clob_client::instruction::{self as sdk, MarketAddresses, Receipt};
use clob_engine::{PostOnlyRejection, SelfTradeBehavior};
use clob_program::state::SizeClass;
use serde_json::{Value, json};
use solana_instruction::Instruction;
use solana_pubkey::Pubkey;

/// Where the shared file lives, relative to this crate.
const VECTORS: &str = "../../spec/wire-vectors.json";

/// Fixed addresses, so the file is stable across runs and readable in a diff.
fn address(tag: u8) -> Pubkey {
    Pubkey::new_from_array([tag; 32])
}

fn addresses() -> MarketAddresses {
    MarketAddresses::new(address(9), address(1), address(4), address(5))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// One instruction, as arguments and as the bytes they must produce.
fn case(name: &str, args: Value, instruction: &Instruction) -> Value {
    json!({
        "name": name,
        "args": args,
        "programAddress": instruction.program_id.to_string(),
        "accounts": instruction.accounts.iter().map(|meta| json!({
            "address": meta.pubkey.to_string(),
            "signer": meta.is_signer,
            "writable": meta.is_writable,
        })).collect::<Vec<_>>(),
        "data": hex(&instruction.data),
    })
}

/// Every instruction, with the arguments a second implementation is given to reproduce it.
///
/// Quantities are strings rather than numbers. A `u64` price is not representable in a
/// JSON number once it passes 2^53, and a fixture that silently rounds is worse than no
/// fixture — the implementation under test would be held to the wrong bytes.
fn instruction_cases() -> Vec<Value> {
    let market = addresses();
    let trader = address(100);
    let base = address(101);
    let quote = address(102);

    let order_id = FIFOOrderId::from_encoded(Ticks(150_000), 0xffff_ffff_ffff_fff0);

    vec![
        case(
            "claimSeat",
            json!({}),
            &sdk::claim_seat(&market, &trader),
        ),
        case(
            "evictSeat",
            json!({}),
            &sdk::evict_seat(&market, &trader),
        ),
        case(
            "deposit",
            json!({ "baseLots": "1000", "quoteLots": "2000000" }),
            &sdk::deposit(&market, &trader, &base, &quote, BaseLots(1_000), QuoteLots(2_000_000)),
        ),
        case(
            "withdraw",
            json!({ "baseLots": "7", "quoteLots": "0" }),
            &sdk::withdraw(&market, &trader, &base, &quote, BaseLots(7), QuoteLots(0)),
        ),
        case(
            "limit",
            json!({
                "side": "ask",
                "priceInTicks": "150000",
                "baseLots": "25",
                "selfTradeBehavior": "decrementTake",
                "matchLimit": 4294967295u32,
            }),
            &sdk::place_order(
                &market,
                &trader,
                &sdk::limit(Side::Ask, Ticks(150_000), BaseLots(25)),
                Receipt::Off,
            ),
        ),
        case(
            "limitWithReceipt",
            json!({
                "side": "bid",
                "priceInTicks": "149000",
                "baseLots": "25",
                "selfTradeBehavior": "decrementTake",
                "matchLimit": 4294967295u32,
                "receipt": true,
            }),
            &sdk::place_order(
                &market,
                &trader,
                &sdk::limit(Side::Bid, Ticks(149_000), BaseLots(25)),
                Receipt::On,
            ),
        ),
        case(
            "postOnlyReject",
            json!({ "side": "bid", "priceInTicks": "149000", "baseLots": "10", "rejection": "reject" }),
            &sdk::place_order(
                &market,
                &trader,
                &sdk::post_only(Side::Bid, Ticks(149_000), BaseLots(10), PostOnlyRejection::Reject),
                Receipt::Off,
            ),
        ),
        case(
            "postOnlySlide",
            json!({ "side": "ask", "priceInTicks": "151000", "baseLots": "10", "rejection": "slide" }),
            &sdk::place_order(
                &market,
                &trader,
                &sdk::post_only(Side::Ask, Ticks(151_000), BaseLots(10), PostOnlyRejection::Slide),
                Receipt::Off,
            ),
        ),
        case(
            "fillOrKill",
            json!({ "side": "bid", "priceInTicks": "151000", "baseLots": "40" }),
            &sdk::place_order(
                &market,
                &trader,
                &sdk::fill_or_kill(Side::Bid, Ticks(151_000), BaseLots(40)),
                Receipt::Off,
            ),
        ),
        // The unpriced variant: the price flag is cleared and the price field is zero,
        // which is the one packet whose encoding is not a function of its arguments alone.
        case(
            "marketOrder",
            json!({ "side": "ask", "baseLots": "40", "matchLimit": 64 }),
            &sdk::place_order(
                &market,
                &trader,
                &sdk::market_order(Side::Ask, BaseLots(40), 64),
                Receipt::Off,
            ),
        ),
        case(
            "cancelOrder",
            json!({ "priceInTicks": "150000", "orderSequenceNumber": "18446744073709551600" }),
            &sdk::cancel_order(&market, &trader, &order_id),
        ),
        case(
            "reduceOrder",
            json!({
                "priceInTicks": "150000",
                "orderSequenceNumber": "18446744073709551600",
                "baseLots": "5",
            }),
            &sdk::reduce_order(&market, &trader, &order_id, BaseLots(5)),
        ),
        case(
            "cancelAllOrders",
            json!({ "side": "bid", "limit": 32 }),
            &sdk::cancel_all_orders(&market, &trader, Side::Bid, 32),
        ),
        case(
            "batchUpdate",
            json!({
                "cancels": [
                    { "priceInTicks": "150000", "orderSequenceNumber": "18446744073709551600" },
                    { "priceInTicks": "149000", "orderSequenceNumber": "5" },
                ],
                "orders": [
                    { "kind": "postOnly", "side": "bid", "priceInTicks": "148000", "baseLots": "10", "rejection": "reject" },
                    { "kind": "postOnly", "side": "ask", "priceInTicks": "152000", "baseLots": "10", "rejection": "reject" },
                ],
            }),
            &sdk::batch_update(
                &market,
                &trader,
                &[order_id, FIFOOrderId::from_encoded(Ticks(149_000), 5)],
                &[
                    sdk::post_only(Side::Bid, Ticks(148_000), BaseLots(10), PostOnlyRejection::Reject),
                    sdk::post_only(Side::Ask, Ticks(152_000), BaseLots(10), PostOnlyRejection::Reject),
                ],
            ),
        ),
        // Empty on both sides. A length-prefixed list is exactly where an encoder gets
        // the zero case wrong, and it is a legal thing for a maker to send.
        case(
            "batchUpdateEmpty",
            json!({ "cancels": [], "orders": [] }),
            &sdk::batch_update(&market, &trader, &[], &[]),
        ),
        case(
            "swap",
            json!({
                "side": "bid",
                "priceInTicks": "151000",
                "baseLots": "30",
                "minBaseLotsToFill": "10",
                "matchLimit": 64,
            }),
            &sdk::swap(
                &market,
                &trader,
                &base,
                &quote,
                Side::Bid,
                Ticks(151_000),
                BaseLots(30),
                BaseLots(10),
                64,
                Receipt::Off,
            ),
        ),
        case(
            "collectFees",
            json!({}),
            &sdk::collect_fees(&market, &address(8)),
        ),
        case(
            "initializeMarket",
            json!({
                "sizeClass": "small",
                "baseLotsPerBaseUnit": "1000",
                "tickSizeInQuoteLotsPerBaseUnit": "1000",
                "baseAtomsPerBaseLot": "1000000",
                "quoteAtomsPerQuoteLot": "1",
                "takerFeeBps": "2",
            }),
            &sdk::initialize_market(
                &market,
                &address(2),
                &address(3),
                &address(7),
                &address(8),
                SizeClass::Small,
                &LotConfig::new(1_000, 1_000, 1_000_000, 1).expect("a valid configuration"),
                2,
            ),
        ),
    ]
}

/// Constants a second implementation has to agree on before any of the above can match.
fn constants() -> Value {
    json!({
        "marketDiscriminator": hex(&clob_program::state::MARKET_DISCRIMINATOR.to_le_bytes()),
        "marketVersion": clob_program::state::MARKET_VERSION,
        "accountHeaderLength": clob_program::state::HEADER_LEN,
        "tokenProgramAddress": sdk::TOKEN_PROGRAM_ID.to_string(),
        "sizeClasses": { "small": 0, "medium": 1, "large": 2 },
        "sides": { "bid": Side::Bid as u8, "ask": Side::Ask as u8 },
        "selfTradeBehaviors": {
            "decrementTake": SelfTradeBehavior::DecrementTake as u8,
            "cancelProvide": SelfTradeBehavior::CancelProvide as u8,
            "abort": SelfTradeBehavior::Abort as u8,
        },
        "postOnlyRejections": {
            "reject": PostOnlyRejection::Reject as u8,
            "slide": PostOnlyRejection::Slide as u8,
        },
    })
}

/// The addresses every case was built from, by name.
///
/// Without these a second implementation could only compare the bytes, because it would
/// have no way to produce an account list except by copying the one it is being checked
/// against. Naming the inputs makes the account order and the signer and writable flags
/// part of the contract rather than part of the answer key.
fn inputs() -> Value {
    let market = addresses();
    json!({
        "programAddress": market.program_id.to_string(),
        "market": market.market.to_string(),
        "baseVault": market.base_vault.to_string(),
        "quoteVault": market.quote_vault.to_string(),
        "vaultSigner": market.vault_signer.to_string(),
        "tokenProgram": market.token_program.to_string(),
        "trader": address(100).to_string(),
        "traderBase": address(101).to_string(),
        "traderQuote": address(102).to_string(),
        "baseMint": address(2).to_string(),
        "quoteMint": address(3).to_string(),
        "authority": address(7).to_string(),
        "feeRecipient": address(8).to_string(),
        // Derived from the program id. Given rather than derived, because deriving it
        // needs an ed25519 on-curve check, and an SDK that shipped one to compute an
        // address the market already knows would be paying a lot for very little.
        "logAuthority": clob_client::address::log_authority(&market.program_id).0.to_string(),
        "logAuthorityBump": clob_client::address::log_authority(&market.program_id).1,
    })
}

/// A real market account's leading bytes, and what they mean.
///
/// Only the part a second implementation is expected to read: the account preamble and
/// the engine's header. The book and the seat table are trees in a Pod arena, and holding
/// another language to a tree traversal it has no reason to implement would be asking it
/// to duplicate the part of this system that property tests already cover.
///
/// Every numeric field is given a distinct value. Offsets are the thing most likely to be
/// wrong in a hand-written decoder, and zeros make a misread offset look correct.
fn market_account() -> Value {
    use clob_engine::{FeeSchedule, MarketHeader};
    use clob_program::state::{HEADER_LEN, MARKET_DISCRIMINATOR, MARKET_VERSION, MarketAccountHeader};

    let market = addresses();
    let config = LotConfig::new(1_000, 1_000, 1_000_000, 1).expect("a valid configuration");
    let mut data = vec![0u8; HEADER_LEN + core::mem::size_of::<MarketHeader>()];

    *bytemuck::from_bytes_mut::<MarketAccountHeader>(&mut data[..HEADER_LEN]) =
        MarketAccountHeader {
            discriminator: MARKET_DISCRIMINATOR,
            version: MARKET_VERSION,
            size_class: SizeClass::Small as u64,
            vault_signer_bump: 254,
            base_mint: address(2).to_bytes(),
            quote_mint: address(3).to_bytes(),
            base_vault: market.base_vault.to_bytes(),
            quote_vault: market.quote_vault.to_bytes(),
            authority: address(7).to_bytes(),
            fee_recipient: address(8).to_bytes(),
        };

    let mut header = MarketHeader::new(config, FeeSchedule::new(2).expect("a valid fee"))
        .expect("a valid header");
    header.base_lots_deposited = BaseLots(111);
    header.quote_lots_deposited = QuoteLots(222);
    header.collected_quote_lot_fees = QuoteLots(333);
    header.unclaimed_quote_lot_fees = QuoteLots(44);
    *bytemuck::from_bytes_mut::<MarketHeader>(&mut data[HEADER_LEN..]) = header;

    json!({
        "bytes": hex(&data),
        "decoded": {
            "sizeClass": "small",
            "vaultSignerBump": 254,
            "baseMint": address(2).to_string(),
            "quoteMint": address(3).to_string(),
            "baseVault": market.base_vault.to_string(),
            "quoteVault": market.quote_vault.to_string(),
            "authority": address(7).to_string(),
            "feeRecipient": address(8).to_string(),
            "lotConfig": {
                "baseLotsPerBaseUnit": "1000",
                "tickSizeInQuoteLotsPerBaseUnit": "1000",
                "baseAtomsPerBaseLot": "1000000",
                "quoteAtomsPerQuoteLot": "1",
            },
            "takerFeeBps": "2",
            "baseLotsDeposited": "111",
            "quoteLotsDeposited": "222",
            "collectedQuoteLotFees": "333",
            "unclaimedQuoteLotFees": "44",
        },
    })
}

fn document() -> Value {
    json!({
        "$comment": "Generated by `cargo test -p clob-client --test wire_vectors`. \
                     Rust is the source of truth; every other implementation is held to this. \
                     Regenerate deliberately with UPDATE_VECTORS=1.",
        "constants": constants(),
        "inputs": inputs(),
        "instructions": instruction_cases(),
        "marketAccount": market_account(),
    })
}

#[test]
fn the_checked_in_vectors_still_describe_this_sdk() {
    let current = serde_json::to_string_pretty(&document()).expect("the document serialises");

    if std::env::var("UPDATE_VECTORS").is_ok() {
        std::fs::create_dir_all("../../spec").expect("creating spec/");
        std::fs::write(VECTORS, format!("{current}\n")).expect("writing the vectors");
        return;
    }

    let checked_in = std::fs::read_to_string(VECTORS).unwrap_or_else(|error| {
        panic!(
            "cannot read {VECTORS}: {error}\n\
             Generate it with: UPDATE_VECTORS=1 cargo test -p clob-client --test wire_vectors"
        )
    });

    // Compared as parsed JSON rather than as text, so reformatting the file is not a
    // failure while a changed byte is.
    let checked_in: Value = serde_json::from_str(&checked_in).expect("the file is valid JSON");
    let current: Value = serde_json::from_str(&current).expect("the document is valid JSON");

    assert_eq!(
        checked_in, current,
        "the wire format changed and spec/wire-vectors.json did not.\n\
         Every other implementation is held to that file, so a change here is a change \
         they have to make too.\n\
         If the change is deliberate: UPDATE_VECTORS=1 cargo test -p clob-client --test wire_vectors"
    );
}

#[test]
fn every_case_is_named_once() {
    // The TypeScript side dispatches on these names. A duplicate would silently test one
    // case twice and leave another untested.
    let cases = instruction_cases();
    let mut names: Vec<&str> = cases
        .iter()
        .map(|case| case["name"].as_str().expect("a name"))
        .collect();
    names.sort_unstable();
    let total = names.len();
    names.dedup();
    assert_eq!(names.len(), total, "two cases share a name");
}

#[test]
fn every_case_carries_bytes_and_accounts() {
    for case in instruction_cases() {
        let name = case["name"].as_str().expect("a name");
        assert!(
            !case["data"].as_str().expect("data").is_empty(),
            "{name} encodes to nothing"
        );
        assert!(
            !case["accounts"].as_array().expect("accounts").is_empty(),
            "{name} names no accounts"
        );
    }
}
