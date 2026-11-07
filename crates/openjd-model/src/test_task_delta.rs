// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// Copyright by contributors to this project.
// SPDX-License-Identifier: (Apache-2.0 OR MIT)

//! Tests for `job::task_delta`, checked against brute-force enumeration.

use std::collections::BTreeSet;

use crate::job::task_delta::{task_delta, ProductDelta, TaskBox, TaskDelta};
use crate::job::{ResolvedChunks, StepParameterSpace, TaskParamRange, TaskParameter};
use crate::step_param_space::StepParameterSpaceIterator;
use crate::template::RangeConstraint;
use crate::types::TaskParameterSet;
use openjd_expr::RangeExpr;

fn space(params: Vec<(&str, TaskParameter)>, combination: Option<&str>) -> StepParameterSpace {
    StepParameterSpace {
        task_parameter_definitions: params
            .into_iter()
            .map(|(n, p)| (n.to_string(), p))
            .collect(),
        combination: combination.map(str::to_string),
    }
}

fn ints(expr: &str) -> TaskParameter {
    TaskParameter::Int {
        range: TaskParamRange::RangeExpr(expr.parse::<RangeExpr>().unwrap()),
        chunks: None,
    }
}

fn strs(v: &[&str]) -> TaskParameter {
    TaskParameter::String {
        range: v.iter().map(|s| (*s).to_string()).collect(),
    }
}

fn key(t: &TaskParameterSet) -> String {
    let mut kv: Vec<String> = t
        .iter()
        .map(|(k, v)| format!("{k}={}", serde_json::to_string(&v.value).unwrap()))
        .collect();
    kv.sort();
    kv.join(",")
}

fn all(s: &StepParameterSpace) -> Vec<TaskParameterSet> {
    StepParameterSpaceIterator::new(s).unwrap().collect()
}

fn keys_of(boxes: &[&TaskBox]) -> (BTreeSet<String>, usize) {
    let mut set = BTreeSet::new();
    let mut n = 0;
    for b in boxes {
        for t in b.tasks().unwrap() {
            set.insert(key(&t));
            n += 1;
        }
        assert_eq!(
            b.len(),
            b.tasks().unwrap().count(),
            "len() matches iteration"
        );
    }
    (set, n)
}

/// Checks the decomposition against enumeration: kept/added/removed are
/// exactly the set differences, boxes are disjoint (no task counted twice),
/// and `old.index_of` reproduces every old task's iteration position.
fn check(old: &StepParameterSpace, new: &StepParameterSpace) -> TaskDelta {
    let d = task_delta(Some(old), Some(new));
    let TaskDelta::Product(pd) = &d else {
        panic!("expected Product, got {d:?}");
    };
    let (old_box, kept, added, removed) = (&pd.old, &pd.kept, &pd.added, &pd.removed);
    let o: BTreeSet<String> = all(old).iter().map(key).collect();
    let n: BTreeSet<String> = all(new).iter().map(key).collect();

    let (k, kn) = keys_of(&[kept]);
    assert_eq!(k, o.intersection(&n).cloned().collect());
    assert_eq!(kn, k.len());

    let (a, an) = keys_of(&added.iter().collect::<Vec<_>>());
    assert_eq!(a, n.difference(&o).cloned().collect());
    assert_eq!(an, a.len(), "added boxes are disjoint");

    let (r, rn) = keys_of(&removed.iter().collect::<Vec<_>>());
    assert_eq!(r, o.difference(&n).cloned().collect());
    assert_eq!(rn, r.len(), "removed boxes are disjoint");

    for (i, t) in all(old).iter().enumerate() {
        assert_eq!(old_box.index_of(t), Some(i));
    }
    d
}

#[test]
fn equal_spaces_are_unchanged() {
    let s = space(vec![("Frame", ints("1-10"))], None);
    assert_eq!(task_delta(Some(&s), Some(&s)), TaskDelta::Unchanged);
    assert_eq!(task_delta(None, None), TaskDelta::Unchanged);
}

#[test]
fn extending_one_range_adds_one_box() {
    let d = check(
        &space(vec![("Frame", ints("1-10"))], None),
        &space(vec![("Frame", ints("1-12"))], None),
    );
    let TaskDelta::Product(pd) = d else {
        unreachable!()
    };
    let ProductDelta {
        kept,
        added,
        removed,
        ..
    } = *pd;
    assert_eq!(kept.len(), 10);
    assert_eq!(added.iter().map(TaskBox::len).collect::<Vec<_>>(), vec![2]);
    assert!(removed.is_empty());
}

#[test]
fn two_dimensions_both_changed() {
    // Frames shift 1-10 -> 5-14, cameras A,B -> B,C.
    let d = check(
        &space(
            vec![("Frame", ints("1-10")), ("Cam", strs(&["A", "B"]))],
            None,
        ),
        &space(
            vec![("Frame", ints("5-14")), ("Cam", strs(&["B", "C"]))],
            None,
        ),
    );
    let TaskDelta::Product(pd) = d else {
        unreachable!()
    };
    let ProductDelta {
        kept,
        added,
        removed,
        ..
    } = *pd;
    // kept = {5..10} x {B}; added = {11..14} x {B,C}  +  {5..10} x {C}
    assert_eq!(kept.len(), 6);
    assert_eq!(
        added.iter().map(TaskBox::len).collect::<Vec<_>>(),
        vec![8, 6]
    );
    assert_eq!(
        removed.iter().map(TaskBox::len).collect::<Vec<_>>(),
        vec![8, 6]
    );
}

#[test]
fn explicit_product_order_is_respected_for_old_indices() {
    check(
        &space(
            vec![("Frame", ints("1-4")), ("Cam", strs(&["A", "B", "C"]))],
            Some("Cam * Frame"),
        ),
        &space(
            vec![("Frame", ints("1-6")), ("Cam", strs(&["A", "C"]))],
            Some("Frame * Cam"),
        ),
    );
}

#[test]
fn huge_product_is_decomposed_without_enumeration() {
    // 10^5 frames x 10^3 tiles = 10^8 tasks; only the 1-D ranges are touched.
    let old = space(
        vec![("Frame", ints("1-100000")), ("Tile", ints("1-1000"))],
        None,
    );
    let new = space(
        vec![("Frame", ints("1-100001")), ("Tile", ints("1-1000"))],
        None,
    );
    let TaskDelta::Product(pd) = task_delta(Some(&old), Some(&new)) else {
        panic!("expected Product")
    };
    let ProductDelta {
        kept,
        added,
        removed,
        ..
    } = *pd;
    assert_eq!(kept.len(), 100_000_000);
    assert_eq!(added.iter().map(TaskBox::len).sum::<usize>(), 1000);
    assert!(removed.is_empty());
}

#[test]
fn association_chunking_and_new_parameters_fall_back() {
    let fb = |d: TaskDelta| assert!(matches!(d, TaskDelta::Fallback { .. }), "{d:?}");
    let a = space(vec![("A", ints("1-3")), ("B", ints("4-6"))], Some("(A, B)"));
    let b = space(vec![("A", ints("1-3")), ("B", ints("4-7"))], Some("(A, B)"));
    fb(task_delta(Some(&a), Some(&b)));

    let chunked = |r: &str| {
        space(
            vec![(
                "F",
                TaskParameter::ChunkInt {
                    range: TaskParamRange::RangeExpr(r.parse().unwrap()),
                    chunks: ResolvedChunks {
                        default_task_count: 3,
                        target_runtime_seconds: None,
                        range_constraint: RangeConstraint::Contiguous,
                    },
                },
            )],
            None,
        )
    };
    fb(task_delta(Some(&chunked("1-9")), Some(&chunked("1-12"))));

    let one = space(vec![("A", ints("1-3"))], None);
    let two = space(vec![("A", ints("1-3")), ("B", ints("1-2"))], None);
    fb(task_delta(Some(&one), Some(&two)));
    fb(task_delta(None, Some(&one)));
}
