//! The SDK writes bytes the program reads.
//!
//! Every builder here is decoded with [`clob_program::instruction::Reader`] — the
//! program's own parser, not a copy of it. A builder that writes something the program
//! cannot read fails here rather than on a validator, and a change to either side that
//! the other does not follow is a test failure rather than a rejected transaction.

use clob_book::{BaseLots, FIFOOrderId, LotConfig, QuoteLots, Side, Ticks};
use clob_client::instruction::{self, MarketAddresses, Receipt};
use clob_engine::{OrderPacket, PostOnlyRejection, SelfTradeBehavior};
use clob_program::instruction::{Discriminant, Reader};
use clob_program::state::SizeClass;
use proptest::prelude::*;
use solana_instruction::Instruction;
use solana_pubkey::Pubkey;

fn addresses() -> MarketAddresses {
    MarketAddresses::new(
        Pubkey::new_from_array([1u8; 32]),
        Pubkey::new_from_array([2u8; 32]),
        Pubkey::new_from_array([3u8; 32]),
        Pubkey::new_from_array([4u8; 32]),
    )
}

fn key(byte: u8) -> Pubkey {
    Pubkey::new_from_array([byte; 32])
}

/// Splits an instruction into its discriminant and a reader over the rest, exactly as
/// the program's dispatcher does.
fn parse(instruction: &Instruction) -> (Discriminant, Reader<'_>) {
    let (&tag, rest) = instruction.data.split_first().expect("data is never empty");
    (
        Discriminant::parse(tag).expect("builder emitted an unknown discriminant"),
        Reader::new(rest),
    )
}

#[test]
fn initialize_market_round_trips() {
    let lots = LotConfig::new(1_000, 2_000, 1_000_000, 1).unwrap();
    let instruction = instruction::initialize_market(
        &addresses(),
        &key(5),
        &key(6),
        &key(7),
        &key(8),
        SizeClass::Medium,
        &lots,
        7,
    );

    let (tag, mut reader) = parse(&instruction);
    assert_eq!(tag, Discriminant::InitializeMarket);
    assert_eq!(SizeClass::from_u64(reader.u64().unwrap()), Ok(SizeClass::Medium));
    assert_eq!(reader.u64().unwrap(), lots.base_lots_per_base_unit);
    assert_eq!(reader.u64().unwrap(), lots.tick_size_in_quote_lots_per_base_unit);
    assert_eq!(reader.u64().unwrap(), lots.base_atoms_per_base_lot);
    assert_eq!(reader.u64().unwrap(), lots.quote_atoms_per_quote_lot);
    assert_eq!(reader.u64().unwrap(), 7);
    // The bump the program verifies the vault signer against.
    assert_eq!(
        reader.u8().unwrap(),
        clob_client::address::vault_signer(&addresses().program_id, &addresses().market).1
    );
    assert_eq!(instruction.accounts.len(), 8);
}

#[test]
fn deposit_and_withdraw_round_trip() {
    let a = addresses();
    for (withdraw, expected) in [(false, Discriminant::Deposit), (true, Discriminant::Withdraw)] {
        let instruction = if withdraw {
            instruction::withdraw(&a, &key(9), &key(10), &key(11), BaseLots(30), QuoteLots(5_000))
        } else {
            instruction::deposit(&a, &key(9), &key(10), &key(11), BaseLots(30), QuoteLots(5_000))
        };

        let (tag, mut reader) = parse(&instruction);
        assert_eq!(tag, expected);
        assert_eq!(reader.base_lots().unwrap(), BaseLots(30));
        assert_eq!(reader.quote_lots().unwrap(), QuoteLots(5_000));
        // Withdrawing needs the vault signer; depositing does not.
        assert_eq!(instruction.accounts.len(), if withdraw { 8 } else { 7 });
    }
}

#[test]
fn cancel_and_reduce_round_trip() {
    let a = addresses();
    let id = FIFOOrderId::new(Side::Bid, Ticks(1234), 99);

    let cancel = instruction::cancel_order(&a, &key(9), &id);
    let (tag, mut reader) = parse(&cancel);
    assert_eq!(tag, Discriminant::CancelOrder);
    assert_eq!(reader.order_id().unwrap(), id);

    let reduce = instruction::reduce_order(&a, &key(9), &id, BaseLots(4));
    let (tag, mut reader) = parse(&reduce);
    assert_eq!(tag, Discriminant::ReduceOrder);
    assert_eq!(reader.order_id().unwrap(), id);
    assert_eq!(reader.base_lots().unwrap(), BaseLots(4));
}

#[test]
fn cancel_all_round_trips() {
    let instruction = instruction::cancel_all_orders(&addresses(), &key(9), Side::Ask, 12);
    let (tag, mut reader) = parse(&instruction);

    assert_eq!(tag, Discriminant::CancelAllOrders);
    assert_eq!(reader.side().unwrap(), Side::Ask);
    assert_eq!(reader.u32().unwrap(), 12);
}

#[test]
fn swap_round_trips() {
    let instruction = instruction::swap(
        &addresses(),
        &key(9),
        &key(10),
        &key(11),
        Side::Bid,
        Ticks(105),
        BaseLots(25),
        BaseLots(20),
        16,
        Receipt::Off,
    );

    let (tag, mut reader) = parse(&instruction);
    assert_eq!(tag, Discriminant::Swap);
    assert_eq!(reader.side().unwrap(), Side::Bid);
    assert_eq!(reader.u64().unwrap(), 105);
    assert_eq!(reader.u64().unwrap(), 25);
    assert_eq!(reader.u64().unwrap(), 20);
    assert_eq!(reader.u32().unwrap(), 16);
    assert_eq!(instruction.accounts.len(), 8);
}

#[test]
fn the_receipt_adds_two_accounts_and_a_bump() {
    let a = addresses();
    let packet = instruction::market_order(Side::Bid, BaseLots(10), 8);

    let plain = instruction::place_order(&a, &key(9), &packet, Receipt::Off);
    let receipt = instruction::place_order(&a, &key(9), &packet, Receipt::On);

    assert_eq!(plain.accounts.len(), 2);
    assert_eq!(receipt.accounts.len(), 4);
    assert_eq!(receipt.data.len(), plain.data.len() + 1);
    assert_eq!(
        *receipt.data.last().unwrap(),
        clob_client::address::log_authority(&a.program_id).1
    );

    // The log authority and the program, in that order.
    assert_eq!(
        receipt.accounts[2].pubkey,
        clob_client::address::log_authority(&a.program_id).0
    );
    assert_eq!(receipt.accounts[3].pubkey, a.program_id);
}

#[test]
fn a_receipt_does_not_disturb_the_packet_the_program_reads() {
    // The bump is appended after the packet, so both forms must decode identically up
    // to that point.
    let a = addresses();
    let packet = instruction::limit(Side::Ask, Ticks(101), BaseLots(7));

    for receipt in [Receipt::Off, Receipt::On] {
        let instruction = instruction::place_order(&a, &key(9), &packet, receipt);
        let (tag, mut reader) = parse(&instruction);
        assert_eq!(tag, Discriminant::PlaceOrder);
        assert_eq!(reader.order_packet().unwrap(), packet);
    }
}

// ---------------------------------------------------------------------------------
// Order packets, exhaustively
// ---------------------------------------------------------------------------------

fn side() -> impl Strategy<Value = Side> {
    prop_oneof![Just(Side::Bid), Just(Side::Ask)]
}

fn self_trade() -> impl Strategy<Value = SelfTradeBehavior> {
    prop_oneof![
        Just(SelfTradeBehavior::DecrementTake),
        Just(SelfTradeBehavior::CancelProvide),
        Just(SelfTradeBehavior::Abort),
    ]
}

fn packet() -> impl Strategy<Value = OrderPacket> {
    prop_oneof![
        (side(), any::<u64>(), any::<u64>(), self_trade(), any::<u32>()).prop_map(
            |(side, price, size, stb, limit)| OrderPacket::Limit {
                side,
                price_in_ticks: Ticks(price),
                num_base_lots: BaseLots(size),
                self_trade_behavior: stb,
                match_limit: limit,
            }
        ),
        (side(), any::<u64>(), any::<u64>(), any::<bool>()).prop_map(
            |(side, price, size, slide)| OrderPacket::PostOnly {
                side,
                price_in_ticks: Ticks(price),
                num_base_lots: BaseLots(size),
                rejection: if slide {
                    PostOnlyRejection::Slide
                } else {
                    PostOnlyRejection::Reject
                },
            }
        ),
        (
            side(),
            proptest::option::of(any::<u64>()),
            any::<u64>(),
            any::<u64>(),
            self_trade(),
            any::<u32>()
        )
            .prop_map(|(side, price, size, min, stb, limit)| {
                OrderPacket::ImmediateOrCancel {
                    side,
                    price_in_ticks: price.map(Ticks),
                    num_base_lots: BaseLots(size),
                    min_base_lots_to_fill: BaseLots(min),
                    self_trade_behavior: stb,
                    match_limit: limit,
                }
            }),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    /// Any packet the SDK can build, the program reads back identically. This is the
    /// property that keeps the two encodings from drifting apart field by field.
    #[test]
    fn every_order_packet_survives_the_program_s_parser(packet in packet()) {
        let instruction = instruction::place_order(&addresses(), &key(9), &packet, Receipt::Off);
        let (tag, mut reader) = parse(&instruction);

        prop_assert_eq!(tag, Discriminant::PlaceOrder);
        prop_assert_eq!(reader.order_packet().unwrap(), packet);
    }

    /// The same, with a receipt appended.
    #[test]
    fn a_receipt_never_changes_how_a_packet_decodes(packet in packet()) {
        let a = addresses();
        let instruction = instruction::place_order(&a, &key(9), &packet, Receipt::On);
        let (_, mut reader) = parse(&instruction);

        prop_assert_eq!(reader.order_packet().unwrap(), packet);
        prop_assert_eq!(
            reader.optional_u8(),
            Some(clob_client::address::log_authority(&a.program_id).1)
        );
    }
}
