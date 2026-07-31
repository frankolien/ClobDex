//! Rotations — the only operations that change tree shape.
//!
//! Written out in full rather than parameterised by direction: a generic version would
//! halve the line count and double the time it takes to verify the pointer updates.

use bytemuck::Pod;

use super::node::NIL;
use super::{Handle, RedBlackTree};

impl<K: Pod, V: Pod, const N: usize> RedBlackTree<K, V, N> {
    /// Rotates `x` down-left, promoting its right child.
    ///
    /// # Panics
    ///
    /// Debug builds assert that `x` has a right child; release builds would corrupt the
    /// tree, so this is a caller obligation, not a runtime check.
    pub(crate) fn rotate_left(&mut self, x: Handle) {
        let y = self.node(x).right;
        debug_assert!(y != NIL, "rotate_left requires a right child");

        // y's left subtree becomes x's right subtree.
        let y_left = self.node(y).left;
        self.node_mut(x).right = y_left;
        if y_left != NIL {
            self.node_mut(y_left).parent = x;
        }

        // y takes x's place under x's parent.
        let x_parent = self.node(x).parent;
        self.node_mut(y).parent = x_parent;
        if x_parent == NIL {
            self.set_root(y);
        } else if self.node(x_parent).left == x {
            self.node_mut(x_parent).left = y;
        } else {
            self.node_mut(x_parent).right = y;
        }

        // x becomes y's left child.
        self.node_mut(y).left = x;
        self.node_mut(x).parent = y;
    }

    /// Rotates `x` down-right, promoting its left child. Mirror of
    /// [`RedBlackTree::rotate_left`].
    ///
    /// # Panics
    ///
    /// Debug builds assert that `x` has a left child.
    pub(crate) fn rotate_right(&mut self, x: Handle) {
        let y = self.node(x).left;
        debug_assert!(y != NIL, "rotate_right requires a left child");

        let y_right = self.node(y).right;
        self.node_mut(x).left = y_right;
        if y_right != NIL {
            self.node_mut(y_right).parent = x;
        }

        let x_parent = self.node(x).parent;
        self.node_mut(y).parent = x_parent;
        if x_parent == NIL {
            self.set_root(y);
        } else if self.node(x_parent).right == x {
            self.node_mut(x_parent).right = y;
        } else {
            self.node_mut(x_parent).left = y;
        }

        self.node_mut(y).right = x;
        self.node_mut(x).parent = y;
    }
}
