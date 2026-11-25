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
  value: "1-3" -> "1-5" [parameters]
~ steps[Render]
  ~ parameterSpace
    ~ taskParameterDefinitions[Frame]
      range: "1-3" -> "1-5" [parameters]
tasks:
Render: 3 kept, 2 added, 0 removed
1 step changed
```

`~` marks a changed part of the job, and each `field: before -> after` line is one field
that differs. The tag at the end of the line says what caused it: `[parameters]` here,
since both sides use the same template. A side's parameters that match its template's
defaults create the same job, so this diff is empty:

```console
$ openjd diff frames.yaml --old-job-param Frames=1-3
frames.yaml: old parameters -> new parameters
no changes
```

## Two templates

`render_v2.yaml` changes the command `Render` runs and adds an `Encode` step. `+` marks the
added step, and `[template]` says the template edit caused each change.

```console
$ openjd diff render_v1.yaml render_v2.yaml
render_v1.yaml -> render_v2.yaml
~ steps[Render]
  ~ script
    ~ onRun
      command: "render" -> "render-gpu" [template]
+ steps[Encode] [template]
2 steps changed
```

## Parameters from a file

`--new-job-param` takes the same forms as `-p`, including a JSON or YAML file. Here the new
side reads `Frames` and `Quality` from `shot.json`. The `resolvedSymTab` line shows the
`Render` step reading the changed `Quality`. The template edit and the parameter changes
land on the same step, and each line keeps its own cause: the command is the template's,
the frame range and the `Quality` it reads are the parameters'.

```console
$ openjd diff render_v1.yaml render_v2.yaml --new-job-param file://shot.json
render_v1.yaml -> render_v2.yaml
~ parameters[Frames]
  value: "1-3" -> "1-4" [parameters]
~ parameters[Quality]
  value: "draft" -> "final" [parameters]
~ steps[Render]
  resolvedSymTab[Param.Quality]: {"type":"string","value":"draft"} -> {"type":"string","value":"final"} [parameters]
  ~ script
    ~ onRun
      command: "render" -> "render-gpu" [template]
  ~ parameterSpace
    ~ taskParameterDefinitions[Frame]
      range: "1-3" -> "1-4" [parameters]
+ steps[Encode] [template]
tasks:
Render: 3 kept, 1 added, 0 removed
2 steps changed
```

## Template and parameters on one field

`frames_v2.yaml` gives `Frames` a longer default and a description, and the new side sets
`Frames` itself. A parameter definition is not part of the job, so its changes show under
`parameterDefinitions[Frames]`, as the template's. The value of `Frames`, and the frame
range it sets, are `[both]`: the new template alone would make them 1-10, and the new
value makes them 1-4.

```console
$ openjd diff frames.yaml frames_v2.yaml --new-job-param Frames=1-4
frames.yaml -> frames_v2.yaml
~ parameterDefinitions[Frames]
  description: null -> "The frames to render" [template]
  default: "1-3" -> "1-10" [template]
~ parameters[Frames]
  value: "1-3" -> "1-4" [both]
~ steps[Render]
  ~ parameterSpace
    ~ taskParameterDefinitions[Frame]
      range: "1-3" -> "1-4" [both]
tasks:
Render: 3 kept, 1 added, 0 removed
1 step changed
```

## A plan for a running job

`--plan` adds the operations a system performs, in order, to change a running job from
the old side to the new one. Each line names what the operation applies to and how it
changes it; a modify of the job, an environment or a step lists the kinds of change in
brackets, so a system can refuse kinds it does not support. The new `Encode` step comes
after `Render`, which it depends on. `StepParameterSpace` says `Render`'s frame range
changed, and `TasksAdded` that the change adds tasks, which the indented line names.

```console
$ openjd diff render_v1.yaml render_v2.yaml --new-job-param file://shot.json --plan
render_v1.yaml -> render_v2.yaml
~ parameters[Frames]
  value: "1-3" -> "1-4" [parameters]
~ parameters[Quality]
  value: "draft" -> "final" [parameters]
~ steps[Render]
  resolvedSymTab[Param.Quality]: {"type":"string","value":"draft"} -> {"type":"string","value":"final"} [parameters]
  ~ script
    ~ onRun
      command: "render" -> "render-gpu" [template]
  ~ parameterSpace
    ~ taskParameterDefinitions[Frame]
      range: "1-3" -> "1-4" [parameters]
+ steps[Encode] [template]
tasks:
Render: 3 kept, 1 added, 0 removed
2 steps changed
plan:
1. parameter Frames: modify
2. parameter Quality: modify
3. step Render: modify [StepScript, StepSymbols, StepParameterSpace, TasksAdded]
   add 1 task: Frame=4
4. step Encode: add
```

## Which tasks change

A system that applies the plan keeps the state of the tasks a change does not touch. The
`tasks:` block after the tree counts, for each step, the tasks the change keeps, adds and
removes, without listing them. Doubling the frame range keeps the first 100 frames and
adds 100:

```console
$ openjd diff frames.yaml --old-job-param Frames=1-100 --new-job-param Frames=1-200 --plan
frames.yaml: old parameters -> new parameters
~ parameters[Frames]
  value: "1-100" -> "1-200" [parameters]
~ steps[Render]
  ~ parameterSpace
    ~ taskParameterDefinitions[Frame]
      range: "1-100" -> "1-200" [parameters]
tasks:
Render: 100 kept, 100 added, 0 removed
1 step changed
plan:
1. parameter Frames: modify
2. step Render: modify [StepParameterSpace, TasksAdded]
   add 100 tasks: Frame=101-200
```

## In a script

`--exit-code` exits 3 when the diff has a change and 0 when it does not, so a script can
test for a change. Errors exit 1.

```console
$ openjd diff frames.yaml --new-job-param Frames=1-5 --exit-code; echo $?
frames.yaml: old parameters -> new parameters
~ parameters[Frames]
  value: "1-3" -> "1-5" [parameters]
~ steps[Render]
  ~ parameterSpace
    ~ taskParameterDefinitions[Frame]
      range: "1-3" -> "1-5" [parameters]
tasks:
Render: 3 kept, 2 added, 0 removed
1 step changed
3
```
