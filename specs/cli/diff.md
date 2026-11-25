# diff Command

**Status: unstable.** The flags and output of `openjd diff` may change in any release
without notice. `openjd --help` lists the command with `(unstable)`.

## Purpose

`openjd diff` creates two jobs and prints `openjd_model::job::diff::diff_jobs` on them
([model/job-diff.md](../model/job-diff.md)). It compares two job templates, or one template
under two sets of job parameters, so a template author can see what an edit or a
parameter change does to the job.

## Interface

```
openjd diff <OLD> [NEW] [-p <PARAM>]... [--old-job-param <PARAM>]... [--new-job-param <PARAM>]...
            [--environment <FILE>]... [--extensions <EXT>] [--exit-code] [--plan]
```

| Argument | Description |
|----------|-------------|
| `OLD` | The old side's job template file |
| `NEW` | The new side's job template file. When omitted, `OLD` is both sides, which then differ only by their parameters |
| `-p`, `--job-param` | Job parameter for both sides. Alias `--parameter` |
| `--old-job-param`, `--new-job-param` | Job parameter for one side |
| `--environment` | Environment template file, for both sides; repeatable. Alias `--env` |
| `--extensions` | Extensions to support, as for `summary` |
| `--exit-code` | See "Exit codes" |
| `--plan` | Also print the plan for applying the diff to a running job; see "Plan" |

Each side's job is built by `job_from_file::create_job_from_file`, the function `summary`
uses: the template is decoded with the CLI's caller limits, its directory sets relative
`PATH` defaults, parameters are parsed by `parse_cli_parameters`
([parameter-parsing.md](parameter-parsing.md)) and checked against the template and the
environments, and `create_job` builds the job. `run` keeps its own copy of these steps,
because it also loads path-mapping rules and appends the template's help to errors.

A side's parameters are its `-p` values followed by its `--old-job-param` or
`--new-job-param` values. `parse_cli_parameters` keeps the last value of a parameter, so
the side flag wins over `-p` wherever it appears on the command line.

## Output

The first line names the two sides: `OLD -> NEW`, or `OLD: old parameters -> new
parameters` when there is one template, each path as given. When the jobs are equal, the
second and last line is `no changes`. Otherwise the tree's `Display` follows, then a line
counting the changed, added and removed steps: `1 step changed`, `2 steps changed`, or
`0 steps changed` when only the job's own fields, parameters or job environments differ.
A step that is renamed counts twice, as one removed and one added. A pure reorder of the
steps changes only the job's `steps.order` field, so it counts 0 steps.

```
$ openjd diff render_v1.yaml render_v2.yaml
render_v1.yaml -> render_v2.yaml
~ steps[Render]
  ~ script
    ~ onRun
      command: "render" -> "render-gpu"
+ steps[Encode]
2 steps changed
```

Output goes to stdout. The command has no `--output` (see "Out of scope").

## Plan

With `--plan`, when the jobs differ, the output ends with a line `plan:` and the
`Display` of `openjd_model::job::apply::plan` on the two jobs
([model/job-apply.md](../model/job-apply.md)): one numbered line per operation, naming its
target, its action and its mutation kinds. A template author sees what a system would do to
a running job, and which kinds of change it would need to support. When the jobs are equal
the output is unchanged, since the plan is empty.

```
$ openjd diff render_v1.yaml render_v2.yaml --plan
render_v1.yaml -> render_v2.yaml
~ steps[Render]
  ~ script
    ~ onRun
      command: "render" -> "render-gpu"
+ steps[Encode]
2 steps changed
plan:
1. step Render: modify [StepScript]
2. step Encode: add [StepAdd]
3. tasks of step Encode: add [TaskCreate]
```

`plan` fails only for a job whose steps have no topological order, which `create_job`
never produces, so `--plan` adds no error case.

## Exit codes

| case | exit |
|---|---|
| success, without `--exit-code` | 0, whether or not the jobs differ |
| `--exit-code`, jobs equal | 0 |
| `--exit-code`, jobs differ | 3 |
| any error | 1, with `ERROR: …` on stderr |
| usage error (clap) | 2 |

`git diff --exit-code` exits 1 for a difference, but `openjd` exits 1 for every error, so
a difference gets the next free code. An error names the side whose job failed to build:
`ERROR: old job: …` or `ERROR: new job: …`. The old side is built first, so an error in a
`-p` value names the old side.

## Examples

`crates/openjd-cli/examples/diff/` holds small templates and a `README.md` that walks
through them with their real output. `tests/diff_tests.rs` runs each README command and
compares its output with the README, and fails when a file in the directory is not used
by a command. A command ending in `; echo $?` also prints its exit code, and a command
with `--exit-code` must exit 3 unless its output is `no changes`.

## Out of scope

- `--output json` and `yaml`: `DiffTree` has no serde form, and adding one is a model
  API decision outside this command.
- Reading a template from stdin. No other command reads one, and a template's directory
  sets relative `PATH` defaults.
- Which tasks a change keeps, adds or removes. The plan replaces a changed step's tasks
  whole.
- Executing the plan, or checking it against the kinds a system supports.

Tests: `crates/openjd-cli/tests/diff_tests.rs`.
