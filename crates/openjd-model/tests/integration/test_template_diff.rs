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
    ~ Eye: [left] -> [left, right]  [template]
    ~ Frame: 1-100 -> 1-200  [parameters]
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
- parameter definition Gone: {default: g, type: STRING}  [template]
~ parameter definition I.maxValue: 10 -> 20  [template]
~ parameter definition I.minValue: 1 -> 2  [template]
+ parameter definition New: {default: n, type: STRING}  [template]
~ parameter definition S.allowedValues: [x, y] -> [x, y, z]  [template]
~ parameter definition S.description: a -> b  [template]
~ parameter definition S.maxLength: 5 -> 9  [template]
~ parameter definition S.minLength: 1 -> 0  [template]
~ parameter definition S.userInterface.label: L -> M  [template]
~ parameter definition T.default: "3" -> 3  [template]
~ parameter definition T.type: STRING -> INT  [template]
- parameter Gone: g  [template]
+ parameter New: n  [template]
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

0 steps changed. Tasks: 1 kept, 0 added, 0 removed."
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
        "Validation error: new template: Values missing for required job parameters: Scene"
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
    // The step's tasks come from the jobs, not the tree, so they still show.
    assert_eq!(
        d.to_string(),
        "\
~ parameter Frames: 1-100 -> 1-200  [parameters]
~ step Render
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
