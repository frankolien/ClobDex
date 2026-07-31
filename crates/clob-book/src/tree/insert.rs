//! Insertion and the red-red rebalance.

use core::cmp::Ordering;

use bytemuck::Pod;

use super::node::{BLACK, NIL, RED};
use super::{Handle, RedBlackTree};

impl<K: Pod + Ord, V: Pod, const N: usize> RedBlackTree<K, V, N> {
    /// Inserts `key`, or overwrites the value if `key` is already present.
    ///
    /// Returns the entry's handle, or `None` if the tree is at capacity and `key` is
    /// new. Overwriting an existing key succeeds even at capacity, which is what lets a
    /// full book still take partial fills.
    pub fn insert(&mut self, key: K, value: V) -> Option<Handle> {
        let mut parent = NIL;
        let mut current = self.root();
        let mut branch = Ordering::Equal;

        while current != NIL {
            let node = self.node(current);
            parent = current;
            branch = key.cmp(&node.key);
            match branch {
                Ordering::Less => current = node.left,
                Ordering::Greater => current = node.right,
                Ordering::Equal => {
                    self.node_mut(current).value = value;
                    return Some(current);
                }
            }
        }

        // Allocate before mutating any links, so a full tree leaves the tree untouched.
        let handle = self.alloc()?;
        {
            let node = self.node_mut(handle);
            node.left = NIL;
            node.right = NIL;
            node.parent = parent;
            node.color = RED;
            node.key = key;
            node.value = value;
        }

        if parent == NIL {
            self.set_root(handle);
        } else if branch == Ordering::Less {
            self.node_mut(parent).left = handle;
        } else {
            self.node_mut(parent).right = handle;
        }

        self.increment_len();
        self.insert_fixup(handle);
        Some(handle)
    }

    /// CLRS `RB-INSERT-FIXUP`.
    ///
    /// `z` is red and may violate the red-red property. Each iteration either recolours
    /// and moves the violation two levels up (case 1), or rotates and terminates
    /// (cases 2 and 3) — so this runs in O(log n) with at most two rotations.
    fn insert_fixup(&mut self, mut z: Handle) {
        while self.color_of(self.parent_of(z)) == RED {
            let mut parent = self.parent_of(z);
            // The parent is red, so it is not the root, so the grandparent exists.
            let grandparent = self.parent_of(parent);

            if parent == self.left_of(grandparent) {
                let uncle = self.right_of(grandparent);
                if self.color_of(uncle) == RED {
                    // Case 1: red uncle. Recolour and push the violation upward.
                    self.set_color(parent, BLACK);
                    self.set_color(uncle, BLACK);
                    self.set_color(grandparent, RED);
                    z = grandparent;
                } else {
                    if z == self.right_of(parent) {
                        // Case 2: zig-zag. Straighten it into a zig-zig.
                        z = parent;
                        self.rotate_left(z);
                        parent = self.parent_of(z);
                    }
                    // Case 3: zig-zig. Recolour, rotate, done.
                    self.set_color(parent, BLACK);
                    let grandparent = self.parent_of(parent);
                    self.set_color(grandparent, RED);
                    self.rotate_right(grandparent);
                }
            } else {
                // Mirror image of the branch above.
                let uncle = self.left_of(grandparent);
                if self.color_of(uncle) == RED {
                    self.set_color(parent, BLACK);
                    self.set_color(uncle, BLACK);
                    self.set_color(grandparent, RED);
                    z = grandparent;
                } else {
                    if z == self.left_of(parent) {
                        z = parent;
                        self.rotate_right(z);
                        parent = self.parent_of(z);
                    }
                    self.set_color(parent, BLACK);
                    let grandparent = self.parent_of(parent);
                    self.set_color(grandparent, RED);
                    self.rotate_left(grandparent);
                }
            }
        }
        // Case 1 can leave the root red; this is the only place the black height grows.
        let root = self.root();
        self.set_color(root, BLACK);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type Tree = RedBlackTree<u64, u64, 64>;

    fn height(tree: &Tree, handle: Handle) -> u32 {
        if handle == NIL {
            return 0;
        }
        1 + height(tree, tree.node(handle).left).max(height(tree, tree.node(handle).right))
    }

    #[test]
    fn ascending_insertion_stays_balanced() {
        // The pathological case for an unbalanced BST.
        let mut tree = Tree::new_boxed();
        for key in 0..64u64 {
            assert!(tree.insert(key, key).is_some());
            assert_eq!(tree.check(), Ok(()));
        }

        assert_eq!(tree.len(), 64);
        // A balanced tree of 64 nodes has height at most 2*log2(65) ~= 12.
        assert!(height(&tree, tree.root()) <= 12);
    }

    #[test]
    fn duplicate_keys_overwrite_without_growing() {
        let mut tree = Tree::new_boxed();
        tree.insert(7, 1);
        tree.insert(7, 2);

        assert_eq!(tree.len(), 1);
        assert_eq!(tree.get(&7), Some(&2));
        assert_eq!(tree.check(), Ok(()));
    }

    #[test]
    fn a_full_tree_rejects_new_keys_but_still_overwrites() {
        let mut tree = Tree::new_boxed();
        for key in 0..64u64 {
            tree.insert(key, key);
        }

        assert!(tree.is_full());
        assert_eq!(tree.insert(999, 999), None);
        assert!(tree.insert(0, 111).is_some());
        assert_eq!(tree.get(&0), Some(&111));
        assert_eq!(tree.len(), 64);
    }
}
