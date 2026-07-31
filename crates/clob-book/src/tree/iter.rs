//! Ordered iteration.
//!
//! Walks via successor/predecessor rather than an explicit stack, which would need
//! either an allocator or a fixed depth bound. Amortised O(1) per step.

use bytemuck::Pod;

use super::node::NIL;
use super::{Entry, Handle, RedBlackTree};

impl<K: Pod + Ord, V: Pod, const N: usize> RedBlackTree<K, V, N> {
    /// Ascending iteration by key. On an ask book this walks from the best offer down.
    #[inline]
    pub fn iter(&self) -> Iter<'_, K, V, N> {
        Iter {
            tree: self,
            cursor: self.min_handle(),
            remaining: self.len(),
        }
    }

    /// Descending iteration by key. On a bid book this walks from the best bid down.
    #[inline]
    pub fn iter_rev(&self) -> IterRev<'_, K, V, N> {
        IterRev {
            tree: self,
            cursor: self.max_handle(),
            remaining: self.len(),
        }
    }
}

/// Ascending iterator. See [`RedBlackTree::iter`].
pub struct Iter<'a, K, V, const N: usize> {
    tree: &'a RedBlackTree<K, V, N>,
    cursor: Handle,
    remaining: usize,
}

impl<K: Pod + Ord, V: Pod, const N: usize> Iterator for Iter<'_, K, V, N> {
    type Item = Entry<K, V>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.cursor == NIL {
            return None;
        }
        let handle = self.cursor;
        self.cursor = self.tree.successor(handle);
        self.remaining -= 1;
        self.tree.entry_at(handle)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.remaining, Some(self.remaining))
    }
}

impl<K: Pod + Ord, V: Pod, const N: usize> ExactSizeIterator for Iter<'_, K, V, N> {}

/// Descending iterator. See [`RedBlackTree::iter_rev`].
pub struct IterRev<'a, K, V, const N: usize> {
    tree: &'a RedBlackTree<K, V, N>,
    cursor: Handle,
    remaining: usize,
}

impl<K: Pod + Ord, V: Pod, const N: usize> Iterator for IterRev<'_, K, V, N> {
    type Item = Entry<K, V>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.cursor == NIL {
            return None;
        }
        let handle = self.cursor;
        self.cursor = self.tree.predecessor(handle);
        self.remaining -= 1;
        self.tree.entry_at(handle)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.remaining, Some(self.remaining))
    }
}

impl<K: Pod + Ord, V: Pod, const N: usize> ExactSizeIterator for IterRev<'_, K, V, N> {}

#[cfg(test)]
mod tests {
    use super::*;

    type Tree = RedBlackTree<u64, u64, 64>;

    fn seeded() -> std::boxed::Box<Tree> {
        let mut tree = Tree::new_boxed();
        for key in [8u64, 3, 15, 1, 6, 9, 20, 4] {
            tree.insert(key, key * 10);
        }
        tree
    }

    #[test]
    fn ascending_and_descending_are_mirror_images() {
        let tree = seeded();
        let ascending: std::vec::Vec<u64> = tree.iter().map(|e| e.key).collect();
        let mut descending: std::vec::Vec<u64> = tree.iter_rev().map(|e| e.key).collect();
        descending.reverse();

        assert_eq!(ascending, std::vec![1, 3, 4, 6, 8, 9, 15, 20]);
        assert_eq!(ascending, descending);
    }

    #[test]
    fn length_is_known_up_front() {
        let tree = seeded();
        assert_eq!(tree.iter().len(), 8);
        assert_eq!(tree.iter_rev().len(), 8);
    }

    #[test]
    fn handles_resolve_back_to_their_entries() {
        let tree = seeded();
        for entry in tree.iter() {
            assert_eq!(tree.get_by_handle(entry.handle), Some(&entry.value));
            assert_eq!(tree.find(&entry.key), entry.handle);
        }
    }

    #[test]
    fn an_empty_tree_yields_nothing() {
        let tree = Tree::new_boxed();
        assert_eq!(tree.iter().count(), 0);
        assert_eq!(tree.iter_rev().count(), 0);
    }
}
