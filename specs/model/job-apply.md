# Job Apply (unstable)

**Unstable.** This API may change or be removed in any release without notice. Don't
depend on it outside openjd-rs. Applies to `job::apply`.

`job::apply` turns a [job diff](job-diff.md) into an ordered, gated list of writes against
a store that it knows only through a trait.

## Stages

```rust
pub fn plan<'a>(old: &'a Job, new: &'a Job) -> Result<ApplyPlan<'a>, ModelError>;
pub fn validate<A: JobApplier + ?Sized>(plan: &ApplyPlan<'_>, policy: &dyn MutationPolicy, store: &mut A) -> Result<(), ApplyError<A::Error>>;
pub fn execute<A: JobApplier + ?Sized>(plan: &ApplyPlan<'_>, store: &mut A, resume_from: usize) -> Result<ApplyReport, ApplyError<A::Error>>;
pub fn apply<A: JobApplier + ?Sized>(old: &Job, new: &Job, policy: &dyn MutationPolicy, store: &mut A) -> Result<ApplyReport, ApplyError<A::Error>>;
```

`apply` is `plan`, then `validate`, then `execute(…, 0)`. `A: ?Sized` and an object-safe
`JobApplier` let a caller pass `&mut dyn JobApplier<Error = E>` (see
[Host adaptors](#host-adaptors)).

`plan` diffs the two jobs itself, always with the default `DiffOptions`, and keeps the
tree in `ApplyPlan::diff`. `Op::Modify::node` indexes that tree. A caller's tree built
with `max_depth` or `with_resolved_symtab(false)` can mark a changed step `Unchanged`, so
the plan never takes one.

`plan` refuses, with `ModelError::ModelValidation`, a job that repeats a step name, a job
environment name, or a step environment name within one step. Template validation rejects
these, so only a hand-built `Job` has one. The diff reports the copies as a single
`<collection>.duplicates` field change, which has no op and no kind of its own to gate.

## Plan order

1. `Action::Job`: one `Modify` when the root has its own field changes (name,
   description, extensions, step or job-environment order).
2. `Action::Parameter`: one op per changed parameter.
3. `Action::Environment(index, op)`: one op per changed **job** environment.
   - Removes come first. They are matched by name; the index is the old one and is
     informational.
   - Adds and modifies follow in ascending new index. Inserting each `Add` at its index
     after the removes, and after any `Job` op's `EnvReorder` of the shared environments,
     rebuilds the new list order.

   Step environments travel inside their step's op, because a step is one item in a
   store.
4. Steps in **topological order of the new step graph**, origins first:
   - `Added`: `Step(j, Add)`, then `Tasks { change: All }`.
   - `Modified`: `Step(j, Modify)` unless the only change is the parameter space, then
     `Tasks` when the task set or task order changed (`task_delta`: `Product` with
     added or removed boxes → `Boxes`, `Fallback` → `Replace`). A change of dimension
     order is a `Fallback`, because every kept task moves. If the space changed but the
     tasks and their order didn't (e.g. `1-2` rewritten as `[1, 2]`), there is no
     `Tasks` op. The `Step(j, Modify)` is emitted with `StepParameterSpace`, so the store
     still writes the new definition.
5. Removed steps in **reverse topological order of the old step graph**, dependents first.

Every step op names its step with a `StepRef { name, index }`; `j` and `i` above are its
`index`. The name is the step's name in the job the index points into, so a store keyed by
name never resolves the index. The index in `Step(j, Add)` is the step's final position,
`new.steps[j]`. Adds arrive in
topological order, not index order, so `j` can be past the end of the store's list at the
time (`[A]` to `[B (dependsOn C), A, C]` adds C at 2, then B at 0). A store that keeps step
rows in order places the added steps by `j` once the plan is done, in ascending `j`, after
the removes and after any `Job` op's `StepReorder` of the shared steps. `Tasks { step: j }`
also indexes `new.steps`, and needs no `Step` op before it: a store resolves `j` from the
new job, as it does for `Step(j, …)`.

Adds and modifies come before removes. A step therefore exists before any step gains an
edge to it, and by the time a step is removed nothing still depends on it, because the
new job can't name it. `StepIndex` is `DiffNode::step_index`: `new.steps[j]` for steps 4,
and `old.steps[i]` for steps 5.

The planner classifies each modified step and job environment with a full
`diff_steps` / `diff_environments`.

A task-definition reorder under an explicit `combination` produces no op: the
combination fixes task order, and the diff reports such a step `Unchanged`. A store
that persists definition order verbatim keeps the old order. `combination: "*"` is not
explicit: it is the product in definition order, so there a reorder is a `Replace`.

A float parameter whose literal changes form only (`1.0` to `1.00`) compares equal, so it
plans no `ParameterValue` op and the store keeps the old spelling. A step or environment
that reads the parameter still gets its symbol-table kind (see [job diff](job-diff.md)).

## Mutation kinds

Each `PlannedOp` carries a `BTreeSet<MutationKind>`, and a `MutationPolicy` must allow
every one. `AllowList` is the default policy.

| kind | from |
|---|---|
| `JobMetadata`, `JobExtensions`, `StepReorder`, `EnvReorder` | root field changes; `EnvReorder` also from a step's `stepEnvironments` order |
| `ParameterValue`, `ParameterAdd`, `ParameterRemove` | parameter node status |
| `EnvVariables`, `EnvScript`, `EnvMetadata`, `EnvSymbols`, `EnvAdd`, `EnvRemove` | environment fields, `script` child, status (job or step scope) |
| `StepMetadata`, `StepScript`, `StepHostRequirements`, `StepDependencies`, `StepSymbols` | step fields and children |
| `StepParameterSpace` | the space changed with the same tasks in the same order |
| `StepAdd`, `StepRemove` | step status; also `EnvAdd` / `EnvRemove` when that step has step environments |
| `TaskAppend`, `TaskRemove`, `TaskReplace` | `TaskChange`: added boxes, removed boxes, or `Replace`; `All` is `TaskAppend` |

A kind's stable name is its variant name (`MutationKind::name`, also `Display`), and
`FromStr` reads it back, exactly and case-sensitively. `MutationKind::ALL` lists every kind.
`AllowList::from_names` builds a policy from names, such as a host's configuration, and
fails with `UnknownMutationKind` (`unknown mutation kind 'Foo'`) on the first name that is
not a kind.

"Render more frames" (a `Frames` parameter change) plans as `ParameterValue` +
`TaskAppend`, because the step's symbol table doesn't hold the symbols its parameter
space uses.

## Validation and execution

- `validate` asks the policy about every op first, then `JobApplier::check` about every
  op. The first refusal returns `ApplyError::Refused` (the policy: not allowed yet) or
  `ApplyError::Vetoed` (the store: not allowed in its current state) with the op's index,
  and nothing has been written.
- `execute` does not validate. `ApplyPlan::ops` is public, so a caller that edits a plan
  after `validate` must validate it again. `apply` runs both.
- `execute` calls `begin`, the trait method for each op from `resume_from` on, then
  `commit`. A failed write returns `ApplyError::Store { op }`. Ops before `op` were
  applied, so `execute(plan, store, op)` resumes, provided the store's writes are
  idempotent.

## Refusals and errors

A `Refusal` names the refused kind, the op's target (`OpTarget`: the job, a parameter or
job environment by name, or a step by name and index) and a reason. `Refusal::new` fills
the target from the op, for a custom policy. It displays as `{target}: {reason}`;
`AllowList`'s reason is `{kind} is not allowed`:

| op | message |
|---|---|
| a `Tasks` op of step `Render` | `step Render: TaskReplace is not allowed` |
| a `Parameter` op | `parameter Frames: ParameterValue is not allowed` |
| an `Environment` op | `job environment Conda: EnvVariables is not allowed` |
| a `Job` op | `job: JobMetadata is not allowed` |

`ApplyError<E>` implements `Display` when `E` does, and `std::error::Error` when `E` is
`Debug + Display`, so `Box<dyn Error + Send + Sync>` (which is not itself an `Error`)
works. `Refused` displays as its refusal, `Vetoed` as `{target}: {error}`, `Store` as
`write {op} failed: {error}`, and `Model` as the model error. `source` is the `Refusal`
or the `ModelError`; a store error appears in the message only, since `E` need not be an
`Error`.

## Task order

A `TaskChange::Boxes` keeps the kept tasks at their old indices. The host gives the new
tasks indices `old.len()`, `old.len() + 1`, … in `ProductDelta::appended` order: the
`added` boxes in turn, each rightmost parameter fastest. Box `k` holds the new tasks whose
first value missing from the old space is in dimension `k` (in iteration order), so the
order groups tasks by that dimension and is not the new space's iteration order. For
`Frame` `1-2` × `Cam` `[a, b]` to `1-3` × `[b, c]`:

| host index | task | box |
|---|---|---|
| 4 | `Frame=3, Cam=b` | 0 |
| 5 | `Frame=3, Cam=c` | 0 |
| 6 | `Frame=1, Cam=c` | 1 |
| 7 | `Frame=2, Cam=c` | 1 |

`ProductDelta::removed` yields each removed task with its old index (its index at job
creation), ascending, merging the `removed` boxes as it goes. Both orders depend only on
the two parameter spaces, so every plan of the same jobs, in any process, yields the same
sequences. `appended_count` and `removed_count` give the sizes without iterating.

## Host adaptors

A host store implements `JobApplier`. A store written in another language does so through
an adaptor: one Rust type that implements the trait by converting each call's arguments to
the host's values and calling the host. The trait is object safe, so the adaptor can be
used as `&mut dyn JobApplier<Error = E>`.

Every payload has a plain-data form that needs nothing outside these types:

| call | argument | plain data |
|---|---|---|
| `check` | `&PlannedOp` | `PlannedOp::as_document`: `{"kinds": [names], "action": "job" \| "parameter" \| "environment" \| "step" \| "tasks", …}` |
| `job`, `parameter`, `environment`, `step` | `Op<'_, T>` | `Op::as_document`: `{"change": "add" \| "modify" \| "remove", "old"?: …, "new"?: …}` |
| `step`, `tasks` | `StepRef` | `Serialize`: `{"name": …, "index": …}` |
| `tasks` | `Option<&StepParameterSpace>` | `Serialize`, the `JobDocument` form |
| `tasks` | `&TaskChange` | `TaskChange::as_document` |

Objects inside an op (`Job`, `Step`, `Environment`, `JobParameter`, through `ToDocument`)
are written as `JobDocument` writes them: `parameters` and environment `variables` sorted
by name, task parameter definitions in definition order. `Op::Modify::node` is left out,
because it indexes the plan's diff. `TaskChange::as_document` is `{"change": "all"}`,
`{"change": "replace"}` (the host enumerates the new space for both, as at job creation),
or

```text
{"change": "boxes", "oldCount": 2, "keptCount": 2, "appendedCount": 2,
 "appended": [{"Frame": {"type": "INT", "value": "3"}}, …], "removedCount": 0, "removed": []}
```

with `appended` in [task order](#task-order) and `removed` as ascending old indices. A task
is written by `task_parameter_set_document`: name to `{type, value}`, the value as text so a
float keeps its literal. The document lists every changed task, so its size is linear in
them.

`as_document` returns a `Serialize` view that borrows the plan. Serializing it straight
into the host's value keeps key order; `to_value` produces a `serde_json::Value`, which
sorts object keys, so there `taskParameterDefinitions` is in name order. Task order is
unaffected: it is `appended`'s list order.

The reference adaptor is `DynStore` in `test_job_apply.rs`. It is used through
`&mut dyn JobApplier<Error = Box<dyn Error + Send + Sync>>`, appends one
`serde_json::Value` per call (`{"call": "step", "step": …, "op": …}`), and forwards the call
to an inner store. The tests replay the oracle cases through it, assert the whole log for
one case, and resume after an injected write failure.

A PyO3 adaptor (in openjd-model-for-python, not here) has the same shape: a struct holding
a `Py<PyAny>`, the Python store object. Each trait method takes the GIL, converts its
arguments with `as_document` (serialized into a `dict`) or `to_value`, and calls the
matching Python method (`store.step(step, op)`). A Python exception becomes the adaptor's
`Error` (a `PyErr`, or a boxed error wrapping it). The binding maps `ApplyError` back to a
Python exception; the variants keep apart what a host treats differently (`Refused` and
`Vetoed` wrote nothing, `Store` wrote the ops before `op`). `execute(plan, store, op)`
resumes after a failed write, as for any store.

## Tests

`crates/openjd-model/src/test_job_apply.rs`. `JobRebuilder` is an in-memory store that,
like a real adapter, maps new step indices to its own ids once. It rejects:
- a step whose dependency is missing;
- the removal of a step that still has dependents;
- an environment insert past the end of the list;
- a modify at the wrong index;
- an added step placed past the end of the list. It places adds by index on `commit`.

The oracle: after `apply(a, b)` the store holds `b`. That covers parameters, steps by
name and in order, and job environments in order. It also covers each step's task-definition order
where that decides task order (no `combination`), and its task list. The task list is
compared in order, except after a `Boxes` op, where appends are expected to land after
the old tasks.
It is run in both directions for the example templates, for parameter-only changes,
for job-environment inserts, moves and deletes, for a symbol-table-only step change,
for a definition reorder, and for step graphs where topological order differs from index
order (adds and removes). Other tests cover plan order and kinds, policy refusal with zero
writes, store veto, resuming after a failed write, repeated names, and `combination: "*"`.
Every kind is produced by some test, and for each one a policy that allows every other kind
in that plan refuses it with no writes. The adaptor tests above cover the dyn path; others
assert each refusal message in full, every kind's name round trip, `AllowList::from_names`,
and that each op's document equals the matching part of the `JobDocument`.
`test_task_delta.rs` checks `appended` and `removed` against enumeration for every product
case.
