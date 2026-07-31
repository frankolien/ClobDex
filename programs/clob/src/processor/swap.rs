//! Atomic swap: trade without holding a balance on the market.
//!
//! Every other order instruction needs the caller's funds already deposited. That is
//! right for a market maker, which keeps inventory on the venue anyway, and wrong for
//! an aggregator routing a stranger's wallet through in a single transaction.
//!
//! This deposits, matches, and withdraws in one instruction. It costs eight accounts
//! against `PlaceOrder`'s two, which is the honest price of not pre-funding.
//!
//! # Only the swap's own proceeds come back out
//!
//! The naive version — withdraw everything free at the end — would drain the standing
//! balance of any trader who also happens to be a maker on this market. So the handler
//! records the seat's balances before matching and returns only the *difference*. For a
//! caller with no seat that difference is everything, which is the common case; for a
//! caller with inventory it is exactly the trade.
//!
//! # The input is computed, not supplied
//!
//! The caller names a limit price and a size; the program works out the most that can
//! cost and moves exactly that in. A caller-supplied input amount would be one more
//! thing that can disagree with the order it is meant to fund.
//!
//! This is why a swap must be priced. An unpriced market buy has no bounded cost, so
//! there is no amount to move in — aggregators always have a limit anyway.

use clob_book::{BaseLots, QuoteLots, Side, Ticks};
use clob_engine::{OrderPacket, SelfTradeBehavior, TraderKey};
use pinocchio::account::AccountView;
use pinocchio::address::Address;
use pinocchio::error::ProgramResult;
use pinocchio::instruction::cpi::{Seed, Signer};
use pinocchio_token::instructions::Transfer;

use super::trade::{emit_receipt, to_bytes};
use super::{at, expect_address, expect_market_account, expect_signer, split_market};
use crate::dispatch_market;
use crate::error::{ClobError, map_engine};
use crate::event::EventBuffer;
use crate::instruction::Reader;
use crate::state::{SizeClass, VAULT_SIGNER_SEED, split_initialized};

/// Accounts: market, trader (signer), trader base, trader quote, base vault, quote
/// vault, vault signer, token program. Optionally followed by log authority and this
/// program for an event receipt.
pub fn swap(
    program_id: &Address,
    accounts: &mut [AccountView],
    reader: &mut Reader<'_>,
) -> ProgramResult {
    let side = reader.side()?;
    let price_in_ticks = Ticks(reader.u64()?);
    let num_base_lots = BaseLots(reader.u64()?);
    let min_base_lots_to_fill = BaseLots(reader.u64()?);
    let match_limit = reader.u32()?;
    let log_bump = reader.optional_u8();

    if num_base_lots.is_zero() {
        return Err(map_engine::<()>(Err(clob_engine::EngineError::ZeroSize)).unwrap_err());
    }

    let (market_account, rest) = split_market(accounts)?;
    expect_market_account(market_account, program_id)?;

    let trader = at(rest, 0)?;
    let trader_base = at(rest, 1)?;
    let trader_quote = at(rest, 2)?;
    let base_vault = at(rest, 3)?;
    let quote_vault = at(rest, 4)?;
    let vault_signer = at(rest, 5)?;
    expect_signer(trader)?;

    let market_address = *market_account.address();
    let key = TraderKey(to_bytes(trader.address()));

    // Phase 1: work out what has to come in, and remember what the seat already held.
    let (seat, created, before_base, before_quote, base_in, quote_in, bump) = {
        let mut data = market_account.try_borrow_mut()?;
        let (header, market_bytes) = split_initialized(&mut data)?;
        expect_address(base_vault, &header.base_vault, ClobError::VaultMismatch)?;
        expect_address(quote_vault, &header.quote_vault, ClobError::VaultMismatch)?;
        let bump = header.vault_signer_bump as u8;
        let size_class = SizeClass::from_u64(header.size_class)?;

        dispatch_market!(size_class, market_bytes, |market| {
            let created = market.seat_index(&key) == clob_engine::NO_SEAT;
            let seat = map_engine(market.claim_seat(key))?;
            let state = *market.traders().state(seat).map_err(map_seat)?;

            let (base_in, quote_in) = match side {
                // Selling: the size itself is the input.
                Side::Ask => (num_base_lots, QuoteLots::ZERO),
                // Buying: the most this can cost is the whole size at the limit price,
                // plus the fee on that.
                Side::Bid => {
                    let gross = market
                        .lot_config()
                        .quote_lots_for(price_in_ticks, num_base_lots)
                        .ok_or(ClobError::AmountOverflow)?;
                    let fee = map_engine(market.header().fees.fee_on(gross))?;
                    let total = gross.checked_add(fee).ok_or(ClobError::AmountOverflow)?;
                    (BaseLots::ZERO, total)
                }
            };

            (
                seat,
                created,
                state.base_lots_free,
                state.quote_lots_free,
                base_in,
                quote_in,
                bump,
            )
        })
    };

    let (base_in_atoms, quote_in_atoms) = atoms(market_account, base_in, quote_in)?;

    // Phase 2: pull the funds in. Borrows are released, so the CPI is legal.
    if base_in_atoms > 0 {
        Transfer::new(trader_base, base_vault, trader, base_in_atoms).invoke()?;
    }
    if quote_in_atoms > 0 {
        Transfer::new(trader_quote, quote_vault, trader, quote_in_atoms).invoke()?;
    }

    // Phase 3: credit, match, and debit back out only what the trade produced.
    let mut events = EventBuffer::new();
    let (outcome, base_out, quote_out) = {
        let mut data = market_account.try_borrow_mut()?;
        let (header, market_bytes) = split_initialized(&mut data)?;
        let size_class = SizeClass::from_u64(header.size_class)?;

        dispatch_market!(size_class, market_bytes, |market| {
            map_engine(market.deposit(seat, base_in, quote_in))?;

            let outcome = map_engine(market.place_order(
                seat,
                OrderPacket::ImmediateOrCancel {
                    side,
                    price_in_ticks: Some(price_in_ticks),
                    num_base_lots,
                    min_base_lots_to_fill,
                    self_trade_behavior: SelfTradeBehavior::DecrementTake,
                    match_limit,
                },
                &mut events,
            ))?;

            let state = *market.traders().state(seat).map_err(map_seat)?;
            // Non-negative by construction: the input was the maximum the order could
            // consume, so a fill can only leave the seat holding more than it started
            // with on both legs.
            let base_out = state.base_lots_free.saturating_sub(before_base);
            let quote_out = state.quote_lots_free.saturating_sub(before_quote);
            map_engine(market.withdraw(seat, base_out, quote_out))?;

            // A seat this instruction created has no orders and, now, no balance, so
            // returning its slot costs the caller nothing and keeps the trader table
            // from filling up with one-shot swappers.
            if created {
                let _ = market.release_seat(&key);
            }
            (outcome, base_out, quote_out)
        })
    };

    let (base_out_atoms, quote_out_atoms) = atoms(market_account, base_out, quote_out)?;

    // Phase 4: pay the trader out.
    let bump_seed = [bump];
    let seeds = [
        Seed::from(VAULT_SIGNER_SEED),
        Seed::from(market_address.as_ref()),
        Seed::from(&bump_seed[..]),
    ];
    if base_out_atoms > 0 {
        Transfer::new(base_vault, trader_base, vault_signer, base_out_atoms)
            .invoke_signed(&[Signer::from(&seeds[..])])?;
    }
    if quote_out_atoms > 0 {
        Transfer::new(quote_vault, trader_quote, vault_signer, quote_out_atoms)
            .invoke_signed(&[Signer::from(&seeds[..])])?;
    }

    if let (Some(log_bump), Ok(authority)) = (log_bump, at(rest, 7)) {
        emit_receipt(program_id, authority, log_bump, &events, &outcome, seat)?;
    }
    Ok(())
}

/// Converts a lot pair to atoms using the market's geometry.
fn atoms(
    market_account: &AccountView,
    base: BaseLots,
    quote: QuoteLots,
) -> Result<(u64, u64), pinocchio::error::ProgramError> {
    let data = market_account.try_borrow()?;
    let header_len = crate::state::HEADER_LEN;
    let header: &crate::state::MarketAccountHeader =
        bytemuck::try_from_bytes(&data[..header_len])
            .map_err(|_| ClobError::MarketDataUnaligned)?;
    let size_class = SizeClass::from_u64(header.size_class)?;

    // Read-only, so a shared cast is enough; the dispatch macro needs a mutable slice,
    // and the lot config is the only thing wanted here.
    let config = match size_class {
        SizeClass::Small => read_config::<128, 128, 32>(&data[header_len..])?,
        SizeClass::Medium => read_config::<512, 512, 128>(&data[header_len..])?,
        SizeClass::Large => read_config::<2048, 2048, 512>(&data[header_len..])?,
    };

    Ok((
        config.base_atoms(base).ok_or(ClobError::AmountOverflow)?.as_u64(),
        config.quote_atoms(quote).ok_or(ClobError::AmountOverflow)?.as_u64(),
    ))
}

fn read_config<const BIDS: usize, const ASKS: usize, const SEATS: usize>(
    bytes: &[u8],
) -> Result<clob_book::LotConfig, pinocchio::error::ProgramError> {
    let needed = clob_engine::Market::<BIDS, ASKS, SEATS>::SIZE_IN_BYTES;
    if bytes.len() < needed {
        return Err(ClobError::MarketAccountTooSmall.into());
    }
    let market: &clob_engine::Market<BIDS, ASKS, SEATS> =
        bytemuck::try_from_bytes(&bytes[..needed]).map_err(|_| ClobError::MarketDataUnaligned)?;
    Ok(*market.lot_config())
}

fn map_seat(error: clob_engine::EngineError) -> pinocchio::error::ProgramError {
    pinocchio::error::ProgramError::Custom(crate::error::engine_error_code(error))
}
