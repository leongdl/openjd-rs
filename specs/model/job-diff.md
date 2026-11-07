# Job Diff (experimental)

**Status: experimental prototype.** `job::diff` may change without notice.

`job::diff` compares two instantiated jobs, steps, or environments and returns a
`DiffTree`: an arena of `DiffNode`s in breadth-first order, node 0 the root.

## Entry points

```rust
pub fn diff_jobs(old: &Job, new: &Job, options: &DiffOptions) -> DiffTree;
pub fn diff_steps(old: &Step, new: &Step, options: &DiffOptions) -> DiffTree;
pub fn diff_environments(old: &Environment, new: &Environment, options: &DiffOptions) -> DiffTree;
```

Two environment templates are diffed by converting each with `convert_environment` and
calling `diff_environments`.

## Algorithm

1. Unless `with_resolved_symtab(true)`, clear every `resolved_symtab` on both sides. The
   step-scope table holds every `Param.*`, so comparing it marks every step modified when
   any job parameter changes.
2. Push the root pair. For each pair, the node's `DiffStatus` is decided **when the node is
   created**, by one `PartialEq` of the whole subtree: `Unchanged`, `Modified`, `Added`
   (new side only), or `Removed` (old side only).
3. Pop pairs from a FIFO queue. Expanding a `Modified` node records its own differing
   scalar fields as `FieldChange`s and creates one child node per child pair.
4. A child is queued only if it is `Modified` and its depth is below `max_depth`. An
   `Unchanged`, `Added` or `Removed` node is never expanded. A `Modified` node at the
   depth limit has `expanded == false`.

Because status is decided before expansion, the root answers "any delta?" in O(1) after
the walk, every step node answers "delta in this step?", and `with_max_depth(1)` reports
every step's status without expanding any of them.

Cost: each node's equality check walks its subtree, so the walk is O(N·D) for N model
nodes and depth D (D ≤ 5). Unchanged subtrees cost one equality check each and are never
descended.

## Matching

| collection | matched by | order recorded as |
|---|---|---|
| `steps`, `jobEnvironments`, `stepEnvironments`, `embeddedFiles` | `name` | `<collection>.order` field change |
| `parameters`, `taskParameterDefinitions` | map key | not a change (`IndexMap` equality is order-insensitive) |
| `variables` | map key | not a change; entries are `variables.<KEY>` |
| `hostRequirements.amounts` / `attributes` | `name` | `<collection>` field change; entries are `<collection>[<name>]` |
| `dependencies` | set of `dependsOn` | `dependencies.order` |

Children are ordered old-side first, then new-only entries. With duplicate names (not
possible in a validated job) the first occurrence wins.

## Tree shape

| node kind | segment from parent | own fields | children |
|---|---|---|---|
| Job | (root) | `name`, `description`, `extensions`, `steps.order`, `jobEnvironments.order` | `parameters[K]`, `jobEnvironments[N]`, `steps[N]` |
| JobParameter | `parameters[K]` | `name`, `type`, `value` | — |
| Step | `steps[N]` | `name`, `description`, `dependencies`, `stepEnvironments.order`, `resolvedSymTab` | `script`, `stepEnvironments[N]`, `parameterSpace`, `hostRequirements` |
| StepScript | `script` | `let`, `embeddedFiles.order` | `onRun`, `embeddedFiles[N]` |
| Environment | `jobEnvironments[N]` / `stepEnvironments[N]` | `name`, `description`, `variables.K`, `resolvedSymTab` | `script` |
| EnvironmentScript | `script` | `let`, `embeddedFiles.order` | `onEnter`, `onWrapEnvEnter`, `onWrapTaskRun`, `onWrapEnvExit`, `onExit`, `embeddedFiles[N]` |
| Action | `onRun`, `onEnter`, ... | `command`, `args`, `timeout`, `cancelation` | — |
| EmbeddedFile | `embeddedFiles[N]` | `name`, `type`, `filename`, `data`, `runnable`, `endOfLine` | — |
| ParameterSpace | `parameterSpace` | `combination` | `taskParameterDefinitions[K]` |
| TaskParameter | `taskParameterDefinitions[K]` | `definition` (whole value) | — |
| HostRequirements | `hostRequirements` | `amounts[N]`, `attributes[N]` | — |

Each `expand_*` destructures its model type exhaustively, so adding a model field fails
to compile until the diff handles it.

Tasks are not enumerated. A step's tasks change exactly when its `parameterSpace` node
does.

`DiffTree::find` takes a dotted path of segments (`steps[Render].script.onRun`), so it
cannot address a name that contains `.`; walk with `DiffTree::child` instead.

Tests: `crates/openjd-model/src/test_job_diff.rs`.
