# Host Support (unstable)

**Unstable.** This API may change or be removed in any release without notice. Don't
depend on it outside openjd-rs. Applies to the host support items in `job::template_diff`.

A job store (a host) keeps a job as its own rows and bodies, and applies changes through a
[`JobApplier`](job-apply.md). These functions answer the template questions such a host
has beside the plan, so it does not re-implement template knowledge. openjd-rs owns
decoding, parameters, building, symbol tables and task order; the host owns ids, rows,
bodies and execution.

```rust
pub fn template_step_fragment(document: &serde_json::Value, step: &str) -> Result<TemplateStepFragment, ModelError>;
pub fn template_job_environment_fragment(document: &serde_json::Value, name: &str) -> Result<serde_json::Value, ModelError>;
pub fn build_job_symbol_table(job: &Job) -> Result<SerializedSymbolTable, ModelError>;
pub fn job_digest(job: &Job) -> Result<String, ModelError>;
pub fn merge_parameters(stored: &JobParameterValues, requested: &JobParameterInputValues,
    new: &TemplateInput) -> Result<JobParameterValues, ModelError>;
impl TemplateInput { pub fn preprocessed(self, values: JobParameterValues) -> Self; }
```

They live in `job::template_diff` (`template_diff/host.rs`), beside `TemplateInput` and
`JobDocument`, which they build on.

## Host encoding

Every document a host receives writes a job the same way: `JobDocument` (what `openjd
build` prints and `job_from_str` reads), the op documents (`Op::as_document`,
`PlannedOp::as_document`, `ToDocument`), and the task change documents. The code is
`job/document.rs`.

A typed value is `{"type": <spec type name>, "value": <transport value>}`. The type is the
template's name for it (`INT`, `PATH`, `LIST[STRING]`, `CHUNK[INT]`). The value is the
transport form a symbol table uses: a scalar as text, so a float keeps its literal, and a
list as an array of those. A `RANGE_EXPR` value is its canonical range expression. Nothing
internal to the Rust types is written: no cached sizes, type descriptors, Rust variant
names or range-expression indices. A PATH value is its text, and `job_from_str` reads it
back as the STRING `create_job` stores.

| item | document |
|---|---|
| job parameter | `{"name": "Frames", "type": "STRING", "value": "1-100"}` |
| list parameter | `{"name": "LL", "type": "LIST[LIST[INT]]", "value": [["1", "2"], ["3"]]}` |
| parameter space | `{"taskParameterDefinitions": [<definition>, …], "combination": null}` |
| `INT` definition | `{"name": "Frame", "type": "INT", "range": "1-100"}`, or `"list": ["1", "3"]` when the range was a list |
| `CHUNK[INT]` definition | `{"name": "F", "type": "CHUNK[INT]", "range": "1-40", "chunks": {"defaultTaskCount": 10, "targetRuntimeSeconds": null, "rangeConstraint": "CONTIGUOUS"}}` |
| `FLOAT`, `STRING`, `PATH` definition | `{"name": "Eye", "type": "STRING", "list": ["left", "right"]}` |
| task parameter set | `{"Frame": {"type": "INT", "value": "3"}}` (`task_parameter_set_document`) |
| symbol table entry | `{"name": "Param.Frames", "type": "string", "value": "1-100"}` (`SerializedSymbolTable`, unchanged) |

`taskParameterDefinitions` is a list in definition order, which is task order. No part of a
document depends on the order of an object's keys, so `serde_json::to_value` of a document
and the document parsed back from its JSON text are the same value, and a host may parse
either into its own maps. `parameters` and environment `variables` are objects sorted by
name. An `INT` definition with `chunks` (which `create_job` never builds) writes them as a
`CHUNK[INT]` does.

`job_from_str` reads exactly this form. A parameter must have exactly `name`, `type` and
`value`, its `name` must be its key, its `type` must be a spec type name, case and all,
and its `value` must have that type
(`parameters[P].value: Cannot convert 'x' to int: invalid digit found in string`). A
definition must have exactly the keys its type takes
(`steps[0].parameterSpace.taskParameterDefinitions[0].bogus: unknown field`), and two
definitions of a step may not have the same name.

## Template fragments

`template_step_fragment` returns the step named `step` from a job template document, and
its `stepEnvironments` list, as the document holds them: format strings unresolved,
nothing defaulted. `template_job_environment_fragment` does the same for a job
environment. `document` is the parsed template, the value a host passes to
`decode_job_template`. The decoded `JobTemplate` cannot serve: the template types
implement `Deserialize` only, so they do not write the document back.

The step object includes every field the template wrote, `stepEnvironments` among them. A
host that stores a step body without some fields removes them itself. A missing step,
environment or list is an error:

| case | message |
|---|---|
| no such step | `Validation error: the template has no step named 'Nope'` |
| no `steps` list | `Validation error: the template document has no 'steps' list` |
| `stepEnvironments` not a list | `Validation error: the 'stepEnvironments' of step 'S' is not a list` |
| no such job environment | `Validation error: the template has no job environment named 'Nope'` |
| no `jobEnvironments` list | `Validation error: the template document has no 'jobEnvironments' list` |

## Job-scope symbol table

`build_job_symbol_table` returns, in transport form, the table `create_job` resolves
job-scope format strings against: `Param.*` and `RawParam.*` for every parameter (PATH and
LIST[PATH] as `RawParam.*` only, since their `Param.*` value depends on session path
mapping), and `Job.Name` when the job declares `EXPR`. It calls `build_symbol_table` on the
job's parameters. `Job.Name` follows the job's declared extensions, which is what
`create_job` does with the template's default validation context, as `TemplateInput`
uses.

Whether a host stores a job-scope table at all, and which extensions add `Job.Name` to it,
is the host's own rule; openjd-rs returns the table above for every job. A host whose rule
differs from `create_job`'s adjusts the table itself. For a host that adds `Job.Name` for
`FEATURE_BUNDLE_1` or `EXPR` and stores no table for a job with no extension, the
difference is:

| job's extensions | `build_job_symbol_table` | that host |
|---|---|---|
| none | the table, without `Job.Name` | no table |
| `FEATURE_BUNDLE_1`, no `EXPR` | the table, without `Job.Name` | the table, with `Job.Name` |
| `EXPR` | the table, with `Job.Name` | the table, with `Job.Name` |

Each `Environment::resolved_symtab`, job or step scoped, is public and holds this table
filtered to the symbols the environment's format strings read. An environment that reads
every symbol holds exactly this table; the test checks that.

## Job digest

`job_digest` is the lowercase hex SHA-256 of the job's `JobDocument` JSON text. The document
is in the host encoding and sorts parameters and environment variables, so value-equal jobs
have the same digest in any process: a list's cached size, for example, is not written. The
exception is a float, whose document keeps its literal, so `1.0` and `1.00` compare equal
but digest differently. Any change `diff_jobs` reports changes the document, so a host can
check "the stored job is the one I built" with one comparison. The converse does not hold:
a document change the diff ignores, such as a task parameter definition reorder under an
explicit `combination`, also changes the digest. The digest of a fixed job is pinned in a
test, so a change of the document format shows up there.

The hash is `sha2`, a dependency of openjd-model for this function only.

## Building from stored values

`TemplateInput::preprocessed(values)` builds a side from parameter values that were already
preprocessed, such as those a host stored at job creation. They go to `create_job` as given,
which still checks them against the template's constraints. Preprocessing again resolves
PATH values against the side's directories and the host's path format, so it can change a
stored value: a Windows path stored by a Windows host is relative on a POSIX host and is
joined to the working directory. The preprocessed values replace those set with `param` and
`params`. They already include environment templates' parameters.

In `diff_templates`, two sides compare their values as typed input values; a preprocessed
side and an input side never count as the same parameters. The origin split's middle job
(the new template with the old parameters) preprocesses the old values against the new
template, because they were resolved for the old one. `diff_job_template_with_parameter_sets`
overlays its parameter sets on the side's values and preprocesses the result.

## Parameter merge

`merge_parameters` gives the values to build a job from a new template with, given the
job's stored (preprocessed) values and the values the caller requests. The result goes to
`TemplateInput::preprocessed`. `new` is the side the values are for: its template,
environments and directories are used, and its own parameter values are not.

| parameter | result |
|---|---|
| requested | the requested value, preprocessed against the new template |
| not requested, declared by the new template (with `new`'s environments) with the stored type | the stored value, untouched |
| not requested, retyped, or not stored | the new default, preprocessed; a missing required value is an error |
| stored, no longer declared | left out |

A kept value is not preprocessed again, because that can change it: a Windows PATH stored by
a Windows host is relative on a POSIX host and would be joined to the working directory, so a
no-op update would plan `ParameterValue` and `StepSymbols`. It is passed to preprocessing so
that a required parameter is present, and the stored value replaces the result. `create_job`
checks it against the new template's constraints when the job is built. A requested value the
new template does not declare is preprocessing's error (`Job parameter values provided for
parameters that are not defined in the template: Z`).

## Tests

`crates/openjd-model/tests/integration/test_host_support.rs`: the `Denoise` step and `Conda`
environment fragments of `examples/job_diff/v2.yaml` against the YAML, each error message,
the job table for a template with every parameter type (and the equal table of an
environment that reads every symbol), the table without `EXPR`, the digest under reordered
maps, pinned, equal for value-equal jobs (a list with a different cached size, and 50
rebuilds and their documents read back), and different for each changed pair, a stored
Windows PATH value built unchanged with `preprocessed` and changed by a second preprocess,
constraint checks on preprocessed values, two preprocessed sides giving a `template`
origin, and each merge rule: a kept, a retyped, a removed and a new parameter, an
undeclared request, a stored Windows PATH that plans nothing, and a requested PATH that is
preprocessed. The host encoding is tested in `tests/integration/test_template_diff.rs`
(the document against the serde form, each parameter type read back, each malformed
parameter and definition) and in `src/test_job_apply.rs` (a host enumerating a space from
the document gets `create_job`'s task order).
