//! Structural self-check.
//!
//! Exists for fuzzing: a coverage-guided fuzzer needs a cheap, allocation-free predicate
//! it can assert after every operation, and given one it finds rebalancing bugs on its
//! own. That is why violations are a `Copy` enum rather than a message.

use bytemuck::Pod;

use super::node::{BLACK, NIL, RED};
use super::{Handle, RedBlackTree};

/// A structural invariant that [`RedBlackTree::check`] found violated.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Invariant {
    /// The root is not black.
    RootNotBlack,
    /// The root has a non-[`NIL`] parent.
    RootParentNotNil,
    /// A red node has a red child.
    RedRedViolation,
    /// Two root-to-leaf paths have different black heights.
    BlackHeightMismatch,
    /// In-order traversal is not strictly ascending by key.
    BstOrderViolation,
    /// A child's parent pointer does not point back at its parent, or the links form a
    /// cycle.
    ParentPointerMismatch,
    /// `len` disagrees with the number of reachable nodes.
    LenMismatch,
    /// The free list is cyclic, or free slots plus live nodes do not account for every
    /// slot the bump pointer handed out.
    FreeListCorrupt,
}

impl core::fmt::Display for Invariant {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        core::fmt::Debug::fmt(self, f)
    }
}

#[cfg(feature = "std")]
impl std::error::Error for Invariant {}

impl<K: Pod + Ord, V: Pod, const N: usize> RedBlackTree<K, V, N> {
    /// Checks every red-black and arena invariant.
    ///
    /// O(n) and allocation-free. Intended for tests and fuzz harnesses; far too slow
    /// for a production instruction, though cheap enough for a debug-build assertion.
    ///
    /// # Errors
    ///
    /// The first [`Invariant`] found violated.
    pub fn check(&self) -> Result<(), Invariant> {
        self.check_root()?;
        let reachable = self.check_ordering_and_parents()?;

        if reachable != self.len() {
            return Err(Invariant::LenMismatch);
        }

        self.check_black_height(self.root())?;
        self.check_free_list()
    }

    fn check_root(&self) -> Result<(), Invariant> {
        if self.color_of(self.root()) != BLACK {
            return Err(Invariant::RootNotBlack);
        }
        if self.parent_of(self.root()) != NIL {
            return Err(Invariant::RootParentNotNil);
        }
        Ok(())
    }

    /// One in-order walk covering key ordering, parent pointers, and node count.
    ///
    /// Returns the number of reachable nodes.
    fn check_ordering_and_parents(&self) -> Result<usize, Invariant> {
        let mut count = 0usize;
        let mut previous = NIL;
        let mut cursor = self.min_handle();

        while cursor != NIL {
            if previous != NIL && self.node(previous).key >= self.node(cursor).key {
                return Err(Invariant::BstOrderViolation);
            }

            let left = self.node(cursor).left;
            let right = self.node(cursor).right;
            if (left != NIL && self.node(left).parent != cursor)
                || (right != NIL && self.node(right).parent != cursor)
            {
                return Err(Invariant::ParentPointerMismatch);
            }

            count += 1;
            if count > N {
                // A link cycle would otherwise spin here forever.
                return Err(Invariant::ParentPointerMismatch);
            }

            previous = cursor;
            cursor = self.successor(cursor);
        }

        Ok(count)
    }

    /// Returns the black height of the subtree at `handle`, checking the red-red and
    /// equal-black-height properties on the way.
    fn check_black_height(&self, handle: Handle) -> Result<u32, Invariant> {
        if handle == NIL {
            return Ok(1);
        }

        let left = self.node(handle).left;
        let right = self.node(handle).right;

        if self.color_of(handle) == RED
            && (self.color_of(left) == RED || self.color_of(right) == RED)
        {
            return Err(Invariant::RedRedViolation);
        }

        let left_height = self.check_black_height(left)?;
        let right_height = self.check_black_height(right)?;
        if left_height != right_height {
            return Err(Invariant::BlackHeightMismatch);
        }

        Ok(left_height + u32::from(self.color_of(handle) == BLACK))
    }

    /// Free slots plus live nodes must exactly account for everything the bump pointer
    /// handed out — a leak or a double-free shows up here as a mismatch.
    fn check_free_list(&self) -> Result<(), Invariant> {
        let mut free = 0usize;
        let mut cursor = self.free_head();

        while cursor != NIL {
            free += 1;
            if free > N {
                return Err(Invariant::FreeListCorrupt);
            }
            cursor = self.node(cursor).left;
        }

        if free + self.len() != self.allocated_slots() {
            return Err(Invariant::FreeListCorrupt);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type Tree = RedBlackTree<u64, u64, 16>;

    #[test]
    fn a_healthy_tree_passes() {
        let mut tree = Tree::new_boxed();
        for key in 0..16u64 {
            tree.insert(key, key);
        }
        assert_eq!(tree.check(), Ok(()));
    }

    #[test]
    fn detects_a_corrupted_root_colour() {
        let mut tree = Tree::new_boxed();
        tree.insert(1, 1);

        let root = tree.root();
        tree.node_mut(root).color = RED;

        assert_eq!(tree.check(), Err(Invariant::RootNotBlack));
    }

    #[test]
    fn detects_a_broken_ordering() {
        let mut tree = Tree::new_boxed();
        for key in 0..8u64 {
            tree.insert(key, key);
        }

        // Rewrite a key in place, breaking the BST property without touching links.
        let handle = tree.find(&0);
        tree.node_mut(handle).key = 999;

        assert_eq!(tree.check(), Err(Invariant::BstOrderViolation));
    }

    #[test]
    fn detects_a_stale_length() {
        let mut tree = Tree::new_boxed();
        tree.insert(1, 1);
        tree.increment_len();

        assert_eq!(tree.check(), Err(Invariant::LenMismatch));
    }
}
