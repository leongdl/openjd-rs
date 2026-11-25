// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// Copyright by contributors to this project.
// SPDX-License-Identifier: (Apache-2.0 OR MIT)

//! Which tasks a parameter space change keeps, adds and removes.
//!
//! **Status: unstable.** This module may change or be removed in any
//! release without notice. [`task_delta`] compares two step parameter spaces
//! without enumerating their tasks; `specs/model/task-delta.md` gives the
//! semantics.

use std::collections::HashSet;
use std::fmt;
use std::hash::Hash;

use indexmap::IndexMap;
use openjd_expr::range_expr::IntRange;
use openjd_expr::value::Float64;
use openjd_expr::{RangeExpr, MAX_RANGE_EXPR_CHUNKS};

use crate::job::{StepParameterSpace, TaskParamRange, TaskParameter};

/// A set of tasks that is the Cartesian product of one value list per task
/// parameter.
#[derive(Debug, Clone, PartialEq)]
pub struct TaskBox {
    space: StepParameterSpace,
    len: u64,
}

impl TaskBox {
    /// The box as a parameter space with no `combination`, so
    /// [`StepParameterSpaceIterator`](crate::StepParameterSpaceIterator)
    /// iterates its tasks.
    #[must_use]
    pub fn space(&self) -> &StepParameterSpace {
        &self.space
    }

    /// The number of tasks in the box.
    #[must_use]
    pub fn len(&self) -> u64 {
        self.len
    }

    /// Whether the box has no tasks.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

impl fmt::Display for TaskBox {
    /// Writes each parameter as `name=values`, comma-separated: a range
    /// expression in its canonical form, a list as compact JSON.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, (name, p)) in self.space.task_parameter_definitions.iter().enumerate() {
            let sep = if i == 0 { "" } else { ", " };
            let values = match p {
                TaskParameter::Int { range, .. } | TaskParameter::ChunkInt { range, .. } => {
                    match range {
                        TaskParamRange::RangeExpr(r) => r.to_string(),
                        TaskParamRange::List(v) => json(v),
                    }
                }
                TaskParameter::Float { range } => json(range),
                TaskParameter::String { range } | TaskParameter::Path { range } => json(range),
            };
            write!(f, "{sep}{name}={values}")?;
        }
        Ok(())
    }
}

fn json<T: serde::Serialize>(v: &T) -> String {
    serde_json::to_string(v).unwrap_or_default()
}

/// How a step's tasks change from one parameter space to another.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum TaskDelta {
    /// The spaces are equal.
    Unchanged,
    /// Both spaces are plain products over the same parameters: the tasks
    /// kept, added and removed.
    Product(ProductDelta),
    /// No product decomposition applies, so every old task is replaced by
    /// the new space's tasks.
    Fallback {
        /// Why the decomposition does not apply.
        reason: FallbackReason,
    },
}

/// The tasks a change between two product spaces keeps, adds and removes.
/// Every task is in exactly one box of its side.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct ProductDelta {
    /// The tasks both spaces have. May be empty.
    pub kept: TaskBox,
    /// The tasks only the new space has, as disjoint non-empty boxes.
    pub added: Vec<TaskBox>,
    /// The tasks only the old space has, as disjoint non-empty boxes.
    pub removed: Vec<TaskBox>,
}

/// Why [`task_delta`] returned [`TaskDelta::Fallback`]. Each variant with a
/// name names the task parameter.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum FallbackReason {
    /// One side has a parameter space and the other has none.
    SpaceAddedOrRemoved,
    /// A `combination` other than a product of every parameter once.
    NotProduct,
    /// A chunked parameter: its tasks are chunks, not values.
    Chunked(String),
    /// A range lists a value twice, so its tasks are not a set.
    RepeatedValue(String),
    /// A side's task count does not fit a `u64`.
    TaskCountOverflow,
    /// A parameter only the new space has.
    ParameterAdded(String),
    /// A parameter only the old space has.
    ParameterRemoved(String),
    /// A parameter whose type differs between the spaces.
    ParameterRetyped(String),
    /// An `INT` parameter's values, or its kept, added or removed values,
    /// that no range expression holds: more than
    /// [`MAX_RANGE_EXPR_CHUNKS`] runs, or `i64::MIN`.
    RangeTooComplex(String),
}

impl fmt::Display for FallbackReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SpaceAddedOrRemoved => write!(f, "parameter space added or removed"),
            Self::NotProduct => write!(f, "combination is not a product of every parameter"),
            Self::Chunked(n) => write!(f, "task parameter {n} is chunked"),
            Self::RepeatedValue(n) => write!(f, "task parameter {n} repeats a value"),
            Self::TaskCountOverflow => write!(f, "task count does not fit 64 bits"),
            Self::ParameterAdded(n) => write!(f, "task parameter {n} added"),
            Self::ParameterRemoved(n) => write!(f, "task parameter {n} removed"),
            Self::ParameterRetyped(n) => write!(f, "task parameter {n} changed type"),
            Self::RangeTooComplex(n) => {
                write!(
                    f,
                    "task parameter {n} changes in more runs than a range holds"
                )
            }
        }
    }
}

/// The task delta from `old` to `new`, where `None` is a step without a
/// parameter space. See `specs/model/task-delta.md`.
#[must_use]
pub fn task_delta(old: Option<&StepParameterSpace>, new: Option<&StepParameterSpace>) -> TaskDelta {
    if old == new {
        return TaskDelta::Unchanged;
    }
    match product_delta(old, new) {
        Ok(d) => TaskDelta::Product(d),
        Err(reason) => TaskDelta::Fallback { reason },
    }
}

fn product_delta(
    old: Option<&StepParameterSpace>,
    new: Option<&StepParameterSpace>,
) -> Result<ProductDelta, FallbackReason> {
    let (Some(old), Some(new)) = (old, new) else {
        return Err(FallbackReason::SpaceAddedOrRemoved);
    };
    check_product(old)?;
    check_product(new)?;
    let (o, n) = (
        &old.task_parameter_definitions,
        &new.task_parameter_definitions,
    );
    if let Some(name) = n.keys().find(|k| !o.contains_key(*k)) {
        return Err(FallbackReason::ParameterAdded(name.clone()));
    }
    if let Some(name) = o.keys().find(|k| !n.contains_key(*k)) {
        return Err(FallbackReason::ParameterRemoved(name.clone()));
    }
    let dims = n
        .iter()
        .filter_map(|(name, np)| o.get(name).map(|op| dim(name, op, np)))
        .collect::<Result<Vec<_>, _>>()?;

    // With I_k, N_k \ O_k and O_k \ N_k per dimension: kept is I_0 × … and
    // added_k is I_0 × … × I_{k-1} × (N_k \ O_k) × N_{k+1} × …, so box k
    // holds the tasks whose first value outside the old space is in
    // dimension k. Removed is the same with the sides swapped.
    Ok(ProductDelta {
        kept: make_box(dims.iter().map(|d| (d.name, &d.kept))),
        added: side_boxes(&dims, |d| (&d.added, d.new)),
        removed: side_boxes(&dims, |d| (&d.removed, d.old)),
    })
}

/// The non-empty boxes `k = 0, 1, …` of one side, where `pick` gives a
/// dimension's values only that side has and all of that side's values.
fn side_boxes<'a>(
    dims: &'a [Dim<'a>],
    pick: impl Fn(&'a Dim<'a>) -> (&'a TaskParameter, &'a TaskParameter),
) -> Vec<TaskBox> {
    (0..dims.len())
        .map(|k| {
            make_box(dims.iter().enumerate().map(|(j, d)| {
                let (only, full) = pick(d);
                let p = match j.cmp(&k) {
                    std::cmp::Ordering::Less => &d.kept,
                    std::cmp::Ordering::Equal => only,
                    std::cmp::Ordering::Greater => full,
                };
                (d.name, p)
            }))
        })
        .filter(|b| !b.is_empty())
        .collect()
}

/// One parameter of both spaces, with the values both sides have, and the
/// values only the new and only the old side have.
struct Dim<'a> {
    name: &'a str,
    old: &'a TaskParameter,
    new: &'a TaskParameter,
    kept: TaskParameter,
    added: TaskParameter,
    removed: TaskParameter,
}

fn dim<'a>(
    name: &'a str,
    old: &'a TaskParameter,
    new: &'a TaskParameter,
) -> Result<Dim<'a>, FallbackReason> {
    use TaskParameter as P;
    let (kept, added, removed) = match (old, new) {
        (P::Int { range: a, .. }, P::Int { range: b, .. }) => {
            let too_complex = || FallbackReason::RangeTooComplex(name.to_string());
            let (a, b) = (
                int_runs(a).ok_or_else(too_complex)?,
                int_runs(b).ok_or_else(too_complex)?,
            );
            let int = |runs: Option<Vec<IntRange>>| {
                runs.and_then(int_range)
                    .map(|range| P::Int {
                        range,
                        chunks: None,
                    })
                    .ok_or_else(too_complex)
            };
            (
                int(intersect(&b, &a))?,
                int(subtract(&b, &a))?,
                int(subtract(&a, &b))?,
            )
        }
        (P::Float { range: a }, P::Float { range: b }) => {
            let (k, ad, r) = split(a, b, FloatKey);
            (
                P::Float { range: k },
                P::Float { range: ad },
                P::Float { range: r },
            )
        }
        (P::String { range: a }, P::String { range: b }) => {
            let (k, ad, r) = split(a, b, String::as_str);
            (
                P::String { range: k },
                P::String { range: ad },
                P::String { range: r },
            )
        }
        (P::Path { range: a }, P::Path { range: b }) => {
            let (k, ad, r) = split(a, b, String::as_str);
            (
                P::Path { range: k },
                P::Path { range: ad },
                P::Path { range: r },
            )
        }
        _ => return Err(FallbackReason::ParameterRetyped(name.to_string())),
    };
    Ok(Dim {
        name,
        old,
        new,
        kept,
        added,
        removed,
    })
}

/// Kept, added and removed values of a list dimension: the new values the
/// old side has, the new values it lacks, and the old values the new side
/// lacks, each in its side's order.
fn split<'a, T: Clone, K: Eq + Hash>(
    old: &'a [T],
    new: &'a [T],
    key: impl Fn(&'a T) -> K,
) -> (Vec<T>, Vec<T>, Vec<T>) {
    let old_keys: HashSet<K> = old.iter().map(&key).collect();
    let new_keys: HashSet<K> = new.iter().map(&key).collect();
    let (kept, added): (Vec<&T>, Vec<&T>) = new.iter().partition(|v| old_keys.contains(&key(v)));
    let removed = old.iter().filter(|v| !new_keys.contains(&key(v)));
    (
        kept.into_iter().cloned().collect(),
        added.into_iter().cloned().collect(),
        removed.cloned().collect(),
    )
}

/// A float compared as `Float64` compares. `Float64` never holds a NaN, so
/// that comparison is an equivalence.
#[derive(PartialEq, Hash)]
struct FloatKey<'a>(&'a Float64);

impl Eq for FloatKey<'_> {}

/// Whether `space` is a plain product with a `u64` task count over distinct
/// values; the reason it is not otherwise.
fn check_product(space: &StepParameterSpace) -> Result<(), FallbackReason> {
    let defs = &space.task_parameter_definitions;
    if let Some(c) = space.combination.as_deref() {
        if c.trim() != "*" {
            let names: Vec<&str> = c.split('*').map(str::trim).collect();
            let unique: HashSet<&str> = names.iter().copied().collect();
            if names.len() != defs.len()
                || unique.len() != names.len()
                || !names.iter().all(|n| defs.contains_key(*n))
            {
                return Err(FallbackReason::NotProduct);
            }
        }
    }
    let mut count = 1u64;
    for (name, p) in defs {
        let distinct = match p {
            TaskParameter::Int {
                chunks: Some(_), ..
            }
            | TaskParameter::ChunkInt { .. } => {
                return Err(FallbackReason::Chunked(name.clone()));
            }
            TaskParameter::Int {
                range: TaskParamRange::RangeExpr(_),
                ..
            } => true,
            TaskParameter::Int {
                range: TaskParamRange::List(v),
                ..
            } => all_distinct(v.iter()),
            TaskParameter::Float { range } => all_distinct(range.iter().map(FloatKey)),
            TaskParameter::String { range } | TaskParameter::Path { range } => {
                all_distinct(range.iter())
            }
        };
        if !distinct {
            return Err(FallbackReason::RepeatedValue(name.clone()));
        }
        count = count
            .checked_mul(values_len(p))
            .ok_or(FallbackReason::TaskCountOverflow)?;
    }
    Ok(())
}

fn all_distinct<T: Eq + Hash>(values: impl ExactSizeIterator<Item = T>) -> bool {
    let n = values.len();
    values.collect::<HashSet<T>>().len() == n
}

fn values_len(p: &TaskParameter) -> u64 {
    let n = match p {
        TaskParameter::Int { range, .. } | TaskParameter::ChunkInt { range, .. } => match range {
            TaskParamRange::RangeExpr(r) => return r.len(),
            TaskParamRange::List(v) => v.len(),
        },
        TaskParameter::Float { range } => range.len(),
        TaskParameter::String { range } | TaskParameter::Path { range } => range.len(),
    };
    u64::try_from(n).unwrap_or(u64::MAX)
}

fn make_box<'a>(dims: impl Iterator<Item = (&'a str, &'a TaskParameter)>) -> TaskBox {
    let defs: IndexMap<String, TaskParameter> =
        dims.map(|(n, p)| (n.to_string(), p.clone())).collect();
    // Each box is a subset of one side, whose count fits a u64.
    let len = defs.values().map(values_len).fold(1, u64::saturating_mul);
    TaskBox {
        space: StepParameterSpace {
            task_parameter_definitions: defs,
            combination: None,
        },
        len,
    }
}

// INT values are sets of runs, the ascending arithmetic progressions of a
// range expression's canonical form, so a range costs its run count and not
// its value count.

/// The runs of an `INT` range; `None` when a list has no range expression.
fn int_runs(range: &TaskParamRange<i64>) -> Option<Vec<IntRange>> {
    match range {
        TaskParamRange::RangeExpr(r) => Some(r.ranges().to_vec()),
        TaskParamRange::List(v) => RangeExpr::from_values(v.clone())
            .ok()
            .map(|r| r.ranges().to_vec()),
    }
}

/// Runs as a range: a range expression, or an empty list, which no range
/// expression is.
fn int_range(runs: Vec<IntRange>) -> Option<TaskParamRange<i64>> {
    if runs.is_empty() {
        return Some(TaskParamRange::List(Vec::new()));
    }
    RangeExpr::from_ranges(runs)
        .ok()
        .map(TaskParamRange::RangeExpr)
}

/// The values both run lists have, as ascending runs.
fn intersect(a: &[IntRange], b: &[IntRange]) -> Option<Vec<IntRange>> {
    let mut out = Vec::new();
    let (mut i, mut j) = (0, 0);
    while let (Some(x), Some(y)) = (a.get(i), b.get(j)) {
        if let Some(r) = intersect_run(x, y)? {
            out.push(r);
        }
        if x.end() < y.end() {
            i += 1;
        } else {
            j += 1;
        }
    }
    Some(out)
}

/// The values of `a` that `b` lacks, as ascending runs; `None` past
/// [`MAX_RANGE_EXPR_CHUNKS`] runs, so a fragmented result is never built.
fn subtract(a: &[IntRange], b: &[IntRange]) -> Option<Vec<IntRange>> {
    let mut out = Vec::new();
    let mut j = 0;
    for x in a {
        // Runs of b that end before x can meet no later run of a.
        while b.get(j).is_some_and(|y| y.end() < x.start()) {
            j += 1;
        }
        let step = x.step();
        // The next value of x not yet emitted or removed; `None` past i64.
        let mut cursor = Some(x.start());
        for y in b.get(j..).unwrap_or_default() {
            if y.start() > x.end() {
                break;
            }
            // `common` is on x's lattice with a step that is a multiple of
            // x's, so the values of x before it and between its values are
            // runs of x's step. Each gap is a non-empty run, so `push_run`
            // bounds the loop.
            let (Some(c), Some(common)) = (cursor, intersect_run(x, y)?) else {
                continue;
            };
            if let Some(end) = common.start().checked_sub(step) {
                push_run(&mut out, c, end, step)?;
            }
            if common.step() != step {
                let mut v = common.start();
                while v < common.end() {
                    push_run(&mut out, v + step, v + common.step() - step, step)?;
                    v += common.step();
                }
            }
            cursor = common.end().checked_add(step);
        }
        if let Some(c) = cursor {
            push_run(&mut out, c, x.end(), step)?;
        }
    }
    Some(out)
}

/// Append the run `start..=end` unless it is empty; `None` past
/// [`MAX_RANGE_EXPR_CHUNKS`] runs.
fn push_run(out: &mut Vec<IntRange>, start: i64, end: i64, step: i64) -> Option<()> {
    if start <= end {
        if out.len() >= MAX_RANGE_EXPR_CHUNKS {
            return None;
        }
        out.push(IntRange::new(start, end, step).ok()?);
    }
    Some(())
}

/// The values two runs share: `Ok(None)` for none, `None` when the result
/// does not fit an `IntRange`.
fn intersect_run(a: &IntRange, b: &IntRange) -> Option<Option<IntRange>> {
    let (lo, hi) = (a.start().max(b.start()), a.end().min(b.end()));
    if lo > hi {
        return Some(None);
    }
    let (s1, s2) = (i128::from(a.step()), i128::from(b.step()));
    let (r1, r2) = (i128::from(a.start()), i128::from(b.start()));
    // x ≡ r1 (mod s1) and x ≡ r2 (mod s2): solvable when gcd divides r2 - r1,
    // and then x0 = r1 + s1·t with t = ((r2 - r1)/g)·inv(s1/g) mod (s2/g).
    let g = gcd(s1, s2);
    if (r2 - r1) % g != 0 {
        return Some(None);
    }
    let m = s2 / g;
    let t = ((r2 - r1) / g).rem_euclid(m) * inverse(s1 / g, m) % m;
    let lcm = s1 / g * s2;
    let (lo, hi) = (i128::from(lo), i128::from(hi));
    let first = lo + (r1 + s1 * t - lo).rem_euclid(lcm);
    if first > hi {
        return Some(None);
    }
    let last = first + (hi - first) / lcm * lcm;
    let step = if first == last { 1 } else { lcm };
    let narrow = |v: i128| i64::try_from(v).ok();
    Some(Some(
        IntRange::new(narrow(first)?, narrow(last)?, narrow(step)?).ok()?,
    ))
}

fn gcd(mut a: i128, mut b: i128) -> i128 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}

/// The inverse of `a` modulo `m`, for coprime `a` and `m > 0`.
fn inverse(a: i128, m: i128) -> i128 {
    let (mut r0, mut r1) = (a.rem_euclid(m), m);
    let (mut x0, mut x1) = (1i128, 0i128);
    while r1 != 0 {
        let q = r0 / r1;
        (r0, r1) = (r1, r0 - q * r1);
        (x0, x1) = (x1, x0 - q * x1);
    }
    x0.rem_euclid(m)
}
