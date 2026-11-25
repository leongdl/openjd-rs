// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// Copyright by contributors to this project.
// SPDX-License-Identifier: (Apache-2.0 OR MIT)

//! Tests for `job::task_delta`. `check` compares each decomposition with
//! brute-force enumeration of both spaces.

use std::collections::BTreeSet;

use openjd_expr::value::Float64;
use openjd_expr::RangeExpr;

use crate::job::task_delta::{task_delta, FallbackReason, ProductDelta, TaskBox, TaskDelta};
use crate::job::{ResolvedChunks, StepParameterSpace, TaskParamRange, TaskParameter};
use crate::step_param_space::StepParameterSpaceIterator;
use crate::template::RangeConstraint;
use crate::types::TaskParameterSet;

fn space(params: Vec<(&str, TaskParameter)>) -> StepParameterSpace {
    StepParameterSpace {
        task_parameter_definitions: params
            .into_iter()
            .map(|(n, p)| (n.to_string(), p))
            .collect(),
        combination: None,
    }
}

fn with_combination(mut s: StepParameterSpace, c: &str) -> StepParameterSpace {
    s.combination = Some(c.to_string());
    s
}

fn ints(expr: &str) -> TaskParameter {
    TaskParameter::Int {
        range: TaskParamRange::RangeExpr(expr.parse::<RangeExpr>().unwrap()),
        chunks: None,
    }
}

fn int_list(v: &[i64]) -> TaskParameter {
    TaskParameter::Int {
        range: TaskParamRange::List(v.to_vec()),
        chunks: None,
    }
}

fn strs(v: &[&str]) -> TaskParameter {
    TaskParameter::String {
        range: v.iter().map(|s| (*s).to_string()).collect(),
    }
}

fn paths(v: &[&str]) -> TaskParameter {
    TaskParameter::Path {
        range: v.iter().map(|s| (*s).to_string()).collect(),
    }
}

fn floats(v: &[&str]) -> TaskParameter {
    TaskParameter::Float {
        range: v
            .iter()
            .map(|s| Float64::with_str(s.parse().unwrap(), (*s).to_string()).unwrap())
            .collect(),
    }
}

fn chunks() -> ResolvedChunks {
    ResolvedChunks {
        default_task_count: 2,
        target_runtime_seconds: None,
        range_constraint: RangeConstraint::Contiguous,
    }
}

/// A task as text, with its parameters sorted by name, so the same task from
/// two spaces with different definition orders has one key.
fn key(t: &TaskParameterSet) -> String {
    let mut kv: Vec<String> = t
        .iter()
        .map(|(k, v)| format!("{k}={:?}", v.value))
        .collect();
    kv.sort();
    kv.join(",")
}

fn tasks(s: &StepParameterSpace) -> Vec<String> {
    StepParameterSpaceIterator::new(s)
        .unwrap()
        .map(|t| key(&t))
        .collect()
}

/// The tasks of `boxes`, checking that no task is in two boxes and that
/// each box's `len` is its task count.
fn union(boxes: &[&TaskBox]) -> BTreeSet<String> {
    let mut all = BTreeSet::new();
    for b in boxes {
        let t = tasks(b.space());
        assert_eq!(b.len(), u64::try_from(t.len()).unwrap(), "len of {b}");
        assert_eq!(b.is_empty(), t.is_empty());
        for task in t {
            assert!(all.insert(task.clone()), "{task} is in two boxes");
        }
    }
    all
}

/// The product delta from `old` to `new`, checked against enumeration:
/// kept, added and removed are exactly the intersection and the two
/// differences, as disjoint non-empty boxes.
fn check(old: &StepParameterSpace, new: &StepParameterSpace) -> ProductDelta {
    let d = task_delta(Some(old), Some(new));
    let TaskDelta::Product(pd) = d else {
        panic!("expected a product delta, got {d:?}");
    };
    let o: BTreeSet<String> = tasks(old).into_iter().collect();
    let n: BTreeSet<String> = tasks(new).into_iter().collect();
    assert_eq!(union(&[&pd.kept]), &o & &n, "kept");
    assert_eq!(
        union(&pd.added.iter().collect::<Vec<_>>()),
        &n - &o,
        "added"
    );
    assert_eq!(
        union(&pd.removed.iter().collect::<Vec<_>>()),
        &o - &n,
        "removed"
    );
    assert!(pd.added.iter().chain(&pd.removed).all(|b| !b.is_empty()));
    pd
}

fn counts(d: &ProductDelta) -> (u64, u64, u64) {
    let sum = |b: &[TaskBox]| b.iter().map(TaskBox::len).sum();
    (d.kept.len(), sum(&d.added), sum(&d.removed))
}

fn both_ways(a: &StepParameterSpace, b: &StepParameterSpace) -> (ProductDelta, ProductDelta) {
    (check(a, b), check(b, a))
}

#[test]
fn growing_a_frame_range_adds_only_the_new_frames() {
    let (grow, shrink) = both_ways(
        &space(vec![("Frame", ints("1-100"))]),
        &space(vec![("Frame", ints("1-200"))]),
    );
    assert_eq!(counts(&grow), (100, 100, 0));
    assert_eq!(grow.added[0].to_string(), "Frame=101-200");
    assert_eq!(counts(&shrink), (100, 0, 100));
    assert_eq!(shrink.removed[0].to_string(), "Frame=101-200");
}

#[test]
fn a_shifted_range_both_adds_and_removes() {
    let (d, _) = both_ways(
        &space(vec![("Frame", ints("1-100"))]),
        &space(vec![("Frame", ints("51-150"))]),
    );
    assert_eq!(counts(&d), (50, 50, 50));
    assert_eq!(d.kept.to_string(), "Frame=51-100");
}

#[test]
fn multi_dimension_boxes_are_disjoint() {
    let old = space(vec![
        ("Frame", ints("1-4")),
        ("Eye", strs(&["left", "right"])),
        ("Scale", floats(&["0.5", "1.0"])),
    ]);
    let new = space(vec![
        ("Frame", ints("3-6")),
        ("Eye", strs(&["right", "center"])),
        ("Scale", floats(&["1.0", "2.0"])),
    ]);
    let (d, _) = both_ways(&old, &new);
    // 16 tasks a side, 2 kept; box k holds the tasks whose first new value
    // is in dimension k.
    assert_eq!(counts(&d), (2, 14, 14));
    assert_eq!(d.added.len(), 3);
    let boxes: Vec<String> = d.added.iter().map(ToString::to_string).collect();
    assert_eq!(
        boxes,
        [
            r#"Frame=5,6, Eye=["right","center"], Scale=["1.0","2.0"]"#,
            r#"Frame=3,4, Eye=["center"], Scale=["1.0","2.0"]"#,
            r#"Frame=3,4, Eye=["right"], Scale=["2.0"]"#,
        ]
    );
}

#[test]
fn one_dimension_shrinks_while_another_grows() {
    let (d, _) = both_ways(
        &space(vec![
            ("Frame", ints("1-10")),
            ("Shot", strs(&["a", "b", "c"])),
        ]),
        &space(vec![("Frame", ints("1-20")), ("Shot", strs(&["a"]))]),
    );
    assert_eq!(counts(&d), (10, 10, 20));
}

#[test]
fn disjoint_spaces_keep_nothing() {
    let (d, _) = both_ways(
        &space(vec![("Frame", ints("1-5")), ("Shot", strs(&["a"]))]),
        &space(vec![("Frame", ints("6-8")), ("Shot", strs(&["a"]))]),
    );
    assert_eq!(counts(&d), (0, 3, 5));
    assert!(d.kept.is_empty());
}

#[test]
fn stepped_ranges_intersect_on_their_common_lattice() {
    for (a, b) in [
        ("1-100", "1-100:2"),
        ("1-100:2", "1-100:3"),
        ("1-100:4", "3-99:6"),
        ("1-100:3", "2-100:3"),
        ("1-100:6", "4-100:9"),
        // Steps near 10^18, whose products need the reductions mod the step.
        (
            "1-3000000000000000010:1000000000000000003",
            "0-3000000000000000027:1000000000000000009",
        ),
        ("2-40:4", "1-40:3"),
        ("1,5,9,20-30", "1-30:2"),
        ("-10--1", "-5-5"),
        ("5", "1-10:2"),
    ] {
        both_ways(&space(vec![("N", ints(a))]), &space(vec![("N", ints(b))]));
    }
    let (d, _) = both_ways(
        &space(vec![("N", ints("1-100"))]),
        &space(vec![("N", ints("1-100:2"))]),
    );
    assert_eq!(d.removed[0].to_string(), "N=2-100:2");
}

#[test]
fn int_lists_and_ranges_mix() {
    for (a, b) in [
        (int_list(&[3, 1, 2]), int_list(&[2, 4])),
        (int_list(&[10, 1, 5]), ints("1-6")),
        (ints("1-6"), int_list(&[])),
        (int_list(&[i64::MAX]), ints("1-3")),
    ] {
        both_ways(&space(vec![("N", a)]), &space(vec![("N", b)]));
    }
}

#[test]
fn strings_paths_and_floats_compare_by_value() {
    both_ways(
        &space(vec![("P", paths(&["/a", "/b"]))]),
        &space(vec![("P", paths(&["/b", "/c"]))]),
    );
    // A float's spelling is part of its value, as a task renders it.
    let (d, _) = both_ways(
        &space(vec![("S", floats(&["2.5", "3"]))]),
        &space(vec![("S", floats(&["2.50", "3"]))]),
    );
    assert_eq!(counts(&d), (1, 1, 1));
}

#[test]
fn order_changes_keep_every_task() {
    let ab = space(vec![("A", ints("1-2")), ("B", strs(&["x", "y"]))]);
    let ba = space(vec![("B", strs(&["x", "y"])), ("A", ints("1-2"))]);
    let reordered_values = space(vec![("A", ints("1-2")), ("B", strs(&["y", "x"]))]);
    for new in [
        with_combination(ab.clone(), "B * A"),
        with_combination(ab.clone(), " A*B "),
        with_combination(ba.clone(), "*"),
        reordered_values,
    ] {
        let d = check(&ab, &new);
        assert_eq!(counts(&d), (4, 0, 0), "{new:?}");
    }
    // Definition order alone is not a change of the space's value.
    assert_eq!(task_delta(Some(&ab), Some(&ba)), TaskDelta::Unchanged);
}

#[test]
fn a_range_rewritten_as_its_list_keeps_every_task() {
    let d = check(
        &space(vec![("N", ints("1-2"))]),
        &space(vec![("N", int_list(&[1, 2]))]),
    );
    assert_eq!(counts(&d), (2, 0, 0));
}

#[test]
fn equal_spaces_are_unchanged() {
    let s = space(vec![("A", ints("1-3"))]);
    assert_eq!(task_delta(Some(&s), Some(&s.clone())), TaskDelta::Unchanged);
    assert_eq!(task_delta(None, None), TaskDelta::Unchanged);
}

fn reason(old: Option<&StepParameterSpace>, new: Option<&StepParameterSpace>) -> String {
    match task_delta(old, new) {
        TaskDelta::Fallback { reason } => reason.to_string(),
        d => panic!("expected a fallback, got {d:?}"),
    }
}

#[test]
fn every_fallback_reason() {
    let frames = space(vec![("Frame", ints("1-3"))]);
    let two = space(vec![("A", ints("1-2")), ("B", ints("1-2"))]);
    let chunked = space(vec![(
        "Frame",
        TaskParameter::ChunkInt {
            range: TaskParamRange::RangeExpr("1-3".parse().unwrap()),
            chunks: chunks(),
        },
    )]);
    let int_chunked = space(vec![(
        "Frame",
        TaskParameter::Int {
            range: TaskParamRange::RangeExpr("1-3".parse().unwrap()),
            chunks: Some(chunks()),
        },
    )]);
    let huge = space(vec![
        ("A", ints("1-4294967296")),
        ("B", ints("1-4294967296")),
    ]);
    let cases: Vec<(Option<&StepParameterSpace>, StepParameterSpace, &str)> = vec![
        (None, frames.clone(), "parameter space added or removed"),
        (
            Some(&two),
            with_combination(two.clone(), "(A, B)"),
            "combination is not a product of every parameter",
        ),
        (
            Some(&two),
            with_combination(two.clone(), "A * A"),
            "combination is not a product of every parameter",
        ),
        (
            Some(&two),
            with_combination(two.clone(), "A"),
            "combination is not a product of every parameter",
        ),
        (Some(&frames), chunked, "task parameter Frame is chunked"),
        (
            Some(&frames),
            int_chunked,
            "task parameter Frame is chunked",
        ),
        (
            Some(&frames),
            space(vec![("Frame", int_list(&[1, 1]))]),
            "task parameter Frame repeats a value",
        ),
        (
            Some(&frames),
            space(vec![("Frame", floats(&["1.5", "1.5"]))]),
            "task parameter Frame repeats a value",
        ),
        (
            Some(&frames),
            space(vec![("Frame", paths(&["/a", "/a"]))]),
            "task parameter Frame repeats a value",
        ),
        (Some(&two), huge, "task count does not fit 64 bits"),
        (Some(&frames), two.clone(), "task parameter A added"),
        (
            Some(&frames),
            space(vec![("Frame", ints("1-3")), ("Eye", strs(&["l"]))]),
            "task parameter Eye added",
        ),
        (
            Some(&frames),
            space(vec![("Frame", strs(&["1"]))]),
            "task parameter Frame changed type",
        ),
        (
            Some(&frames),
            space(vec![("Frame", int_list(&[i64::MIN]))]),
            "task parameter Frame changes in more runs than a range holds",
        ),
    ];
    for (old, new, want) in &cases {
        assert_eq!(reason(*old, Some(new)), *want, "{old:?} -> {new:?}");
    }
    assert_eq!(
        reason(Some(&frames), None),
        "parameter space added or removed"
    );
    assert_eq!(
        reason(
            Some(&space(vec![("Frame", ints("1-3")), ("Eye", strs(&["l"]))])),
            Some(&frames)
        ),
        "task parameter Eye removed"
    );
    // Each side is checked, the old one first.
    assert_eq!(
        reason(Some(&space(vec![("A", int_list(&[2, 2]))])), Some(&frames)),
        "task parameter A repeats a value"
    );
    // A result with more runs than a range expression holds is not built:
    // removing every other value of 1-200000 leaves 100000 single values.
    assert_eq!(
        reason(
            Some(&space(vec![("N", ints("1-200000"))])),
            Some(&space(vec![("N", ints("1-200000:2"))]))
        ),
        "task parameter N changes in more runs than a range holds"
    );
}

#[test]
fn the_fallback_reason_variants_name_the_parameter() {
    let frames = space(vec![("Frame", ints("1-3"))]);
    assert_eq!(
        task_delta(Some(&frames), Some(&space(vec![("Frame", strs(&["1"]))]))),
        TaskDelta::Fallback {
            reason: FallbackReason::ParameterRetyped("Frame".into())
        }
    );
}

/// A range of 10^18 values costs its run count, so this returns at once;
/// enumerating it would not finish.
#[test]
fn a_large_range_is_not_enumerated() {
    let d = task_delta(
        Some(&space(vec![("N", ints("1-1000000000000000000"))])),
        Some(&space(vec![(
            "N",
            ints("500000000000000001-1500000000000000000"),
        )])),
    );
    let TaskDelta::Product(d) = d else {
        panic!("{d:?}");
    };
    let half = 500_000_000_000_000_000;
    assert_eq!(counts(&d), (half, half, half));
    assert_eq!(
        d.added[0].to_string(),
        "N=1000000000000000001-1500000000000000000"
    );
}
