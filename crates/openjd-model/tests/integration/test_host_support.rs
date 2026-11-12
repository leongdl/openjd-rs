// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// Copyright by contributors to this project.
// SPDX-License-Identifier: (Apache-2.0 OR MIT)

//! Host support functions in `job::template_diff` (unstable): template
//! fragments, the job-scope symbol table, the job digest, building from
//! preprocessed values, and the parameter merge. See
//! `specs/model/host-support.md`.

use openjd_expr::path_mapping::PathFormat;
use openjd_expr::ExprValue;
use openjd_model::job::diff::{diff_jobs, DiffOptions};
use openjd_model::job::template_diff::{
    build_job_symbol_table, diff_templates, job_digest, merge_parameters,
    template_job_environment_fragment, template_step_fragment, TemplateInput, TemplateSource,
};
use openjd_model::job::Job;
use openjd_model::{
    decode_job_template, preprocess_job_parameters, CallerLimits, JobParameterInputValues,
    JobParameterType, JobParameterValue, JobParameterValues, PathParameterOptions,
};
use serde_json::Value;

const V1: &str = include_str!("../../examples/job_diff/v1.yaml");
const V2: &str = include_str!("../../examples/job_diff/v2.yaml");
const CAMS1: &str = include_str!("../../examples/job_diff/cams_v1.yaml");
const CAMS2: &str = include_str!("../../examples/job_diff/cams_v2.yaml");

fn doc(yaml: &str) -> Value {
    serde_saphyr::from_str(yaml).unwrap()
}

/// Absolute on the host: preprocessing requires an absolute template dir.
const TEMPLATE_DIR: &str = if cfg!(windows) { r"C:\t" } else { "/t" };
const CWD: &str = if cfg!(windows) { r"C:\work" } else { "/work" };
const SEP: &str = if cfg!(windows) { r"\" } else { "/" };

fn text(yaml: &str) -> TemplateInput {
    TemplateInput::new(TemplateSource::Text(yaml.to_string()))
        .template_dir(TEMPLATE_DIR)
        .cwd(CWD)
}

// ─── template fragments ─────────────────────────────────────────────

#[test]
fn a_step_fragment_is_the_step_as_written() {
    let fragment = template_step_fragment(&doc(V2), "Denoise").unwrap();
    let want = doc(r#"
name: Denoise
dependencies: [{dependsOn: Render}]
stepEnvironments:
  - name: Gpu
    variables: {CUDA_VISIBLE_DEVICES: "0"}
script:
  actions:
    onRun: {command: oidn, args: ["frames"]}
"#);
    assert_eq!(fragment.step, want);
    assert_eq!(
        fragment.step.to_string(),
        r#"{"dependencies":[{"dependsOn":"Render"}],"name":"Denoise","script":{"actions":{"onRun":{"args":["frames"],"command":"oidn"}}},"stepEnvironments":[{"name":"Gpu","variables":{"CUDA_VISIBLE_DEVICES":"0"}}]}"#
    );
    assert_eq!(
        fragment.step_environments,
        vec![doc("{name: Gpu, variables: {CUDA_VISIBLE_DEVICES: \"0\"}}")]
    );
    // Format strings are unresolved.
    let render = template_step_fragment(&doc(V2), "Render").unwrap();
    assert_eq!(
        render.step["parameterSpace"]["taskParameterDefinitions"][0]["range"],
        "{{Param.Frames}}"
    );
    assert!(render.step_environments.is_empty());
}

#[test]
fn a_job_environment_fragment_is_the_environment_as_written() {
    assert_eq!(
        template_job_environment_fragment(&doc(V2), "Conda")
            .unwrap()
            .to_string(),
        r#"{"name":"Conda","variables":{"CONDA_PACKAGES":"blender=4.2","PYTHONUNBUFFERED":"1"}}"#
    );
}

#[test]
fn a_missing_fragment_is_an_error() {
    let cases = [
        (
            template_step_fragment(&doc(V2), "Nope").map(|f| f.step),
            "Validation error: the template has no step named 'Nope'",
        ),
        (
            template_step_fragment(&doc("name: X"), "Nope").map(|f| f.step),
            "Validation error: the template document has no 'steps' list",
        ),
        (
            template_step_fragment(&doc("steps: [{name: S, stepEnvironments: {name: E}}]"), "S")
                .map(|f| f.step),
            "Validation error: the 'stepEnvironments' of step 'S' is not a list",
        ),
        (
            template_job_environment_fragment(&doc(V2), "Nope"),
            "Validation error: the template has no job environment named 'Nope'",
        ),
        (
            template_job_environment_fragment(&doc("steps: []"), "E"),
            "Validation error: the template document has no 'jobEnvironments' list",
        ),
    ];
    for (got, want) in cases {
        assert_eq!(got.unwrap_err().to_string(), want);
    }
}

// ─── job-scope symbol table ─────────────────────────────────────────

const EVERY_TYPE: &str = r#"
specificationVersion: jobtemplate-2023-09
extensions: [EXPR]
name: "Sym {{Param.S}}"
parameterDefinitions:
  - {name: S, type: STRING, default: s}
  - {name: I, type: INT, default: 2}
  - {name: F, type: FLOAT, default: "1.50"}
  - {name: P, type: PATH, default: in}
  - {name: B, type: BOOL, default: true}
  - {name: LS, type: "LIST[STRING]", default: [a, b]}
  - {name: LI, type: "LIST[INT]", default: [1, 2]}
  - {name: LF, type: "LIST[FLOAT]", default: [0.5]}
  - {name: LP, type: "LIST[PATH]", default: [x]}
  - {name: LB, type: "LIST[BOOL]", default: [false]}
  - {name: LL, type: "LIST[LIST[INT]]", default: [[1, 2], [3]]}
jobEnvironments:
  - name: All
    variables:
      JOB: "{{Job.Name}}"
      S: "{{Param.S}} {{RawParam.S}}"
      I: "{{Param.I}} {{RawParam.I}}"
      F: "{{Param.F}} {{RawParam.F}}"
      P: "{{RawParam.P}}"
      B: "{{Param.B}} {{RawParam.B}}"
      LS: "{{repr_json(Param.LS)}} {{repr_json(RawParam.LS)}}"
      LI: "{{repr_json(Param.LI)}} {{repr_json(RawParam.LI)}}"
      LF: "{{repr_json(Param.LF)}} {{repr_json(RawParam.LF)}}"
      LP: "{{repr_json(RawParam.LP)}}"
      LB: "{{repr_json(Param.LB)}} {{repr_json(RawParam.LB)}}"
      LL: "{{repr_json(Param.LL)}} {{repr_json(RawParam.LL)}}"
steps:
  - name: A
    script: {actions: {onRun: {command: a}}}
"#;

#[test]
fn the_job_symbol_table_holds_every_parameter_and_the_job_name() {
    let job = text(EVERY_TYPE).build().unwrap();
    let table = build_job_symbol_table(&job).unwrap();
    let under_t = |p: &str| serde_json::to_string(&format!("{TEMPLATE_DIR}{SEP}{p}")).unwrap();
    assert_eq!(
        serde_json::to_string(&table).unwrap(),
        format!(
            concat!(
                r#"[{{"name":"Job.Name","type":"string","value":"Sym s"}},"#,
                r#"{{"name":"Param.B","type":"bool","value":"true"}},"#,
                r#"{{"name":"Param.F","type":"float","value":"1.50"}},"#,
                r#"{{"name":"Param.I","type":"int","value":"2"}},"#,
                r#"{{"name":"Param.LB","type":"list[bool]","value":["false"]}},"#,
                r#"{{"name":"Param.LF","type":"list[float]","value":["0.5"]}},"#,
                r#"{{"name":"Param.LI","type":"list[int]","value":["1","2"]}},"#,
                r#"{{"name":"Param.LL","type":"list[list[int]]","value":[["1","2"],["3"]]}},"#,
                r#"{{"name":"Param.LS","type":"list[string]","value":["a","b"]}},"#,
                r#"{{"name":"Param.S","type":"string","value":"s"}},"#,
                r#"{{"name":"RawParam.B","type":"bool","value":"true"}},"#,
                r#"{{"name":"RawParam.F","type":"float","value":"1.50"}},"#,
                r#"{{"name":"RawParam.I","type":"int","value":"2"}},"#,
                r#"{{"name":"RawParam.LB","type":"list[bool]","value":["false"]}},"#,
                r#"{{"name":"RawParam.LF","type":"list[float]","value":["0.5"]}},"#,
                r#"{{"name":"RawParam.LI","type":"list[int]","value":["1","2"]}},"#,
                r#"{{"name":"RawParam.LL","type":"list[list[int]]","value":[["1","2"],["3"]]}},"#,
                r#"{{"name":"RawParam.LP","type":"list[string]","value":[{lp}]}},"#,
                r#"{{"name":"RawParam.LS","type":"list[string]","value":["a","b"]}},"#,
                r#"{{"name":"RawParam.P","type":"string","value":{p}}},"#,
                r#"{{"name":"RawParam.S","type":"string","value":"s"}}]"#,
            ),
            lp = under_t("x"),
            p = under_t("in")
        )
    );
    // An environment that reads every symbol gets exactly this table.
    let env = &job.job_environments.as_ref().unwrap()[0];
    assert_eq!(env.resolved_symtab.as_ref(), Some(&table));
}

#[test]
fn the_job_symbol_table_has_no_job_name_without_expr() {
    let job = text(V1).build().unwrap();
    assert_eq!(
        serde_json::to_string(&build_job_symbol_table(&job).unwrap()).unwrap(),
        concat!(
            r#"[{"name":"Param.Frames","type":"string","value":"1-10"},"#,
            r#"{"name":"Param.Scene","type":"string","value":"shot010.blend"},"#,
            r#"{"name":"RawParam.Frames","type":"string","value":"1-10"},"#,
            r#"{"name":"RawParam.Scene","type":"string","value":"shot010.blend"}]"#,
        )
    );
}

// ─── job digest ─────────────────────────────────────────────────────

#[test]
fn the_digest_ignores_map_order() {
    let a = text(EVERY_TYPE).build().unwrap();
    let mut b = text(EVERY_TYPE).build().unwrap();
    b.parameters.reverse();
    b.parameters.move_index(0, 3);
    let env = &mut b.job_environments.as_mut().unwrap()[0];
    let vars: Vec<_> = env.variables.take().unwrap().into_iter().collect();
    env.variables = Some(vars.into_iter().rev().collect());
    assert_eq!(job_digest(&a).unwrap(), job_digest(&b).unwrap());
    let d = job_digest(&a).unwrap();
    assert_eq!(d.len(), 64);
    assert!(d.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')), "{d}");
    // Pinned: a change here is a change of the document format.
    assert_eq!(
        job_digest(&text(V1).build().unwrap()).unwrap(),
        "95423ad8a6e4bc246de8ea22402a20597a164af1a563defb458a7f3ba1ed3cc5"
    );
}

#[test]
fn any_diff_changes_the_digest() {
    let pairs: Vec<(Job, Job)> = vec![
        (text(V1).build().unwrap(), text(V2).build().unwrap()),
        (
            text(V1).build().unwrap(),
            text(V1).param("Frames", "1-5").build().unwrap(),
        ),
        (
            text(V1).build().unwrap(),
            text(V1).param("Scene", "shot020.blend").build().unwrap(),
        ),
        (text(CAMS1).build().unwrap(), text(CAMS2).build().unwrap()),
        (
            text(EVERY_TYPE).build().unwrap(),
            text(EVERY_TYPE).param("F", "1.5").build().unwrap(),
        ),
    ];
    for (a, b) in &pairs {
        assert!(diff_jobs(a, b, &DiffOptions::default()).is_changed());
        assert_ne!(job_digest(a).unwrap(), job_digest(b).unwrap());
    }
}

// ─── building from stored values ────────────────────────────────────

const OUT: &str = r#"
specificationVersion: jobtemplate-2023-09
name: Out
parameterDefinitions:
  - {name: Out, type: PATH}
steps:
  - name: A
    script: {actions: {onRun: {command: a, args: ["{{RawParam.Out}}"]}}}
"#;

/// `Out` preprocessed as a Windows path, as a Windows host stores it.
fn stored_windows_out() -> JobParameterValues {
    let template = decode_job_template(doc(OUT), None, &CallerLimits::default()).unwrap();
    let input: JobParameterInputValues = [("Out".to_string(), ExprValue::String("out".into()))]
        .into_iter()
        .collect();
    preprocess_job_parameters(
        &template,
        &input,
        &[],
        &PathParameterOptions {
            job_template_dir: r"C:\t",
            current_working_dir: r"C:\work",
            path_format: PathFormat::Windows,
            allow_template_dir_walk_up: false,
            allow_uri_path_values: false,
        },
    )
    .unwrap()
}

fn out_value(job: &Job) -> String {
    job.parameters["Out"].value.to_display_string()
}

#[test]
fn preprocessed_values_are_built_as_given() {
    let stored = stored_windows_out();
    assert_eq!(stored["Out"].value.to_display_string(), r"C:\work\out");
    let job = text(OUT).preprocessed(stored.clone()).build().unwrap();
    assert_eq!(out_value(&job), r"C:\work\out");
    // A second preprocess resolves the value for this host. On a POSIX host
    // a Windows path is relative, so it is joined to the working directory.
    let again: JobParameterInputValues = stored
        .iter()
        .map(|(k, v)| (k.clone(), v.value.clone()))
        .collect();
    let rebuilt = text(OUT).params(again).build().unwrap();
    if cfg!(windows) {
        assert_eq!(out_value(&rebuilt), r"C:\work\out");
    } else {
        assert_eq!(out_value(&rebuilt), r"/work/C:\work\out");
    }
}

#[test]
fn preprocessed_values_replace_input_values_and_still_meet_constraints() {
    let stored = stored_windows_out();
    let job = text(OUT)
        .param("Out", "ignored")
        .preprocessed(stored.clone())
        .build()
        .unwrap();
    assert_eq!(out_value(&job), r"C:\work\out");
    // The same preprocessed values on both sides: nothing differs.
    let same = diff_templates(
        &text(OUT).preprocessed(stored.clone()),
        &text(OUT).preprocessed(stored),
        &DiffOptions::default(),
    )
    .unwrap();
    assert!(!same.is_changed());
    // `create_job` still checks constraints.
    let limited = OUT.replace("type: PATH}", "type: PATH, maxLength: 3}");
    let err = text(&limited)
        .preprocessed(stored_windows_out())
        .build()
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        "Validation error: Parameter 'Out': value length 11 exceeds maximum 3"
    );
}

// ─── parameter merge ────────────────────────────────────────────────

const OLD: &str = r#"
specificationVersion: jobtemplate-2023-09
name: M
parameterDefinitions:
  - {name: A, type: STRING, default: a}
  - {name: B, type: INT, default: 1}
  - {name: C, type: STRING, default: c}
  - {name: E, type: STRING, default: e}
steps:
  - name: S
    script: {actions: {onRun: {command: x}}}
"#;

const NEW: &str = r#"
specificationVersion: jobtemplate-2023-09
name: M
parameterDefinitions:
  - {name: A, type: STRING, default: a2}
  - {name: B, type: STRING, default: b2}
  - {name: C, type: STRING, default: c2}
  - {name: New, type: INT, default: 7}
steps:
  - name: S
    script: {actions: {onRun: {command: x}}}
"#;

fn sorted(values: &JobParameterInputValues) -> Vec<String> {
    let mut v: Vec<String> = values
        .iter()
        .map(|(k, v)| format!("{k}={}", serde_json::to_string(v).unwrap()))
        .collect();
    v.sort();
    v
}

#[test]
fn merge_keeps_stored_values_the_new_template_still_declares() {
    let old = text(OLD)
        .param("A", "x")
        .param("B", "5")
        .param("C", "y")
        .param("E", "z")
        .build()
        .unwrap();
    let stored: JobParameterValues = old
        .parameters
        .values()
        .map(|p| {
            (
                p.name.clone(),
                JobParameterValue {
                    param_type: p.param_type,
                    value: p.value.clone(),
                },
            )
        })
        .collect();
    let new_template = decode_job_template(doc(NEW), None, &CallerLimits::default()).unwrap();
    let requested: JobParameterInputValues = [
        ("C".to_string(), ExprValue::String("req".into())),
        ("Z".to_string(), ExprValue::String("undeclared".into())),
    ]
    .into_iter()
    .collect();
    let merged = merge_parameters(&stored, &requested, &new_template, &[]).unwrap();
    // A kept (same type); B retyped and E removed: left out; C requested;
    // New: left out; Z requested though undeclared.
    assert_eq!(
        sorted(&merged),
        vec![
            r#"A={"String":"x"}"#,
            r#"C={"String":"req"}"#,
            r#"Z={"String":"undeclared"}"#,
        ]
    );
    // Without the undeclared request, the new template builds with its
    // defaults for B and N.
    let mut merged = merged;
    merged.remove("Z");
    let job = text(NEW).params(merged).build().unwrap();
    let mut built: Vec<String> = job
        .parameters
        .values()
        .map(|p| format!("{}={}", p.name, p.value.to_display_string()))
        .collect();
    built.sort();
    assert_eq!(built, vec!["A=x", "B=b2", "C=req", "New=7"]);
    assert_eq!(stored["B"].param_type, JobParameterType::Int);
}

#[test]
fn merge_leaves_out_a_retyped_parameter_without_a_default() {
    let stored: JobParameterValues = [(
        "B".to_string(),
        JobParameterValue {
            param_type: JobParameterType::Int,
            value: ExprValue::Int(5),
        },
    )]
    .into_iter()
    .collect();
    let retyped = NEW.replace(
        "{name: B, type: STRING, default: b2}",
        "{name: B, type: STRING}",
    );
    let template = decode_job_template(doc(&retyped), None, &CallerLimits::default()).unwrap();
    let merged =
        merge_parameters(&stored, &JobParameterInputValues::new(), &template, &[]).unwrap();
    assert!(merged.is_empty(), "{merged:?}");
    let err = text(&retyped).params(merged).build().unwrap_err();
    assert_eq!(
        err.to_string(),
        "Validation error: Values missing for required job parameters: B"
    );
}
