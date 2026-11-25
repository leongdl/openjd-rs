# Template Diff

**Status: unstable.** `job::template_diff` may change or be removed in any release
without notice.

`job::template_diff` diffs the jobs two templates and their parameter values build, and
says of each change whether the template, the parameter values or both caused it. A user
who edits a template and changes its parameters at once sees what caused what.

```rust
pub fn diff_templates(old: &TemplateInput<'_>, new: &TemplateInput<'_>) -> Result<TemplateDiff, TemplateDiffError>;
```

A `TemplateInput` is what builds one side's job: the template, its environment
templates, the parameter values as given (before defaults), the PATH options and the
`create_job` context. `TemplateInput::build` runs `preprocess_job_parameters` and
`create_job` on it. `diff_templates` builds the old side, then the new one, and fails
with `TemplateDiffError::Old` or `New`, displayed as `old job: …` or `new job: …`.

`TemplateDiff` holds the tree, both jobs, so a caller can pass them to `job::apply::plan`
([job-apply.md](job-apply.md)), and `cross_build_error` (see "Origin").

## Origin

The tree is `diff_jobs(old, new)` ([job-diff.md](job-diff.md)). Each field change, and
each added or removed node, gets an `Origin`. It comes from one more job, `mid`: the new
side built with the old parameter values. The change from old to `mid` is the template's
with the values held fixed, and the change from `mid` to new is the values' under the new
template.

| key in `diff_jobs(old, mid)` | key in `diff_jobs(mid, new)` | origin |
|---|---|---|
| yes | no | `Template` |
| no | yes | `Parameters` |
| yes | yes, or neither | `Both` |

A change's key is its node's segments from the root with its field name, or with
`Added` or `Removed` for a whole node. The status is all a whole node's key says, so a step
the template adds is `Template` even when its contents read a changed parameter.

The values are judged under the new template because the new job runs it. The other
cross build, the old template with the new values, gets a template that stops reading a
parameter wrong: with `args: ["{{Param.Quality}}"]` edited to `args: [q]` and `Quality`
changed from `low` to `high`, the `resolvedSymTab[Param.Quality]` entry the new job drops
is the template's, but in that order the entry first changes with the value and is then
dropped, so it would be `Both`.

`mid`'s values are, for each parameter the new side defines, the old value; for one with
no default that the old values do not give, the new value, so a template that adds a
required parameter still builds `mid`, and the new parameter is the template's. A value for
a parameter the new side no longer defines is dropped, so a removed parameter is the
template's. A parameter that the old values do not give and the new side gives a default
takes that default, so a default change the values do not override is the template's.

When `mid` still does not build, such as for an old value the new template's
`allowedValues` reject, every change the jobs show is `Both`, and `cross_build_error`
holds the error.

When the two sides share a template, `mid` is the old job and every change is
`Parameters`; when they share parameter values, `mid` is the new job and every change is
`Template`.

## Template layer

`create_job` keeps only part of a template. These changes it drops are added to the tree,
each with origin `Template`; a node that gains one is `Modified`, as is the root:

| node | field | why it matters |
|---|---|---|
| `parameterDefinitions[NAME]`, a root child before the job's own | `description` | the parameter's documentation for whoever sets it |
| | `default` | the value a side that does not give one gets, as `JobParameterDefinition::default_value` writes it |
| | `allowedValues`, `minValue`, `maxValue`, `minLength`, `maxLength`, `item.*`, `item.item.*` | which values a submission accepts |
| | `objectType`, `dataFlow` | how a PATH value is checked and moved |
| `steps[NAME]` | `let` | the step's bindings; the job keeps only the values the step reads |

A definition is compared when both templates define its name; one added or removed shows
as its `parameters[NAME]` node. A field is `null` when unset. Its type, name and value are
in the job. `userInterface` is not compared, since it changes only how a submitter
presents the parameter. A template-scope field that resolves to the same value, such as a
job `name` of `{{Param.Shot}}` edited to `Shot010` while `Shot` is `Shot010`, builds the
same job, and is not reported.

```
~ parameterDefinitions[Frames]
  default: "1-3" -> "1-10" [template]
~ parameters[Frames]
  value: "1-3" -> "1-4" [both]
```

Tests: `crates/openjd-model/src/test_template_diff.rs` has one case per origin rule and
one per template layer field.
