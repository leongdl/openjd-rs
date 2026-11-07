# Job Apply (experimental)

**Status: experimental prototype.** `job::apply` may change without notice.

`job::apply` turns a [job diff](job-diff.md) into an ordered, gated list of writes against
a store that it knows only through a trait. Design: SuperDaveDocs
`docs/dag-diff/02-apply-dag-diff.md`.

## Stages

```rust
pub fn plan<'a>(diff: &DiffTree, old: &'a Job, new: &'a Job) -> Result<ApplyPlan<'a>, ModelError>;
pub fn validate<A: JobApplier>(plan: &ApplyPlan<'_>, policy: &dyn MutationPolicy, store: &mut A) -> Result<(), ApplyError<A::Error>>;
pub fn execute<A: JobApplier>(plan: &ApplyPlan<'_>, store: &mut A, resume_from: usize) -> Result<ApplyReport, ApplyError<A::Error>>;
pub fn apply<A: JobApplier>(diff: &DiffTree, old: &Job, new: &Job, policy: &dyn MutationPolicy, store: &mut A) -> Result<ApplyReport, ApplyError<A::Error>>;
```

`apply` is `plan`, then `validate`, then `execute(…, 0)`.

## Plan order

1. `Action::Job`: one `Modify` when the root has its own field changes (name,
   description, extensions, step or job-environment order).
2. `Action::Parameter`: one op per changed parameter.
3. `Action::Environment`: one op per changed **job** environment. Step environments
   travel inside their step's op, because a step is one item in a store.
4. Steps in **topological order of the new step graph**, origins first:
   - `Added`: `Step(j, Add)`, then `Tasks { change: All }`.
   - `Modified`: `Step(j, Modify)` unless the only change is the parameter space, then
     `Tasks` when the task set changed (`task_delta`: `Product` → `Boxes`, `Fallback` →
     `Replace`).
5. Removed steps in **reverse topological order of the old step graph**, dependents first.

Adds and modifies come before removes. A step therefore exists before any step gains an
edge to it, and by the time a step is removed nothing still depends on it, because the
new job can't name it. `StepIndex` is `DiffNode::step_index`: `new.steps[j]` for steps 4,
and `old.steps[i]` for steps 5.

The planner classifies each modified step and job environment with its own full
`diff_steps` / `diff_environments` (default options). So a `max_depth` cut-off on the
caller's diff loses nothing. A root cut off at depth 0 is re-diffed in full.

## Mutation kinds

Each `PlannedOp` carries a `BTreeSet<MutationKind>`, and a `MutationPolicy` must allow
every one. `AllowList` is the default policy.

| kind | from |
|---|---|
| `JobMetadata`, `JobExtensions`, `StepReorder`, `EnvReorder` | root field changes |
| `ParameterValue`, `ParameterAdd`, `ParameterRemove` | parameter node status |
| `EnvVariables`, `EnvScript`, `EnvMetadata`, `EnvSymbols`, `EnvAdd`, `EnvRemove` | environment fields, `script` child, status (job or step scope) |
| `StepMetadata`, `StepScript`, `StepHostRequirements`, `StepDependencies`, `StepSymbols` | step fields and children |
| `StepAdd`, `StepRemove` | step status |
| `TaskAppend`, `TaskRemove`, `TaskReplace` | `TaskChange`: added boxes, removed boxes, or `Replace`; `All` is `TaskAppend` |

"Render more frames" (a `Frames` parameter change) plans as `ParameterValue` +
`TaskAppend`, because the step's symbol table doesn't hold the symbols its parameter
space uses.

## Validation and execution

- `validate` asks the policy about every op first, then `JobApplier::check` about every
  op. The first refusal returns `ApplyError::Refused` or `ApplyError::Store` with the
  op's index, and nothing has been written.
- `execute` calls `begin`, the trait method for each op from `resume_from` on, then
  `commit`. A failed write returns `ApplyError::Store { op }`. Ops before `op` were
  applied, so `execute(plan, store, op)` resumes, provided the store's writes are
  idempotent.

## Tests

`crates/openjd-model/src/test_job_apply.rs`. `JobRebuilder` is an in-memory store that,
like a real adapter, maps new step indices to its own ids once. It also rejects a step
whose dependency is missing, and the removal of a step that still has dependents. The
oracle `apply(diff(a, b), a)` holds `b` (parameters, job environments and steps by
name; each step's task set) for the example templates in both directions, for
parameter-only changes, and for `max_depth` 0, 1 and unlimited. Other tests cover plan
order and kinds, policy refusal with zero writes, store veto, and resuming after a failed
write.
