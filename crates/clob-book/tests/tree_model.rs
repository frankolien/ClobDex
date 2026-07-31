//! Differential test: the arena tree against `BTreeMap`.
//!
//! `BTreeMap` is the oracle. Any divergence in contents, ordering, or length is a bug in
//! the arena tree, and the structural self-check runs after every single operation so a
//! rebalancing fault is caught at the operation that caused it rather than several
//! operations later.

use std::collections::BTreeMap;

use clob_book::{NIL, RedBlackTree};
use proptest::prelude::*;

/// Small enough that random operation sequences actually hit the capacity limit and the
/// free-list reuse path, rather than only ever bumping into virgin slots.
const CAPACITY: usize = 32;

type Tree = RedBlackTree<u64, u64, CAPACITY>;

#[derive(Debug, Clone)]
enum Op {
    Insert(u64, u64),
    Remove(u64),
    Clear,
}

/// Keys are drawn from a small space so collisions, overwrites, and removals of absent
/// keys all occur often.
fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        8 => (0u64..48, any::<u64>()).prop_map(|(k, v)| Op::Insert(k, v)),
        6 => (0u64..48).prop_map(Op::Remove),
        1 => Just(Op::Clear),
    ]
}

/// Applies `op` to both implementations, mirroring the tree's capacity rejection in the
/// model so the two stay comparable.
fn apply(tree: &mut Tree, model: &mut BTreeMap<u64, u64>, op: &Op) -> Result<(), TestCaseError> {
    match *op {
        Op::Insert(key, value) => {
            let was_present = model.contains_key(&key);
            match tree.insert(key, value) {
                Some(_) => {
                    model.insert(key, value);
                }
                None => {
                    // The only legitimate refusal is a new key into a full tree.
                    prop_assert!(!was_present, "refused to overwrite an existing key");
                    prop_assert_eq!(model.len(), CAPACITY, "refused an insert below capacity");
                }
            }
        }
        Op::Remove(key) => {
            prop_assert_eq!(tree.remove(&key), model.remove(&key));
        }
        Op::Clear => {
            tree.clear();
            model.clear();
        }
    }
    Ok(())
}

/// Every observable property of the tree must match the model.
fn assert_agrees(tree: &Tree, model: &BTreeMap<u64, u64>) -> Result<(), TestCaseError> {
    prop_assert_eq!(tree.check(), Ok(()));
    prop_assert_eq!(tree.len(), model.len());
    prop_assert_eq!(tree.is_empty(), model.is_empty());

    let ascending: Vec<(u64, u64)> = tree.iter().map(|e| (e.key, e.value)).collect();
    let expected: Vec<(u64, u64)> = model.iter().map(|(k, v)| (*k, *v)).collect();
    prop_assert_eq!(&ascending, &expected);

    let mut descending: Vec<(u64, u64)> = tree.iter_rev().map(|e| (e.key, e.value)).collect();
    descending.reverse();
    prop_assert_eq!(&descending, &expected);

    match (model.keys().next(), model.keys().next_back()) {
        (Some(&min), Some(&max)) => {
            prop_assert_eq!(tree.node(tree.min_handle()).key(), &min);
            prop_assert_eq!(tree.node(tree.max_handle()).key(), &max);
        }
        _ => {
            prop_assert_eq!(tree.min_handle(), NIL);
            prop_assert_eq!(tree.max_handle(), NIL);
        }
    }

    for (key, value) in model {
        prop_assert_eq!(tree.get(key), Some(value));
    }

    Ok(())
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    /// The core differential property, checked after every individual operation.
    #[test]
    fn matches_btreemap_under_arbitrary_operations(ops in prop::collection::vec(op(), 0..400)) {
        let mut tree = Tree::new_boxed();
        let mut model = BTreeMap::new();

        for op in &ops {
            apply(&mut tree, &mut model, op)?;
            assert_agrees(&tree, &model)?;
        }
    }

    /// Capacity is genuinely reclaimed: after filling and emptying the tree repeatedly,
    /// it still accepts a full complement of new keys. A leak in the free list shows up
    /// here as a premature refusal.
    #[test]
    fn slots_are_never_leaked(rounds in 1usize..6, offset in 0u64..1000) {
        let mut tree = Tree::new_boxed();

        for round in 0..rounds {
            let base = offset + (round as u64) * 1000;
            for i in 0..CAPACITY as u64 {
                prop_assert!(tree.insert(base + i, i).is_some(), "refused at round {}", round);
            }
            prop_assert!(tree.is_full());
            prop_assert_eq!(tree.check(), Ok(()));

            for i in 0..CAPACITY as u64 {
                prop_assert_eq!(tree.remove(&(base + i)), Some(i));
            }
            prop_assert!(tree.is_empty());
            prop_assert_eq!(tree.check(), Ok(()));
        }
    }

    /// Handles stay valid across unrelated mutations, and resolve back to the same key.
    /// The matching engine holds handles while walking the book, so this is load-bearing.
    #[test]
    fn handles_survive_unrelated_mutations(
        pinned in 0u64..20,
        churn in prop::collection::vec(20u64..48, 0..30),
    ) {
        let mut tree = Tree::new_boxed();
        tree.insert(pinned, 12345);
        let handle = tree.find(&pinned);
        prop_assert_ne!(handle, NIL);

        for key in &churn {
            tree.insert(*key, *key);
        }
        for key in &churn {
            tree.remove(key);
        }

        prop_assert_eq!(tree.find(&pinned), handle, "handle moved");
        prop_assert_eq!(tree.get_by_handle(handle), Some(&12345));
        prop_assert_eq!(tree.check(), Ok(()));
    }
}
