//! Exactness properties of the tick/lot conversions.
//!
//! The invariant that matters is that a fill's quote value is computed *exactly*. Every
//! truncated division is dust the venue silently keeps, and dust breaks conservation of
//! funds — the one thing a book holding user money must never get wrong.

use clob_book::{BaseAtoms, BaseLots, LotConfig, QuoteAtoms, QuoteLots, Ticks};
use proptest::prelude::*;

/// Generates configurations that satisfy the exactness invariant by construction: the
/// tick size is always a whole multiple of `base_lots_per_base_unit`.
fn valid_config() -> impl Strategy<Value = LotConfig> {
    (1u64..1_000_000, 1u64..10_000, 1u64..1_000_000_000, 1u64..1_000_000).prop_map(
        |(base_lots_per_base_unit, multiplier, base_atoms, quote_atoms)| {
            LotConfig::new(
                base_lots_per_base_unit,
                base_lots_per_base_unit * multiplier,
                base_atoms,
                quote_atoms,
            )
            .expect("constructed to satisfy the invariant")
        },
    )
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(1024))]

    /// Any config built as a whole multiple of `base_lots_per_base_unit` is accepted,
    /// and any config that is not is rejected. This pins the admissible set exactly.
    #[test]
    fn the_invariant_is_exactly_divisibility(
        base_lots_per_base_unit in 2u64..10_000,
        tick_size in 1u64..100_000_000,
    ) {
        let config = LotConfig::new(base_lots_per_base_unit, tick_size, 1, 1);
        prop_assert_eq!(
            config.is_ok(),
            tick_size % base_lots_per_base_unit == 0,
            "acceptance did not match divisibility"
        );
    }

    /// The folded constant must agree with the full unfolded expression for every price
    /// and size that does not overflow. This is the whole justification for folding.
    #[test]
    fn folding_the_division_changes_nothing(
        config in valid_config(),
        price in 0u64..1_000_000_000,
        size in 0u64..1_000_000_000,
    ) {
        let Some(folded) = config.quote_lots_for(Ticks(price), BaseLots(size)) else {
            return Ok(()); // Overflow is reported, not silently wrapped; nothing to compare.
        };

        let unfolded = (price as u128)
            * (config.tick_size_in_quote_lots_per_base_unit as u128)
            * (size as u128)
            / (config.base_lots_per_base_unit as u128);

        prop_assert_eq!(folded.as_u64() as u128, unfolded);
    }

    /// No fill ever leaves a remainder — the property that folding buys us, stated
    /// directly rather than via the formula.
    #[test]
    fn fills_are_exact_for_every_size(
        config in valid_config(),
        price in 0u64..100_000,
        size in 0u64..100_000,
    ) {
        let numerator = (price as u128)
            * (config.tick_size_in_quote_lots_per_base_unit as u128)
            * (size as u128);
        prop_assert_eq!(numerator % (config.base_lots_per_base_unit as u128), 0);
    }

    /// A budget conversion must never authorise spending more than the budget, and must
    /// leave less than one base lot of it unspent.
    #[test]
    fn budget_conversion_is_tight_and_never_overspends(
        config in valid_config(),
        price in 1u64..1_000_000,
        budget in 0u64..1_000_000_000_000,
    ) {
        let Some(size) = config.base_lots_for(Ticks(price), QuoteLots(budget)) else {
            return Ok(()); // Price times the folded constant overflowed.
        };
        let Some(cost) = config.quote_lots_for(Ticks(price), size) else {
            return Ok(());
        };

        prop_assert!(cost.as_u64() <= budget, "budget conversion overspent");

        // Tight: one more base lot would exceed the budget.
        if let Some(cost_of_one_more) = config.quote_lots_for(Ticks(price), size + BaseLots(1)) {
            prop_assert!(cost_of_one_more.as_u64() > budget, "left a whole lot unspent");
        }
    }

    /// Lots round-trip through atoms without loss; the reverse direction loses only
    /// sub-lot dust, which stays with the depositor rather than the venue.
    #[test]
    fn atom_conversions_round_trip(
        config in valid_config(),
        lots in 0u64..1_000_000,
        atoms in 0u64..1_000_000_000_000,
    ) {
        if let Some(base_atoms) = config.base_atoms(BaseLots(lots)) {
            prop_assert_eq!(config.base_lots_from_atoms(base_atoms), BaseLots(lots));
        }
        if let Some(quote_atoms) = config.quote_atoms(QuoteLots(lots)) {
            prop_assert_eq!(config.quote_lots_from_atoms(quote_atoms), QuoteLots(lots));
        }

        // Converting atoms down to lots and back never invents value.
        let down = config.base_lots_from_atoms(BaseAtoms(atoms));
        if let Some(back) = config.base_atoms(down) {
            prop_assert!(back.as_u64() <= atoms, "atom round-trip gained value");
            prop_assert!(
                atoms - back.as_u64() < config.base_atoms_per_base_lot,
                "lost more than sub-lot dust"
            );
        }

        let down = config.quote_lots_from_atoms(QuoteAtoms(atoms));
        if let Some(back) = config.quote_atoms(down) {
            prop_assert!(back.as_u64() <= atoms);
            prop_assert!(atoms - back.as_u64() < config.quote_atoms_per_quote_lot);
        }
    }

    /// Value is monotonic in both price and size — a larger order at a weakly higher
    /// price is never worth less. Cheap to state, and it would catch a sign or operator
    /// slip that the exactness properties above would not.
    #[test]
    fn value_is_monotonic(
        config in valid_config(),
        price in 0u64..100_000,
        size in 0u64..100_000,
        delta in 0u64..1_000,
    ) {
        let base = config.quote_lots_for(Ticks(price), BaseLots(size));
        let bigger = config.quote_lots_for(Ticks(price), BaseLots(size + delta));
        let dearer = config.quote_lots_for(Ticks(price + delta), BaseLots(size));

        if let (Some(base), Some(bigger)) = (base, bigger) {
            prop_assert!(bigger >= base);
        }
        if let (Some(base), Some(dearer)) = (base, dearer) {
            prop_assert!(dearer >= base);
        }
    }

    /// A zero-valued leg is worth zero, whatever the other leg is.
    #[test]
    fn zero_price_or_zero_size_is_worthless(config in valid_config(), value in 0u64..1_000_000) {
        prop_assert_eq!(
            config.quote_lots_for(Ticks::ZERO, BaseLots(value)),
            Some(QuoteLots::ZERO)
        );
        prop_assert_eq!(
            config.quote_lots_for(Ticks(value), BaseLots::ZERO),
            Some(QuoteLots::ZERO)
        );
    }
}
