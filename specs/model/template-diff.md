# Template Diff (experimental)

**Status: experimental prototype.** `job::template_diff` may change without notice.

`job::template_diff` diffs two job templates, or one template under two parameter sets.
It builds both jobs, runs [`diff_jobs`](job-diff.md) on them, adds a *template layer* for
what a built job cannot show, and tags every change with its origin and stage. The result
is one `TemplateDiff`, which can be printed, serialized, compared, or turned into an
[apply plan](job-apply.md). Design: SuperDaveDocs `docs/dag-diff/03-template-diff.md`.
The command-line form is [`openjd diff`](../cli/diff.md).

## Entry points

```rust
pub fn diff_templates(old: &TemplateInput, new: &TemplateInput, options: &DiffOptions) -> Result<TemplateDiff, ModelError>;
pub fn diff_parameter_sets(template: &TemplateInput, old: &JobParameterInputValues, new: &JobParameterInputValues, options: &DiffOptions) -> Result<TemplateDiff, ModelError>;
pub fn diff_built_jobs(old: Job, new: Job, options: &DiffOptions) -> TemplateDiff;
pub fn job_from_str(text: &str) -> Result<Job, ModelError>;
```

- `diff_parameter_sets` lays `old` and `new` over the template input's own parameters (a
  later value wins), decodes the template once and builds it twice. It has no template
  layer, and every change has origin `Parameters`.
- `diff_built_jobs` is for callers that already hold two jobs. It has no template layer,
  and every change has origin `Job`, because the cause is not known.
- `job_from_str` reads a job back from its serde form (JSON when the text starts with `{`,
  YAML otherwise), as `openjd build` prints it. It takes text rather than a
  `serde_json::Value` because a `Value` object sorts its keys, which would reorder task
  parameter definitions and so change task order. Unknown fields are rejected.

## Inputs

`TemplateSource` is owned: `Path(PathBuf)`, `Text(String)`, `Value(serde_json::Value)`,
or `Decoded(Box<JobTemplate>)`. A path is JSON when its extension is `.json` and YAML
otherwise, as in the CLI. Text is JSON when it starts with `{`. Both go through
`document_string_to_object`. `TemplateSource::guess(s)` is `Path` when `s` names an
existing file and `Text` otherwise; the library never guesses on its own.

`TemplateInput` is one side: the source, its parameter values, environment templates, the
extension allowlist, `CallerLimits`, and the two directories for PATH parameters. Its
defaults match `openjd check`: every extension in `ModelExtension::ALL`, default limits,
the template file's directory for relative PATH defaults (the process working directory
for other sources), and the process working directory for relative PATH values.

Each side is built exactly as `openjd summary` and `openjd run` build a job:

1. `decode_job_template` with the allowlist and limits (skipped for `Decoded`, so the
   allowlist is not applied to it), and `decode_environment_template` for each
   environment template;
2. `preprocess_job_parameters` with host path format, no template-directory walk-up, and
   URI path values allowed;
3. `create_job` with `default_validation_context().with_caller_limits(limits)`.

`TemplateInput::build` runs these for one side. A failure in `diff_templates` or
`diff_parameter_sets` names the side: `old template: …` or `new template: …`, and
`the template: …` when the shared template of `diff_parameter_sets` does not decode. The
error keeps its variant when it carries a message (`DecodeValidation`, `Compatibility`,
`UnsupportedSchema`); any other error becomes `DecodeValidation` with its display text.

## The job layer

`diff_jobs(old_job, new_job, options)` runs unchanged and is kept as `TemplateDiff::job`.
Every changed node becomes `Change`s:

| node | changes |
|---|---|
| `Added` / `Removed` | one change with an empty `field`; the other side is `None` |
| `Modified`, not expanded (depth limit) | one change with an empty `field` and both sides |
| `Modified`, expanded | one change per `FieldChange`; a `null` side is `None` (field added or removed) |

Before diffing, both jobs' `parameters` are sorted by name. `create_job` fills them from a
`HashMap`, so their order varies between runs, and parameter order is not a change.
Without the sort, two diffs of the same inputs would build trees with nodes in different
orders.

## The template layer

What the built job cannot show, compared on the decoded templates:

| path | field | compared |
|---|---|---|
| (root) | `specificationVersion` | the string |
| (root) | `extensions` | the list as written |
| `parameterDefinitions[P]` | `type`, `description`, `default`, `allowedValues`, `minValue`, `maxValue`, `minLength`, `maxLength`, `objectType`, `dataFlow`, `item.*`, `item.item.*`, `userInterface.*` | field by field; numbers by value, so `default: 5` and `default: "5"` are equal |
| `parameterDefinitions[P]` | (empty) | a definition added or removed, shown as its fields |
| `steps[S]` | `let` | the step-level `let` text, for steps on both sides |

Definitions are matched by name. `extensions` is also a field of the built job, so both
layers report it; `Display` prints the line once.

**Deferred**, and recorded here so they are not mistaken for gaps:

- sugar form (`bash:` versus `script`);
- the source format strings of resolved fields (`name`, host requirements, ranges);
- a render check that says whether a changed format string renders differently;
- `diff-step` (a step template in the context of its job) and `diff-space` (two
  parameter spaces alone).

## Origin

Each change says where it came from:

| origin | meaning |
|---|---|
| `Template` | the template changed; the parameter values alone would not change it |
| `Parameters` | the parameter values changed; the template alone would not change it |
| `Both` | both, or the split could not be computed |
| `Job` | the inputs were built jobs (`diff_built_jobs`) |

Template-layer changes are always `Template`. For job-layer changes in `diff_templates`:

1. Parameter inputs equal (`==` on the two `JobParameterInputValues`): every change is
   `Template`.
2. Otherwise, templates equal: every change is `Parameters`. "Equal" compares the parsed
   documents of the template and its environment templates, the allowlist, and the two
   PATH directories. A `Decoded` source has no document, so it never takes this shortcut;
   step 3 gives the same answer at the cost of one more build.
3. Otherwise build `mid`, the new template with the old parameters. A change whose
   `(path, field)` appears in `diff_jobs(old, mid)` only is `Template`, in
   `diff_jobs(mid, new)` only is `Parameters`, and in both or neither is `Both`. The two
   extra diffs use the caller's options, so the keys line up with the main diff.
4. If `mid` does not build (the new template needs a parameter the old set lacks, for
   example), every job-layer change is `Both` and `TemplateDiff::notes` says why.

Comparing parameter values alone would get a template default change wrong: if `Frames`
is not supplied and its `default` changes, the value changes but the cause is the template
(rule 1 gives `Template`).

## Stage

The earliest generic OpenJD stage a change shows at. openjd-rs has no knowledge of any
store; a service maps the stage and path to its own records.

| stage | rule |
|---|---|
| `Template` | the change is in the template layer |
| `Parameters` | the path starts with `parameters[` |
| `RunTime` | the field is a format string (`command`, `args`, `timeout`, `cancelation`, `data`, `filename`, `let`, `variables.*`) and its before or after text references `Task.` or `Session.`, inside `{{ }}` or, for `let`, anywhere |
| `Job` | everything else |

A PATH parameter change shows in `resolvedSymTab` as `RawParam.P` and is stage `Job`; PATH
mapping itself happens on the worker.

## Steps and tasks

`TemplateDiff::steps` has one `StepSummary` per step of either job, in old order and then
new-only steps. `status` is the step's status under the caller's options (one
`diff_steps` at depth 0), so it does not depend on `max_depth`. `tasks` comes from
`task_delta` on the two parameter spaces:

| `TaskDelta` | `TaskSummary` |
|---|---|
| `Unchanged` | `Unchanged { count }` |
| `Product(d)` | `Product { kept, added, removed }`, each a `BoxSummary` |
| `Fallback { reason }` | `Replaced { old_count, new_count, reason }` |
| step added or removed | `Added { count }` / `Removed { count }` |

Counts are `u64` and count what `StepParameterSpaceIterator` yields (chunks, for a chunked
step). A count that cannot be computed is 0, with a note. A `BoxSummary` has each
dimension's name and its values: integer runs as range expressions (`1-100`, `1-9:2`),
other values comma-separated.

`DiffStat` (`TemplateDiff::stat`) counts steps changed, added and removed, and tasks kept,
added, removed and replaced over all steps. Kept tasks include unchanged steps' tasks.
`tasks_replaced` counts the new tasks of `Replaced` steps; their old tasks are not
counted as removed.

## Result types

```rust
pub struct TemplateDiff { pub changes: Vec<Change>, pub steps: Vec<StepSummary>, pub job: DiffTree,
                          pub old_job: Job, pub new_job: Job, pub notes: Vec<String> }
pub struct Change { pub layer: Layer, pub path: String, pub field: String,
                    pub before: Option<Shown>, pub after: Option<Shown>, pub origin: Origin, pub stage: Stage }
pub struct Shown { pub raw: serde_json::Value, pub text: String }
```

`changes` is sorted by path, field and layer. `path` uses `DiffTree` segments
(`steps[Render].parameterSpace.taskParameterDefinitions[Eye]`); `field` is empty for a
whole node. Helpers: `is_changed`, `changes_at(prefix)` (the prefix and everything below
it, not `steps[Render2]` for `steps[Render]`), `step(name)`, `plan()` (`job::apply::plan`
on the kept jobs), and `stat()`.

Every type derives `Debug`, `Clone`, `PartialEq` (and `Eq` where its fields allow; not
`TemplateDiff`, whose jobs hold floats) and `Serialize` with camelCase field names. Enums
serialize as lowercase camelCase strings (`template`, `parameters`, `both`, `job`,
`runTime`); `TaskSummary` is tagged with `kind`; `StepSummary::status` is `unchanged`,
`added`, `removed` or `modified`; `TemplateDiff::job` serializes as its changed nodes,
`{path, status, expanded, fieldChanges}`.

### Rendering

`Shown::raw` is the value in full. `Shown::text` is for people:

- a task parameter range is its range expression (`1-100`, `1-9:2`) or a list
  (`[left, right]`); a chunked one adds `chunks of N`;
- a job parameter value is bare (`1-200`, not `{"String":"1-200"}`), and its type is the
  spec name (`INT`);
- a symbol table is `name=value` pairs;
- other strings are bare, lists are `[a, b]`, objects are `{key: value}`;
- when both sides render the same text but differ (`"3"` and `3`), both show as JSON;
- text longer than 120 characters is cut to 119 and ends with `…`.

`Display` is deterministic, so tests assert whole outputs:

```text
~ parameter Frames: 1-100 -> 1-200  [parameters]
~ step Render
    ~ Eye: [left] -> [left, right]  [template]
    ~ Frame: 1-100 -> 1-200  [parameters]
    tasks: 100 -> 400
      keep 100   Frame 1-100 × Eye left
      add  200   Frame 101-200 × Eye left, right
      add  100   Frame 1-100 × Eye right

1 step changed. Tasks: 100 kept, 300 added, 0 removed.
```

- Job-level changes come first, then each step that changed or has a change under it,
  with its changes relative to the step and its task summary, then the `DiffStat` line.
  Nothing changed prints `no changes`.
- `~` is changed, `+` added, `-` removed. Every line ends with the origin tag.
- Labels: `parameter P` (`parameters[P]`), `parameter definition P.field`, a task
  parameter definition by its name, other paths relative to the step.
- `Change` alone prints `~ path.field: before -> after  [origin]`.
- `StepSummary` prints the step header and its task summary; `stat_line` prints
  `~ Render  tasks 100 -> 400  (+300 -0)`.
- `DiffStat` prints `N step(s) changed[, A added][, R removed]. Tasks: K kept, A added,
  R removed[, P replaced].`

## Examples

The samples are in `crates/openjd-model/examples/template_diff/` and are the cases of the
design's §7.4:

| case | inputs | result |
|---|---|---|
| 1 | `render_v1.yaml`, `Frames` 1-100 → 1-200 | `parameters[Frames]` and `Frame` [parameters]; keep 100, add 100 |
| 2 | `render_v1.yaml` → `render_v2.yaml` (`Eye` gains `right`) | `Eye` [template]; keep 100, add 100 |
| 3 | both | mixed origins; keep 100, add 300 |
| 4 | `render_no_eye.yaml` → `render_v1.yaml` | `+ Eye`, `args` (stage run time); every task replaced |
| 5 | `Scene` /a → /b | `parameters[Scene]` and `resolvedSymTab` `RawParam.Scene` (stage job) |
| A4 | `render_v1.yaml` → `render_default_1_200.yaml`, `Frames` not supplied | `parameterDefinitions[Frames].default` and the job changes, all [template] |

Tests: `crates/openjd-model/tests/integration/test_template_diff.rs`.
