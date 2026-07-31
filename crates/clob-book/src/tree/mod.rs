//! A fixed-capacity red-black tree over a `Pod` node arena.
//!
//! This lives inside a Solana account, which rules out any allocating map:
//!
//! - **No allocator.** One `#[repr(C)]` value with an inline node array, so a market
//!   account is a single cast away from being a live tree.
//! - **Zeroed memory is already valid.** [`NIL`] is handle `0` and handles are 1-based,
//!   so an all-zero arena is an empty tree at full capacity — no init pass needed.
//! - **Bounded worst case.** Capacity is a const generic, so rent and maximum compute
//!   cost are known at market creation.
//!
//! Allocation is a bump pointer plus a free list threaded through the `left` field of
//! freed slots. Freed slots keep their old key and value bytes; that is sound, since
//! reachability is defined by tree structure, but it does mean a raw dump of a market
//! account can show cancelled orders until their slots are reused.
//!
//! `insert`, `remove`, and `find` are O(log n) — at capacity 4096, height is at most 24.
//! [`min_handle`](RedBlackTree::min_handle) and [`max_handle`](RedBlackTree::max_handle)
//! (best ask and best bid) are descents rather than cached handles; caching them would
//! save a dozen loads per lookup at the cost of something that can desynchronise during
//! a rebalance. Measure before adding it.

mod invariants;
mod node;
mod rotate;

use core::mem::size_of;

use bytemuck::{Pod, Zeroable};

pub use invariants::Invariant;
pub use node::{Entry, Handle, NIL, Node};

use node::BLACK;

/// A fixed-capacity red-black tree stored entirely inline, castable straight out of
/// account bytes. `N` is the capacity in nodes.
///
/// See the [module docs](self) for the design rationale.
#[repr(C)]
pub struct RedBlackTree<K, V, const N: usize> {
    root: Handle,
    free_head: Handle,
    /// Slots ever handed out by the bump pointer. Slots `bump..N` are untouched.
    bump: u32,
    len: u32,
    nodes: [Node<K, V>; N],
}

impl<K: Pod, V: Pod, const N: usize> Clone for RedBlackTree<K, V, N> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<K: Pod, V: Pod, const N: usize> Copy for RedBlackTree<K, V, N> {}

// SAFETY: `repr(C)` over four `u32`s followed by an array of `Pod` nodes. The header is
// 16 bytes and `Node`'s alignment is at most that of `K`/`V`, so the array starts
// aligned with no padding before it; `ASSERT_NO_PADDING` covers the rest.
unsafe impl<K: Pod, V: Pod, const N: usize> Zeroable for RedBlackTree<K, V, N> {}
unsafe impl<K: Pod, V: Pod, const N: usize> Pod for RedBlackTree<K, V, N> {}

impl<K: Pod, V: Pod, const N: usize> Default for RedBlackTree<K, V, N> {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------------
// Construction and metadata
// ---------------------------------------------------------------------------------

impl<K: Pod, V: Pod, const N: usize> RedBlackTree<K, V, N> {
    /// Fails compilation if the tree would contain padding between header and arena.
    const ASSERT_NO_PADDING: () = assert!(
        size_of::<Self>() == 16 + N * size_of::<Node<K, V>>(),
        "RedBlackTree has padding between its header and node arena"
    );

    /// Account space this tree needs, in bytes.
    pub const SIZE_IN_BYTES: usize = size_of::<Self>();

    /// An empty tree.
    ///
    /// This is just `zeroed`, which is why on-chain the equivalent operation is
    /// "allocate a zeroed account" with no initialization instruction.
    ///
    /// The returned value is `SIZE_IN_BYTES` on the stack; for realistic capacities use
    /// [`RedBlackTree::new_boxed`] off-chain, or cast from account bytes on-chain.
    #[inline]
    pub fn new() -> Self {
        Self::assert_layout();
        Zeroable::zeroed()
    }

    /// An empty tree on the heap, avoiding the large stack temporary
    /// [`RedBlackTree::new`] creates.
    #[cfg(feature = "std")]
    #[inline]
    pub fn new_boxed() -> std::boxed::Box<Self> {
        Self::assert_layout();
        bytemuck::zeroed_box()
    }

    /// Forces evaluation of the layout assertions, which are otherwise inert.
    #[inline(always)]
    fn assert_layout() {
        let () = Self::ASSERT_NO_PADDING;
        let () = Node::<K, V>::ASSERT_NO_PADDING;
    }

    /// Maximum entries this tree can hold.
    #[inline(always)]
    pub const fn capacity(&self) -> usize {
        N
    }

    /// Entries currently held.
    #[inline(always)]
    pub const fn len(&self) -> usize {
        self.len as usize
    }

    /// Whether the tree is empty.
    #[inline(always)]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Whether the next insert of a *new* key will fail.
    #[inline(always)]
    pub const fn is_full(&self) -> bool {
        self.len as usize >= N
    }

    /// The root handle, or [`NIL`] if empty.
    #[inline(always)]
    pub const fn root(&self) -> Handle {
        self.root
    }

    /// Head of the free list, or [`NIL`] if no slot has been recycled.
    #[inline(always)]
    pub(crate) const fn free_head(&self) -> Handle {
        self.free_head
    }

    /// Slots ever handed out by the bump pointer, live or recycled.
    #[inline(always)]
    pub(crate) const fn allocated_slots(&self) -> usize {
        self.bump as usize
    }

    /// Empties the tree in O(1) by resetting the header. Arena bytes are left stale;
    /// see the [module docs](self#allocation).
    pub fn clear(&mut self) {
        self.root = NIL;
        self.free_head = NIL;
        self.bump = 0;
        self.len = 0;
    }
}

// ---------------------------------------------------------------------------------
// Arena access
// ---------------------------------------------------------------------------------

impl<K: Pod, V: Pod, const N: usize> RedBlackTree<K, V, N> {
    /// Borrows a slot.
    ///
    /// # Panics
    ///
    /// Panics if `handle` is [`NIL`] or out of range. Hot-path callers are expected to
    /// have already established that the handle is live.
    #[inline(always)]
    pub fn node(&self, handle: Handle) -> &Node<K, V> {
        &self.nodes[handle as usize - 1]
    }

    /// Mutably borrows a slot.
    ///
    /// # Panics
    ///
    /// Panics if `handle` is [`NIL`] or out of range.
    #[inline(always)]
    pub fn node_mut(&mut self, handle: Handle) -> &mut Node<K, V> {
        &mut self.nodes[handle as usize - 1]
    }

    /// The value at `handle`, or `None` for [`NIL`].
    #[inline(always)]
    pub fn get_by_handle(&self, handle: Handle) -> Option<&V> {
        (handle != NIL).then(|| &self.node(handle).value)
    }

    /// Mutable access to the value at `handle`, or `None` for [`NIL`].
    ///
    /// Safe to expose because only `K` participates in the ordering, so no caller can
    /// invalidate the tree through this. That is precisely why
    /// [`RestingOrder`](crate::RestingOrder) carries no price — a partial fill must be
    /// able to shrink an order without moving its node.
    #[inline(always)]
    pub fn get_mut_by_handle(&mut self, handle: Handle) -> Option<&mut V> {
        (handle != NIL).then(|| &mut self.node_mut(handle).value)
    }

    #[inline(always)]
    pub(crate) fn left_of(&self, handle: Handle) -> Handle {
        if handle == NIL { NIL } else { self.node(handle).left }
    }

    #[inline(always)]
    pub(crate) fn right_of(&self, handle: Handle) -> Handle {
        if handle == NIL { NIL } else { self.node(handle).right }
    }

    #[inline(always)]
    pub(crate) fn parent_of(&self, handle: Handle) -> Handle {
        if handle == NIL { NIL } else { self.node(handle).parent }
    }

    /// The colour of `handle`, treating [`NIL`] as black — the sentinel convention that
    /// lets the fixup routines avoid special-casing missing children.
    #[inline(always)]
    pub(crate) fn color_of(&self, handle: Handle) -> u32 {
        if handle == NIL { BLACK } else { self.node(handle).color }
    }

    #[inline(always)]
    pub(crate) fn set_color(&mut self, handle: Handle, color: u32) {
        if handle != NIL {
            self.node_mut(handle).color = color;
        }
    }

    #[inline(always)]
    pub(crate) fn set_root(&mut self, handle: Handle) {
        self.root = handle;
    }

    #[inline(always)]
    pub(crate) fn increment_len(&mut self) {
        self.len += 1;
    }

    #[inline(always)]
    pub(crate) fn decrement_len(&mut self) {
        self.len -= 1;
    }

    /// Takes a slot from the free list, or bumps into virgin capacity.
    pub(crate) fn alloc(&mut self) -> Option<Handle> {
        if self.free_head != NIL {
            let handle = self.free_head;
            self.free_head = self.node(handle).left;
            Some(handle)
        } else if (self.bump as usize) < N {
            self.bump += 1;
            Some(self.bump)
        } else {
            None
        }
    }

    /// Returns a slot to the free list. Key and value bytes are left as they were; see
    /// the [module docs](self#allocation).
    pub(crate) fn free(&mut self, handle: Handle) {
        let head = self.free_head;
        let node = self.node_mut(handle);
        node.left = head;
        node.right = NIL;
        node.parent = NIL;
        self.free_head = handle;
    }
}

// ---------------------------------------------------------------------------------
// Lookup and navigation
// ---------------------------------------------------------------------------------

impl<K: Pod + Ord, V: Pod, const N: usize> RedBlackTree<K, V, N> {
    /// The handle of `key`, or [`NIL`] if absent.
    pub fn find(&self, key: &K) -> Handle {
        let mut current = self.root;
        while current != NIL {
            let node = self.node(current);
            match key.cmp(&node.key) {
                core::cmp::Ordering::Less => current = node.left,
                core::cmp::Ordering::Greater => current = node.right,
                core::cmp::Ordering::Equal => return current,
            }
        }
        NIL
    }

    /// The value for `key`, if present.
    #[inline]
    pub fn get(&self, key: &K) -> Option<&V> {
        self.get_by_handle(self.find(key))
    }

    /// Mutable access to the value for `key`, if present.
    #[inline]
    pub fn get_mut(&mut self, key: &K) -> Option<&mut V> {
        let handle = self.find(key);
        self.get_mut_by_handle(handle)
    }

    /// Whether `key` is present.
    #[inline]
    pub fn contains_key(&self, key: &K) -> bool {
        self.find(key) != NIL
    }

    /// Handle of the smallest key, or [`NIL`] if empty. On an ask book, the best offer.
    #[inline]
    pub fn min_handle(&self) -> Handle {
        self.min_from(self.root)
    }

    /// Handle of the largest key, or [`NIL`] if empty. On a bid book, the best bid.
    #[inline]
    pub fn max_handle(&self) -> Handle {
        self.max_from(self.root)
    }

    pub(crate) fn min_from(&self, mut handle: Handle) -> Handle {
        if handle == NIL {
            return NIL;
        }
        while self.node(handle).left != NIL {
            handle = self.node(handle).left;
        }
        handle
    }

    pub(crate) fn max_from(&self, mut handle: Handle) -> Handle {
        if handle == NIL {
            return NIL;
        }
        while self.node(handle).right != NIL {
            handle = self.node(handle).right;
        }
        handle
    }

    /// The next-largest key after `handle`, or [`NIL`] at the maximum.
    pub fn successor(&self, handle: Handle) -> Handle {
        if handle == NIL {
            return NIL;
        }
        let right = self.node(handle).right;
        if right != NIL {
            return self.min_from(right);
        }
        // No right subtree: climb until we come up from a left child.
        let mut child = handle;
        let mut parent = self.node(handle).parent;
        while parent != NIL && child == self.node(parent).right {
            child = parent;
            parent = self.node(parent).parent;
        }
        parent
    }

    /// The next-smallest key before `handle`, or [`NIL`] at the minimum.
    pub fn predecessor(&self, handle: Handle) -> Handle {
        if handle == NIL {
            return NIL;
        }
        let left = self.node(handle).left;
        if left != NIL {
            return self.max_from(left);
        }
        let mut child = handle;
        let mut parent = self.node(handle).parent;
        while parent != NIL && child == self.node(parent).left {
            child = parent;
            parent = self.node(parent).parent;
        }
        parent
    }

    /// The smallest entry, if any.
    #[inline]
    pub fn first(&self) -> Option<Entry<K, V>> {
        self.entry_at(self.min_handle())
    }

    /// The largest entry, if any.
    #[inline]
    pub fn last(&self) -> Option<Entry<K, V>> {
        self.entry_at(self.max_handle())
    }

    #[inline]
    pub(crate) fn entry_at(&self, handle: Handle) -> Option<Entry<K, V>> {
        (handle != NIL).then(|| {
            let node = self.node(handle);
            Entry {
                handle,
                key: node.key,
                value: node.value,
            }
        })
    }
}

