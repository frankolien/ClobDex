//! Mapping engine errors onto Solana program errors.
//!
//! Every [`EngineError`] gets a stable numeric code. These are part of the program's
//! public interface — a client decoding a failed transaction sees the number, not the
//! name — so the discriminants are written out explicitly and must never be reordered.

use clob_engine::EngineError;
use pinocchio::error::ProgramError;

/// Program-specific failures that have no engine equivalent.
///
/// Numbered from 1000 to leave the low range to [`EngineError`], so the two can never
/// collide as the engine grows.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum ClobError {
    /// The account discriminator did not match a market.
    NotAMarket = 1000,
    /// The market account was written by an incompatible program version.
    VersionMismatch = 1001,
    /// The market account is not large enough for its declared size class.
    MarketAccountTooSmall = 1002,
    /// The declared size class is not one this program supports.
    UnknownSizeClass = 1003,
    /// The market account is already initialized.
    MarketAlreadyInitialized = 1004,
    /// A vault account did not match the address recorded in the market header.
    VaultMismatch = 1005,
    /// The mint did not match the one recorded in the market header.
    MintMismatch = 1006,
    /// The signer is not this market's authority.
    NotMarketAuthority = 1007,
    /// The fee recipient account did not match the market header.
    FeeRecipientMismatch = 1008,
    /// The vault signer address did not derive from the expected seeds.
    InvalidVaultSigner = 1009,
    /// Instruction data was shorter than the instruction requires.
    InstructionDataTooShort = 1010,
    /// The instruction discriminant is not one this program handles.
    UnknownInstruction = 1011,
    /// The market data could not be cast — wrong length or misaligned.
    MarketDataUnaligned = 1012,
    /// A converted token amount exceeded `u64`.
    AmountOverflow = 1013,
}

impl From<ClobError> for ProgramError {
    fn from(error: ClobError) -> Self {
        ProgramError::Custom(error as u32)
    }
}

/// Stable numeric code for an engine error.
///
/// Written as an explicit match rather than a `#[repr(u32)]` cast on [`EngineError`] so
/// that adding or reordering a variant in the engine cannot silently renumber a code a
/// client is already decoding.
pub const fn engine_error_code(error: EngineError) -> u32 {
    match error {
        EngineError::SeatTableFull => 1,
        EngineError::SeatNotFound => 2,
        EngineError::SeatNotEmpty => 3,
        EngineError::BookSideFull => 4,
        EngineError::InsufficientBaseFunds => 5,
        EngineError::InsufficientQuoteFunds => 6,
        EngineError::OrderNotFound => 7,
        EngineError::NotOrderOwner => 8,
        EngineError::SelfTradeAborted => 9,
        EngineError::PostOnlyWouldCross => 10,
        EngineError::PostOnlyNoRoom => 11,
        EngineError::MinimumFillNotMet => 12,
        EngineError::ZeroSize => 13,
        EngineError::Overflow => 14,
        EngineError::InvalidFeeRate => 15,
        EngineError::InvalidLotConfig(_) => 16,
        EngineError::MarketAlreadyInitialized => 17,
    }
}

/// Converts an engine result into a program result.
pub fn map_engine<T>(result: Result<T, EngineError>) -> Result<T, ProgramError> {
    result.map_err(|error| ProgramError::Custom(engine_error_code(error)))
}
