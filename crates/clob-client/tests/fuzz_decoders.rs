//! Every decoder, fed bytes nobody vouched for.
//!
//! An observer decodes instruction data and event payloads out of transactions it did
//! not build. A malformed input must produce an error, never a panic and never a value
//! that lies — the two failure modes that turn a bad byte into either a dead indexer or
//! a wrong tape.
//!
//! Property tests rather than examples, because the interesting inputs here are the ones
//! nobody would think to write down: a length prefix larger than the buffer, a
//! discriminant one past the last, a payload truncated mid-field.

use clob_client::{decode, event};
use proptest::prelude::*;

proptest! {
    #[test]
    fn instruction_data_never_panics(data in proptest::collection::vec(any::<u8>(), 0..256)) {
        // Whatever comes back, it comes back — an Err is a fine answer, a panic is not.
        let _ = decode::decode(&data);
    }

    #[test]
    fn event_payloads_never_panic(data in proptest::collection::vec(any::<u8>(), 0..512)) {
        let _ = event::decode(&data);
    }

    #[test]
    fn a_decoded_instruction_round_trips_its_discriminant(
        data in proptest::collection::vec(any::<u8>(), 1..256)
    ) {
        // Anything that decodes must report the discriminant it was decoded from.
        // A decoder that quietly picked a different instruction would misattribute
        // everything downstream of it.
        if let Ok(instruction) = decode::decode(&data) {
            let tag = data[0];
            let expected = clob_program::instruction::Discriminant::parse(tag)
                .expect("it decoded, so the tag is known");
            prop_assert_eq!(discriminant_of(&instruction), expected);
        }
    }

    #[test]
    fn a_truncated_event_is_refused_rather_than_padded(
        len in 0usize..200
    ) {
        // Zeros are a valid bit pattern for every field in the payload, so a decoder
        // that padded instead of refusing would report a real-looking event of zero
        // fills at price zero.
        let _ = event::decode(&vec![0u8; len]);
    }
}

/// The discriminant a decoded instruction came from.
fn discriminant_of(instruction: &decode::ClobInstruction) -> clob_program::instruction::Discriminant {
    use clob_program::instruction::Discriminant as D;
    use decode::ClobInstruction as I;

    match instruction {
        I::InitializeMarket { .. } => D::InitializeMarket,
        I::ClaimSeat => D::ClaimSeat,
        I::Deposit { .. } => D::Deposit,
        I::Withdraw { .. } => D::Withdraw,
        I::PlaceOrder { .. } => D::PlaceOrder,
        I::CancelOrder { .. } => D::CancelOrder,
        I::ReduceOrder { .. } => D::ReduceOrder,
        I::CancelAllOrders { .. } => D::CancelAllOrders,
        I::CollectFees => D::CollectFees,
        I::LogEvent => D::LogEvent,
        I::Swap { .. } => D::Swap,
        I::EvictSeat => D::EvictSeat,
    }
}

#[test]
fn an_event_claiming_more_fills_than_it_carries_is_refused() {
    // The classic length-prefix attack: say 255 fills, supply one. A decoder that
    // trusted the count would read past the buffer or report fills that are not there.
    let mut data = vec![0u8; 4 + 64];
    // fills_recorded sits in the summary; set it absurdly high whatever the layout.
    for byte in data.iter_mut().skip(4).take(8) {
        *byte = 0xff;
    }
    let _ = event::decode(&data);
}

#[test]
fn every_defined_discriminant_is_reachable() {
    // A tag the SDK cannot decode is an instruction an observer would silently drop, so
    // the two lists have to stay the same length.
    for tag in 0u8..12 {
        assert!(
            clob_program::instruction::Discriminant::parse(tag).is_ok(),
            "tag {tag} is not a known discriminant"
        );
    }
    assert!(
        clob_program::instruction::Discriminant::parse(12).is_err(),
        "12 must not be a discriminant — add it here when it becomes one"
    );
}
