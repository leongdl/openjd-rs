// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// Copyright by contributors to this project.
// SPDX-License-Identifier: (Apache-2.0 OR MIT)

//! Tests for `job::diff`.

use crate::job::diff::{
    diff_environments, diff_jobs, diff_steps, DiffNodeKind, DiffOptions, DiffStatus, DiffTree,
};
use crate::job::Job;
use crate::{
    convert_environment, create_job, decode_environment_template, decode_job_template,
    preprocess_job_parameters, CallerLimits, JobParameterInputValues,
};
use openjd_expr::ExprValue;

const BASE: &str = r#"
specificationVersion: jobtemplate-2023-09
name: Demo
parameterDefinitions:
  - {name: Frames, type: STRING, default: "1-10"}
  - {name: Scene, type: STRING, default: a.blend}
jobEnvironments:
  - name: Conda
    variables: {CONDA_PACKAGES: blender, PYTHONUNBUFFERED: "1"}
steps:
  - name: Prep
    script:
      actions: {onRun: {command: prep, args: ["{{Param.Scene}}"]}}
  - name: Render
    dependencies: [{dependsOn: Prep}]
    parameterSpace:
      taskParameterDefinitions:
        - {name: Frame, type: INT, range: "{{Param.Frames}}"}
    script:
      actions: {onRun: {command: render, args: ["{{Task.Param.Frame}}"]}}
"#;

fn job(yaml: &str, params: &[(&str, &str)]) -> Job {
    let v: serde_json::Value = serde_saphyr::from_str(yaml).unwrap();
    let jt = decode_job_template(v, None, &CallerLimits::default()).unwrap();
    let mut input = JobParameterInputValues::new();
    for (k, val) in params {
        input.insert((*k).into(), ExprValue::String((*val).into()));
    }
    let values = preprocess_job_parameters(
        &jt,
        &input,
        &[],
        &crate::PathParameterOptions {
            job_template_dir: "/tmp",
            current_working_dir: "/tmp",
            path_format: openjd_expr::path_mapping::PathFormat::Posix,
            allow_template_dir_walk_up: true,
            allow_uri_path_values: true,
        },
    )
    .unwrap();
    create_job(&jt, &values, &jt.default_validation_context()).unwrap()
}

fn status(t: &DiffTree, path: &str) -> DiffStatus {
    let id = t
        .find(path)
        .unwrap_or_else(|| panic!("no node at {path}\n{t}"));
    t.node(id).unwrap().status
}

fn fields(t: &DiffTree, path: &str) -> Vec<String> {
    let id = t.find(path).unwrap();
    t.node(id)
        .unwrap()
        .field_changes
        .iter()
        .map(|c| c.field.clone())
        .collect()
}

/// Every expanded modified node must explain itself: a field change or a
/// changed child. Otherwise a model field is missing from the diff.
fn assert_attributed(t: &DiffTree) {
    for (id, n) in t.changed() {
        if n.status == DiffStatus::Modified && n.expanded {
            let child_changed = n.children.iter().any(|&c| t.node(c).unwrap().is_changed());
            assert!(
                !n.field_changes.is_empty() || child_changed,
                "unattributed change at {}\n{t}",
                t.path(id)
            );
        }
    }
}

#[test]
fn identical_jobs_are_unchanged_with_a_single_root_node() {
    let t = diff_jobs(&job(BASE, &[]), &job(BASE, &[]), &DiffOptions::default());
    assert!(!t.is_changed());
    assert_eq!(t.nodes().len(), 1);
    assert_eq!(t.root().kind, DiffNodeKind::Job);
    assert!(!t.root().expanded);
}

#[test]
fn command_change_is_localized_to_one_action() {
    let new = BASE.replace("command: render", "command: render2");
    let t = diff_jobs(&job(BASE, &[]), &job(&new, &[]), &DiffOptions::default());
    assert!(t.is_changed());
    assert_eq!(status(&t, "steps[Prep]"), DiffStatus::Unchanged);
    assert_eq!(status(&t, "jobEnvironments[Conda]"), DiffStatus::Unchanged);
    assert_eq!(status(&t, "steps[Render]"), DiffStatus::Modified);
    assert_eq!(status(&t, "steps[Render].script"), DiffStatus::Modified);
    assert_eq!(fields(&t, "steps[Render].script.onRun"), vec!["command"]);
    // The unchanged step was not descended into.
    let prep = t.find("steps[Prep]").unwrap();
    assert!(t.node(prep).unwrap().children.is_empty());
    assert_attributed(&t);
}

#[test]
fn max_depth_one_reports_step_status_without_expanding() {
    let new = BASE.replace("command: render", "command: render2");
    let opts = DiffOptions::default().with_max_depth(1);
    let t = diff_jobs(&job(BASE, &[]), &job(&new, &[]), &opts);
    let render = t.node(t.find("steps[Render]").unwrap()).unwrap();
    assert_eq!(render.status, DiffStatus::Modified);
    assert!(!render.expanded);
    assert!(render.children.is_empty());
    assert!(t.nodes().iter().all(|n| n.depth <= 1));
}

#[test]
fn max_depth_zero_reports_only_the_root() {
    let new = BASE.replace("command: render", "command: render2");
    let opts = DiffOptions::default().with_max_depth(0);
    let t = diff_jobs(&job(BASE, &[]), &job(&new, &[]), &opts);
    assert!(t.is_changed());
    assert_eq!(t.nodes().len(), 1);
}

#[test]
fn unexpanded_step_can_be_diffed_on_its_own() {
    let (a, b) = (
        job(BASE, &[]),
        job(&BASE.replace("command: render", "command: render2"), &[]),
    );
    let t = diff_steps(&a.steps[1], &b.steps[1], &DiffOptions::default());
    assert_eq!(fields(&t, "script.onRun"), vec!["command"]);
}

#[test]
fn added_and_removed_steps() {
    let new = BASE
        .replace("  - name: Prep\n", "  - name: Setup\n")
        .replace("dependsOn: Prep", "dependsOn: Setup");
    let t = diff_jobs(&job(BASE, &[]), &job(&new, &[]), &DiffOptions::default());
    assert_eq!(status(&t, "steps[Prep]"), DiffStatus::Removed);
    assert_eq!(status(&t, "steps[Setup]"), DiffStatus::Added);
    assert_eq!(fields(&t, "steps[Render]"), vec!["dependencies"]);
    assert_attributed(&t);
}

#[test]
fn job_parameter_change_does_not_dirty_steps_unless_symtab_is_compared() {
    let (a, b) = (job(BASE, &[]), job(BASE, &[("Scene", "b.blend")]));
    let t = diff_jobs(&a, &b, &DiffOptions::default());
    assert_eq!(fields(&t, "parameters[Scene]"), vec!["value"]);
    assert_eq!(status(&t, "steps[Prep]"), DiffStatus::Unchanged);
    assert_attributed(&t);

    let t = diff_jobs(&a, &b, &DiffOptions::default().with_resolved_symtab(true));
    assert_eq!(fields(&t, "steps[Prep]"), vec!["resolvedSymTab"]);
    assert_attributed(&t);
}

#[test]
fn task_range_change_names_the_task_parameter() {
    let (a, b) = (job(BASE, &[]), job(BASE, &[("Frames", "1-20")]));
    let t = diff_jobs(&a, &b, &DiffOptions::default());
    assert_eq!(
        fields(
            &t,
            "steps[Render].parameterSpace.taskParameterDefinitions[Frame]"
        ),
        vec!["definition"]
    );
    assert_attributed(&t);
}

#[test]
fn step_reorder_is_one_order_change_not_n_modifications() {
    let new = r#"
specificationVersion: jobtemplate-2023-09
name: Demo
steps:
  - name: B
    script: {actions: {onRun: {command: b}}}
  - name: A
    script: {actions: {onRun: {command: a}}}
"#;
    let old = r#"
specificationVersion: jobtemplate-2023-09
name: Demo
steps:
  - name: A
    script: {actions: {onRun: {command: a}}}
  - name: B
    script: {actions: {onRun: {command: b}}}
"#;
    let t = diff_jobs(&job(old, &[]), &job(new, &[]), &DiffOptions::default());
    assert_eq!(fields(&t, ""), vec!["steps.order"]);
    assert_eq!(status(&t, "steps[A]"), DiffStatus::Unchanged);
    assert_eq!(status(&t, "steps[B]"), DiffStatus::Unchanged);
}

#[test]
fn environment_variable_change_is_keyed() {
    let new = BASE.replace("CONDA_PACKAGES: blender", "CONDA_PACKAGES: blender=4.2");
    let t = diff_jobs(&job(BASE, &[]), &job(&new, &[]), &DiffOptions::default());
    assert_eq!(
        fields(&t, "jobEnvironments[Conda]"),
        vec!["variables.CONDA_PACKAGES"]
    );
    assert_eq!(status(&t, "steps[Render]"), DiffStatus::Unchanged);
    assert_attributed(&t);
}

#[test]
fn environment_templates_diff_through_convert_environment() {
    let tpl = |cmd: &str| {
        let yaml = format!(
            r#"
specificationVersion: environment-2023-09
environment:
  name: Env
  script:
    actions:
      onEnter: {{command: {cmd}}}
"#
        );
        let v: serde_json::Value = serde_saphyr::from_str(&yaml).unwrap();
        let et = decode_environment_template(v, None, &CallerLimits::default()).unwrap();
        convert_environment(et.environment())
    };
    let t = diff_environments(&tpl("start"), &tpl("start2"), &DiffOptions::default());
    assert_eq!(t.root().kind, DiffNodeKind::Environment);
    assert_eq!(fields(&t, "script.onEnter"), vec!["command"]);
    assert!(t.find("script.onExit").is_none(), "absent on both sides");
    assert_attributed(&t);
}

#[test]
fn display_lists_only_changes() {
    let new = BASE.replace("command: render", "command: render2");
    let t = diff_jobs(&job(BASE, &[]), &job(&new, &[]), &DiffOptions::default());
    let s = t.to_string();
    assert!(s.contains("~ steps[Render]"), "{s}");
    assert!(s.contains("command: \"render\" -> \"render2\""), "{s}");
    assert!(!s.contains("Prep"), "{s}");
}
