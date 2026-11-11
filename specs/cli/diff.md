# diff Command

## Purpose

`openjd diff` reports what changes between two jobs and where each change came from. It
compares two job templates, one template under two parameter sets, or two jobs that
`openjd build` printed. It is the command-line form of `openjd_model::job::template_diff`
([model/template-diff.md](../model/template-diff.md)), which builds both jobs the same way
`summary` and `run` do and diffs them with `job::diff`.

## Interface

```
openjd diff <OLD> [NEW] [-p <PARAM>]... [--old-param <PARAM>]... [--new-param <PARAM>]...
            [--environment <FILE>]... [--extensions <EXT>] [--jobs]
            [--step <NAME>] [--tasks | --stat] [--plan] [--depth <N>] [--ignore-symbols]
            [--output human-readable|json|yaml] [--verbose] [--exit-code]
```

| Argument | Description |
|----------|-------------|
| `OLD`, `NEW` | Template files, or `-` for stdin (at most one side). With only `OLD`, both sides use that template and differ in `--old-param` / `--new-param` |
| `-p`, `--job-param` | Job parameter for both sides. Same parser as `run` ([parameter-parsing.md](parameter-parsing.md)): `Key=Value`, `file://path`, inline JSON; repeatable, later wins |
| `--old-param`, `--new-param` | Job parameter for one side, same parser; applied after `-p`, so it wins |
| `--environment`, `--env` | Environment template file for both sides; repeatable |
| `--extensions` | Extension allowlist, parsed as for `check` and `summary`; default every supported extension |
| `--jobs` | `OLD` and `NEW` are built jobs (JSON or YAML from `openjd build`). Conflicts with the parameter, environment and extension flags |
| `--step` | Keep only this step and the job-level changes. A step in neither job is an error |
| `--tasks` | Print only each shown step's task summary |
| `--stat` | Print one line per shown step, then the totals |
| `--plan` | Also print `job::apply::plan` on the two jobs: one op per line with its mutation kinds |
| `--depth` | `DiffOptions::with_max_depth` |
| `--ignore-symbols` | `DiffOptions::with_resolved_symtab(false)` |
| `--output` | `human-readable` (default), `json`, `yaml` |
| `--verbose` | With `json` or `yaml`, also include `oldJob`, `newJob` and the `job` tree |
| `--exit-code` | Exit 0 with no differences, 4 with differences |

Which library call runs:

| inputs | call |
|---|---|
| `OLD` only | `diff_parameter_sets(OLD, -p + --old-param, -p + --new-param)` |
| `OLD NEW` | `diff_templates(OLD with -p + --old-param, NEW with -p + --new-param)` |
| `--jobs OLD NEW` | `diff_built_jobs(job_from_str(OLD), job_from_str(NEW))` |

Each template side gets the CLI's caller limits (`common::caller_limits`), as `summary`
and `run` do. A template is read as a file (the format from its extension) and its
directory sets relative PATH defaults. Stdin text is JSON when it starts with `{` and YAML
otherwise, with the working directory as its template directory. Inline template text is
not accepted on the command line, because YAML is hard to quote in a shell argument.

`--help` prints an example of each form.

## Output

Human-readable is `TemplateDiff`'s `Display` ([model/template-diff.md](../model/template-diff.md#rendering)):

```
$ openjd diff render_v1.yaml render_v2.yaml --new-param Frames=1-200 -p Scene=/proj/shot010.blend
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

`--stat`:

```
~ Render  tasks 100 -> 400  (+300 -0)
1 step changed. Tasks: 100 kept, 300 added, 0 removed.
```

`--plan` appends:

```

apply plan:
  1. parameter modify Frames  [ParameterValue]
  2. tasks of Render: keep 100, add 300 at index 100..399, drop 0  [TaskAppend]
kinds: ParameterValue, TaskAppend
```

A step is shown when its status is not `unchanged` or when a change is under it (a
step-level `let` that nothing reads, for example). With nothing to show, `--tasks` and
`--stat` print `no changes`.

`--output json` and `yaml` print one document and nothing else on stdout: the
`TemplateDiff` serde form with `changes`, `steps` and `notes`, plus `plan` with `--plan`
(`{ops: [{action, kinds}], kinds}`). `--verbose` adds `oldJob`, `newJob` and `job`. The
document is serialized directly rather than through `serde_json::Value`, so the jobs keep
their task parameter definition order. `--tasks` and `--stat` affect only human-readable
output.

## Exit codes

| case | exit |
|---|---|
| success, without `--exit-code` | 0, whether or not the inputs differ |
| `--exit-code`, no differences | 0 |
| `--exit-code`, differences | 4 |
| any error (missing file, bad parameter, a template that does not build) | 1, with `ERROR: …` on stderr |
| usage error (clap) | 2 |

4 keeps "the inputs differ" apart from 1, which `openjd` already uses for every error,
and from clap's 2. Errors name the path (`'nope.yaml' does not exist.`) or the side
(`old template: …`).

## Not implemented

`diff-step` (a step template in the context of its job), `diff-space` (two parameter
spaces), `--origin` as a switch (the origin tag is always printed), and a render check.
See [model/template-diff.md](../model/template-diff.md#the-template-layer).

Tests: `crates/openjd-cli/tests/diff_tests.rs`.
