# build Command

## Purpose

`openjd build <path>` prints the job a template builds into: the output of `create_job`,
including each step's resolved symbol table. It shows what a template turns into, and its
output is the input of `openjd diff --jobs` ([diff.md](diff.md)).

## Interface

```
openjd build <PATH> [-p <PARAM>]... [--environment <FILE>]... [--extensions <EXT>] [--output json|yaml]
```

| Argument | Description |
|----------|-------------|
| `PATH` | Job template file |
| `-p`, `--job-param` | Job parameters, as for `run` ([parameter-parsing.md](parameter-parsing.md)) |
| `--environment`, `--env` | Environment template files; their parameter definitions are merged |
| `--extensions` | Extension allowlist, as for `check`; default every supported extension |
| `--output` | `json` (default) or `yaml` |

The job is built by `TemplateInput::build` (`openjd_model::job::template_diff`) with the
CLI's caller limits: the same decode, parameter preprocessing and `create_job` calls as
`summary` and `run`.

## Output

The `Job`'s serde form, pretty JSON or YAML, serialized directly from the `Job` rather
than through `serde_json::Value`. A `Value` object sorts its keys, which would reorder task
parameter definitions and so change task order when the job is read back.
`job_from_str` reads either format back, and `openjd diff --jobs` on two outputs of
`openjd build` for the same inputs prints `no changes`.

Errors print `ERROR: …` and exit 1, as for every command.

Tests: `crates/openjd-cli/tests/diff_tests.rs` (`jobs_round_trip_through_build`,
`build_errors_and_bad_jobs`).
