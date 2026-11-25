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

`plan` fails only when a job's steps have no topological order, a dependency
on an unknown step or a cycle, with the error `StepDependencyGraph` returns for
it. No order of step ops is correct for such a job, and `create_job` never
produces one.

## Ops

An `ApplyOp` has a `target`, an `action` and a non-empty set of `kinds`.

| target | segment it comes from | actions |
|---|---|---|
| `Job` | the root's own field changes | `Modify` |
| `Parameter(name)` | `parameters[name]` | `Add`, `Modify`, `Remove` |
| `JobEnvironment(name)` | `jobEnvironments[name]` | `Add`, `Modify`, `Remove` |
| `Step(name)` | `steps[name]` | `Add`, `Modify`, `Remove` |
| `StepTasks(name)` | `steps[name]`, and its `parameterSpace` | `Add`, `Replace` |

An element's name is its segment's text between `collection[` and the final
`]`. Step environments get no ops of their own: a step is one unit in a
system, so they travel as kinds of their step's op.

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
   then exist before it gains an edge to them. Each step's tasks op follows its
   step op.
5. Step removes, in reverse topological order of the old job's graph. They come
   last because a step that depended on a removed step has dropped that
   dependency in its modify, and dependents go before the steps they depend on.

The plan does not order the step list. `StepReorder` says the shared steps'
order changed, and an added step's position is its index in the new job.

## Tasks

The plan replaces whole task sets; keeping a step's unchanged tasks needs a
task delta, which this plan does not compute.

- An added step gets `StepTasks(name)` `Add` (`TaskCreate`): create all of its
  tasks.
- A modified step whose `parameterSpace` child changed gets `StepTasks(name)`
  `Replace` (`TaskReplace`): replace all of its tasks with those of its new
  parameter space. When the parameter space is the step's only change, there is
  no step op, and a system writes the step's new parameter space with its
  tasks.
- A removed step's tasks go with it.

## Mutation kinds

Each op's kinds name what it changes, so a system can refuse a plan that needs
a change it does not support. `MutationKind` displays as its variant name.

| kind | derived from |
|---|---|
| `JobMetadata` | root field `name` or `description` |
| `JobExtensions` | root field `extensions` |
| `StepReorder` | root field `steps.order` |
| `EnvReorder` | root field `jobEnvironments.order`, or step field `stepEnvironments.order` |
| `ParameterAdd`, `ParameterRemove`, `ParameterValue` | a `parameters[K]` node `Added`, `Removed`, `Modified` |
| `EnvAdd`, `EnvRemove` | a job or step environment node `Added`, `Removed` |
| `EnvMetadata` | environment field `description` |
| `EnvVariables` | environment field `variables[K]` or `variables` |
| `EnvScript` | environment child `script` |
| `EnvSymbols` | environment field `resolvedSymTab[NAME]` or `resolvedSymTab` |
| `StepAdd`, `StepRemove` | a `steps[N]` node `Added`, `Removed`; the step's environments and tasks go with it |
| `StepMetadata` | step field `description` |
| `StepScript` | step child `script` |
| `StepHostRequirements` | step child `hostRequirements` |
| `StepDependencies` | step field `dependencies` |
| `StepSymbols` | step field `resolvedSymTab[NAME]` or `resolvedSymTab` |
| `TaskCreate`, `TaskReplace` | see "Tasks" |

A step environment's kinds are its step's. A step environment has no symbol
table of its own (see [job-diff.md](job-diff.md) "Resolved symbol tables"), so
a parameter it reads shows as the step's `StepSymbols`. A field the table does
not name, such as `name`, maps to its node's metadata kind, so no change goes
without a kind; `name` differs only on the job, since elements are matched by
name.

## Display

`Display` writes one line per op, numbered from 1: `{n}. {target}: {action}
[{kinds}]`, the kinds comma-separated in declaration order. An empty plan
writes nothing.

```
1. parameter Frames: modify [ParameterValue]
2. step Render: modify [StepScript]
3. tasks of step Render: replace [TaskReplace]
4. step Encode: add [StepAdd]
5. tasks of step Encode: add [TaskCreate]
```

## Out of scope

- A store or applier trait that executes the plan.
- A policy that checks an op's kinds against the ones a system supports.
- Executing, validating against a store, or resuming a plan.
- Which tasks a parameter space change keeps, adds or removes.
- Which changes come from the template and which from the parameters.

Tests: `crates/openjd-model/src/test_job_apply.rs`.
