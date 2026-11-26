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

The values are judged under the new template because the new job runs it, so a template
that stops reading a changed parameter drops its `resolvedSymTab` entry as the template's.

`mid`'s values are, for each parameter the new side defines, the old value given. For one
the old values do not give, `mid` takes the new side's default when it has one, so a
default change the values do not override is the template's. Without a new default, it
takes the old job's value, from the old side's default, so a template that drops a default
the old side relied on leaves a new value the parameters'. Failing both, it takes the new
value, so a template that adds a required parameter still builds `mid`, and the new
parameter is the template's. A value for a parameter the new side no longer defines is
dropped, so a removed parameter is the template's.

When `mid` still does not build, such as for an old value the new template's
`allowedValues` reject, every change the jobs show is `Both`, and `cross_build_error`
holds the error.

When the two sides share a template, `mid` is the old job and every change is
`Parameters`; when they share parameter values, `mid` is the new job and every change is
`Template`. A side's PATH options and `create_job` context belong to its template here, so
two sides that share a template and values but resolve PATH values from different working
directories show the difference as `Template`.

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

The definitions compared are each side's `merge_job_parameter_definitions`, so an
environment template's count, with the `description` of the last template that gives one;
merging keeps `allowedValues` in no order, so they compare sorted. A definition is compared
when both sides define its name; one added or removed shows as its `parameters[NAME]` node. A field is `null` when unset. Its type, name and value are
in the job. `userInterface` is not compared, since it changes only how a submitter
presents the parameter. A template-scope field that resolves to the same value, such as a
job `name` of `{{Param.Shot}}` edited to `Shot010` while `Shot` is `Shot010`, builds the
same job, and is not reported. On an otherwise unchanged job, the root and a `steps[NAME]`
node the layer adds hold only its change, not the unchanged children a `Modified` node
from `diff_jobs` lists.

Here the old side takes the default 1-3 and the new side sets 1-4, so the value is
`Both`: the new default alone makes it 1-10, and the new value 1-4.

```
~ parameterDefinitions[Frames]
  default: "1-3" -> "1-10" [template]
~ parameters[Frames]
  value: "1-3" -> "1-4" [both]
```

Tests: `crates/openjd-model/src/test_template_diff.rs` has one case per origin rule and
one per template layer field.
