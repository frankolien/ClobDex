//! Removal and the double-black rebalance.
//!
//! One detail differs from CLRS: it relies on a real `NIL` sentinel *node* whose parent
//! pointer can be written during the fixup. There is none here — [`NIL`] is handle `0` —
//! so the fixup carries the parent explicitly alongside the node it rebalances.

use bytemuck::Pod;

use super::node::{BLACK, NIL, RED};
use super::{Handle, RedBlackTree};

impl<K: Pod + Ord, V: Pod, const N: usize> RedBlackTree<K, V, N> {
    /// Removes `key`, returning its value if it was present.
    pub fn remove(&mut self, key: &K) -> Option<V> {
        let handle = self.find(key);
        (handle != NIL).then(|| self.remove_handle(handle))
    }

    /// Removes the entry at `handle`, returning its value.
    ///
    /// This is the form the matching engine wants: it already holds the handle from
    /// walking the book, so re-descending the tree to find the key again is wasted
    /// compute.
    ///
    /// # Panics
    ///
    /// Panics if `handle` is [`NIL`] or out of range.
    pub fn remove_handle(&mut self, handle: Handle) -> V {
        let z = handle;
        let value = self.node(z).value;

        let z_left = self.node(z).left;
        let z_right = self.node(z).right;

        // `x` is whatever moves into the removed node's structural position, and
        // `removed_color` is the colour that actually leaves the tree. `x` may be NIL,
        // which is why its parent is tracked separately instead of read back from it.
        let mut removed_color = self.node(z).color;
        let x;
        let x_parent;

        if z_left == NIL {
            x = z_right;
            x_parent = self.node(z).parent;
            self.transplant(z, z_right);
        } else if z_right == NIL {
            x = z_left;
            x_parent = self.node(z).parent;
            self.transplant(z, z_left);
        } else {
            // Two children: splice in the in-order successor, which by construction has
            // no left child. The successor keeps z's colour, so the colour leaving the
            // tree is the successor's own.
            let y = self.min_from(z_right);
            removed_color = self.node(y).color;
            x = self.node(y).right;

            if self.node(y).parent == z {
                // y is z's direct right child: it stays put and becomes x's parent.
                x_parent = y;
            } else {
                x_parent = self.node(y).parent;
                let y_right = self.node(y).right;
                self.transplant(y, y_right);
                self.node_mut(y).right = z_right;
                self.node_mut(z_right).parent = y;
            }

            self.transplant(z, y);
            self.node_mut(y).left = z_left;
            self.node_mut(z_left).parent = y;
            let z_color = self.node(z).color;
            self.node_mut(y).color = z_color;
        }

        // Removing a red node changes no path's black height.
        if removed_color == BLACK {
            self.delete_fixup(x, x_parent);
        }

        // Freed last: z is fully detached by now, and the fixup never touches it.
        self.free(z);
        self.decrement_len();
        value
    }

    /// Replaces the subtree rooted at `u` with the one rooted at `v`.
    fn transplant(&mut self, u: Handle, v: Handle) {
        let u_parent = self.parent_of(u);
        if u_parent == NIL {
            self.set_root(v);
        } else if u == self.left_of(u_parent) {
            self.node_mut(u_parent).left = v;
        } else {
            self.node_mut(u_parent).right = v;
        }
        if v != NIL {
            self.node_mut(v).parent = u_parent;
        }
    }

    /// CLRS `RB-DELETE-FIXUP`, with the parent passed in rather than read from `x`.
    ///
    /// `x` carries one extra black. Each iteration either pushes that extra black up to
    /// the parent (case 2) or resolves it with at most three rotations (cases 1, 3, 4).
    fn delete_fixup(&mut self, mut x: Handle, mut parent: Handle) {
        while x != self.root() && self.color_of(x) == BLACK {
            if parent == NIL {
                // Non-root with no parent: only reachable if the tree is already
                // corrupt. Bail rather than spin.
                debug_assert!(false, "delete_fixup lost the parent pointer");
                break;
            }

            if x == self.left_of(parent) {
                let mut sibling = self.right_of(parent);
                // x carries an extra black, so the sibling subtree has black height at
                // least one and the sibling cannot be NIL.
                debug_assert!(sibling != NIL, "doubly-black node with no sibling");

                if self.color_of(sibling) == RED {
                    // Case 1: red sibling. Rotate it away to reach a black-sibling case.
                    self.set_color(sibling, BLACK);
                    self.set_color(parent, RED);
                    self.rotate_left(parent);
                    sibling = self.right_of(parent);
                }

                let sibling_left = self.left_of(sibling);
                let sibling_right = self.right_of(sibling);

                if self.color_of(sibling_left) == BLACK && self.color_of(sibling_right) == BLACK {
                    // Case 2: both nephews black. Move the extra black up to the parent.
                    self.set_color(sibling, RED);
                    x = parent;
                    parent = self.parent_of(x);
                } else {
                    if self.color_of(sibling_right) == BLACK {
                        // Case 3: only the near nephew is red. Rotate it into the far
                        // position, reducing to case 4.
                        self.set_color(sibling_left, BLACK);
                        self.set_color(sibling, RED);
                        self.rotate_right(sibling);
                        sibling = self.right_of(parent);
                    }
                    // Case 4: far nephew red. Terminal — rotate the extra black away.
                    let parent_color = self.color_of(parent);
                    self.set_color(sibling, parent_color);
                    self.set_color(parent, BLACK);
                    let sibling_right = self.right_of(sibling);
                    self.set_color(sibling_right, BLACK);
                    self.rotate_left(parent);
                    x = self.root();
                    parent = NIL;
                }
            } else {
                // Mirror image of the branch above.
                let mut sibling = self.left_of(parent);
                debug_assert!(sibling != NIL, "doubly-black node with no sibling");

                if self.color_of(sibling) == RED {
                    self.set_color(sibling, BLACK);
                    self.set_color(parent, RED);
                    self.rotate_right(parent);
                    sibling = self.left_of(parent);
                }

                let sibling_left = self.left_of(sibling);
                let sibling_right = self.right_of(sibling);

                if self.color_of(sibling_left) == BLACK && self.color_of(sibling_right) == BLACK {
                    self.set_color(sibling, RED);
                    x = parent;
                    parent = self.parent_of(x);
                } else {
                    if self.color_of(sibling_left) == BLACK {
                        self.set_color(sibling_right, BLACK);
                        self.set_color(sibling, RED);
                        self.rotate_left(sibling);
                        sibling = self.left_of(parent);
                    }
                    let parent_color = self.color_of(parent);
                    self.set_color(sibling, parent_color);
                    self.set_color(parent, BLACK);
                    let sibling_left = self.left_of(sibling);
                    self.set_color(sibling_left, BLACK);
                    self.rotate_right(parent);
                    x = self.root();
                    parent = NIL;
                }
            }
        }
        // Absorbing the extra black into a red node, or into the root.
        self.set_color(x, BLACK);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type Tree = RedBlackTree<u64, u64, 64>;

    /// Removal in several orders exercises the leaf, one-child, and two-child splice
    /// paths, and both mirror images of the fixup.
    #[test]
    fn every_removal_order_preserves_the_invariants() {
        let keys: std::vec::Vec<u64> = (0..40).collect();
        let ascending: std::vec::Vec<u64> = keys.clone();
        let descending: std::vec::Vec<u64> = keys.iter().rev().copied().collect();
        let evens_then_odds: std::vec::Vec<u64> = keys
            .iter()
            .filter(|k| *k % 2 == 0)
            .chain(keys.iter().filter(|k| *k % 2 == 1))
            .copied()
            .collect();
        let rotated: std::vec::Vec<u64> = keys.iter().skip(13).chain(keys.iter().take(13)).copied().collect();

        for order in [ascending, descending, evens_then_odds, rotated] {
            let mut tree = Tree::new_boxed();
            for key in &keys {
                tree.insert(*key, *key);
            }

            for (removed, key) in order.iter().enumerate() {
                assert_eq!(tree.remove(key), Some(*key));
                assert_eq!(tree.check(), Ok(()), "after removing {key}");
                assert_eq!(tree.len(), keys.len() - removed - 1);
            }

            assert!(tree.is_empty());
            assert_eq!(tree.root(), NIL);
        }
    }

    #[test]
    fn freed_slots_are_reused() {
        let mut tree = Tree::new_boxed();
        for key in 0..64u64 {
            tree.insert(key, key);
        }
        assert_eq!(tree.insert(999, 0), None);

        tree.remove(&30);

        assert!(tree.insert(999, 999).is_some());
        assert_eq!(tree.len(), 64);
        assert_eq!(tree.check(), Ok(()));
    }

    #[test]
    fn removing_by_handle_matches_removing_by_key() {
        let mut tree = Tree::new_boxed();
        for key in 0..20u64 {
            tree.insert(key, key * 2);
        }

        let handle = tree.find(&7);
        assert_ne!(handle, NIL);
        assert_eq!(tree.remove_handle(handle), 14);
        assert_eq!(tree.find(&7), NIL);
        assert_eq!(tree.check(), Ok(()));
    }

    #[test]
    fn removing_an_absent_key_is_a_no_op() {
        let mut tree = Tree::new_boxed();
        tree.insert(1, 1);

        assert_eq!(tree.remove(&2), None);
        assert_eq!(tree.len(), 1);
        assert_eq!(tree.check(), Ok(()));
    }
}
