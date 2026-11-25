# Job Apply

**Status: unstable.** `job::apply` may change or be removed in any release
without notice.

`job::apply` turns two jobs into an ordered plan of the operations a system
performs to change a running job from the old job to the new one, such as when
a user edits the template or the parameters of a job that has already started.

```rust
pub fn plan(old: &Job, new: &Job) -> Result<ApplyPlan, ModelError>;
```

`plan` diffs the jobs with `diff_jobs` ([job-diff.md](job-diff.md)) and reads
each op from that tree, so the plan has an op for every change the diff
reports. Inputs are what `create_job` produces, as for the diff. The plan
names each target by name and carries no part of either job: a system that
applies it reads the old and new values from the two jobs it already holds.

`plan` fails when a job's steps have no topological order, a dependency on an
unknown step or a cycle, with the error `StepDependencyGraph` returns for it. No
order of step ops is correct for such a job, and `create_job` never produces
one. It also fails, with `ModelError::DecodeValidation`, for a field or child
the diff names that "Mutation kinds" does not map, rather than plan a change
no kind names; a test pins every field [job-diff.md](job-diff.md) "Tree shape"
lists, so this happens only if the diff gains one.

## Ops

An `ApplyOp` has a `target`, an `action` and a set of `kinds`.

| target | segment it comes from | actions |
|---|---|---|
| `Job` | the root's own field changes | `Modify` |
| `Parameter(name)` | `parameters[name]` | `Add`, `Modify`, `Remove` |
| `JobEnvironment(name)` | `jobEnvironments[name]` | `Add`, `Modify`, `Remove` |
| `Step(name)` | `steps[name]` | `Add`, `Modify`, `Remove` |

An element's name is its segment's text between `collection[` and the final
`]`. Step environments and tasks get no ops of their own: a step is one unit
in a system, so they travel with their step's op. An added step's op creates
its tasks and a removed step's op deletes them.

Kinds name what a `Modify` changes inside its target, so a `Modify` of the job,
a job environment or a step has at least one. An `Add` or `Remove` creates or
deletes the whole target, and a parameter is one value, so those ops have no
kinds.

## Order

1. The job op, when the root has field changes.
2. Parameter ops, in the diff's order (by name). Environments and steps read
   parameters, so parameters change first.
3. Job environment ops: removes first, then adds and modifies in the new job's
   order. Inserting each added environment at its index in the new job, after
   the removes and after the job op's `EnvReorder` of the shared ones, gives
   the new list.
4. Step adds and modifies, in the topological order of the new job's step
   dependency graph (`StepDependencyGraph::topo_sorted`). A step's dependencies
   then exist before it gains an edge to them.
5. Step removes, in reverse topological order of the old job's graph. They come
   last because a step that depended on a removed step has dropped that
   dependency in its modify, and dependents go before the steps they depend on.

The plan does not order the step list. `StepReorder` says the shared steps'
order changed, and an added step's position is its index in the new job.

## Mutation kinds

Each `Modify` op's kinds name what it changes, so a system can refuse a plan
that needs a change it does not support. `MutationKind` displays as its variant
name.

| kind | derived from |
|---|---|
| `JobMetadata` | root field `name` or `description` |
| `JobExtensions` | root field `extensions` |
| `StepReorder` | root field `steps.order` |
| `EnvReorder` | root field `jobEnvironments.order`, or step field `stepEnvironments.order` |
| `EnvAdd`, `EnvRemove` | a step's `stepEnvironments[N]` child `Added`, `Removed` |
| `EnvMetadata` | environment field `name` or `description` |
| `EnvVariables` | environment field `variables[K]` or `variables` |
| `EnvScript` | environment child `script` |
| `EnvSymbols` | environment field `resolvedSymTab[NAME]` or `resolvedSymTab` |
| `StepMetadata` | step field `name` or `description` |
| `StepScript` | step child `script` |
| `StepHostRequirements` | step child `hostRequirements` |
| `StepDependencies` | step field `dependencies` |
| `StepSymbols` | step field `resolvedSymTab[NAME]` or `resolvedSymTab` |
| `StepParameterSpace` | step child `parameterSpace`: all of the step's tasks are replaced by those of its new parameter space |

A step environment's kinds are its step's. A step environment has no symbol
table of its own (see [job-diff.md](job-diff.md) "Resolved symbol tables"), so
a parameter it reads shows as the step's `StepSymbols`. An environment's or a
step's `name` never differs, since elements are matched by name.

## Display

`Display` writes one line per op, numbered from 1: `{n}. {target}: {action}`,
then ` [{kinds}]` when the op has kinds, comma-separated in declaration order.
An empty plan writes nothing.

```
1. parameter Frames: modify
2. step Render: modify [StepScript, StepParameterSpace]
3. step Encode: add
```

## Out of scope

- A store or applier trait that executes the plan.
- A policy that checks an op's kinds against the ones a system supports.
- Executing, validating against a store, or resuming a plan.
- Which tasks a parameter space change keeps, adds or removes.
- Which changes come from the template and which from the parameters.

Tests: `crates/openjd-model/src/test_job_apply.rs`.
