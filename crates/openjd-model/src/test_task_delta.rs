// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// Copyright by contributors to this project.
// SPDX-License-Identifier: (Apache-2.0 OR MIT)

//! Tests for `job::task_delta`. `check` compares each decomposition with
//! brute-force enumeration of both spaces.

use std::collections::BTreeSet;

use openjd_expr::value::Float64;
use openjd_expr::{RangeExpr, MAX_RANGE_EXPR_CHUNKS};

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
    let Some(TaskDelta::Product(pd)) = d else {
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

/// An order change keeps every task and reports `reordered` exactly when
/// the kept tasks iterate in another order.
#[test]
fn order_changes_keep_every_task() {
    let ab = space(vec![("A", ints("1-2")), ("B", strs(&["x", "y"]))]);
    let ba = space(vec![("B", strs(&["x", "y"])), ("A", ints("1-2"))]);
    for (new, reordered) in [
        (with_combination(ab.clone(), "B * A"), true),
        (with_combination(ab.clone(), " A*B "), false),
        (with_combination(ba.clone(), "*"), true),
        (with_combination(ba.clone(), " * "), true),
        (ba.clone(), true),
        (
            space(vec![("A", ints("1-2")), ("B", strs(&["y", "x"]))]),
            true,
        ),
        (
            space(vec![("A", int_list(&[2, 1])), ("B", strs(&["x", "y"]))]),
            true,
        ),
        (
            space(vec![("A", int_list(&[1, 2])), ("B", strs(&["x", "y"]))]),
            false,
        ),
        (with_combination(ab.clone(), "*"), false),
        (with_combination(ab.clone(), " * "), false),
    ] {
        let d = check(&ab, &new);
        assert_eq!(counts(&d), (4, 0, 0), "{new:?}");
        assert_eq!(d.reordered, reordered, "{new:?}");
    }
    // Two INT lists compare their shared values in list order.
    let d = check(
        &space(vec![("N", int_list(&[3, 1, 2]))]),
        &space(vec![("N", int_list(&[1, 2, 3, 4]))]),
    );
    assert!(d.reordered);
    // A dimension with one kept value orders nothing, and neither do values
    // only one side has.
    for (old, new) in [
        (
            space(vec![("A", ints("1-2")), ("B", strs(&["x"]))]),
            space(vec![("B", strs(&["x"])), ("A", ints("1-2"))]),
        ),
        (
            space(vec![("A", strs(&["x", "y", "z"]))]),
            space(vec![("A", strs(&["w", "x", "y"]))]),
        ),
        (
            space(vec![("A", strs(&["x", "y"]))]),
            space(vec![("A", strs(&["z", "w"]))]),
        ),
        (
            space(vec![
                ("A", strs(&["x"])),
                ("B", ints("1-2")),
                ("C", ints("1-2")),
            ]),
            space(vec![
                ("B", ints("1-2")),
                ("A", strs(&["x"])),
                ("C", ints("1-2")),
            ]),
        ),
        // B and C swap, but A keeps nothing, so no task is kept.
        (
            space(vec![
                ("A", strs(&["x"])),
                ("B", ints("1-2")),
                ("C", ints("1-2")),
            ]),
            space(vec![
                ("A", strs(&["y"])),
                ("C", ints("1-2")),
                ("B", ints("1-2")),
            ]),
        ),
    ] {
        let (d, back) = both_ways(&old, &new);
        assert!(!d.reordered && !back.reordered, "{old:?} -> {new:?}");
    }
    // A combination other than `*` fixes the order, so definition order
    // alone is then no change.
    assert_eq!(
        task_delta(
            Some(&with_combination(ab.clone(), "A * B")),
            Some(&with_combination(ba.clone(), "A * B"))
        ),
        None
    );
}

/// A padded `*` is the definition-order product.
#[test]
fn a_padded_star_combination_is_a_product() {
    let d = check(
        &with_combination(space(vec![("A", ints("1-3"))]), " * "),
        &space(vec![("A", ints("2-4"))]),
    );
    assert_eq!(counts(&d), (2, 1, 1));
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
    assert_eq!(task_delta(Some(&s), Some(&s.clone())), None);
    assert_eq!(task_delta(None, None), None);
}

fn reason(old: Option<&StepParameterSpace>, new: Option<&StepParameterSpace>) -> String {
    match task_delta(old, new) {
        Some(TaskDelta::Fallback { reason }) => reason.to_string(),
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
    let not_product = "parameter space is not a product of distinct values";
    let cases: Vec<(Option<&StepParameterSpace>, StepParameterSpace, &str)> = vec![
        (None, frames.clone(), "parameter space added or removed"),
        (
            Some(&two),
            with_combination(two.clone(), "(A, B)"),
            not_product,
        ),
        (
            Some(&two),
            with_combination(two.clone(), "A * A"),
            not_product,
        ),
        (Some(&two), with_combination(two.clone(), "A"), not_product),
        (Some(&frames), chunked, not_product),
        (Some(&frames), int_chunked, not_product),
        (
            Some(&frames),
            space(vec![("Frame", int_list(&[1, 1]))]),
            not_product,
        ),
        (
            Some(&frames),
            space(vec![("Frame", floats(&["1.5", "1.5"]))]),
            not_product,
        ),
        (
            Some(&frames),
            space(vec![("Frame", paths(&["/a", "/a"]))]),
            not_product,
        ),
        (Some(&two), huge, not_product),
        (
            Some(&frames),
            two.clone(),
            "task parameter A added, removed or retyped",
        ),
        (
            Some(&frames),
            space(vec![("Frame", ints("1-3")), ("Eye", strs(&["l"]))]),
            "task parameter Eye added, removed or retyped",
        ),
        (
            Some(&frames),
            space(vec![("Frame", strs(&["1"]))]),
            "task parameter Frame added, removed or retyped",
        ),
        (
            Some(&frames),
            space(vec![("Frame", int_list(&[i64::MIN]))]),
            "task parameter Frame needs more runs than a range holds",
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
        "task parameter Eye added, removed or retyped"
    );
    // Each side is checked, the old one first.
    assert_eq!(
        reason(Some(&space(vec![("A", int_list(&[2, 2]))])), Some(&frames)),
        not_product
    );
    // A result whose canonical form has more runs than a range expression
    // holds: removing every third value of 1-300000 leaves 200000 single
    // values.
    let (old, new) = ("1-300000", "1-300000:3");
    assert!(RangeExpr::from_values(minus(old, new)).is_err());
    assert_eq!(
        reason(
            Some(&space(vec![("N", ints(old))])),
            Some(&space(vec![("N", ints(new))]))
        ),
        "task parameter N needs more runs than a range holds"
    );
    // Every parameter's type is checked before any range is split, so a
    // retyped S wins over N's too-complex result, though N comes first.
    let (old, new) = (
        space(vec![("N", ints(old)), ("S", strs(&["a"]))]),
        space(vec![("N", ints(new)), ("S", paths(&["a"]))]),
    );
    for (a, b) in [(&old, &new), (&new, &old)] {
        assert_eq!(
            task_delta(Some(a), Some(b)),
            Some(TaskDelta::Fallback {
                reason: FallbackReason::ParameterMismatch("S".into())
            })
        );
    }
}

#[test]
fn the_fallback_reason_variants_name_the_parameter() {
    let frames = space(vec![("Frame", ints("1-3"))]);
    assert_eq!(
        task_delta(Some(&frames), Some(&space(vec![("Frame", strs(&["1"]))]))),
        Some(TaskDelta::Fallback {
            reason: FallbackReason::ParameterMismatch("Frame".into())
        })
    );
}

fn values(expr: &str) -> BTreeSet<i64> {
    expr.parse::<RangeExpr>().unwrap().iter().collect()
}

/// The values of `a` that `b` lacks.
fn minus(a: &str, b: &str) -> Vec<i64> {
    values(a).difference(&values(b)).copied().collect()
}

fn box_values(boxes: &[&TaskBox]) -> BTreeSet<i64> {
    let mut all = BTreeSet::new();
    for b in boxes {
        let Some(TaskParameter::Int { range, .. }) = b.space().task_parameter_definitions.get("N")
        else {
            panic!("{b}");
        };
        let v: Vec<i64> = match range {
            TaskParamRange::RangeExpr(r) => r.iter().collect(),
            TaskParamRange::List(v) => v.clone(),
        };
        assert_eq!(b.len(), u64::try_from(v.len()).unwrap());
        for x in v {
            assert!(all.insert(x), "{x} is in two boxes");
        }
    }
    all
}

/// The delta from range `a` to range `b` of one `INT` parameter `N`, checked
/// against the value sets: a product whose boxes are exactly the
/// intersection and the differences, or a `RangeTooComplex` fallback for
/// which one of those has no range expression. Returns whether it is a
/// product.
fn check_ints(a: &str, b: &str) -> bool {
    let (o, n) = (values(a), values(b));
    let parts = [&o & &n, &n - &o, &o - &n];
    match task_delta(
        Some(&space(vec![("N", ints(a))])),
        Some(&space(vec![("N", ints(b))])),
    ) {
        Some(TaskDelta::Product(d)) => {
            assert_eq!(box_values(&[&d.kept]), parts[0], "kept {a} -> {b}");
            assert_eq!(
                box_values(&d.added.iter().collect::<Vec<_>>()),
                parts[1],
                "added {a} -> {b}"
            );
            assert_eq!(
                box_values(&d.removed.iter().collect::<Vec<_>>()),
                parts[2],
                "removed {a} -> {b}"
            );
            true
        }
        Some(TaskDelta::Fallback {
            reason: FallbackReason::RangeTooComplex(name),
        }) => {
            assert_eq!(name, "N");
            assert!(
                parts
                    .iter()
                    .any(|p| RangeExpr::from_values(p.iter().copied().collect())
                        .is_err_and(|e| e.to_string().contains("too many sub-ranges"))),
                "{a} -> {b} fell back, but each result has a range expression"
            );
            false
        }
        d => panic!("{a} -> {b}: {d:?}"),
    }
}

/// Subtracting a progression whose step is `m` times the other's, at sizes
/// whose value counts pass `MAX_RANGE_EXPR_CHUNKS`: a product exactly when
/// each result's canonical form fits the cap.
#[test]
fn stepped_subtractions_past_the_run_cap() {
    assert_eq!(
        MAX_RANGE_EXPR_CHUNKS, 50_000,
        "the cases are sized to this cap"
    );
    for (a, b, product) in [
        // m = 2: the remainder is one progression.
        ("1-200000", "1-200000:2", true),
        // m = 3: two single values a gap, so 20000 gaps fit and 30000 do not.
        ("1-60000", "1-60000:3", true),
        ("1-90000", "1-90000:3", false),
        ("1-300000", "1-300000:3", false),
        // m = 7: one run a gap.
        ("1-140000", "1-140000:7", true),
        ("1-700000", "1-700000:7", false),
        // The common lattice is an lcm with a nonzero offset: 4 mod 6 from
        // 1 mod 3 and 0 mod 2, or from 1 mod 3 and 4 mod 6.
        ("1-600001:3", "4-600004:6", true),
        ("0-120000:2", "1-120001:3", true),
        ("0-600000:2", "1-600001:3", false),
    ] {
        assert_eq!(check_ints(a, b), product, "{a} -> {b}");
        assert_eq!(check_ints(b, a), product, "{b} -> {a}");
    }
    // The m = 2 remainder is stored as one run.
    let Some(TaskDelta::Product(d)) = task_delta(
        Some(&space(vec![("N", ints("1-200000"))])),
        Some(&space(vec![("N", ints("1-200000:2"))])),
    ) else {
        panic!();
    };
    assert_eq!(d.removed[0].to_string(), "N=2-200000:2");
}

/// A result whose pieces number more than `MAX_RANGE_EXPR_CHUNKS` but merge
/// into one run is a product: blocks `8i+1..8i+5` minus `8i+2..8i+4`, for
/// 50000 blocks, leave 100000 single values that are one progression.
#[test]
fn pieces_that_continue_one_progression_merge() {
    let runs = |lo: i64, hi: i64| {
        (0..50_000_i64)
            .map(|i| format!("{}-{}", 8 * i + lo, 8 * i + hi))
            .collect::<Vec<_>>()
            .join(",")
    };
    let (a, b) = (runs(1, 5), runs(2, 4));
    assert!(check_ints(&a, &b));
    assert!(check_ints(&b, &a));
    let Some(TaskDelta::Product(d)) = task_delta(
        Some(&space(vec![("N", ints(&a))])),
        Some(&space(vec![("N", ints(&b))])),
    ) else {
        panic!();
    };
    assert_eq!(d.removed[0].to_string(), "N=1-399997:4");
}

/// Pieces past `MAX_RANGE_EXPR_CHUNKS` whose canonical form regroups values
/// across pieces to fit the cap are a product. Each block of `a` is 2 runs
/// and each result 2 runs a block, but the pieces of the subtractions number
/// more, so 20000 blocks give 40000 runs and more than 50000 pieces.
#[test]
fn pieces_that_regroup_into_the_run_cap_are_a_product() {
    let blocks = |runs: &[(i64, i64, i64)]| {
        (0..20_000_i64)
            .flat_map(|j| {
                runs.iter()
                    .map(move |(s, e, step)| format!("{}-{}:{step}", s + 100 * j, e + 100 * j))
            })
            .collect::<Vec<_>>()
            .join(",")
    };
    let a = blocks(&[(5, 12, 1), (16, 24, 4)]);
    let b = blocks(&[(10, 28, 2), (34, 58, 6)]);
    assert!(check_ints(&a, &b));
    assert!(check_ints(&b, &a));
    // Past the cap after regrouping, with pieces between the cap and twice
    // it, and past twice it: still a fallback.
    assert!(!check_ints("1-90000", "1-90000:3"));
    assert!(!check_ints("1-300000", "1-300000:3"));
}

/// Steps whose lcm does not fit an `i64` meet once, a one-value run.
#[test]
fn coprime_steps_past_i64_meet_in_one_value() {
    let (s, t) = (4_294_967_311_i64, 4_294_967_291_i64);
    let (d, _) = both_ways(
        &space(vec![("N", ints(&format!("0-{}:{s}", 2 * s)))]),
        &space(vec![("N", ints(&format!("0-{}:{t}", 2 * t)))]),
    );
    assert_eq!(counts(&d), (1, 2, 2));
    assert_eq!(d.kept.to_string(), "N=0");
}

/// Steps whose lcm does not fit an `i64` can still meet twice, at the ends
/// of a span wider than `i64::MAX`: here `±3·2^61`, kept as two runs.
#[test]
fn steps_past_i64_meet_in_two_values() {
    let v = 3_i64 << 61;
    let (d, _) = both_ways(
        &space(vec![("N", ints(&format!("{}-{v}:{v}", -v)))]),
        &space(vec![("N", ints(&format!("{}-{v}:{}", -v, 1_i64 << 62)))]),
    );
    assert_eq!(counts(&d), (2, 2, 1));
    assert_eq!(d.kept.to_string(), format!("N={},{v}", -v));
}

/// Subtracting every second value of 10^18 leaves one run, found from the
/// runs alone.
#[test]
fn a_large_stepped_subtraction_is_one_run() {
    let d = task_delta(
        Some(&space(vec![("N", ints("1-1000000000000000000"))])),
        Some(&space(vec![("N", ints("1-1000000000000000000:2"))])),
    );
    let Some(TaskDelta::Product(d)) = d else {
        panic!("{d:?}");
    };
    let half = 500_000_000_000_000_000;
    assert_eq!(counts(&d), (half, 0, half));
    assert_eq!(d.removed.len(), 1);
    assert_eq!(d.removed[0].to_string(), "N=2-1000000000000000000:2");
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
    let Some(TaskDelta::Product(d)) = d else {
        panic!("{d:?}");
    };
    let half = 500_000_000_000_000_000;
    assert_eq!(counts(&d), (half, half, half));
    assert_eq!(
        d.added[0].to_string(),
        "N=1000000000000000001-1500000000000000000"
    );
}
