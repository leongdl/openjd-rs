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
    new_template: &JobTemplate, environments: &[EnvironmentTemplate]) -> Result<JobParameterInputValues, ModelError>;
impl TemplateInput { pub fn preprocessed(self, values: JobParameterValues) -> Self; }
```

They live in `job::template_diff` (`template_diff/host.rs`), beside `TemplateInput` and
`JobDocument`, which they build on.

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

Each `Environment::resolved_symtab`, job or step scoped, is public and holds this table
filtered to the symbols the environment's format strings read. An environment that reads
every symbol holds exactly this table; the test checks that.

## Job digest

`job_digest` is the lowercase hex SHA-256 of the job's `JobDocument` JSON text. The document
sorts parameters and environment variables, so two jobs with the same document have the
same digest in any process. Any change `diff_jobs` reports changes the document, so a host
can check "the stored job is the one I built" with one comparison. The converse does not
hold: a document change the diff ignores, such as a task parameter definition reorder under
an explicit `combination`, also changes the digest. The digest of a fixed job is pinned in
a test, so a change of the document format shows up there.

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

`merge_parameters` gives the input values for building a job from a new template, given the
job's stored (preprocessed) values and the values the caller requests:

| parameter | result |
|---|---|
| requested | the requested value, even if the new template does not declare it (preprocessing reports that) |
| not requested, declared by the new template (with `environments`) with the stored type | the stored value |
| not requested, retyped or no longer declared | left out: preprocessing takes the new default, or reports it missing |
| not requested, not stored | left out: the new default |

The result goes to `preprocess_job_parameters` (for example through `TemplateInput::params`),
which checks a kept value against the new template's constraints.

## Tests

`crates/openjd-model/tests/integration/test_host_support.rs`: the `Denoise` step and `Conda`
environment fragments of `examples/job_diff/v2.yaml` against the YAML, each error message,
the job table for a template with every parameter type (and the equal table of an
environment that reads every symbol), the table without `EXPR`, the digest under reordered
maps, pinned, and different for each changed pair, a stored Windows PATH value built
unchanged with `preprocessed` and changed by a second preprocess, constraint checks on
preprocessed values, and each merge rule including a retyped and a removed parameter.
