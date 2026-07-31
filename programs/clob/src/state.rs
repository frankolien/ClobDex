//! The market account: its header, its size classes, and how to load one.
//!
//! # Layout
//!
//! ```text
//! [ MarketAccountHeader (224 bytes) ][ clob_engine::Market<BIDS, ASKS, SEATS> ]
//! ```
//!
//! The header is a fixed multiple of eight bytes, so the engine's `Market` — which
//! needs eight-byte alignment — starts aligned and can be cast in place. Nothing is
//! deserialized at any point.
//!
//! # Size classes
//!
//! Book and seat capacities are const generics, so a single program cannot serve
//! arbitrary sizes without monomorphising for each. Three classes cover the useful
//! range, and the header records which one this account is. [`dispatch_market!`] turns
//! that runtime value back into the right static type.
//!
//! The alternative — one capacity for everything — would either price small markets out
//! with large-market rent or cap large ones at small-market depth.

use bytemuck::{Pod, Zeroable};
use clob_engine::Market;
use pinocchio::error::ProgramError;

use crate::error::ClobError;

/// Marks an account as a ClobDex market. First eight bytes of the account.
///
/// Chosen so that an all-zero account — which is what `CreateAccount` hands back — can
/// never be mistaken for an initialized market.
pub const MARKET_DISCRIMINATOR: u64 = u64::from_le_bytes(*b"CLOBMKT1");

/// Account format version. Bumped whenever the layout changes incompatibly.
pub const MARKET_VERSION: u64 = 1;

/// Seed prefix for a market's vault signer.
pub const VAULT_SIGNER_SEED: &[u8] = b"vault";

/// Fixed-size preamble describing the market account.
///
/// Addresses are stored as raw bytes rather than as `Address` so the header is `Pod`
/// without depending on that type's representation. Recording the vault addresses here
/// is a deliberate compute optimisation: verifying a vault becomes a 32-byte comparison
/// instead of a `find_program_address` call, which costs on the order of a thousand
/// compute units and would otherwise be paid on every deposit and withdrawal.
#[repr(C)]
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct MarketAccountHeader {
    /// [`MARKET_DISCRIMINATOR`].
    pub discriminator: u64,
    /// [`MARKET_VERSION`].
    pub version: u64,
    /// Which [`SizeClass`] this account was created at.
    pub size_class: u64,
    /// Bump for the vault signer PDA, stored so it never has to be searched for again.
    pub vault_signer_bump: u64,
    /// Base token mint.
    pub base_mint: [u8; 32],
    /// Quote token mint.
    pub quote_mint: [u8; 32],
    /// Token account holding all base deposits.
    pub base_vault: [u8; 32],
    /// Token account holding all quote deposits.
    pub quote_vault: [u8; 32],
    /// May change the fee recipient. Not able to touch trader funds.
    pub authority: [u8; 32],
    /// Receives swept fees.
    pub fee_recipient: [u8; 32],
}

// SAFETY: repr(C) over four u64s followed by six 32-byte arrays. Size 224, align 8,
// no padding.
unsafe impl Zeroable for MarketAccountHeader {}
unsafe impl Pod for MarketAccountHeader {}

/// Size of the header in bytes. A multiple of eight, which is what keeps the engine's
/// `Market` aligned when it is cast out of the bytes that follow.
pub const HEADER_LEN: usize = core::mem::size_of::<MarketAccountHeader>();

/// The supported market capacities.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[repr(u64)]
pub enum SizeClass {
    /// 128 orders per side, 32 seats. Roughly 19 KiB.
    Small = 0,
    /// 512 orders per side, 128 seats. Roughly 76 KiB.
    Medium = 1,
    /// 2048 orders per side, 512 seats. Roughly 304 KiB.
    Large = 2,
}

impl SizeClass {
    /// Parses a header's stored discriminant.
    ///
    /// # Errors
    ///
    /// [`ClobError::UnknownSizeClass`] for any other value.
    pub const fn from_u64(value: u64) -> Result<Self, ClobError> {
        match value {
            0 => Ok(Self::Small),
            1 => Ok(Self::Medium),
            2 => Ok(Self::Large),
            _ => Err(ClobError::UnknownSizeClass),
        }
    }

    /// Total account size for this class, header included.
    pub const fn account_len(self) -> usize {
        HEADER_LEN
            + match self {
                Self::Small => Market::<128, 128, 32>::SIZE_IN_BYTES,
                Self::Medium => Market::<512, 512, 128>::SIZE_IN_BYTES,
                Self::Large => Market::<2048, 2048, 512>::SIZE_IN_BYTES,
            }
    }
}

/// Runs `$body` with `$market` bound to a `&mut Market<..>` of the right capacities.
///
/// The market's capacities are const generics, so a runtime [`SizeClass`] has to be
/// turned back into a static type somewhere. Doing it here, once, means every
/// instruction handler is written against a single generic body rather than three
/// copies — and the monomorphised code is what actually runs, so there is no dynamic
/// dispatch on the hot path.
#[macro_export]
macro_rules! dispatch_market {
    ($size_class:expr, $bytes:expr, |$market:ident| $body:block) => {
        match $size_class {
            $crate::state::SizeClass::Small => {
                let $market = $crate::state::cast_market::<128, 128, 32>($bytes)?;
                $body
            }
            $crate::state::SizeClass::Medium => {
                let $market = $crate::state::cast_market::<512, 512, 128>($bytes)?;
                $body
            }
            $crate::state::SizeClass::Large => {
                let $market = $crate::state::cast_market::<2048, 2048, 512>($bytes)?;
                $body
            }
        }
    };
}

/// Casts the trailing bytes of a market account into a live market.
///
/// # Errors
///
/// [`ClobError::MarketAccountTooSmall`] if the slice is short, or
/// [`ClobError::MarketDataUnaligned`] if the cast would violate alignment — which
/// cannot happen for a correctly created account, but is checked rather than assumed
/// because the alternative is a panic inside `bytemuck`.
pub fn cast_market<const BIDS: usize, const ASKS: usize, const SEATS: usize>(
    bytes: &mut [u8],
) -> Result<&mut Market<BIDS, ASKS, SEATS>, ProgramError> {
    let needed = Market::<BIDS, ASKS, SEATS>::SIZE_IN_BYTES;
    if bytes.len() < needed {
        return Err(ClobError::MarketAccountTooSmall.into());
    }
    bytemuck::try_from_bytes_mut(&mut bytes[..needed])
        .map_err(|_| ClobError::MarketDataUnaligned.into())
}

/// Splits a market account's data into its header and its market bytes.
///
/// # Errors
///
/// [`ClobError::MarketAccountTooSmall`], [`ClobError::NotAMarket`] if the discriminator
/// is wrong, or [`ClobError::VersionMismatch`].
pub fn split_initialized(
    data: &mut [u8],
) -> Result<(&mut MarketAccountHeader, &mut [u8]), ProgramError> {
    if data.len() < HEADER_LEN {
        return Err(ClobError::MarketAccountTooSmall.into());
    }
    let (header_bytes, market_bytes) = data.split_at_mut(HEADER_LEN);
    let header: &mut MarketAccountHeader = bytemuck::try_from_bytes_mut(header_bytes)
        .map_err(|_| ProgramError::from(ClobError::MarketDataUnaligned))?;

    if header.discriminator != MARKET_DISCRIMINATOR {
        return Err(ClobError::NotAMarket.into());
    }
    if header.version != MARKET_VERSION {
        return Err(ClobError::VersionMismatch.into());
    }
    Ok((header, market_bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_header_has_no_padding_and_keeps_the_market_aligned() {
        assert_eq!(HEADER_LEN, 224);
        assert_eq!(HEADER_LEN % 8, 0);
        assert_eq!(core::mem::align_of::<MarketAccountHeader>(), 8);
    }

    #[test]
    fn a_zeroed_account_is_not_mistaken_for_a_market() {
        let mut data = std::vec![0u8; HEADER_LEN + 64];
        assert!(split_initialized(&mut data).is_err());
    }

    #[test]
    fn size_classes_round_trip_through_the_header() {
        for (value, class) in [
            (0, SizeClass::Small),
            (1, SizeClass::Medium),
            (2, SizeClass::Large),
        ] {
            assert_eq!(SizeClass::from_u64(value), Ok(class));
        }
        assert_eq!(SizeClass::from_u64(3), Err(ClobError::UnknownSizeClass));
    }

    #[test]
    fn account_sizes_are_what_the_docs_claim() {
        // Rent is the main cost of creating a market, so these numbers are part of the
        // product, not an implementation detail.
        assert_eq!(SizeClass::Small.account_len(), 19_296);
        assert!(SizeClass::Medium.account_len() < 80 * 1024);
        assert!(SizeClass::Large.account_len() < 320 * 1024);
    }
}
