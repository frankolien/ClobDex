//! Turning ticks and lots into something a person reads.
//!
//! The geometry comes from [`clob_book::LotConfig`] rather than being reimplemented, for
//! the reason every client in this repository says: a second copy of a tick conversion is a
//! screen that disagrees with what settled.
//!
//! Only the rendering lives here, and rendering is the one place a fraction is correct.

use anyhow::{Context, Result};
use clob_book::LotConfig;

use crate::wire;

/// Reads a quoted `u64`, naming the field if it will not parse.
///
/// Strict on purpose. An empty or malformed field becoming `0` would be a real price and a
/// real size, and it would be rendered as one.
pub fn quantity(value: &str, field: &str) -> Result<u64> {
    value
        .trim()
        .parse::<u64>()
        .with_context(|| format!("{field} is not a number: {value:?}"))
}

/// The same, for a field the indexer sends as `null` when it has no answer.
pub fn maybe(value: &Option<String>, field: &str) -> Result<Option<u64>> {
    match value {
        None => Ok(None),
        Some(text) => quantity(text, field).map(Some),
    }
}

/// Builds the engine's lot geometry from what came over the wire.
pub fn config(lots: &wire::Lots) -> Result<LotConfig> {
    LotConfig::new(
        quantity(&lots.base_lots_per_base_unit, "base_lots_per_base_unit")?,
        quantity(
            &lots.tick_size_in_quote_lots_per_base_unit,
            "tick_size_in_quote_lots_per_base_unit",
        )?,
        quantity(&lots.base_atoms_per_base_lot, "base_atoms_per_base_lot")?,
        quantity(&lots.quote_atoms_per_quote_lot, "quote_atoms_per_quote_lot")?,
    )
    // Validated rather than trusted. These arrive over a network from a process that
    // decoded them out of an account, and the exactness invariant is what every price on
    // screen rests on.
    .context("the market's lot configuration is not one a market could have been created with")
}

/// How many quote lots one base lot costs at one tick.
fn quote_lots_per_lot_per_tick(config: &LotConfig) -> u64 {
    // Exact by the invariant `LotConfig::new` enforces: the tick size is a whole number of
    // quote lots per base unit, and there are `base_lots_per_base_unit` lots in a unit.
    config.tick_size_in_quote_lots_per_base_unit / config.base_lots_per_base_unit
}

/// A price in ticks, as a decimal in whole quote tokens.
pub fn price(config: &LotConfig, ticks: u64, quote_decimals: u32) -> String {
    let quote_lots = u128::from(ticks) * u128::from(quote_lots_per_lot_per_tick(config))
        * u128::from(config.base_lots_per_base_unit);
    decimal(
        quote_lots * u128::from(config.quote_atoms_per_quote_lot),
        quote_decimals,
    )
}

/// A size in base lots, as a decimal in whole base tokens.
pub fn size(config: &LotConfig, base_lots: u64, base_decimals: u32) -> String {
    decimal(
        u128::from(base_lots) * u128::from(config.base_atoms_per_base_lot),
        base_decimals,
    )
}

/// A quote-lot amount, as a decimal in whole quote tokens.
pub fn quote(config: &LotConfig, quote_lots: u64, quote_decimals: u32) -> String {
    decimal(
        u128::from(quote_lots) * u128::from(config.quote_atoms_per_quote_lot),
        quote_decimals,
    )
}

/// Renders an atom count as a decimal, trimming trailing zeros but never the value.
///
/// Integer arithmetic throughout. A float here would round exactly the quantities this
/// whole codebase keeps exact, and it would do it silently.
fn decimal(atoms: u128, decimals: u32) -> String {
    let divisor = 10u128.pow(decimals);
    let whole = atoms / divisor;
    let fraction = atoms % divisor;
    if fraction == 0 {
        return whole.to_string();
    }
    let mut digits = format!("{fraction:0width$}", width = decimals as usize);
    while digits.ends_with('0') {
        digits.pop();
    }
    format!("{whole}.{digits}")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The SOL/USDC shape the CLI creates: 9-decimal base, 6-decimal quote.
    fn solusdc() -> LotConfig {
        LotConfig::new(1_000, 1_000, 1_000_000, 1).unwrap()
    }

    #[test]
    fn a_price_renders_in_whole_quote_tokens() {
        let config = solusdc();
        assert_eq!(price(&config, 150_000, 6), "150");
        assert_eq!(price(&config, 150_250, 6), "150.25");
        assert_eq!(price(&config, 1, 6), "0.001");
    }

    #[test]
    fn a_size_renders_in_whole_base_tokens() {
        let config = solusdc();
        assert_eq!(size(&config, 1_000, 9), "1");
        assert_eq!(size(&config, 1, 9), "0.001");
        assert_eq!(size(&config, 25, 9), "0.025");
    }

    #[test]
    fn a_quantity_past_the_range_a_double_covers_survives() {
        // The reason the indexer quotes these. A bid's stored sequence number sits just
        // below u64::MAX; Rust parses it exactly, which is the whole difference between
        // this client and a browser.
        let text = "18446744073709551610";
        assert_eq!(quantity(text, "id").unwrap(), 18_446_744_073_709_551_610);
        assert_eq!(quantity(text, "id").unwrap().to_string(), text);
    }

    #[test]
    fn a_field_that_is_not_a_number_is_an_error_rather_than_a_zero() {
        // A zero here is a real price and a real size, and it would be rendered as one.
        for bad in ["", " ", "1.5", "0x10", "abc", "-1"] {
            assert!(quantity(bad, "price").is_err(), "for {bad:?}");
        }
    }

    #[test]
    fn a_lot_configuration_no_market_could_exist_with_is_refused() {
        // These arrive over a network. The exactness invariant is what every price on
        // screen rests on, so it is checked rather than assumed.
        let broken = wire::Lots {
            base_lots_per_base_unit: "0".into(),
            tick_size_in_quote_lots_per_base_unit: "1000".into(),
            base_atoms_per_base_lot: "1000000".into(),
            quote_atoms_per_quote_lot: "1".into(),
        };
        assert!(config(&broken).is_err());
    }

    #[test]
    fn trailing_zeros_go_but_the_value_never_does() {
        assert_eq!(decimal(1_500_000, 6), "1.5");
        assert_eq!(decimal(1_000_000, 6), "1");
        assert_eq!(decimal(1, 6), "0.000001");
        assert_eq!(decimal(0, 6), "0");
    }
}
