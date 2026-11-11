// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// Copyright by contributors to this project.
// SPDX-License-Identifier: (Apache-2.0 OR MIT)

//! `job::template_diff`: the cases in `specs/model/template-diff.md` § Examples,
//! with the sample templates in `examples/template_diff/`.

use std::path::PathBuf;

use openjd_expr::ExprValue;
use openjd_model::job::apply::MutationKind;
use openjd_model::job::diff::{DiffOptions, DiffStatus};
use openjd_model::job::template_diff::*;
use openjd_model::JobParameterInputValues;

fn sample_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("examples/template_diff")
        .join(name)
}

fn sample(name: &str) -> TemplateSource {
    TemplateSource::Path(sample_path(name))
}

/// A sample with `Scene` set; it has no default.
fn input(name: &str) -> TemplateInput {
    TemplateInput::new(sample(name)).param("Scene", "shot.blend")
}

fn text(s: &str) -> TemplateInput {
    TemplateInput::new(TemplateSource::Text(s.to_string()))
}

fn params(pairs: &[(&str, &str)]) -> JobParameterInputValues {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), ExprValue::String(v.to_string())))
        .collect()
}

fn opts() -> DiffOptions {
    DiffOptions::default()
}

fn lines(c: &[Change]) -> Vec<String> {
    c.iter().map(ToString::to_string).collect()
}

const CASE1: &str = "\
~ parameter Frames: 1-100 -> 1-200  [parameters]
~ step Render
    ~ Frame: 1-100 -> 1-200  [parameters]
    tasks: 100 -> 200
      keep 100   Frame 1-100 × Eye left
      add  100   Frame 101-200 × Eye left

1 step changed. Tasks: 100 kept, 100 added, 0 removed.";

// ─── the five cases ─────────────────────────────────────────────────

#[test]
fn case1_frames_parameter_set_appends_100_tasks() {
    let d = diff_parameter_sets(
        &input("render_v1.yaml"),
        &params(&[("Frames", "1-100")]),
        &params(&[("Frames", "1-200")]),
        &opts(),
    )
    .unwrap();
    assert_eq!(d.to_string(), CASE1);
    assert!(d.changes.iter().all(|c| c.origin == Origin::Parameters));
    let stages: Vec<Stage> = d.changes.iter().map(|c| c.stage).collect();
    assert_eq!(stages, [Stage::Parameters, Stage::Job]);

    // The same through diff_templates with one file on both sides.
    let same = diff_templates(
        &input("render_v1.yaml"),
        &input("render_v1.yaml").param("Frames", "1-200"),
        &opts(),
    )
    .unwrap();
    assert_eq!(same.to_string(), CASE1);
    assert!(same.changes.iter().all(|c| c.origin == Origin::Parameters));
}

#[test]
fn case2_template_adds_right_eye() {
    let d = diff_templates(&input("render_v1.yaml"), &input("render_v2.yaml"), &opts()).unwrap();
    assert_eq!(
        d.to_string(),
        "\
~ step Render
    ~ Eye: [left] -> [left, right]  [template]
    tasks: 100 -> 200
      keep 100   Frame 1-100 × Eye left
      add  100   Frame 1-100 × Eye right

1 step changed. Tasks: 100 kept, 100 added, 0 removed."
    );
    assert_eq!(d.changes[0].origin, Origin::Template);
    assert_eq!(d.changes[0].stage, Stage::Job);
}

const CASE3: &str = "\
~ parameter Frames: 1-100 -> 1-200  [parameters]
~ step Render
    ~ Frame: 1-100 -> 1-200  [parameters]
    ~ Eye: [left] -> [left, right]  [template]
    tasks: 100 -> 400
      keep 100   Frame 1-100 × Eye left
      add  200   Frame 101-200 × Eye left, right
      add  100   Frame 1-100 × Eye right

1 step changed. Tasks: 100 kept, 300 added, 0 removed.";

fn case3() -> TemplateDiff {
    diff_templates(
        &input("render_v1.yaml"),
        &input("render_v2.yaml").param("Frames", "1-200"),
        &opts(),
    )
    .unwrap()
}

#[test]
fn case3_both_changes_have_mixed_origins() {
    let d = case3();
    assert_eq!(d.to_string(), CASE3);
    let origins: Vec<(&str, Origin)> = d
        .changes
        .iter()
        .map(|c| (c.path.as_str(), c.origin))
        .collect();
    assert_eq!(
        origins,
        [
            ("parameters[Frames]", Origin::Parameters),
            (
                "steps[Render].parameterSpace.taskParameterDefinitions[Eye]",
                Origin::Template
            ),
            (
                "steps[Render].parameterSpace.taskParameterDefinitions[Frame]",
                Origin::Parameters
            ),
        ]
    );
    assert_eq!(
        d.stat(),
        DiffStat {
            steps_changed: 1,
            tasks_kept: 100,
            tasks_added: 300,
            ..DiffStat::default()
        }
    );
    assert!(d.notes.is_empty());
}

#[test]
fn case4_new_dimension_replaces_every_task() {
    let d = diff_templates(
        &input("render_no_eye.yaml"),
        &input("render_v1.yaml"),
        &opts(),
    )
    .unwrap();
    assert_eq!(
        d.to_string(),
        "\
~ step Render
    + Eye: [left]  [template]
    ~ script.onRun.args: [{{Param.Scene}}, {{Task.Param.Frame}}] -> [{{Param.Scene}}, {{Task.Param.Frame}}, {{Task.Param.Eye}}]  [template]
    tasks: 100 -> 100, all replaced (task parameter added or removed)

1 step changed. Tasks: 0 kept, 0 added, 0 removed, 100 replaced."
    );
    assert_eq!(
        d.step("Render").map(|s| &s.tasks),
        Some(&TaskSummary::Replaced {
            old_count: 100,
            new_count: 100,
            reason: "task parameter added or removed".into(),
        })
    );
    // The args reference `Task.` symbols, so only a worker resolves them.
    assert_eq!(d.changes[1].field, "args");
    assert_eq!(d.changes[1].stage, Stage::RunTime);
}

#[test]
fn case5_scene_path_changes_the_raw_param_symbol() {
    let (a, b) = if cfg!(windows) {
        (r"C:\proj\a.blend", r"C:\proj\b.blend")
    } else {
        ("/proj/a.blend", "/proj/b.blend")
    };
    let d = diff_parameter_sets(
        &input("render_v1.yaml"),
        &params(&[("Scene", a)]),
        &params(&[("Scene", b)]),
        &opts(),
    )
    .unwrap();
    assert_eq!(
        d.to_string(),
        format!(
            "\
~ parameter Scene: {a} -> {b}  [parameters]
~ step Render
    ~ resolvedSymTab: RawParam.Scene={a} -> RawParam.Scene={b}  [parameters]
    tasks: 100 unchanged

1 step changed. Tasks: 100 kept, 0 added, 0 removed."
        )
    );
    let stages: Vec<(&str, Stage)> = d
        .changes
        .iter()
        .map(|c| (c.field.as_str(), c.stage))
        .collect();
    assert_eq!(
        stages,
        [("value", Stage::Parameters), ("resolvedSymTab", Stage::Job)]
    );
}

// ─── template layer ─────────────────────────────────────────────────

#[test]
fn template_default_change_with_parameter_unsupplied_is_template_origin() {
    let d = diff_templates(
        &input("render_v1.yaml"),
        &input("render_default_1_200.yaml"),
        &opts(),
    )
    .unwrap();
    assert_eq!(
        d.to_string(),
        "\
~ parameter definition Frames.default: 1-100 -> 1-200  [template]
~ parameter Frames: 1-100 -> 1-200  [template]
~ step Render
    ~ Frame: 1-100 -> 1-200  [template]
    tasks: 100 -> 200
      keep 100   Frame 1-100 × Eye left
      add  100   Frame 101-200 × Eye left

1 step changed. Tasks: 100 kept, 100 added, 0 removed."
    );
    assert_eq!(d.changes[0].layer, Layer::Template);
    assert_eq!(d.changes[0].stage, Stage::Template);
}

const PARAMS_OLD: &str = r#"
specificationVersion: jobtemplate-2023-09
name: P
parameterDefinitions:
  - {name: S, type: STRING, description: a, default: x, allowedValues: [x, y], minLength: 1, maxLength: 5, userInterface: {control: DROPDOWN_LIST, label: L}}
  - {name: I, type: INT, default: 5, minValue: 1, maxValue: 10}
  - {name: F, type: FLOAT, default: 1.5, minValue: 0}
  - {name: T, type: STRING, default: "3"}
  - {name: Gone, type: STRING, default: g}
steps:
  - name: Render
    script:
      actions:
        onRun: {command: echo}
"#;

const PARAMS_NEW: &str = r#"
specificationVersion: jobtemplate-2023-09
name: P
parameterDefinitions:
  - {name: S, type: STRING, description: b, default: x, allowedValues: [x, y, z], minLength: 0, maxLength: 9, userInterface: {control: DROPDOWN_LIST, label: M}}
  - {name: I, type: INT, default: 5, minValue: 2, maxValue: 20}
  - {name: F, type: FLOAT, default: 1.5, maxValue: 9.5}
  - {name: T, type: INT, default: 3}
  - {name: New, type: STRING, default: n}
steps:
  - name: Render
    script:
      actions:
        onRun: {command: echo}
"#;

#[test]
fn parameter_definition_changes_are_compared_field_by_field() {
    let d = diff_templates(&text(PARAMS_OLD), &text(PARAMS_NEW), &opts()).unwrap();
    assert_eq!(
        d.to_string(),
        r#"+ parameter definition F.maxValue: 9.5  [template]
- parameter definition F.minValue: 0.0  [template]
~ parameter definition I.maxValue: 10 -> 20  [template]
~ parameter definition I.minValue: 1 -> 2  [template]
~ parameter definition S.allowedValues: [x, y] -> [x, y, z]  [template]
~ parameter definition S.description: a -> b  [template]
~ parameter definition S.maxLength: 5 -> 9  [template]
~ parameter definition S.minLength: 1 -> 0  [template]
~ parameter definition S.userInterface.label: L -> M  [template]
~ parameter definition T.default: "3" -> 3  [template]
~ parameter definition T.type: STRING -> INT  [template]
- parameter Gone: g, defined as {default: g, type: STRING}  [template]
+ parameter New: n, defined as {default: n, type: STRING}  [template]
~ parameter T.type: STRING -> INT  [template]
~ parameter T: {"String":"3"} -> {"Int":3}  [template]

0 steps changed. Tasks: 1 kept, 0 added, 0 removed."#
    );
    let defs: Vec<&Change> = d.changes_at("parameterDefinitions[S]").collect();
    assert_eq!(defs.len(), 5);
    assert!(defs
        .iter()
        .all(|c| c.layer == Layer::Template && c.stage == Stage::Template));
}

const LET_OLD: &str = r#"
specificationVersion: jobtemplate-2023-09
extensions: [EXPR]
name: L
steps:
  - name: Render
    let: ["unused = 1"]
    script:
      actions:
        onRun: {command: echo}
"#;

#[test]
fn step_let_nothing_reads_shows_only_in_the_template_layer() {
    let d = diff_templates(
        &text(LET_OLD),
        &text(&LET_OLD.replace("unused = 1", "unused = 2")),
        &opts(),
    )
    .unwrap();
    assert_eq!(
        d.to_string(),
        "\
~ step Render
    ~ let: [unused = 1] -> [unused = 2]  [template]
    tasks: 1 unchanged

1 step changed. Tasks: 1 kept, 0 added, 0 removed."
    );
    assert!(!d.job.is_changed());
    assert_eq!(
        lines(&d.changes),
        ["~ steps[Render].let: [unused = 1] -> [unused = 2]  [template]"]
    );
}

#[test]
fn task_param_reference_change_is_run_time() {
    let old = std::fs::read_to_string(sample_path("render_v1.yaml")).unwrap();
    let new = old.replace("{{Task.Param.Frame}}", "{{Task.RawParam.Frame}}");
    let d = diff_templates(
        &text(&old).param("Scene", "s"),
        &text(&new).param("Scene", "s"),
        &opts(),
    )
    .unwrap();
    assert_eq!(
        lines(&d.changes),
        ["~ steps[Render].script.onRun.args: [{{Param.Scene}}, {{Task.Param.Frame}}, {{Task.Param.Eye}}] -> [{{Param.Scene}}, {{Task.RawParam.Frame}}, {{Task.Param.Eye}}]  [template]"]
    );
    assert_eq!(d.changes[0].stage, Stage::RunTime);
}

const NO_EXT: &str = r#"
specificationVersion: jobtemplate-2023-09
name: E
steps:
  - name: Render
    script:
      actions:
        onRun: {command: echo}
"#;

#[test]
fn extensions_change_is_reported_by_both_layers_and_listed_once() {
    let with = NO_EXT.replace("name: E", "extensions: [EXPR]\nname: E");
    let d = diff_templates(&text(NO_EXT), &text(&with), &opts()).unwrap();
    assert_eq!(
        d.to_string(),
        "\
+ extensions: [EXPR]  [template]

0 steps changed. Tasks: 1 kept, 0 added, 0 removed."
    );
    let layers: Vec<(Layer, &str)> = d
        .changes
        .iter()
        .map(|c| (c.layer, c.field.as_str()))
        .collect();
    assert_eq!(
        layers,
        [(Layer::Template, "extensions"), (Layer::Job, "extensions")]
    );
}

// ─── identity, equality, serde ──────────────────────────────────────

#[test]
fn identical_inputs_have_no_changes() {
    let d = diff_templates(&input("render_v1.yaml"), &input("render_v1.yaml"), &opts()).unwrap();
    assert!(!d.is_changed());
    assert_eq!(d.to_string(), "no changes");
    assert!(d.changes.is_empty());
    assert_eq!(d.stat().tasks_kept, 100);
    assert_eq!(d.plan().unwrap().ops.len(), 0);
}

#[test]
fn two_diffs_of_the_same_inputs_are_equal() {
    assert_eq!(case3(), case3());
    assert_ne!(
        case3(),
        diff_templates(&input("render_v1.yaml"), &input("render_v2.yaml"), &opts()).unwrap()
    );
}

#[test]
fn serde_shape_is_pinned() {
    let d = diff_templates(&input("render_v1.yaml"), &input("render_v2.yaml"), &opts()).unwrap();
    let mut v = serde_json::to_value(&d).unwrap();
    let obj = v.as_object_mut().unwrap();
    assert!(obj.remove("oldJob").is_some());
    assert!(obj.remove("newJob").is_some());
    assert_eq!(
        v,
        serde_json::json!({
            "changes": [{
                "layer": "job",
                "path": "steps[Render].parameterSpace.taskParameterDefinitions[Eye]",
                "field": "definition",
                "before": {"raw": {"string": {"range": ["left"]}}, "text": "[left]"},
                "after": {"raw": {"string": {"range": ["left", "right"]}}, "text": "[left, right]"},
                "origin": "template",
                "stage": "job",
            }],
            "steps": [{
                "name": "Render",
                "status": "modified",
                "oldIndex": 0,
                "newIndex": 0,
                "tasks": {
                    "kind": "product",
                    "kept": {"count": 100, "dims": [["Frame", "1-100"], ["Eye", "left"]]},
                    "added": [{"count": 100, "dims": [["Frame", "1-100"], ["Eye", "right"]]}],
                    "removed": [],
                },
            }],
            "job": [
                {"path": "", "status": "modified", "expanded": true, "fieldChanges": []},
                {"path": "steps[Render]", "status": "modified", "expanded": true, "fieldChanges": []},
                {"path": "steps[Render].parameterSpace", "status": "modified", "expanded": true, "fieldChanges": []},
                {"path": "steps[Render].parameterSpace.taskParameterDefinitions[Eye]", "status": "modified",
                 "expanded": true, "fieldChanges": [{
                    "field": "definition",
                    "before": {"string": {"range": ["left"]}},
                    "after": {"string": {"range": ["left", "right"]}},
                 }]},
            ],
            "notes": [],
        })
    );
}

#[test]
fn replaced_task_summary_serializes_camel_case() {
    let v = serde_json::to_value(TaskSummary::Replaced {
        old_count: 1,
        new_count: 2,
        reason: "r".into(),
    })
    .unwrap();
    assert_eq!(
        v,
        serde_json::json!({"kind": "replaced", "oldCount": 1, "newCount": 2, "reason": "r"})
    );
}

// ─── origin fallbacks and errors ────────────────────────────────────

#[test]
fn mid_build_failure_marks_job_changes_both_with_a_note() {
    let old = std::fs::read_to_string(sample_path("render_v1.yaml")).unwrap();
    let new = old.replace(
        "  - {name: Scene, type: PATH}",
        "  - {name: Scene, type: PATH}\n  - {name: Quality, type: INT}",
    );
    let d = diff_templates(
        &text(&old).param("Scene", "s"),
        &text(&new).param("Scene", "s").param("Quality", "3"),
        &opts(),
    )
    .unwrap();
    assert_eq!(d.notes, [MID_NOTE]);
    assert_eq!(
        lines(&d.changes),
        [
            "+ parameterDefinitions[Quality]: {type: INT}  [template]",
            "+ parameters[Quality]: 3  [both]",
        ]
    );
}

const MID_NOTE: &str = "origin: the new template does not build with the old parameters, so job-layer changes are marked both: Validation error: Values missing for required job parameters: Quality";

#[test]
fn errors_name_the_side() {
    let old = std::fs::read_to_string(sample_path("render_v1.yaml")).unwrap();
    let err = diff_templates(&text(&old).param("Scene", "s"), &text(&old), &opts()).unwrap_err();
    assert_eq!(
        err.to_string(),
        "Validation error: new template: Values missing for required job parameters: Scene"
    );
    let err = diff_templates(&text("name: x"), &text(&old), &opts()).unwrap_err();
    assert_eq!(
        err.to_string(),
        "Validation error: old template: Template is missing Open Job Description schema version key: specificationVersion"
    );
    let err = diff_parameter_sets(
        &text(&old),
        &params(&[("Scene", "s")]),
        &params(&[]),
        &opts(),
    )
    .unwrap_err();
    assert_eq!(
        err.to_string(),
        "Validation error: new parameters: Values missing for required job parameters: Scene"
    );
}

#[test]
fn extension_allowlist_applies() {
    let err = diff_templates(&text(LET_OLD).no_extensions(), &text(LET_OLD), &opts()).unwrap_err();
    assert_eq!(
        err.to_string(),
        "Validation error: old template: Model validation error: 1 validation error for JobTemplate\nextensions:\n\tUnsupported extension names: EXPR"
    );
    assert!(diff_templates(
        &text(LET_OLD).extensions(&["EXPR"]),
        &text(LET_OLD),
        &opts()
    )
    .is_ok());
}

// ─── sources, built jobs, helpers ───────────────────────────────────

#[test]
fn guess_picks_a_path_only_for_an_existing_file() {
    let p = sample_path("render_v1.yaml");
    let s = p.to_str().unwrap();
    assert!(matches!(TemplateSource::guess(s), TemplateSource::Path(x) if x == p));
    assert!(matches!(TemplateSource::guess("name: x"), TemplateSource::Text(t) if t == "name: x"));
}

#[test]
fn decoded_and_value_sources_give_the_same_diff() {
    let doc: serde_json::Value =
        serde_saphyr::from_str(&std::fs::read_to_string(sample_path("render_v1.yaml")).unwrap())
            .unwrap();
    let jt = openjd_model::decode_job_template(doc.clone(), None, &Default::default()).unwrap();
    let decoded =
        TemplateInput::new(TemplateSource::Decoded(Box::new(jt))).param("Scene", "shot.blend");
    let value = TemplateInput::new(TemplateSource::Value(doc)).param("Scene", "shot.blend");
    // A decoded template has no document to compare, so the origin comes
    // from building the `mid` job; it agrees with the shortcut.
    for side in [decoded, value] {
        let d = diff_templates(&side, &side.clone().param("Frames", "1-200"), &opts()).unwrap();
        assert_eq!(d.to_string(), CASE1);
    }
}

#[test]
fn json_text_source_is_parsed_as_json() {
    let json = r#"{"specificationVersion": "jobtemplate-2023-09", "name": "J",
        "steps": [{"name": "S", "script": {"actions": {"onRun": {"command": "a"}}}}]}"#;
    let d = diff_templates(&text(json), &text(&json.replace("\"a\"", "\"b\"")), &opts()).unwrap();
    assert_eq!(
        lines(&d.changes),
        ["~ steps[S].script.onRun.command: a -> b  [template]"]
    );
}

#[test]
fn environment_template_parameters_are_merged() {
    let env = r#"
specificationVersion: environment-2023-09
parameterDefinitions:
  - {name: Tool, type: STRING, default: t1}
environment:
  name: Env
  variables: {TOOL: "{{Param.Tool}}"}
"#;
    let job = TemplateInput::new(sample("render_v1.yaml"))
        .param("Scene", "s")
        .environment(TemplateSource::Text(env.into()));
    let d = diff_parameter_sets(&job, &params(&[]), &params(&[("Tool", "t2")]), &opts()).unwrap();
    assert_eq!(
        lines(&d.changes),
        ["~ parameters[Tool].value: t1 -> t2  [parameters]"]
    );
}

#[test]
fn built_jobs_round_trip_and_diff_with_job_origin() {
    let old = input("render_v1.yaml").build().unwrap();
    let new = input("render_v2.yaml")
        .param("Frames", "1-200")
        .build()
        .unwrap();
    let back = job_from_str(&serde_json::to_string(&old).unwrap()).unwrap();
    // `==` ignores task parameter definition order; the diff does not.
    assert!(!openjd_model::job::diff::diff_jobs(&back, &old, &opts()).is_changed());
    let d = diff_built_jobs(back, new, &opts());
    assert_eq!(
        d.to_string(),
        CASE3
            .replace("[parameters]", "[job]")
            .replace("[template]", "[job]")
    );
    assert!(d.changes.iter().all(|c| c.origin == Origin::Job));
}

#[test]
fn job_from_value_reads_every_parameter_type() {
    let t = r#"
specificationVersion: jobtemplate-2023-09
extensions: [EXPR]
name: All
parameterDefinitions:
  - {name: S, type: STRING, default: s}
  - {name: I, type: INT, default: 3}
  - {name: F, type: FLOAT, default: "1.50"}
  - {name: P, type: PATH, default: p}
  - {name: B, type: BOOL, default: true}
  - {name: R, type: RANGE_EXPR, default: "1-3"}
  - {name: LS, type: "LIST[STRING]", default: [a, b]}
  - {name: LE, type: "LIST[STRING]", default: []}
  - {name: LI, type: "LIST[INT]", default: [1, 2]}
  - {name: LF, type: "LIST[FLOAT]", default: [1.5]}
  - {name: LP, type: "LIST[PATH]", default: [x]}
  - {name: LB, type: "LIST[BOOL]", default: [false]}
  - {name: LL, type: "LIST[LIST[INT]]", default: [[1], [2, 3]]}
steps:
  - name: Render
    script:
      actions:
        onRun: {command: echo}
"#;
    let job = text(t).build().unwrap();
    for text in [
        serde_json::to_string(&job).unwrap(),
        serde_saphyr::to_string(&job).unwrap(),
    ] {
        let back = job_from_str(&text).unwrap();
        assert_eq!(back, job);
        assert!(!openjd_model::job::diff::diff_jobs(&back, &job, &opts()).is_changed());
    }
    let err = job_from_str(r#"{"name": "x", "steps": [], "bogus": 1}"#).unwrap_err();
    assert_eq!(err.to_string(), "Validation error: not a built job: unknown field `bogus`, expected one of `name`, `description`, `extensions`, `parameters`, `steps`, `jobEnvironments` at line 1 column 34");
}

#[test]
fn helpers_find_changes_steps_and_plan() {
    let d = case3();
    assert_eq!(d.changes_at("steps[Render]").count(), 2);
    assert_eq!(d.changes_at("steps[Rend]").count(), 0);
    assert_eq!(d.changes_at("").count(), 3);
    let s = d.step("Render").unwrap();
    assert_eq!(
        (s.status, s.old_index, s.new_index),
        (DiffStatus::Modified, Some(0), Some(0))
    );
    assert!(d.step("Nope").is_none());
    assert_eq!(s.stat_line(), "~ Render  tasks 100 -> 400  (+300 -0)");
    let kinds: Vec<MutationKind> = d.plan().unwrap().kinds().into_iter().collect();
    assert_eq!(
        kinds,
        [MutationKind::ParameterValue, MutationKind::TaskAppend]
    );
}

#[test]
fn max_depth_reports_unexpanded_nodes_whole() {
    let d = diff_templates(
        &input("render_v1.yaml"),
        &input("render_v2.yaml").param("Frames", "1-200"),
        &DiffOptions::default().with_max_depth(1),
    )
    .unwrap();
    // The step itself is not expanded; its tasks come from the jobs, not
    // the tree, so they still show.
    assert_eq!(
        d.to_string(),
        "\
~ parameter Frames: 1-100 -> 1-200  [parameters]
~ step Render
    ~ (not expanded)  [both]
    tasks: 100 -> 400
      keep 100   Frame 1-100 × Eye left
      add  200   Frame 101-200 × Eye left, right
      add  100   Frame 1-100 × Eye right

1 step changed. Tasks: 100 kept, 300 added, 0 removed."
    );
    let fields: Vec<(&str, &str, Origin)> = d
        .changes
        .iter()
        .map(|c| (c.path.as_str(), c.field.as_str(), c.origin))
        .collect();
    assert_eq!(
        fields,
        [
            ("parameters[Frames]", "", Origin::Parameters),
            ("steps[Render]", "", Origin::Both)
        ]
    );
}

#[test]
fn integer_boxes_render_as_range_expressions_and_long_text_is_cut() {
    let base = r#"
specificationVersion: jobtemplate-2023-09
name: R
steps:
  - name: S
    parameterSpace:
      taskParameterDefinitions:
        - {name: N, type: INT, range: [1, 3, 5, 7]}
    script:
      actions:
        onRun: {command: echo, args: ["ARG"]}
"#;
    let long = "x".repeat(200);
    let new = base
        .replace("[1, 3, 5, 7]", "[1, 3, 5, 7, 8, 20]")
        .replace("ARG", &long);
    let d = diff_templates(&text(base), &text(&new), &opts()).unwrap();
    let args = d.changes.iter().find(|c| c.field == "args").unwrap();
    let after = args.after.as_ref().unwrap();
    assert_eq!(after.raw, serde_json::json!([long]));
    assert_eq!(after.text.chars().count(), 120);
    assert!(after.text.ends_with('…'));
    assert_eq!(
        d.step("S").unwrap().tasks.to_string(),
        "tasks: 4 -> 6\n  keep 4     N 1-7:2\n  add  2     N 8,20"
    );
}

#[test]
fn added_and_removed_steps() {
    let one = r#"
specificationVersion: jobtemplate-2023-09
name: J
steps:
  - name: A
    script: {actions: {onRun: {command: a}}}
"#;
    let two = r#"
specificationVersion: jobtemplate-2023-09
name: J
steps:
  - name: B
    parameterSpace:
      taskParameterDefinitions:
        - {name: N, type: INT, range: "1-3"}
    script: {actions: {onRun: {command: b}}}
"#;
    let d = diff_templates(&text(one), &text(two), &opts()).unwrap();
    assert_eq!(
        d.to_string(),
        "\
- step A
    tasks: 1 removed
+ step B
    tasks: 3 added

0 steps changed, 1 added, 1 removed. Tasks: 0 kept, 3 added, 1 removed."
    );
    let stat: Vec<String> = d.steps.iter().map(StepSummary::stat_line).collect();
    assert_eq!(
        stat,
        ["- A  tasks 1 -> 0  (+0 -1)", "+ B  tasks 0 -> 3  (+3 -0)"]
    );
    assert_eq!(d.steps[1].to_string(), "+ step B\n    tasks: 3 added");
}

// ─── audit round 1 regressions ──────────────────────────────────────

const ENV_JOB: &str = r#"
specificationVersion: jobtemplate-2023-09
name: J
jobEnvironments:
  - name: E
    script:
      actions:
        onEnter: {command: hi}
steps:
  - name: S
    script:
      actions:
        onRun: {command: a}
"#;

#[test]
fn whole_action_added_or_removed_carries_its_side() {
    let new = ENV_JOB.replace(
        "onEnter: {command: hi}",
        "onEnter: {command: hi}\n        onExit: {command: bye}",
    );
    let d = diff_templates(&text(ENV_JOB), &text(&new), &opts()).unwrap();
    assert_eq!(
        lines(&d.changes),
        ["+ jobEnvironments[E].script.onExit: {args: null, cancelation: null, command: bye, timeout: null}  [template]"]
    );
    let d = diff_templates(&text(&new), &text(ENV_JOB), &opts()).unwrap();
    assert_eq!(
        lines(&d.changes),
        ["- jobEnvironments[E].script.onExit: {args: null, cancelation: null, command: bye, timeout: null}  [template]"]
    );
    // `onRun` cut off by the depth limit is reported whole, with both sides.
    let d = diff_templates(
        &text(ENV_JOB),
        &text(&ENV_JOB.replace("command: a", "command: b")),
        &DiffOptions::default().with_max_depth(3),
    )
    .unwrap();
    assert_eq!(
        lines(&d.changes),
        ["~ steps[S].script.onRun: {args: null, cancelation: null, command: a, timeout: null} -> {args: null, cancelation: null, command: b, timeout: null}  [template]"]
    );
}

const COUNT_OVERFLOW: &str = r#"
specificationVersion: jobtemplate-2023-09
name: Big
steps:
  - name: A
    parameterSpace:
      taskParameterDefinitions:
        - {name: F, type: INT, range: "1-4000000000"}
        - {name: G, type: INT, range: "1-4000000000"}
    script: {actions: {onRun: {command: a}}}
  - name: B
    parameterSpace:
      taskParameterDefinitions:
        - {name: F, type: INT, range: "1-4000000000"}
        - {name: G, type: INT, range: "1-4000000000"}
    script: {actions: {onRun: {command: a}}}
"#;

#[test]
fn task_counts_saturate_instead_of_overflowing() {
    let d = diff_templates(
        &text(COUNT_OVERFLOW),
        &text(&COUNT_OVERFLOW.replace("command: a", "command: b")),
        &opts(),
    )
    .unwrap();
    assert_eq!(
        d.stat(),
        DiffStat {
            steps_changed: 2,
            tasks_kept: u64::MAX,
            ..DiffStat::default()
        }
    );
    assert_eq!(
        d.stat().to_string(),
        "2 steps changed. Tasks: 18446744073709551615 kept, 0 added, 0 removed."
    );
}

const ADAPTIVE: &str = r#"
specificationVersion: jobtemplate-2023-09
extensions: [TASK_CHUNKING]
name: C
steps:
  - name: Chunked
    parameterSpace:
      taskParameterDefinitions:
        - name: F
          type: CHUNK[INT]
          range: "1-100"
          chunks: {defaultTaskCount: 10, targetRuntimeSeconds: 60, rangeConstraint: CONTIGUOUS}
    script: {actions: {onRun: {command: old, args: ["{{Task.Param.F}}"]}}}
"#;

#[test]
fn adaptive_chunking_task_count_is_unknown_with_a_note() {
    let d = diff_templates(
        &text(ADAPTIVE),
        &text(&ADAPTIVE.replace("command: old", "command: new")),
        &opts(),
    )
    .unwrap();
    assert_eq!(
        d.to_string(),
        "\
~ step Chunked
    ~ script.onRun.command: old -> new  [template]
    tasks: unknown (adaptive chunking)

1 step changed. Tasks: 0 kept, 0 added, 0 removed."
    );
    assert_eq!(
        d.notes,
        ["steps[Chunked]: cannot count tasks: the chunk size is adaptive (targetRuntimeSeconds), so the number of chunks is decided at run time"]
    );
    let s = d.step("Chunked").unwrap();
    assert_eq!(s.tasks, TaskSummary::Unchanged { count: 0 });
    assert_eq!(
        s.stat_line(),
        "~ Chunked  tasks unknown (adaptive chunking)"
    );
}

fn env_vars_job(n: usize) -> String {
    let vars: Vec<String> = (0..n).map(|i| format!("V{i}: v{i}")).collect();
    format!(
        r#"
specificationVersion: jobtemplate-2023-09
name: J
jobEnvironments:
  - name: E
    variables: {{{}}}
steps:
  - name: S
    script: {{actions: {{onRun: {{command: a}}}}}}
"#,
        vars.join(", ")
    )
}

#[test]
fn serialized_diff_is_deterministic_with_many_environment_variables() {
    let old = env_vars_job(10);
    let new = old.replace("command: a", "command: b");
    let runs: Vec<String> = (0..20)
        .map(|_| {
            let d = diff_templates(&text(&old), &text(&new), &opts()).unwrap();
            serde_json::to_string(&d).unwrap()
        })
        .collect();
    assert!(runs.iter().all(|r| r == &runs[0]));
    // The variables are written sorted by name.
    let v: serde_json::Value = serde_json::from_str(&runs[0]).unwrap();
    let names: Vec<&String> = v["oldJob"]["jobEnvironments"][0]["variables"]
        .as_object()
        .unwrap()
        .keys()
        .collect();
    assert_eq!(
        names,
        ["V0", "V1", "V2", "V3", "V4", "V5", "V6", "V7", "V8", "V9"]
    );
}

#[test]

fn job_document_is_deterministic_and_reads_back() {
    let old = env_vars_job(10);
    let job = text(&old).build().unwrap();
    let docs: Vec<String> = (0..20)
        .map(|_| serde_json::to_string(&JobDocument(&text(&old).build().unwrap())).unwrap())
        .collect();
    assert!(docs.iter().all(|r| r == &docs[0]));
    assert_eq!(job_from_str(&docs[0]).unwrap(), job);
}

#[test]
fn long_values_that_truncate_alike_show_where_they_differ() {
    let base = format!("{}a", "x".repeat(200));
    let t = |arg: &str| {
        format!(
            "specificationVersion: jobtemplate-2023-09\nname: J\nsteps:\n  - name: S\n    script: {{actions: {{onRun: {{command: c, args: [\"{arg}\"]}}}}}}\n"
        )
    };
    let d = diff_templates(
        &text(&t(&base)),
        &text(&t(&base.replace('a', "b"))),
        &opts(),
    )
    .unwrap();
    let c = &d.changes[0];
    let (b, a) = (c.before.as_ref().unwrap(), c.after.as_ref().unwrap());
    assert_ne!(b.text, a.text);
    assert_eq!(b.text, format!("…{}a\"]", "x".repeat(40)));
    assert_eq!(a.text, format!("…{}b\"]", "x".repeat(40)));
}

#[test]
fn unexpanded_step_change_is_shown_under_the_step() {
    let d = diff_templates(
        &input("render_v1.yaml"),
        &input("render_v2.yaml"),
        &DiffOptions::default().with_max_depth(1),
    )
    .unwrap();
    assert_eq!(
        d.to_string(),
        "\
~ step Render
    ~ (not expanded)  [template]
    tasks: 100 -> 200
      keep 100   Frame 1-100 × Eye left
      add  100   Frame 1-100 × Eye right

1 step changed. Tasks: 100 kept, 100 added, 0 removed."
    );
}

/// A job whose one step's `onRun` args are `args`, with an EXPR script
/// `let` and a step environment variable `V`.
fn stage_job(args: &str, script_let: &str, var: &str) -> String {
    format!(
        r#"
specificationVersion: jobtemplate-2023-09
extensions: [EXPR]
name: J
parameterDefinitions:
  - {{name: Scene, type: STRING, default: s}}
  - {{name: P, type: STRING, default: p}}
steps:
  - name: S
    parameterSpace:
      taskParameterDefinitions:
        - {{name: Frame, type: INT, range: "1-2"}}
    stepEnvironments:
      - name: E
        variables: {{V: "{var}"}}
    script:
      let: ["{script_let}"]
      actions:
        onRun: {{command: c, args: ["{args}"]}}
"#
    )
}

/// `(args, script let, variable V)` of one side.
type Fields<'a> = (&'a str, &'a str, &'a str);

#[test]
fn stage_is_run_time_only_for_task_or_session_references_in_format_strings() {
    let (a, l, v) = ("{{Param.Scene}} a", "f = 1", "x");
    let cases: &[(Fields, Fields, &str, Stage)] = &[
        // a command change with no run-time reference
        ((a, l, v), ("{{Param.Scene}} b", l, v), "args", Stage::Job),
        // `Task.` outside `{{ }}` is literal text
        (
            ("{{Param.P}} Task.log a", l, v),
            ("{{Param.P}} Task.log b", l, v),
            "args",
            Stage::Job,
        ),
        (
            ("Session.x a", l, v),
            ("Session.x b", l, v),
            "args",
            Stage::Job,
        ),
        // a run-time reference on the new side only, and on the old side only
        (
            (a, l, v),
            ("{{Session.WorkingDirectory}}", l, v),
            "args",
            Stage::RunTime,
        ),
        (
            ("{{Session.WorkingDirectory}}", l, v),
            (a, l, v),
            "args",
            Stage::RunTime,
        ),
        (
            ("{{Task.Param.Frame}} a", l, v),
            ("{{Task.Param.Frame}} b", l, v),
            "args",
            Stage::RunTime,
        ),
        // a script `let` references symbols without braces
        (
            (a, "f = Task.Param.Frame", v),
            (a, "f = Task.Param.Frame + 1", v),
            "let",
            Stage::RunTime,
        ),
        ((a, "f = 1", v), (a, "f = 2", v), "let", Stage::Job),
        // an environment variable
        (
            (a, l, "{{Session.WorkingDirectory}}/a"),
            (a, l, "{{Session.WorkingDirectory}}/b"),
            "variables.V",
            Stage::RunTime,
        ),
        ((a, l, "x"), (a, l, "y"), "variables.V", Stage::Job),
    ];
    for ((oa, ol, ov), (na, nl, nv), field, stage) in cases {
        let d = diff_templates(
            &text(&stage_job(oa, ol, ov)),
            &text(&stage_job(na, nl, nv)),
            &opts(),
        )
        .unwrap();
        let c = d
            .changes
            .iter()
            .find(|c| c.layer == Layer::Job && c.field == *field)
            .unwrap_or_else(|| panic!("no {field} change: {d}"));
        assert_eq!(c.stage, *stage, "{c}");
    }
}

#[test]
fn errors_name_the_parameter_set_in_a_parameter_set_diff() {
    let old = std::fs::read_to_string(sample_path("render_v1.yaml")).unwrap();
    let err = diff_parameter_sets(
        &text(&old),
        &params(&[]),
        &params(&[("Scene", "s")]),
        &opts(),
    )
    .unwrap_err();
    assert_eq!(
        err.to_string(),
        "Validation error: old parameters: Values missing for required job parameters: Scene"
    );
    let err = diff_parameter_sets(
        &text(&old),
        &params(&[("Scene", "s")]),
        &params(&[]),
        &opts(),
    )
    .unwrap_err();
    assert_eq!(
        err.to_string(),
        "Validation error: new parameters: Values missing for required job parameters: Scene"
    );
    let err =
        diff_parameter_sets(&text("name: x"), &params(&[]), &params(&[]), &opts()).unwrap_err();
    assert_eq!(
        err.to_string(),
        "Validation error: the template: Template is missing Open Job Description schema version key: specificationVersion"
    );
}

/// Every field the template layer records, for every parameter type that
/// has it, changed one at a time.
#[test]
fn every_parameter_definition_field_is_compared() {
    let cases: &[(&str, &str, &str, &str)] = &[
        ("STRING", "description: a", "description: b", "description"),
        ("STRING", "default: a", "default: b", "default"),
        ("STRING", "allowedValues: [a]", "allowedValues: [a, b]", "allowedValues"),
        ("STRING", "minLength: 0", "minLength: 1", "minLength"),
        ("STRING", "maxLength: 5", "maxLength: 6", "maxLength"),
        ("STRING", "userInterface: {control: LINE_EDIT}", "userInterface: {control: MULTILINE_EDIT}", "userInterface.control"),
        ("STRING", "userInterface: {label: a}", "userInterface: {label: b}", "userInterface.label"),
        ("STRING", "userInterface: {groupLabel: a}", "userInterface: {groupLabel: b}", "userInterface.groupLabel"),
        ("PATH", "default: a", "default: b", "default"),
        ("PATH", "allowedValues: ['ABS']", "allowedValues: ['ABS', b]", "allowedValues"),
        ("PATH", "minLength: 0", "minLength: 1", "minLength"),
        ("PATH", "maxLength: 100", "maxLength: 101", "maxLength"),
        ("PATH", "objectType: FILE", "objectType: DIRECTORY", "objectType"),
        ("PATH", "dataFlow: IN", "dataFlow: OUT", "dataFlow"),
        ("PATH", "objectType: FILE, userInterface: {control: CHOOSE_INPUT_FILE}", "objectType: FILE, userInterface: {control: CHOOSE_OUTPUT_FILE}", "userInterface.control"),
        ("PATH", "objectType: FILE, userInterface: {control: CHOOSE_INPUT_FILE, fileFilters: [{label: a, patterns: ['*']}]}", "objectType: FILE, userInterface: {control: CHOOSE_INPUT_FILE, fileFilters: [{label: b, patterns: ['*']}]}", "userInterface.fileFilters"),
        ("PATH", "objectType: FILE, userInterface: {control: CHOOSE_INPUT_FILE, fileFilterDefault: {label: a, patterns: ['*']}}", "objectType: FILE, userInterface: {control: CHOOSE_INPUT_FILE, fileFilterDefault: {label: b, patterns: ['*']}}", "userInterface.fileFilterDefault"),
        ("INT", "default: 1", "default: 2", "default"),
        ("INT", "allowedValues: [1]", "allowedValues: [1, 2]", "allowedValues"),
        ("INT", "minValue: 1", "minValue: 0", "minValue"),
        ("INT", "maxValue: 5", "maxValue: 6", "maxValue"),
        ("INT", "userInterface: {control: SPIN_BOX, singleStepDelta: 1}", "userInterface: {control: SPIN_BOX, singleStepDelta: 2}", "userInterface.singleStepDelta"),
        ("INT", "userInterface: {control: SPIN_BOX}", "userInterface: {control: HIDDEN}", "userInterface.control"),
        ("FLOAT", "default: 1.5", "default: 2.5", "default"),
        ("FLOAT", "allowedValues: [1.5]", "allowedValues: [1.5, 2]", "allowedValues"),
        ("FLOAT", "minValue: 1", "minValue: 0", "minValue"),
        ("FLOAT", "maxValue: 5", "maxValue: 6", "maxValue"),
        ("FLOAT", "userInterface: {control: SPIN_BOX, decimals: 1}", "userInterface: {control: SPIN_BOX, decimals: 2}", "userInterface.decimals"),
        ("FLOAT", "userInterface: {control: SPIN_BOX, singleStepDelta: 0.5}", "userInterface: {control: SPIN_BOX, singleStepDelta: 1.5}", "userInterface.singleStepDelta"),
        ("BOOL", "default: true", "default: false", "default"),
        ("BOOL", "userInterface: {control: CHECK_BOX}", "userInterface: {control: HIDDEN}", "userInterface.control"),
        ("RANGE_EXPR", "default: 1-2", "default: 1-3", "default"),
        ("RANGE_EXPR", "minLength: 1", "minLength: 2", "minLength"),
        ("RANGE_EXPR", "maxLength: 5", "maxLength: 6", "maxLength"),
        ("RANGE_EXPR", "userInterface: {control: LINE_EDIT}", "userInterface: {control: HIDDEN}", "userInterface.control"),
        ("LIST[STRING]", "default: [a]", "default: [b]", "default"),
        ("LIST[STRING]", "minLength: 1", "minLength: 0", "minLength"),
        ("LIST[STRING]", "maxLength: 5", "maxLength: 6", "maxLength"),
        ("LIST[STRING]", "item: {allowedValues: [a]}", "item: {allowedValues: [a, b]}", "item.allowedValues"),
        ("LIST[STRING]", "item: {minLength: 0}", "item: {minLength: 1}", "item.minLength"),
        ("LIST[STRING]", "item: {maxLength: 5}", "item: {maxLength: 6}", "item.maxLength"),
        ("LIST[STRING]", "userInterface: {control: LINE_EDIT_LIST}", "userInterface: {control: HIDDEN}", "userInterface.control"),
        ("LIST[PATH]", "default: [a]", "default: [b]", "default"),
        ("LIST[PATH]", "minLength: 1", "minLength: 0", "minLength"),
        ("LIST[PATH]", "maxLength: 5", "maxLength: 6", "maxLength"),
        ("LIST[PATH]", "objectType: FILE", "objectType: DIRECTORY", "objectType"),
        ("LIST[PATH]", "dataFlow: IN", "dataFlow: OUT", "dataFlow"),
        ("LIST[PATH]", "item: {allowedValues: ['ABS']}", "item: {allowedValues: ['ABS', b]}", "item.allowedValues"),
        ("LIST[PATH]", "item: {minLength: 1}", "item: {minLength: 2}", "item.minLength"),
        ("LIST[PATH]", "item: {maxLength: 100}", "item: {maxLength: 101}", "item.maxLength"),
        ("LIST[PATH]", "objectType: FILE, userInterface: {control: CHOOSE_INPUT_FILE_LIST}", "objectType: FILE, userInterface: {control: HIDDEN}", "userInterface.control"),
        ("LIST[PATH]", "objectType: FILE, userInterface: {control: CHOOSE_INPUT_FILE_LIST, fileFilters: [{label: a, patterns: ['*']}]}", "objectType: FILE, userInterface: {control: CHOOSE_INPUT_FILE_LIST, fileFilters: [{label: b, patterns: ['*']}]}", "userInterface.fileFilters"),
        ("LIST[PATH]", "objectType: FILE, userInterface: {control: CHOOSE_INPUT_FILE_LIST, fileFilterDefault: {label: a, patterns: ['*']}}", "objectType: FILE, userInterface: {control: CHOOSE_INPUT_FILE_LIST, fileFilterDefault: {label: b, patterns: ['*']}}", "userInterface.fileFilterDefault"),
        ("LIST[INT]", "default: [1]", "default: [2]", "default"),
        ("LIST[INT]", "minLength: 1", "minLength: 0", "minLength"),
        ("LIST[INT]", "maxLength: 5", "maxLength: 6", "maxLength"),
        ("LIST[INT]", "item: {allowedValues: [1, 2]}", "item: {allowedValues: [1, 2, 3]}", "item.allowedValues"),
        ("LIST[INT]", "item: {minValue: 0}", "item: {minValue: -1}", "item.minValue"),
        ("LIST[INT]", "item: {maxValue: 5}", "item: {maxValue: 6}", "item.maxValue"),
        ("LIST[INT]", "userInterface: {control: SPIN_BOX_LIST, singleStepDelta: 1}", "userInterface: {control: SPIN_BOX_LIST, singleStepDelta: 2}", "userInterface.singleStepDelta"),
        ("LIST[FLOAT]", "default: [1.5]", "default: [2.5]", "default"),
        ("LIST[FLOAT]", "minLength: 1", "minLength: 0", "minLength"),
        ("LIST[FLOAT]", "maxLength: 5", "maxLength: 6", "maxLength"),
        ("LIST[FLOAT]", "item: {allowedValues: [1.5, 2]}", "item: {allowedValues: [1.5, 2, 3]}", "item.allowedValues"),
        ("LIST[FLOAT]", "item: {minValue: 0}", "item: {minValue: -1}", "item.minValue"),
        ("LIST[FLOAT]", "item: {maxValue: 5}", "item: {maxValue: 6}", "item.maxValue"),
        ("LIST[FLOAT]", "userInterface: {control: SPIN_BOX_LIST, decimals: 1}", "userInterface: {control: SPIN_BOX_LIST, decimals: 2}", "userInterface.decimals"),
        ("LIST[FLOAT]", "userInterface: {control: SPIN_BOX_LIST, singleStepDelta: 0.5}", "userInterface: {control: SPIN_BOX_LIST, singleStepDelta: 1.5}", "userInterface.singleStepDelta"),
        ("LIST[BOOL]", "default: [true]", "default: [false]", "default"),
        ("LIST[BOOL]", "minLength: 1", "minLength: 0", "minLength"),
        ("LIST[BOOL]", "maxLength: 5", "maxLength: 6", "maxLength"),
        ("LIST[BOOL]", "userInterface: {control: CHECK_BOX_LIST}", "userInterface: {control: HIDDEN}", "userInterface.control"),
        ("LIST[LIST[INT]]", "default: [[1]]", "default: [[2]]", "default"),
        ("LIST[LIST[INT]]", "minLength: 1", "minLength: 0", "minLength"),
        ("LIST[LIST[INT]]", "maxLength: 5", "maxLength: 6", "maxLength"),
        ("LIST[LIST[INT]]", "item: {minLength: 1}", "item: {minLength: 0}", "item.minLength"),
        ("LIST[LIST[INT]]", "item: {maxLength: 5}", "item: {maxLength: 6}", "item.maxLength"),
        ("LIST[LIST[INT]]", "item: {item: {allowedValues: [1, 2]}}", "item: {item: {allowedValues: [1, 2, 3]}}", "item.item.allowedValues"),
        ("LIST[LIST[INT]]", "item: {item: {minValue: 0}}", "item: {item: {minValue: -1}}", "item.item.minValue"),
        ("LIST[LIST[INT]]", "item: {item: {maxValue: 5}}", "item: {item: {maxValue: 6}}", "item.item.maxValue"),
        ("LIST[LIST[INT]]", "userInterface: {label: a}", "userInterface: {control: HIDDEN, label: a}", "userInterface.control"),
    ];
    let t = |ty: &str, extra: &str| {
        format!(
            "specificationVersion: jobtemplate-2023-09\nextensions: [EXPR]\nname: J\nparameterDefinitions:\n  - {{name: X, type: \"{ty}\", {extra}}}\nsteps:\n  - name: S\n    script: {{actions: {{onRun: {{command: c}}}}}}\n"
        )
    };
    let mut failures = Vec::new();
    for (ty, old, new, field) in cases {
        let (a, b) = (
            t(ty, &old.replace("ABS", ABS)),
            t(ty, &new.replace("ABS", ABS)),
        );
        let side = |s: &str| text(s).param("X", default_for(ty));
        match diff_templates(&side(&a), &side(&b), &opts()) {
            Ok(d) => {
                let fields: Vec<&str> = d
                    .changes_at("parameterDefinitions[X]")
                    .map(|c| c.field.as_str())
                    .collect();
                if fields != [*field] {
                    failures.push(format!("{ty} {old} -> {new}: {fields:?}"));
                }
            }
            Err(e) => failures.push(format!("{ty} {old} -> {new}: {e}")),
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

/// An absolute PATH value on this platform.
const ABS: &str = if cfg!(windows) { r"C:\a" } else { "/a" };

/// A value of `ty` that satisfies every constraint in the table above.
fn default_for(ty: &str) -> String {
    match ty {
        "STRING" => "a".into(),
        "PATH" => ABS.into(),
        "INT" => "1".into(),
        "FLOAT" => "1.5".into(),
        "BOOL" => "true".into(),
        "RANGE_EXPR" => "1-2".into(),
        "LIST[STRING]" => r#"["a"]"#.into(),
        "LIST[PATH]" => serde_json::json!([ABS]).to_string(),
        "LIST[INT]" => "[1]".into(),
        "LIST[FLOAT]" => "[1.5]".into(),
        "LIST[BOOL]" => "[true]".into(),
        _ => "[[1]]".into(),
    }
}

const TASKS_6: &str = r#"
specificationVersion: jobtemplate-2023-09
name: T
steps:
  - name: Render
    parameterSpace:
      taskParameterDefinitions:
        - {name: N, type: INT, range: "1-6"}
    script: {actions: {onRun: {command: c}}}
"#;

#[test]
fn removed_tasks_are_counted_and_listed() {
    let d = diff_templates(
        &text(TASKS_6),
        &text(&TASKS_6.replace("1-6", "1-3")),
        &opts(),
    )
    .unwrap();
    assert_eq!(
        d.to_string(),
        "\
~ step Render
    ~ N: 1-6 -> 1-3  [template]
    tasks: 6 -> 3
      keep 3     N 1-3
      drop 3     N 4-6

1 step changed. Tasks: 3 kept, 0 added, 3 removed."
    );
    assert_eq!(d.steps[0].stat_line(), "~ Render  tasks 6 -> 3  (+0 -3)");
}

#[test]
fn added_and_removed_boxes_both_show_in_the_stat_line() {
    let old = TASKS_6.replace("\"1-6\"", "[1, 2, 3]");
    let new = TASKS_6.replace("\"1-6\"", "[1, 2, 4]");
    let d = diff_templates(&text(&old), &text(&new), &opts()).unwrap();
    assert_eq!(d.steps[0].stat_line(), "~ Render  tasks 3 -> 3  (+1 -1)");
    assert_eq!(
        d.steps[0].tasks.to_string(),
        "tasks: 3 -> 3\n  keep 2     N 1-2\n  add  1     N 4\n  drop 1     N 3"
    );
}

#[test]
fn replaced_tasks_stat_line_and_totals() {
    let new = TASKS_6.replace(
        "        - {name: N, type: INT, range: \"1-6\"}",
        "        - {name: N, type: INT, range: \"1-6\"}\n        - {name: M, type: INT, range: \"1-2\"}",
    );
    let d = diff_templates(&text(TASKS_6), &text(&new), &opts()).unwrap();
    assert_eq!(
        d.steps[0].stat_line(),
        "~ Render  tasks 6 -> 12  (replaced)"
    );
    assert_eq!(
        d.stat().to_string(),
        "1 step changed. Tasks: 0 kept, 0 added, 0 removed, 12 replaced."
    );
}

#[test]
fn integer_lists_render_as_runs_only_with_three_values() {
    let d = |old: &str, new: &str| {
        diff_templates(
            &text(&TASKS_6.replace("\"1-6\"", old)),
            &text(&TASKS_6.replace("\"1-6\"", new)),
            &opts(),
        )
        .unwrap()
    };
    assert_eq!(
        d("[1, 3]", "[1, 3, 5]").steps[0].tasks.to_string(),
        "tasks: 2 -> 3\n  keep 2     N 1,3\n  add  1     N 5"
    );
    assert_eq!(
        d("[1, 3, 5]", "[1, 3, 5, 9]").steps[0].tasks.to_string(),
        "tasks: 3 -> 4\n  keep 3     N 1-5:2\n  add  1     N 9"
    );
    assert_eq!(
        d("[2]", "[1, 5, 7]").steps[0].tasks.to_string(),
        "tasks: 1 -> 3\n  add  3     N 1,5,7\n  drop 1     N 2"
    );
}

#[test]
fn job_from_str_round_trips_empty_lists() {
    let t = r#"
specificationVersion: jobtemplate-2023-09
extensions: [EXPR]
name: Empty
parameterDefinitions:
  - {name: LS, type: "LIST[STRING]", default: []}
  - {name: LI, type: "LIST[INT]", default: []}
  - {name: LF, type: "LIST[FLOAT]", default: []}
  - {name: LP, type: "LIST[PATH]", default: []}
  - {name: LB, type: "LIST[BOOL]", default: []}
  - {name: LL, type: "LIST[LIST[INT]]", default: []}
  - {name: LLE, type: "LIST[LIST[INT]]", default: [[]]}
steps:
  - name: Render
    script:
      actions:
        onRun: {command: echo}
"#;
    let job = text(t).build().unwrap();
    for text in [
        serde_json::to_string(&job).unwrap(),
        serde_saphyr::to_string(&job).unwrap(),
    ] {
        let back = job_from_str(&text).unwrap();
        assert_eq!(back, job);
        for (k, p) in &job.parameters {
            assert_eq!(
                back.parameters[k].value.expr_type(),
                p.value.expr_type(),
                "{k}"
            );
        }
    }
}

#[test]
fn parameter_order_is_deterministic_with_many_parameters() {
    let defs: Vec<String> = (0..10)
        .map(|i| format!("  - {{name: P{i}, type: STRING, default: a}}"))
        .collect();
    let t = format!(
        "specificationVersion: jobtemplate-2023-09\nname: J\nparameterDefinitions:\n{}\nsteps:\n  - name: S\n    script: {{actions: {{onRun: {{command: c}}}}}}\n",
        defs.join("\n")
    );
    let all_b: Vec<(String, String)> = (0..10).map(|i| (format!("P{i}"), "b".into())).collect();
    let run = || {
        let d = diff_parameter_sets(
            &text(&t),
            &params(&[]),
            &params(
                &all_b
                    .iter()
                    .map(|(k, v)| (k.as_str(), v.as_str()))
                    .collect::<Vec<_>>(),
            ),
            &opts(),
        )
        .unwrap();
        (
            d.to_string(),
            d.job.to_string(),
            serde_json::to_string(&d).unwrap(),
        )
    };
    let first = run();
    assert!(first.0.starts_with(
        "~ parameter P0: a -> b  [parameters]\n~ parameter P1: a -> b  [parameters]\n"
    ));
    for _ in 0..20 {
        assert_eq!(run(), first);
    }
}

#[test]
fn layer_and_stage_display_names() {
    assert_eq!(Layer::Template.to_string(), "template");
    assert_eq!(Layer::Job.to_string(), "job");
    let stages = [
        Stage::Template,
        Stage::Parameters,
        Stage::Job,
        Stage::RunTime,
    ];
    let names: Vec<String> = stages.iter().map(ToString::to_string).collect();
    assert_eq!(names, ["template", "parameters", "job", "run time"]);
    let origins = [
        Origin::Template,
        Origin::Parameters,
        Origin::Both,
        Origin::Job,
    ];
    let names: Vec<String> = origins.iter().map(ToString::to_string).collect();
    assert_eq!(names, ["template", "parameters", "both", "job"]);
}

#[test]
fn template_input_parameters_are_the_values_set() {
    let i = TemplateInput::new(sample("render_v1.yaml"))
        .param("Scene", "a")
        .params([("Frames".to_string(), ExprValue::String("1-2".into()))])
        .param("Scene", "b");
    assert_eq!(
        i.parameters(),
        &params(&[("Scene", "b"), ("Frames", "1-2")])
    );
}

const KINDS_OLD: &str = r#"
specificationVersion: jobtemplate-2023-09
name: K
parameterDefinitions:
  - {name: Gone, type: STRING, default: g}
jobEnvironments:
  - name: Kept
    script:
      actions:
        onEnter: {command: a}
  - name: GoneEnv
    variables: {A: a}
steps:
  - name: S
    stepEnvironments:
      - name: StepEnv
        variables: {A: a}
    parameterSpace:
      taskParameterDefinitions:
        - {name: N, type: INT, range: "1-2"}
    hostRequirements:
      amounts: [{name: amount.worker.vcpu, min: 1}]
    script:
      actions:
        onRun: {command: a}
      embeddedFiles:
        - {name: F, type: TEXT, data: x}
  - name: GoneStep
    script: {actions: {onRun: {command: a}}}
"#;

const KINDS_NEW: &str = r#"
specificationVersion: jobtemplate-2023-09
name: K
parameterDefinitions:
  - {name: New, type: STRING, default: n}
jobEnvironments:
  - name: Kept
    script:
      actions:
        onEnter: {command: b}
        onExit: {command: bye}
  - name: NewEnv
    script:
      actions:
        onEnter: {command: a}
steps:
  - name: S
    parameterSpace:
      taskParameterDefinitions:
        - {name: N, type: INT, range: "1-2"}
        - {name: M, type: STRING, range: [x]}
    script:
      actions:
        onRun: {command: b}
      embeddedFiles:
        - {name: G, type: TEXT, data: y}
  - name: NewStep
    script: {actions: {onRun: {command: a}}}
"#;

/// For every node kind and every depth, a whole-node change carries the
/// sides that exist: an added node its new value, a removed node its old
/// value, an unexpanded node both.
#[test]
fn every_whole_node_change_carries_the_sides_that_exist() {
    let mut seen = std::collections::BTreeSet::new();
    for depth in [None, Some(1), Some(2), Some(3), Some(4), Some(5)] {
        let mut o = opts();
        if let Some(n) = depth {
            o = o.with_max_depth(n);
        }
        let d = diff_templates(&text(KINDS_OLD), &text(KINDS_NEW), &o).unwrap();
        for c in d
            .changes
            .iter()
            .filter(|c| c.layer == Layer::Job && c.field.is_empty())
        {
            let id = d.job.find(&c.path).unwrap();
            let node = d.job.node(id).unwrap();
            seen.insert(format!("{:?} {:?}", node.kind, node.status));
            let (b, a) = (c.before.is_some(), c.after.is_some());
            let want = match node.status {
                DiffStatus::Added => (false, true),
                DiffStatus::Removed => (true, false),
                _ => (true, true),
            };
            assert_eq!((b, a), want, "depth {depth:?}: {c}");
        }
    }
    let seen: Vec<String> = seen.into_iter().collect();
    assert_eq!(
        seen,
        [
            "Action Added",
            "Action Modified",
            "EmbeddedFile Added",
            "EmbeddedFile Removed",
            "Environment Added",
            "Environment Modified",
            "Environment Removed",
            "EnvironmentScript Modified",
            "HostRequirements Removed",
            "JobParameter Added",
            "JobParameter Removed",
            "ParameterSpace Modified",
            "Step Added",
            "Step Modified",
            "Step Removed",
            "StepScript Modified",
            "TaskParameter Added",
        ]
    );
}

#[test]
fn job_document_matches_the_job_serde_form_up_to_variable_order() {
    let job = input("render_v2.yaml")
        .environment(TemplateSource::Text(
            "specificationVersion: environment-2023-09\nenvironment:\n  name: Env\n  variables: {A: a}\n  script: {actions: {onEnter: {command: x}}}\n".into(),
        ))
        .build()
        .unwrap();
    let mut job = job;
    // A step environment, so every DocEnvironment path is written.
    job.steps[0].step_environments = job.job_environments.clone();
    assert_eq!(
        serde_json::to_string(&JobDocument(&job)).unwrap(),
        serde_json::to_string(&job).unwrap()
    );
    assert_eq!(
        serde_saphyr::to_string(&JobDocument(&job)).unwrap(),
        serde_saphyr::to_string(&job).unwrap()
    );
}

#[test]
fn product_counts_saturate() {
    let b = |count| BoxSummary {
        count,
        dims: vec![],
    };
    let s = StepSummary {
        name: "S".into(),
        status: DiffStatus::Modified,
        old_index: Some(0),
        new_index: Some(0),
        tasks: TaskSummary::Product {
            kept: b(u64::MAX),
            added: vec![b(1), b(u64::MAX)],
            removed: vec![b(2)],
        },
        tasks_unknown: None,
    };
    assert_eq!(
        s.stat_line(),
        "~ S  tasks 18446744073709551615 -> 18446744073709551615  (+18446744073709551615 -2)"
    );
}
