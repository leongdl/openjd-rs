# Task Delta

**Status: unstable.** `job::task_delta` may change or be removed in any release without
notice.

`job::task_delta` says which tasks a change to a step's parameter space keeps, adds and
removes, without enumerating the tasks. A system that applies a change to a running job
keeps the state of the kept tasks, such as which have run, and creates and deletes only
the others. `job::apply` attaches the delta to a step's op ([job-apply.md](job-apply.md)).

```rust
pub fn task_delta(old: Option<&StepParameterSpace>, new: Option<&StepParameterSpace>) -> TaskDelta;
```

`None` is a step without a parameter space. A task is identified by its parameter values:
the specification gives a task no other identity, and no order beyond the order its space
iterates in.

## Result

| `TaskDelta` | when |
|---|---|
| `Unchanged` | the spaces are equal (`PartialEq`) |
| `Product(ProductDelta { kept, added, removed })` | both sides are plain products over the same parameters |
| `Fallback { reason }` | otherwise: every old task is replaced by the new space's tasks |

A side is a plain product when its `combination` is absent, `*`, or every parameter name
exactly once joined by `*`; no parameter is chunked (`CHUNK[INT]`, or `INT` with
`chunks`); no range lists a value twice, since the algebra is over sets; and its task count
fits a `u64`, as `StepParameterSpaceIterator::len` counts it. The two sides must have the
same parameter names with the same types. The first rule a side breaks, the old side
first, is the `FallbackReason`.

Order is not part of the delta. A different dimension order, such as a `combination` of
`B * A` for `A * B` or reordered definitions, or a different order of the values both
sides share, keeps every task, because the tasks are the same parameter value sets and
nothing in a plan locates a task by its position. Such a change, like a range expression
rewritten as the list of its values, is a `Product` with no added or removed box.

## Decomposition

For each parameter `k` in the new side's definition order, with old values `O_k`, new
values `N_k`, and `I_k = O_k ∩ N_k`:

```
kept      = I_0 × … × I_{D-1}
added_k   = I_0 × … × I_{k-1} × (N_k \ O_k) × N_{k+1} × … × N_{D-1}
removed_k = I_0 × … × I_{k-1} × (O_k \ N_k) × O_{k+1} × … × O_{D-1}
```

`added_k` holds the new tasks whose first value outside the old space is in dimension `k`,
so the `added_k` are disjoint and their union is `new \ kept`; likewise `removed_k` and
`old \ kept`. A delta has at most `2D + 1` boxes; `added` and `removed` omit the empty
ones, and `kept` may be empty.

A `TaskBox` is a `StepParameterSpace` with no `combination` whose ranges are those value
sets, so `StepParameterSpaceIterator` iterates its tasks, and `TaskBox::len` counts them.
A box's iteration order is not either side's task order. Each value keeps its type and
spelling: `FLOAT` values compare as `Float64` does, so `2.5` and `2.50` are different
values, as their tasks render differently.

## Cost

`FLOAT`, `STRING` and `PATH` values, and an `INT` list, are already in memory, and cost
time linear in their count: about 0.2 s for a 10^6-value `INT` list and 0.75 s for a
10^6-value `STRING` list in a release build. An `INT` range expression is never
enumerated: both sides become the runs of their canonical form (an `INT` list is sorted
into that form first), and `I_k`, `N_k \ O_k` and `O_k \ N_k` are computed run by run.
Two runs intersect in one arithmetic progression, solved by the Chinese remainder theorem,
so changing `1-1000000` to `1-2000000` costs microseconds, as does a range of 10^18
values.

Subtracting a progression whose step is a multiple `m > 1` of the other's leaves gaps, one
run each. When a result would have more than `MAX_RANGE_EXPR_CHUNKS` runs, such as
`1-200000` to `1-200000:2`, the delta stops building it and is
`Fallback(RangeTooComplex)`. So are values no range expression holds (`i64::MIN`).

Tests: `crates/openjd-model/src/test_task_delta.rs` compares each decomposition with
brute-force enumeration of both spaces, both ways, and covers every `FallbackReason`; the
`model_job_gen` fuzz target compares the delta with enumeration on generated spaces.
