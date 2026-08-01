//! The quotes the bot wants resting.
//!
//! A ladder is symmetric around `fair + skew`: the first level on each side sits a
//! half-spread away, and each level after that a step further out.
//!
//! # It cannot cross
//!
//! With the centre at `c` and a half-spread of `h`, the best bid is `c - h` and the best
//! ask is `c + h`, so the two sides are `2h` apart and `h` is at least one tick. Against
//! *other people's* book the argument is only slightly longer, and it is the reason
//! [`fair`](crate::fair) clamps to the touch:
//!
//! - Two-sided book: `c` is the midpoint plus a skew smaller than `h`, and the midpoint
//!   is strictly inside a book that cannot itself be crossed. So `c - h` lands below
//!   their ask and `c + h` above their bid.
//! - One-sided book: `fair` was clamped to that side, so the same `± h` puts each quote
//!   on the correct side of it.
//! - Empty book: there is nothing to cross.
//!
//! Every price is computed in `i128` and converted back at the end. Not caution for its
//! own sake — clamping a centre to `u64` would break the arithmetic the proof above is
//! made of, and a level that cannot be expressed is dropped instead.
//!
//! # It cannot overdraw
//!
//! Levels are added from the touch outward until the seat's balance will not cover the
//! next one. Nearest-first because those are the levels most likely to trade: a bot that
//! funded its outermost quotes and skipped the touch would be paying rent to be nowhere
//! near the market.
//!
//! The whole balance counts, locked included, because a refresh cancels before it places
//! — the capital behind the old ladder is exactly what funds the new one.

use clob_book::{BaseLots, LotConfig, Side, Ticks};
use clob_engine::TraderState;

use crate::Params;
use crate::fair::Fair;
use crate::inventory::Inventory;

/// One quote the bot wants resting.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Quote {
    /// Which side.
    pub side: Side,
    /// Price, in ticks.
    pub price_in_ticks: Ticks,
    /// Size, in base lots.
    pub base_lots: BaseLots,
}

/// Builds the ladder, bids best-first followed by asks best-first.
///
/// The order is part of the contract: [`plan`](crate::plan) compares this against what is
/// resting position by position, and both sides of that comparison have to agree on what
/// position means.
pub fn build(
    params: &Params,
    fair: &Fair,
    inventory: &Inventory,
    balances: &TraderState,
    lots: &LotConfig,
) -> Vec<Quote> {
    let centre = i128::from(fair.price_in_ticks) + i128::from(inventory.skew_in_ticks(params));
    let mut quotes = Vec::with_capacity(params.full_ladder_size());

    for side in [Side::Bid, Side::Ask] {
        if !inventory.may_quote(side, params) {
            continue;
        }
        extend(&mut quotes, side, centre, params, balances, lots);
    }
    quotes
}

/// Adds as many levels of one side as the seat can pay for.
fn extend(
    quotes: &mut Vec<Quote>,
    side: Side,
    centre: i128,
    params: &Params,
    balances: &TraderState,
    lots: &LotConfig,
) {
    let size = BaseLots(params.size_in_base_lots);
    // A bid is paid for in quote and an ask is paid for in base, so the two sides draw on
    // separate balances and neither can starve the other.
    let mut remaining = match side {
        Side::Bid => balances.total_quote_lots().as_u64(),
        Side::Ask => balances.total_base_lots().as_u64(),
    };

    for level in 0..params.levels {
        let offset = i128::from(params.half_spread_in_ticks)
            + i128::from(level) * i128::from(params.level_step_in_ticks);
        let price = match side {
            Side::Bid => centre - offset,
            Side::Ask => centre + offset,
        };

        // Every later level is further out, so the first one that cannot be expressed as
        // a price ends the side rather than skipping a hole in the middle of it.
        let Ok(price) = u64::try_from(price) else {
            return;
        };
        if price == 0 {
            return;
        }
        let price = Ticks(price);

        let cost = match side {
            Side::Bid => match lots.quote_lots_for(price, size) {
                Some(quote_lots) => quote_lots.as_u64(),
                // Only reachable at prices no market has, and the levels beyond it cost
                // more still.
                None => return,
            },
            Side::Ask => size.as_u64(),
        };
        let Some(left) = remaining.checked_sub(cost) else {
            return;
        };
        remaining = left;

        quotes.push(Quote {
            side,
            price_in_ticks: price,
            base_lots: size,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fair::{self, Source, Touch};
    use crate::params::MAX_LEVELS;
    use clob_book::QuoteLots;
    use proptest::prelude::*;

    /// The geometry `clob-cli` creates markets with: 0.001 base units per lot, ticks of
    /// 0.001 quote units.
    fn lots() -> LotConfig {
        LotConfig::new(1_000, 1_000, 1_000_000, 1).expect("a valid configuration")
    }

    fn params() -> Params {
        Params {
            reference_in_ticks: 150_000,
            half_spread_in_ticks: 50,
            level_step_in_ticks: 25,
            levels: 3,
            size_in_base_lots: 100,
            target_base_lots: 1_000,
            inventory_limit_lots: 500,
            max_skew_in_ticks: 20,
            drift_tolerance_in_ticks: 10,
        }
    }

    fn balances(base: u64, quote: u64) -> TraderState {
        TraderState {
            base_lots_free: BaseLots(base),
            base_lots_locked: BaseLots::ZERO,
            quote_lots_free: QuoteLots(quote),
            quote_lots_locked: QuoteLots::ZERO,
        }
    }

    /// Funds nothing could exhaust, for tests about price rather than budget.
    fn rich() -> TraderState {
        balances(1_000_000, 1_000_000_000_000)
    }

    fn at(price_in_ticks: u64) -> Fair {
        Fair {
            price_in_ticks,
            source: Source::Reference,
        }
    }

    fn prices(quotes: &[Quote], side: Side) -> Vec<u64> {
        quotes
            .iter()
            .filter(|quote| quote.side == side)
            .map(|quote| quote.price_in_ticks.as_u64())
            .collect()
    }

    #[test]
    fn a_flat_bot_quotes_symmetrically_around_fair() {
        let params = params();
        let flat = Inventory::of(&balances(1_000, 1_000_000_000), &params);
        let quotes = build(&params, &at(150_000), &flat, &rich(), &lots());

        assert_eq!(prices(&quotes, Side::Bid), vec![149_950, 149_925, 149_900]);
        assert_eq!(prices(&quotes, Side::Ask), vec![150_050, 150_075, 150_100]);
    }

    #[test]
    fn bids_come_before_asks_and_each_side_is_best_first() {
        // The ordering `plan` compares against. Best-first on both sides means position
        // zero is the touch, which is where a partial fill shows up first.
        let params = params();
        let flat = Inventory::of(&balances(1_000, 1_000_000_000), &params);
        let quotes = build(&params, &at(150_000), &flat, &rich(), &lots());

        assert_eq!(quotes.len(), params.full_ladder_size());
        assert!(quotes[..3].iter().all(|quote| quote.side == Side::Bid));
        assert!(quotes[3..].iter().all(|quote| quote.side == Side::Ask));
    }

    #[test]
    fn every_level_is_the_configured_size() {
        let params = params();
        let flat = Inventory::of(&balances(1_000, 1_000_000_000), &params);
        let quotes = build(&params, &at(150_000), &flat, &rich(), &lots());
        assert!(quotes.iter().all(|quote| quote.base_lots == BaseLots(100)));
    }

    #[test]
    fn a_long_position_moves_the_whole_ladder_down() {
        // Both sides shift together. The ask becoming cheaper is the point; the bid
        // becoming cheaper is what stops the bot buying more at the old price.
        let params = params();
        let long = Inventory::of(&balances(1_250, 1_000_000_000), &params);
        let quotes = build(&params, &at(150_000), &long, &rich(), &lots());

        assert_eq!(prices(&quotes, Side::Bid)[0], 149_940);
        assert_eq!(prices(&quotes, Side::Ask)[0], 150_040);
    }

    #[test]
    fn a_position_at_the_cap_quotes_only_the_side_that_unwinds_it() {
        let params = params();
        let long = Inventory::of(&balances(1_500, 1_000_000_000), &params);
        let quotes = build(&params, &at(150_000), &long, &rich(), &lots());

        assert!(prices(&quotes, Side::Bid).is_empty(), "must stop buying");
        assert_eq!(prices(&quotes, Side::Ask).len(), 3, "must keep selling");
    }

    #[test]
    fn a_thin_balance_funds_the_levels_nearest_the_market() {
        // Base for two asks, not three. The two that survive are the two closest to the
        // touch, because those are the ones that trade.
        //
        // The target matches what the seat holds, so this is a question about funds and
        // not about position: at a target of 1,000 a seat holding 250 is short past the
        // cap and would not be quoting asks at all.
        let params = Params {
            target_base_lots: 250,
            ..params()
        };
        let seat = balances(250, 1_000_000_000);
        let inventory = Inventory::of(&seat, &params);
        let quotes = build(&params, &at(150_000), &inventory, &seat, &lots());

        assert_eq!(prices(&quotes, Side::Ask), vec![150_050, 150_075]);
    }

    #[test]
    fn an_empty_seat_quotes_nothing_at_all() {
        let params = Params {
            target_base_lots: 0,
            ..params()
        };
        let seat = balances(0, 0);
        let inventory = Inventory::of(&seat, &params);
        assert!(build(&params, &at(150_000), &inventory, &seat, &lots()).is_empty());
    }

    #[test]
    fn the_two_sides_draw_on_separate_balances() {
        // Base but no quote: the asks are fully funded and the bids are not funded at
        // all. One shared budget would have let the asks eat into the bid side.
        let params = params();
        let seat = balances(1_000, 0);
        let inventory = Inventory::of(&seat, &params);
        let quotes = build(&params, &at(150_000), &inventory, &seat, &lots());

        assert!(prices(&quotes, Side::Bid).is_empty());
        assert_eq!(prices(&quotes, Side::Ask).len(), 3);
    }

    #[test]
    fn capital_behind_the_old_ladder_counts_toward_the_new_one() {
        // Everything locked, nothing free. A refresh cancels before it places, so this
        // seat can afford exactly what it could afford before it quoted — a budget that
        // looked only at free balance would refuse to re-quote a fully deployed bot,
        // which is every bot that is working.
        let params = params();
        let deployed = TraderState {
            base_lots_free: BaseLots::ZERO,
            base_lots_locked: BaseLots(1_000),
            quote_lots_free: QuoteLots::ZERO,
            quote_lots_locked: QuoteLots(1_000_000_000),
        };
        let inventory = Inventory::of(&deployed, &params);
        let quotes = build(&params, &at(150_000), &inventory, &deployed, &lots());
        assert_eq!(quotes.len(), params.full_ladder_size());
    }

    #[test]
    fn levels_that_would_price_below_zero_are_dropped_not_wrapped() {
        // Fair at 60 with a half-spread of 50 and a step of 25: the first bid lands at
        // 10 and the second would be at -15. Unsigned arithmetic would have wrapped it
        // to an enormous price and posted a bid far above the ask.
        let params = params();
        let flat = Inventory::of(&balances(1_000, 1_000_000_000), &params);
        let quotes = build(&params, &at(60), &flat, &rich(), &lots());

        assert_eq!(prices(&quotes, Side::Bid), vec![10]);
        assert_eq!(prices(&quotes, Side::Ask), vec![110, 135, 160]);
    }

    #[test]
    fn a_fair_price_beneath_the_half_spread_quotes_only_asks() {
        let params = params();
        let flat = Inventory::of(&balances(1_000, 1_000_000_000), &params);
        let quotes = build(&params, &at(20), &flat, &rich(), &lots());

        assert!(prices(&quotes, Side::Bid).is_empty());
        assert_eq!(prices(&quotes, Side::Ask)[0], 70);
    }

    #[test]
    fn a_price_that_cannot_be_expressed_ends_the_side_instead_of_wrapping() {
        // The mirror of the underflow case: `u64::MAX + 50` is not a price. A geometry of
        // one quote lot per base lot per tick, and a size of one, so that it is the
        // arithmetic that ends the ask side and not the budget.
        let unit = LotConfig::new(1, 1, 1, 1).expect("a valid configuration");
        let params = Params {
            size_in_base_lots: 1,
            ..params()
        };
        let seat = balances(1_000, u64::MAX);
        let inventory = Inventory::of(&seat, &params);
        let quotes = build(&params, &at(u64::MAX), &inventory, &seat, &unit);

        assert!(prices(&quotes, Side::Ask).is_empty());
        // And one bid: at this price a single lot costs very nearly everything the seat
        // has, so the budget ends the bid side one level in.
        assert_eq!(prices(&quotes, Side::Bid), vec![u64::MAX - 50]);
    }

    // ---------------------------------------------------------------------------------
    // The no-cross property, over the whole parameter space
    // ---------------------------------------------------------------------------------

    prop_compose! {
        /// Any configuration that would pass validation.
        fn any_params()(
            half_spread in 1u64..500,
            skew_seed in 0u64..10_000,
            level_step in 0u64..200,
            levels in 1u8..=MAX_LEVELS,
            size in 1u64..1_000,
            target in 0u64..100_000,
            limit in 1u64..100_000,
            reference in 1u64..2_000_000,
        ) -> Params {
            Params {
                reference_in_ticks: reference,
                half_spread_in_ticks: half_spread,
                level_step_in_ticks: level_step,
                levels,
                size_in_base_lots: size,
                target_base_lots: target,
                inventory_limit_lots: limit,
                // Derived rather than generated, so the invariant holds by construction
                // and the test spends its runs on prices instead of on rejected setups.
                max_skew_in_ticks: skew_seed % half_spread,
                drift_tolerance_in_ticks: 0,
            }
        }
    }

    proptest! {
        #[test]
        fn a_ladder_never_crosses_itself_or_anybody_else(
            params in any_params(),
            their_bid in 1u64..1_000_000,
            their_gap in 1u64..10_000,
            show_bid in any::<bool>(),
            show_ask in any::<bool>(),
            held in 0u64..200_000,
        ) {
            prop_assert_eq!(params.validate(), Ok(()));

            // A real book is never crossed — the engine would have matched it — so the
            // ask is always above the bid.
            let touch = Touch {
                bid_in_ticks: show_bid.then_some(their_bid),
                ask_in_ticks: show_ask.then_some(their_bid + their_gap),
            };
            let seat = balances(held, 1_000_000_000_000);
            let inventory = Inventory::of(&seat, &params);
            let fair = fair::price(&touch, params.reference_in_ticks);
            let quotes = build(&params, &fair, &inventory, &seat, &lots());

            let best_bid = prices(&quotes, Side::Bid).into_iter().max();
            let best_ask = prices(&quotes, Side::Ask).into_iter().min();

            if let (Some(bid), Some(ask)) = (best_bid, best_ask) {
                prop_assert!(bid < ask, "self-crossed: bid {bid} >= ask {ask}");
            }
            // Post-only rejects a quote that crosses, which would fail the whole refresh.
            if let (Some(bid), Some(theirs)) = (best_bid, touch.ask_in_ticks) {
                prop_assert!(bid < theirs, "bid {bid} would take their ask at {theirs}");
            }
            if let (Some(ask), Some(theirs)) = (best_ask, touch.bid_in_ticks) {
                prop_assert!(ask > theirs, "ask {ask} would take their bid at {theirs}");
            }
        }

        #[test]
        fn a_ladder_never_costs_more_than_the_seat_holds(
            params in any_params(),
            base in 0u64..100_000,
            quote in 0u64..10_000_000_000u64,
        ) {
            let seat = balances(base, quote);
            let inventory = Inventory::of(&seat, &params);
            let fair = Fair { price_in_ticks: params.reference_in_ticks, source: Source::Reference };
            let quotes = build(&params, &fair, &inventory, &seat, &lots());

            let lots = lots();
            let (mut base_needed, mut quote_needed) = (0u64, 0u64);
            for quote_ in &quotes {
                match quote_.side {
                    Side::Ask => base_needed += quote_.base_lots.as_u64(),
                    Side::Bid => {
                        quote_needed += lots
                            .quote_lots_for(quote_.price_in_ticks, quote_.base_lots)
                            .expect("the builder priced it, so it prices")
                            .as_u64();
                    }
                }
            }
            prop_assert!(base_needed <= base, "{base_needed} base lots against {base} held");
            prop_assert!(quote_needed <= quote, "{quote_needed} quote lots against {quote} held");
        }
    }
}
