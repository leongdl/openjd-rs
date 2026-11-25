# `openjd diff` examples

`openjd diff` creates two jobs, from two job templates or from one template under two
sets of job parameters, and prints what differs between them. The command is unstable:
its flags and output may change in any release.

Run the commands from this directory. The output shown is the real output, checked by a
test.

## One template, more frames

`frames.yaml` takes its frame range from the `Frames` parameter. Give the old side 1-3
and the new side 1-5.

```console
$ openjd diff frames.yaml --old-job-param Frames=1-3 --new-job-param Frames=1-5
frames.yaml: old parameters -> new parameters
~ parameters[Frames]
  value: "1-3" -> "1-5"
~ steps[Render]
  ~ parameterSpace
    ~ taskParameterDefinitions[Frame]
      range: "1-3" -> "1-5"
1 step changed
```

`~` marks a changed part of the job, and each `field: before -> after` line is one field
that differs. A side's parameters that match its template's defaults create the same job,
so this diff is empty:

```console
$ openjd diff frames.yaml --old-job-param Frames=1-3
frames.yaml: old parameters -> new parameters
no changes
```

## Two templates

`render_v2.yaml` changes the command `Render` runs and adds an `Encode` step. `+` marks the
added step.

```console
$ openjd diff render_v1.yaml render_v2.yaml
render_v1.yaml -> render_v2.yaml
~ steps[Render]
  ~ script
    ~ onRun
      command: "render" -> "render-gpu"
+ steps[Encode]
2 steps changed
```

## Parameters from a file

`--new-job-param` takes the same forms as `-p`, including a JSON or YAML file. Here the new
side reads `Frames` and `Quality` from `shot.json`. The `resolvedSymTab` line shows the
`Render` step reading the changed `Quality`.

```console
$ openjd diff render_v1.yaml render_v2.yaml --new-job-param file://shot.json
render_v1.yaml -> render_v2.yaml
~ parameters[Frames]
  value: "1-3" -> "1-4"
~ parameters[Quality]
  value: "draft" -> "final"
~ steps[Render]
  resolvedSymTab[Param.Quality]: {"type":"string","value":"draft"} -> {"type":"string","value":"final"}
  ~ script
    ~ onRun
      command: "render" -> "render-gpu"
  ~ parameterSpace
    ~ taskParameterDefinitions[Frame]
      range: "1-3" -> "1-4"
+ steps[Encode]
2 steps changed
```

## A plan for a running job

`--plan` adds the operations a system performs, in order, to change a running job from
the old side to the new one. Each line names what the operation applies to and how it
changes it; a modify of the job, an environment or a step lists the kinds of change in
brackets, so a system can refuse kinds it does not support. The new `Encode` step comes
after `Render`, which it depends on, and `StepParameterSpace` says a changed frame range
replaces all of `Render`'s tasks.

```console
$ openjd diff render_v1.yaml render_v2.yaml --new-job-param file://shot.json --plan
render_v1.yaml -> render_v2.yaml
~ parameters[Frames]
  value: "1-3" -> "1-4"
~ parameters[Quality]
  value: "draft" -> "final"
~ steps[Render]
  resolvedSymTab[Param.Quality]: {"type":"string","value":"draft"} -> {"type":"string","value":"final"}
  ~ script
    ~ onRun
      command: "render" -> "render-gpu"
  ~ parameterSpace
    ~ taskParameterDefinitions[Frame]
      range: "1-3" -> "1-4"
+ steps[Encode]
2 steps changed
plan:
1. parameter Frames: modify
2. parameter Quality: modify
3. step Render: modify [StepScript, StepSymbols, StepParameterSpace]
4. step Encode: add
```

## In a script

`--exit-code` exits 3 when the jobs differ and 0 when they do not, so a script can test
for a change. Errors exit 1.

```console
$ openjd diff frames.yaml --new-job-param Frames=1-5 --exit-code; echo $?
frames.yaml: old parameters -> new parameters
~ parameters[Frames]
  value: "1-3" -> "1-5"
~ steps[Render]
  ~ parameterSpace
    ~ taskParameterDefinitions[Frame]
      range: "1-3" -> "1-5"
1 step changed
3
```
