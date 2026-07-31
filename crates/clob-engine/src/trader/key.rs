//! Trader identity.

use bytemuck::{Pod, Zeroable};

/// An opaque 32-byte trader identity.
///
/// On-chain this is a Solana `Pubkey`, but this crate carries it as raw bytes so the
/// engine stays testable without a validator and free of any Solana dependency.
/// Ordering is lexicographic over the bytes, which is all the seat table needs.
#[repr(transparent)]
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TraderKey(pub [u8; 32]);

// SAFETY: repr(transparent) over [u8; 32]. No padding, every bit pattern valid.
unsafe impl Zeroable for TraderKey {}
unsafe impl Pod for TraderKey {}

impl TraderKey {
    /// Wraps raw bytes.
    #[inline(always)]
    pub const fn new(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// The underlying bytes.
    #[inline(always)]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl From<[u8; 32]> for TraderKey {
    fn from(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }
}
