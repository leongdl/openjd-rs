# Task Delta

**Status: unstable.** `job::task_delta` may change or be removed in any release without
notice.

`job::task_delta` says which tasks a change to a step's parameter space keeps, adds and
removes, without enumerating the tasks. A system that applies a change to a running job
keeps the state of the kept tasks, such as which have run, and creates and deletes only
the others. `job::apply` attaches the delta to a step's op ([job-apply.md](job-apply.md)).

```rust
pub fn task_delta(
    old: Option<&StepParameterSpace>,
    new: Option<&StepParameterSpace>,
) -> Option<TaskDelta>;
```

A `None` argument is a step without a parameter space. A task is identified by its
parameter values: the specification gives a task no other identity. Its position is the
order its space iterates in, which the delta reports but does not use to match tasks.

## Result

The result is `None` when both arguments are `None`, or when the spaces are equal as the
diff compares them ([job-diff.md](job-diff.md) "Order"): `PartialEq`, and the same
definition order where that is the iteration order. Otherwise it is a `TaskDelta`:

| `TaskDelta` | when |
|---|---|
| `Product(ProductDelta { kept, added, removed, reordered })` | both sides are plain products over the same parameters |
| `Fallback { reason }` | otherwise: every old task is replaced by the new space's tasks |

A side is a plain product when its `combination` is absent, `*`, or every parameter name
exactly once joined by `*`; no parameter is chunked (`CHUNK[INT]`, or `INT` with
`chunks`); no range lists a value twice, since the algebra is over sets; and its task count
fits a `u64`, as `StepParameterSpaceIterator::len` counts it. The two sides must have the
same parameter names with the same types. The first rule a side breaks, the old side
first, gives the `FallbackReason`:

| `FallbackReason` | rule |
|---|---|
| `SpaceAddedOrRemoved` | one side has no parameter space |
| `NotProduct` | the `combination`, a chunked parameter, a repeated value, or the task count |
| `ParameterMismatch(name)` | a parameter on one side only, or with another type on each |
| `RangeTooComplex(name)` | an `INT` result no range expression holds (see "Cost") |

## Order

The diff reports a change in task order, and the system applying it decides whether order
matters, so a reorder alone keeps every task. `reordered` is true when the kept tasks
iterate in a different relative order: the outermost-first order of the dimensions with
two or more kept values differs (the `combination`'s order, or the definition order when
it is absent or `*`), or such a dimension lists its kept values in a different relative
order. An `INT` range expression's values ascend; a list's are in list order. With fewer
than two kept tasks there is no order to change.

A change of order only, such as a `combination` of `B * A` for `A * B`, reordered
definitions with no `combination`, or `[3, 1, 2]` for `[1, 2, 3]`, is a `Product` with no
added or removed box and `reordered` true. A range expression rewritten as the list of its
values is a `Product` with nothing added, removed or reordered. A space that is not a
plain product is a `Fallback` whatever changed, order included.

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
time linear in their count: about 0.4 s for a 10^6-value `INT` list and 1 s for a
10^6-value `STRING` list in a release build. An `INT` range expression is never
enumerated: both sides become the runs of their canonical form (an `INT` list is sorted
into that form first), and `I_k`, `N_k \ O_k` and `O_k \ N_k` are computed run by run.
Two runs intersect in one arithmetic progression, solved by the Chinese remainder theorem,
so changing `1-1000000` to `1-2000000` costs microseconds, as does a range of 10^18
values.

A box's `INT` values are a `RangeExpr`, which holds at most `MAX_RANGE_EXPR_CHUNKS` runs,
so a result with more is `Fallback(RangeTooComplex)`, as are values no range expression
holds (`i64::MIN`). Results are built as runs, merging each into the previous one when
they continue one progression, and the delta stops building past the cap. Subtracting a
progression whose step is `m > 1` times the other's leaves the values between its values:
for `m = 2` one progression (`1-200000` minus `1-200000:2` is `2-200000:2`); for `m > 2`,
`m - 1` interleaved progressions, which no range expression holds, so each gap is a run
of its own. `1-300000` minus `1-300000:3` leaves 200000 runs in canonical form, so that
delta is a `Fallback`.

Tests: `crates/openjd-model/src/test_task_delta.rs` compares each decomposition with
brute-force enumeration of both spaces, both ways, covers every `FallbackReason`, and
checks each `RangeTooComplex` past the cap against `RangeExpr::from_values`; the
`model_job_gen` fuzz target compares the delta with enumeration on generated spaces.
