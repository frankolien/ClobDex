//! Arena slots and handles.

use core::mem::size_of;

use bytemuck::{Pod, Zeroable};

/// The sentinel handle meaning "no node".
///
/// Handles are 1-based so that `0` can mean nothing *and* an all-zero arena can be a
/// valid empty tree — which is what a freshly allocated Solana account is.
pub const NIL: Handle = 0;

/// Red. Zero, so a zeroed slot reads as the colour a freshly inserted node gets.
pub(crate) const RED: u32 = 0;
/// Black.
pub(crate) const BLACK: u32 = 1;

/// A reference to an arena slot. A live node with handle `h` occupies slot `h - 1`.
pub type Handle = u32;

/// One arena slot: red-black links, then the key and value.
///
/// The four `u32` links come first so the key lands at offset 16, correctly aligned for
/// any `K` with alignment up to 16. [`Node::ASSERT_NO_PADDING`] rejects at compile time
/// any `K`/`V` pair that would introduce padding, since padding bytes are exactly what
/// makes a type unsound to treat as `Pod`.
#[repr(C)]
pub struct Node<K, V> {
    pub(crate) left: Handle,
    pub(crate) right: Handle,
    pub(crate) parent: Handle,
    pub(crate) color: u32,
    pub(crate) key: K,
    pub(crate) value: V,
}

impl<K: Copy, V: Copy> Clone for Node<K, V> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<K: Copy, V: Copy> Copy for Node<K, V> {}

// SAFETY: all fields are `Pod`, and `ASSERT_NO_PADDING` — forced at every tree
// construction site — proves there is no padding between or after them.
unsafe impl<K: Pod, V: Pod> Zeroable for Node<K, V> {}
unsafe impl<K: Pod, V: Pod> Pod for Node<K, V> {}

/// A by-value snapshot of a slot, as yielded by lookups and iterators.
///
/// The handle is included so a caller walking the book can mutate or remove the entry
/// without paying for a second lookup — which is what the matching engine does as it
/// consumes liquidity.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Entry<K, V> {
    /// Arena handle, valid until the next mutation of the tree.
    pub handle: Handle,
    /// The entry's key.
    pub key: K,
    /// The entry's value.
    pub value: V,
}

impl<K: Pod, V: Pod> Node<K, V> {
    /// Fails compilation if `K`/`V` would make `Node` contain padding, which would make
    /// the `Pod` impl above unsound.
    pub(crate) const ASSERT_NO_PADDING: () = assert!(
        size_of::<Self>() == 16 + size_of::<K>() + size_of::<V>(),
        "Node<K, V> has padding: pick key/value types whose sizes and alignments pack \
         cleanly after four u32 links"
    );

    /// This slot's key.
    #[inline(always)]
    pub const fn key(&self) -> &K {
        &self.key
    }

    /// This slot's value.
    #[inline(always)]
    pub const fn value(&self) -> &V {
        &self.value
    }
}
