//! What a market account costs, in bytes and in rent.
//!
//! These numbers get published — in the README, in the docs, and on the marketing site —
//! and a published number that nothing checks is a number that drifts. Changing a capacity
//! or adding a header field moves every figure here, and this fails when it does.
//!
//! Rent is computed the way the runtime computes it rather than taken from a table:
//! `(account overhead + data length) x lamports per byte-year x years of exemption`. The
//! constants are the runtime's own and have not changed since rent exemption was
//! introduced, but they are named here so that if they ever do, this is where it shows.

use clob_program::state::SizeClass;

/// Bytes the runtime charges for every account before its data.
const ACCOUNT_OVERHEAD: u64 = 128;

/// Lamports per byte, per year.
const LAMPORTS_PER_BYTE_YEAR: u64 = 3_480;

/// Years of rent an account holds to be exempt from paying any.
const EXEMPTION_YEARS: u64 = 2;

const LAMPORTS_PER_SOL: f64 = 1_000_000_000.0;

fn rent_lamports(len: usize) -> u64 {
    (ACCOUNT_OVERHEAD + len as u64) * LAMPORTS_PER_BYTE_YEAR * EXEMPTION_YEARS
}

#[test]
fn the_published_size_classes_are_what_the_account_actually_needs() {
    let classes = [
        (SizeClass::Small, 128, 32, 19_296),
        (SizeClass::Medium, 512, 128, 76_128),
        (SizeClass::Large, 2048, 512, 303_456),
    ];

    for (class, orders_per_side, seats, expected) in classes {
        let len = class.account_len();
        assert_eq!(
            len, expected,
            "{class:?} holds {orders_per_side} orders a side and {seats} seats, and its \
             account is {len} bytes rather than the {expected} written down"
        );
    }
}

#[test]
fn creating_a_market_costs_what_the_site_says_it_costs() {
    // Printed rather than only asserted: the figures quoted publicly come from this run,
    // so `cargo test -- --nocapture` is where to read them off rather than a calculator.
    for class in [SizeClass::Small, SizeClass::Medium, SizeClass::Large] {
        let lamports = rent_lamports(class.account_len());
        println!(
            "{class:?}: {} bytes, {lamports} lamports, {:.4} SOL",
            class.account_len(),
            lamports as f64 / LAMPORTS_PER_SOL
        );
    }

    // One assertion per class, to the precision anything published would use. Rent scales
    // linearly with size, so these three pin the whole curve.
    let sol = |class: SizeClass| rent_lamports(class.account_len()) as f64 / LAMPORTS_PER_SOL;
    assert!((sol(SizeClass::Small) - 0.1352).abs() < 0.0001, "{}", sol(SizeClass::Small));
    assert!((sol(SizeClass::Medium) - 0.5307).abs() < 0.0001, "{}", sol(SizeClass::Medium));
    assert!((sol(SizeClass::Large) - 2.1129).abs() < 0.0001, "{}", sol(SizeClass::Large));
}
