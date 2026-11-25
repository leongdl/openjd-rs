// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// Copyright by contributors to this project.
// SPDX-License-Identifier: (Apache-2.0 OR MIT)

//! Tests for `job::apply`. Most assert a whole plan through its `Display`
//! text; `single_edits` checks that each one-field edit plans the kind the
//! spec names, and that every kind is planned by some edit.

use std::collections::BTreeSet;

use crate::job::apply::{plan, ApplyPlan, MutationKind, OpAction, OpTarget};
use crate::job::{Job, StepDependency};
use crate::types::ModelExtension;
use crate::{create_job, decode_job_template, preprocess_job_parameters, CallerLimits};
use crate::{JobParameterInputValues, PathParameterOptions};

const BASE: &str = r#"
specificationVersion: jobtemplate-2023-09
name: Demo
parameterDefinitions:
  - {name: Scene, type: STRING, default: a.blend}
  - {name: Quality, type: STRING, default: low}
  - {name: Last, type: INT, default: 3}
jobEnvironments:
  - name: Conda
    variables: {PACKAGES: blender, QUALITY: "{{Param.Quality}}"}
    script:
      actions: {onEnter: {command: activate}}
  - name: Log
    variables: {LEVEL: info}
steps:
  - name: Prep
    script:
      actions: {onRun: {command: prep, args: ["{{Param.Scene}}"]}}
  - name: Render
    dependencies: [{dependsOn: Prep}]
    stepEnvironments: [{name: Gpu, variables: {X: a}}, {name: Lic, variables: {Y: b}}]
    hostRequirements:
      amounts: [{name: amount.worker.vcpu, min: 2}]
    parameterSpace:
      taskParameterDefinitions:
        - {name: Frame, type: INT, range: "1-{{Param.Last}}"}
    script:
      actions: {onRun: {command: render, args: ["{{Task.Param.Frame}}"]}}
"#;

fn job_with(yaml: &str, params: &[(&str, &str)]) -> Job {
    let v: serde_json::Value = serde_saphyr::from_str(yaml).unwrap();
    let extensions: Vec<_> = ModelExtension::ALL
        .iter()
        .map(ModelExtension::as_str)
        .collect();
    let jt = decode_job_template(v, Some(&extensions), &CallerLimits::default()).unwrap();
    let mut input = JobParameterInputValues::new();
    for (k, val) in params {
        input.insert((*k).into(), openjd_expr::ExprValue::String((*val).into()));
    }
    let options = PathParameterOptions {
        job_template_dir: "/tmp",
        current_working_dir: "/tmp",
        path_format: openjd_expr::path_mapping::PathFormat::Posix,
        allow_template_dir_walk_up: true,
        allow_uri_path_values: true,
    };
    let values = preprocess_job_parameters(&jt, &input, &[], &options).unwrap();
    create_job(&jt, &values, &jt.default_validation_context()).unwrap()
}

fn job(yaml: &str) -> Job {
    job_with(yaml, &[])
}

fn edited(yaml: &str, from: &str, to: &str) -> String {
    assert_eq!(
        yaml.matches(from).count(),
        1,
        "`{from}` is not in the template exactly once"
    );
    yaml.replace(from, to)
}

fn plan_text(old: &Job, new: &Job) -> String {
    plan(old, new).unwrap().to_string()
}

#[test]
fn identical_jobs_plan_nothing() {
    let p = plan(&job(BASE), &job(BASE)).unwrap();
    assert!(p.is_empty());
    assert_eq!(p.to_string(), "");
}

#[test]
fn a_parameter_read_by_one_step_modifies_that_step_only() {
    let old = job(BASE);
    let new = job_with(BASE, &[("Scene", "b.blend")]);
    assert_eq!(
        plan_text(&old, &new),
        "1. parameter Scene: modify\n\
         2. step Prep: modify [StepSymbols]\n"
    );
}

#[test]
fn a_parameter_read_by_a_job_environment_modifies_it() {
    let new = job_with(BASE, &[("Quality", "high")]);
    assert_eq!(
        plan_text(&job(BASE), &new),
        "1. parameter Quality: modify\n\
         2. job environment Conda: modify [EnvSymbols]\n"
    );
}

#[test]
fn a_parameter_space_change_is_a_kind_of_its_step_op() {
    let yaml = edited(BASE, "command: render,", "command: render-gpu,");
    let new = job_with(&yaml, &[("Last", "5")]);
    assert_eq!(
        plan_text(&job(BASE), &new),
        "1. parameter Last: modify\n\
         2. step Render: modify [StepScript, StepParameterSpace]\n"
    );
}

const ENCODE: &str = r#"
  - name: Encode
    dependencies: [{dependsOn: Render}]
    script:
      actions: {onRun: {command: encode}}
"#;

#[test]
fn an_added_step_comes_after_its_dependency() {
    // Encode is listed first but depends on Render, which is unchanged.
    let yaml = edited(BASE, "steps:\n", &format!("steps:{ENCODE}"));
    assert_eq!(plan_text(&job(BASE), &job(&yaml)), "1. step Encode: add\n");
}

#[test]
fn a_step_and_its_dependency_both_added_go_in_dependency_order() {
    // A depends on B and is listed before it, so list order would add A first.
    let added = r#"
  - name: A
    dependencies: [{dependsOn: B}]
    script: {actions: {onRun: {command: a}}}
  - name: B
    script: {actions: {onRun: {command: b}}}
"#;
    let yaml = edited(BASE, "steps:\n", &format!("steps:{added}"));
    assert_eq!(
        plan_text(&job(BASE), &job(&yaml)),
        "1. step B: add\n\
         2. step A: add\n"
    );
    // Reversed, the removes go dependents first.
    assert_eq!(
        plan_text(&job(&yaml), &job(BASE)),
        "1. step A: remove\n\
         2. step B: remove\n"
    );
}

#[test]
fn a_removed_step_goes_after_its_dependents_drop_it() {
    let yaml = edited(BASE, "    dependencies: [{dependsOn: Prep}]\n", "");
    let yaml = edited(
        &yaml,
        "  - name: Prep\n    script:\n      actions: {onRun: {command: prep, args: [\"{{Param.Scene}}\"]}}\n",
        "",
    );
    assert_eq!(
        plan_text(&job(BASE), &job(&yaml)),
        "1. step Render: modify [StepDependencies]\n\
         2. step Prep: remove\n"
    );
}

#[test]
fn a_step_that_gains_a_dependency_on_an_added_step_comes_after_it() {
    let yaml = edited(BASE, "  - name: Prep\n", "  - name: Pre\n    script: {actions: {onRun: {command: pre}}}\n  - name: Prep\n    dependencies: [{dependsOn: Pre}]\n");
    assert_eq!(
        plan_text(&job(BASE), &job(&yaml)),
        "1. step Pre: add\n\
         2. step Prep: modify [StepDependencies]\n"
    );
}

#[test]
fn step_removes_follow_the_old_graph_not_the_list() {
    // Late depends on Early and is listed first: list order reversed would
    // remove Early first, while Late still depends on it.
    let old = r#"
specificationVersion: jobtemplate-2023-09
name: Demo
steps:
  - name: Keep
    script: {actions: {onRun: {command: k}}}
  - name: Late
    dependencies: [{dependsOn: Early}]
    script: {actions: {onRun: {command: l}}}
  - name: Early
    script: {actions: {onRun: {command: e}}}
"#;
    let new = r#"
specificationVersion: jobtemplate-2023-09
name: Demo
steps:
  - name: Keep
    script: {actions: {onRun: {command: k}}}
"#;
    assert_eq!(
        plan_text(&job(old), &job(new)),
        "1. step Late: remove\n\
         2. step Early: remove\n"
    );
}

/// Job environment names as a store holding `old`'s list has them after
/// applying `p`: a `jobEnvironments.order` change puts the shared ones in
/// the new order, a remove deletes by name, and an add inserts at the
/// environment's index in `new`.
fn apply_env_ops(old: &Job, new: &Job, p: &ApplyPlan) -> Vec<String> {
    let names = |j: &Job| -> Vec<String> {
        j.job_environments
            .iter()
            .flatten()
            .map(|e| e.name.clone())
            .collect()
    };
    let (mut list, target) = (names(old), names(new));
    for op in p.ops() {
        match (&op.target, op.action) {
            (OpTarget::Job, _) if op.kinds.contains(&MutationKind::EnvReorder) => {
                let in_list = list.clone();
                let mut shared = target.iter().filter(|n| in_list.contains(n));
                for slot in list.iter_mut() {
                    if target.contains(slot) {
                        *slot = shared.next().unwrap().clone();
                    }
                }
            }
            (OpTarget::JobEnvironment(name), OpAction::Remove) => list.retain(|n| n != name),
            (OpTarget::JobEnvironment(name), OpAction::Add) => {
                let at = target.iter().position(|n| n == name).unwrap();
                assert!(at <= list.len(), "{name} inserted past the end: {list:?}");
                list.insert(at, name.clone());
            }
            _ => {}
        }
    }
    list
}

fn with_envs(names: &str) -> String {
    let mut yaml =
        String::from("specificationVersion: jobtemplate-2023-09\nname: Demo\njobEnvironments:\n");
    for (i, name) in names.split(',').enumerate() {
        yaml.push_str(&format!(
            "  - {{name: {name}, variables: {{N: \"{i}\"}}}}\n"
        ));
    }
    yaml.push_str("steps:\n  - name: S\n    script: {actions: {onRun: {command: s}}}\n");
    yaml
}

#[test]
fn job_environment_ops_rebuild_the_new_order() {
    for (from, to) in [
        ("A", "Z,A"),
        ("A,B", "B,A"),
        ("A,B,C", "C,X,A"),
        ("A,B,C,D", "D,Q,B,R"),
        ("A", "B"),
    ] {
        for (a, b) in [(from, to), (to, from)] {
            let (old, new) = (job(&with_envs(a)), job(&with_envs(b)));
            let p = plan(&old, &new).unwrap();
            assert_eq!(
                apply_env_ops(&old, &new, &p).join(","),
                b,
                "{a} -> {b}:\n{p}"
            );
        }
    }
}

#[test]
fn job_environment_removes_come_first_then_adds_in_new_order() {
    // The variables change too, because each one holds its index.
    let (old, new) = (job(&with_envs("A,B,C")), job(&with_envs("C,X,A")));
    assert_eq!(
        plan_text(&old, &new),
        "1. job: modify [EnvReorder]\n\
         2. job environment B: remove\n\
         3. job environment C: modify [EnvVariables]\n\
         4. job environment X: add\n\
         5. job environment A: modify [EnvVariables]\n"
    );
}

#[test]
fn a_job_whose_steps_have_no_order_is_an_error() {
    let mut cyclic = job(BASE);
    cyclic.steps[0].dependencies = Some(vec![StepDependency {
        depends_on: "Render".into(),
    }]);
    let err = plan(&job(BASE), &cyclic).unwrap_err().to_string();
    assert_eq!(
        err,
        "Validation error: A circular dependency was found in the step dependency graph:\nPrep -> Render -> Prep"
    );
    assert_eq!(plan(&cyclic, &job(BASE)).unwrap_err().to_string(), err);
    let mut dangling = job(BASE);
    dangling.steps[0].dependencies = Some(vec![StepDependency {
        depends_on: "Nope".into(),
    }]);
    assert_eq!(
        plan(&job(BASE), &dangling).unwrap_err().to_string(),
        "Validation error: Step 'Prep' depends on unknown step 'Nope'"
    );
}

/// Every kind, indexed by `index`, whose exhaustive match fails to compile
/// when a kind is added.
const ALL: [MutationKind; 16] = [
    MutationKind::JobMetadata,
    MutationKind::JobExtensions,
    MutationKind::StepReorder,
    MutationKind::EnvReorder,
    MutationKind::EnvAdd,
    MutationKind::EnvRemove,
    MutationKind::EnvMetadata,
    MutationKind::EnvVariables,
    MutationKind::EnvScript,
    MutationKind::EnvSymbols,
    MutationKind::StepMetadata,
    MutationKind::StepScript,
    MutationKind::StepHostRequirements,
    MutationKind::StepDependencies,
    MutationKind::StepSymbols,
    MutationKind::StepParameterSpace,
];

fn index(kind: MutationKind) -> usize {
    use MutationKind::*;
    match kind {
        JobMetadata => 0,
        JobExtensions => 1,
        StepReorder => 2,
        EnvReorder => 3,
        EnvAdd => 4,
        EnvRemove => 5,
        EnvMetadata => 6,
        EnvVariables => 7,
        EnvScript => 8,
        EnvSymbols => 9,
        StepMetadata => 10,
        StepScript => 11,
        StepHostRequirements => 12,
        StepDependencies => 13,
        StepSymbols => 14,
        StepParameterSpace => 15,
    }
}

#[test]
fn all_lists_every_kind_once() {
    for (i, kind) in ALL.iter().enumerate() {
        assert_eq!(index(*kind), i, "{kind}");
    }
}

#[test]
fn single_edits() {
    let base = job(BASE);
    let reordered = |f: fn(&mut Job)| {
        let mut j = base.clone();
        f(&mut j);
        j
    };
    let edit = |from: &str, to: &str| job(&edited(BASE, from, to));
    let extra = "  - {name: Extra, type: STRING, default: x}\n";
    let with_extra = edit(
        "parameterDefinitions:\n",
        &format!("parameterDefinitions:\n{extra}"),
    );
    let cases: Vec<(&str, Job, Job, &str)> = vec![
        (
            "job name",
            base.clone(),
            edit("name: Demo", "name: Demo2"),
            "1. job: modify [JobMetadata]\n",
        ),
        (
            "job description",
            base.clone(),
            edit("name: Demo\n", "name: Demo\ndescription: d\n"),
            "1. job: modify [JobMetadata]\n",
        ),
        (
            "job extensions",
            base.clone(),
            edit("name: Demo\n", "name: Demo\nextensions: [TASK_CHUNKING]\n"),
            "1. job: modify [JobExtensions]\n",
        ),
        (
            "step order",
            base.clone(),
            reordered(|j| j.steps.reverse()),
            "1. job: modify [StepReorder]\n",
        ),
        (
            "job environment order",
            base.clone(),
            reordered(|j| j.job_environments.as_mut().unwrap().reverse()),
            "1. job: modify [EnvReorder]\n",
        ),
        (
            "parameter added",
            base.clone(),
            with_extra.clone(),
            "1. parameter Extra: add\n",
        ),
        (
            "parameter removed",
            with_extra,
            base.clone(),
            "1. parameter Extra: remove\n",
        ),
        (
            "parameter value",
            base.clone(),
            job_with(BASE, &[("Scene", "b.blend")]),
            "1. parameter Scene: modify\n2. step Prep: modify [StepSymbols]\n",
        ),
        (
            "job environment added",
            base.clone(),
            edit(
                "jobEnvironments:\n",
                "jobEnvironments:\n  - {name: New, variables: {A: b}}\n",
            ),
            "1. job environment New: add\n",
        ),
        (
            "job environment removed",
            base.clone(),
            edit("  - name: Log\n    variables: {LEVEL: info}\n", ""),
            "1. job environment Log: remove\n",
        ),
        (
            "job environment description",
            base.clone(),
            edit("  - name: Log\n", "  - name: Log\n    description: logs\n"),
            "1. job environment Log: modify [EnvMetadata]\n",
        ),
        (
            "job environment variables",
            base.clone(),
            edit("LEVEL: info", "LEVEL: debug"),
            "1. job environment Log: modify [EnvVariables]\n",
        ),
        (
            "job environment script",
            base.clone(),
            edit("command: activate", "command: activate2"),
            "1. job environment Conda: modify [EnvScript]\n",
        ),
        (
            "job environment symbols",
            base.clone(),
            job_with(BASE, &[("Quality", "high")]),
            "1. parameter Quality: modify\n2. job environment Conda: modify [EnvSymbols]\n",
        ),
        (
            "step description",
            base.clone(),
            edit("  - name: Prep\n", "  - name: Prep\n    description: d\n"),
            "1. step Prep: modify [StepMetadata]\n",
        ),
        (
            "step script",
            base.clone(),
            edit("command: prep", "command: prep2"),
            "1. step Prep: modify [StepScript]\n",
        ),
        (
            "step host requirements",
            base.clone(),
            edit("min: 2", "min: 4"),
            "1. step Render: modify [StepHostRequirements]\n",
        ),
        (
            "step dependencies",
            base.clone(),
            edit("    dependencies: [{dependsOn: Prep}]\n", ""),
            "1. step Render: modify [StepDependencies]\n",
        ),
        (
            "step environment added",
            base.clone(),
            edit(
                "{name: Lic, variables: {Y: b}}",
                "{name: Lic, variables: {Y: b}}, {name: New, variables: {Z: c}}",
            ),
            "1. step Render: modify [EnvAdd]\n",
        ),
        (
            "step environment removed",
            base.clone(),
            edit(", {name: Lic, variables: {Y: b}}", ""),
            "1. step Render: modify [EnvRemove]\n",
        ),
        (
            "step environment variables",
            base.clone(),
            edit("X: a", "X: c"),
            "1. step Render: modify [EnvVariables]\n",
        ),
        (
            "step environment description",
            base.clone(),
            edit(
                "{name: Gpu, variables",
                "{name: Gpu, description: g, variables",
            ),
            "1. step Render: modify [EnvMetadata]\n",
        ),
        (
            "step environment order",
            base.clone(),
            edit(
                "[{name: Gpu, variables: {X: a}}, {name: Lic, variables: {Y: b}}]",
                "[{name: Lic, variables: {Y: b}}, {name: Gpu, variables: {X: a}}]",
            ),
            "1. step Render: modify [EnvReorder]\n",
        ),
        (
            "step added",
            base.clone(),
            job(&format!("{BASE}{ENCODE}")),
            "1. step Encode: add\n",
        ),
        (
            "step removed",
            job(&format!("{BASE}{ENCODE}")),
            base.clone(),
            "1. step Encode: remove\n",
        ),
        (
            "task range",
            base.clone(),
            job_with(BASE, &[("Last", "5")]),
            "1. parameter Last: modify\n2. step Render: modify [StepParameterSpace]\n",
        ),
    ];
    let mut planned = BTreeSet::new();
    for (name, old, new, want) in &cases {
        let p = plan(old, new).unwrap();
        assert_eq!(p.to_string(), *want, "{name}");
        for op in p.ops() {
            // Only a modify of a job, job environment or step has kinds.
            let has_kinds =
                op.action == OpAction::Modify && !matches!(op.target, OpTarget::Parameter(_));
            assert_eq!(!op.kinds.is_empty(), has_kinds, "{name}: {op}");
            planned.extend(op.kinds.iter().copied());
        }
    }
    let missing: Vec<_> = ALL.iter().filter(|k| !planned.contains(k)).collect();
    assert!(missing.is_empty(), "no edit plans {missing:?}");
}

#[test]
fn a_step_environment_script_and_symbols_are_kinds_of_its_step() {
    // A step environment reads no symbols of its own: its reads show on the
    // step, so they plan StepSymbols.
    let yaml = edited(
        BASE,
        "{name: Gpu, variables: {X: a}}",
        "{name: Gpu, variables: {X: a}, script: {actions: {onEnter: {command: \"{{Param.Scene}}\"}}}}",
    );
    let new = job_with(&yaml, &[("Scene", "b.blend")]);
    assert_eq!(
        plan_text(&job(&yaml), &new),
        "1. parameter Scene: modify\n\
         2. step Prep: modify [StepSymbols]\n\
         3. step Render: modify [StepSymbols]\n"
    );
    let edited_script = edited(&yaml, "command: \"{{Param.Scene}}\"", "command: other");
    assert_eq!(
        plan_text(&job(&yaml), &job(&edited_script)),
        "1. step Render: modify [EnvScript, StepSymbols]\n"
    );
}

#[test]
fn op_parts_display_as_the_plan_writes_them() {
    let p = plan(&job(BASE), &job(&format!("{BASE}{ENCODE}"))).unwrap();
    let op = &p.ops()[0];
    assert_eq!(
        (
            &op.target,
            op.action,
            op.kinds.iter().copied().collect::<Vec<_>>()
        ),
        (&OpTarget::Step("Encode".into()), OpAction::Add, vec![])
    );
    assert_eq!(op.to_string(), "step Encode: add");
    assert!(!p.is_empty());
    for (target, text) in [
        (OpTarget::Job, "job"),
        (OpTarget::Parameter("P".into()), "parameter P"),
        (OpTarget::JobEnvironment("E".into()), "job environment E"),
        (OpTarget::Step("S".into()), "step S"),
    ] {
        assert_eq!(target.to_string(), text);
    }
    for (action, text) in [
        (OpAction::Add, "add"),
        (OpAction::Modify, "modify"),
        (OpAction::Remove, "remove"),
    ] {
        assert_eq!(action.to_string(), text);
    }
    assert_eq!(
        MutationKind::StepHostRequirements.to_string(),
        "StepHostRequirements"
    );
}

#[test]
fn every_tree_shape_field_has_its_kind() {
    use crate::job::apply::{env_kind, job_kind, step_kind};
    use MutationKind::*;
    // The own fields job-diff.md "Tree shape" lists for each node with kinds;
    // a parameter's fields need none, since its op is the whole value.
    let job_fields = [
        ("name", JobMetadata),
        ("description", JobMetadata),
        ("extensions", JobExtensions),
        ("jobEnvironments.order", EnvReorder),
        ("steps.order", StepReorder),
    ];
    let step_fields = [
        ("name", StepMetadata),
        ("description", StepMetadata),
        ("dependencies", StepDependencies),
        ("resolvedSymTab[Param.A]", StepSymbols),
        ("resolvedSymTab", StepSymbols),
        ("stepEnvironments.order", EnvReorder),
    ];
    let env_fields = [
        ("name", EnvMetadata),
        ("description", EnvMetadata),
        ("variables[K]", EnvVariables),
        ("variables", EnvVariables),
        ("resolvedSymTab[Param.A]", EnvSymbols),
        ("resolvedSymTab", EnvSymbols),
    ];
    for (field, kind) in job_fields {
        assert_eq!(job_kind(field).unwrap(), kind, "{field}");
    }
    for (field, kind) in step_fields {
        assert_eq!(step_kind(field).unwrap(), kind, "{field}");
    }
    for (field, kind) in env_fields {
        assert_eq!(env_kind(field).unwrap(), kind, "{field}");
    }
    assert_eq!(
        job_kind("other").unwrap_err().to_string(),
        "Validation error: the job diff names job field `other`, which no mutation kind maps"
    );
    for field in ["other", "variables", "variablesX", "extensions"] {
        assert!(step_kind(field).is_err(), "{field}");
    }
    for field in ["other", "dependencies", "resolvedSymTabX"] {
        assert!(env_kind(field).is_err(), "{field}");
    }
}

#[test]
fn whole_collection_fields_plan_their_kind() {
    // A collection that differs with no entry differing is one field named
    // for the collection, such as no `variables` against an empty map.
    use openjd_expr::SerializedSymbolTable;
    let base = job(BASE);
    let mut new = base.clone();
    new.job_environments.as_mut().unwrap()[1].variables = None;
    new.steps[0].resolved_symtab = Some(SerializedSymbolTable::from_value(serde_json::json!({})));
    new.job_environments.as_mut().unwrap()[0].resolved_symtab =
        Some(SerializedSymbolTable::from_value(serde_json::json!({})));
    let mut old = base;
    old.job_environments.as_mut().unwrap()[1].variables = Some(Default::default());
    assert_eq!(
        plan_text(&old, &new),
        "1. job environment Conda: modify [EnvSymbols]\n\
         2. job environment Log: modify [EnvVariables]\n\
         3. step Prep: modify [StepSymbols]\n"
    );
}

#[test]
fn a_job_child_outside_the_planned_collections_is_an_error() {
    // `diff_jobs` gives the job node no such child, so build the node.
    use crate::job::apply::check_job_children;
    use crate::job::diff::{DiffNode, DiffStatus};
    let node = |segment: &str, status| DiffNode {
        segment: segment.into(),
        status,
        field_changes: Vec::new(),
        children: Vec::new(),
    };
    let mut root = node("", DiffStatus::Modified);
    root.children = vec![
        node("parameters[A]", DiffStatus::Added),
        node("jobEnvironments[E]", DiffStatus::Removed),
        node("steps[S]", DiffStatus::Modified),
        node("other[X]", DiffStatus::Unchanged),
    ];
    assert!(check_job_children(&root).is_ok());
    root.children.push(node("other[X]", DiffStatus::Added));
    assert_eq!(
        check_job_children(&root).unwrap_err().to_string(),
        "Validation error: the job diff names job child `other[X]`, which no mutation kind maps"
    );
}
