// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// Copyright by contributors to this project.
// SPDX-License-Identifier: (Apache-2.0 OR MIT)

//! Tests for `job::template_diff`. Each case is a template pair with its
//! parameter values and the `Display` of the diff, so a case shows every
//! change the diff reports with its origin.

use crate::job::diff::{DiffNode, Origin};
use crate::job::template_diff::{diff_templates, TemplateDiffError, TemplateInput};
use crate::template::{EnvironmentTemplate, JobTemplate};
use crate::types::ModelExtension;
use crate::{
    decode_environment_template, decode_job_template, CallerLimits, JobParameterInputValues,
    PathParameterOptions,
};

const BASE: &str = r#"
specificationVersion: jobtemplate-2023-09
extensions: [EXPR]
name: Demo
parameterDefinitions:
  - {name: Frames, type: STRING, default: "1-3"}
  - {name: Quality, type: STRING, default: low}
steps:
  - name: Render
    parameterSpace:
      taskParameterDefinitions:
        - {name: Frame, type: INT, range: "{{Param.Frames}}"}
        - {name: Eye, type: STRING, range: [l, r]}
    script:
      actions:
        onRun: {command: render, args: ["{{Param.Quality}}"]}
"#;

fn extensions() -> Vec<&'static str> {
    ModelExtension::ALL
        .iter()
        .map(ModelExtension::as_str)
        .collect()
}

fn template(yaml: &str) -> JobTemplate {
    let v: serde_json::Value = serde_saphyr::from_str(yaml).unwrap();
    decode_job_template(v, Some(&extensions()), &CallerLimits::default()).unwrap()
}

fn environment(yaml: &str) -> EnvironmentTemplate {
    let v: serde_json::Value = serde_saphyr::from_str(yaml).unwrap();
    decode_environment_template(v, Some(&extensions()), &CallerLimits::default()).unwrap()
}

fn values(params: &[(&str, &str)]) -> JobParameterInputValues {
    params
        .iter()
        .map(|(k, v)| {
            (
                (*k).to_string(),
                openjd_expr::ExprValue::String((*v).into()),
            )
        })
        .collect()
}

/// The diff's `Display`, then `cross: <error>` when the cross build failed.
fn diff(
    old: &str,
    new: &str,
    old_params: &[(&str, &str)],
    new_params: &[(&str, &str)],
) -> Result<String, TemplateDiffError> {
    diff_with_environments((old, &[]), (new, &[]), old_params, new_params)
}

/// [`diff`], with each side's environment templates.
fn diff_with_environments(
    (old, old_envs): (&str, &[EnvironmentTemplate]),
    (new, new_envs): (&str, &[EnvironmentTemplate]),
    old_params: &[(&str, &str)],
    new_params: &[(&str, &str)],
) -> Result<String, TemplateDiffError> {
    let (old, new) = (template(old), template(new));
    let (old_values, new_values) = (values(old_params), values(new_params));
    let (old_ctx, new_ctx) = (
        old.default_validation_context(),
        new.default_validation_context(),
    );
    let input = |template, environments, parameters, ctx| TemplateInput {
        template,
        environments,
        parameters,
        path_options: PathParameterOptions {
            job_template_dir: "/jobs",
            current_working_dir: "/jobs",
            path_format: openjd_expr::path_mapping::PathFormat::Posix,
            allow_template_dir_walk_up: false,
            allow_uri_path_values: true,
        },
        ctx,
    };
    let d = diff_templates(
        &input(&old, old_envs, &old_values, &old_ctx),
        &input(&new, new_envs, &new_values, &new_ctx),
    )?;
    let shown = d.tree.to_string();
    assert_eq!(d.tree.is_changed(), !shown.is_empty(), "{shown}");
    assert_unique_segments(d.tree.root());
    let cross = d
        .cross_build_error
        .map(|e| format!("cross: {e}\n"))
        .unwrap_or_default();
    Ok(format!("{shown}{cross}"))
}

/// No node has two children with the same segment.
fn assert_unique_segments(node: &DiffNode) {
    let mut seen = std::collections::HashSet::new();
    for c in &node.children {
        assert!(seen.insert(&c.segment), "repeated child {}", c.segment);
        assert_unique_segments(c);
    }
}

fn edited(yaml: &str, edits: &[(&str, &str)]) -> String {
    edits.iter().fold(yaml.to_string(), |y, (from, to)| {
        assert_eq!(
            y.matches(from).count(),
            1,
            "`{from}` is not in the template once"
        );
        y.replace(from, to)
    })
}

/// Pairs of `(name, value)`, or `(from, to)` text edits.
type Pairs<'a> = &'a [(&'a str, &'a str)];

/// A case: its name, the edits that make the new template, the old and new
/// values, and the diff.
type Case<'a> = (&'a str, Pairs<'a>, Pairs<'a>, Pairs<'a>, String);

const SYM_LOW_HIGH: &str = r#"resolvedSymTab[Param.Quality]: {"type":"string","value":"low"} -> {"type":"string","value":"high"}"#;

/// Each case: name, edits that make the new template from `BASE`, old and
/// new parameter values, and the expected diff.
#[test]
fn origins() {
    let gpu = [("command: render", "command: render-gpu")];
    let cases: &[Case] = &[
        (
            "a template edit with the same values is the template's",
            &gpu,
            &[],
            &[],
            "~ steps[Render]\n  ~ script\n    ~ onRun\n      command: \"render\" -> \"render-gpu\" [template]\n".into(),
        ),
        (
            "a value change under the same template is the parameters'",
            &[],
            &[],
            &[("Quality", "high")],
            format!("~ parameters[Quality]\n  value: \"low\" -> \"high\" [parameters]\n~ steps[Render]\n  {SYM_LOW_HIGH} [parameters]\n"),
        ),
        (
            "a template edit and a value change on one step keep their own origins",
            &gpu,
            &[],
            &[("Quality", "high")],
            format!("~ parameters[Quality]\n  value: \"low\" -> \"high\" [parameters]\n~ steps[Render]\n  {SYM_LOW_HIGH} [parameters]\n  ~ script\n    ~ onRun\n      command: \"render\" -> \"render-gpu\" [template]\n"),
        ),
        (
            "a field both change is both",
            &[("range: \"{{Param.Frames}}\"", "range: \"{{Param.Frames}},9\"")],
            &[],
            &[("Frames", "1-4")],
            "~ parameters[Frames]\n  value: \"1-3\" -> \"1-4\" [parameters]\n~ steps[Render]\n  ~ parameterSpace\n    ~ taskParameterDefinitions[Frame]\n      range: \"1-3\" -> \"1-4,9\" [both]\n".into(),
        ),
        (
            "a default change the values do not override is the template's",
            &[("default: \"1-3\"", "default: \"1-5\"")],
            &[],
            &[],
            "~ parameterDefinitions[Frames]\n  default: \"1-3\" -> \"1-5\" [template]\n~ parameters[Frames]\n  value: \"1-3\" -> \"1-5\" [template]\n~ steps[Render]\n  ~ parameterSpace\n    ~ taskParameterDefinitions[Frame]\n      range: \"1-3\" -> \"1-5\" [template]\n".into(),
        ),
        (
            "an overridden default shows only on its definition",
            &[("default: \"1-3\"", "default: \"1-5\"")],
            &[("Frames", "1-4")],
            &[("Frames", "1-4")],
            "~ parameterDefinitions[Frames]\n  default: \"1-3\" -> \"1-5\" [template]\n".into(),
        ),
        (
            "a template that stops reading a changed parameter removes its symbol",
            &[("args: [\"{{Param.Quality}}\"]", "args: [q]")],
            &[],
            &[("Quality", "high")],
            "~ parameters[Quality]\n  value: \"low\" -> \"high\" [parameters]\n~ steps[Render]\n  resolvedSymTab[Param.Quality]: {\"type\":\"string\",\"value\":\"low\"} -> null [template]\n  ~ script\n    ~ onRun\n      args: [\"{{Param.Quality}}\"] -> [\"q\"] [template]\n".into(),
        ),
        (
            "a new required parameter takes its new value in the cross build",
            &[
                ("  - {name: Quality, type: STRING, default: low}", "  - {name: Quality, type: STRING, default: low}\n  - {name: Scene, type: STRING}"),
                ("args: [\"{{Param.Quality}}\"]", "args: [\"{{Param.Quality}}\", \"{{Param.Scene}}\"]"),
            ],
            &[],
            &[("Scene", "a")],
            "+ parameters[Scene] [template]\n~ steps[Render]\n  resolvedSymTab[Param.Scene]: null -> {\"type\":\"string\",\"value\":\"a\"} [template]\n  ~ script\n    ~ onRun\n      args: [\"{{Param.Quality}}\"] -> [\"{{Param.Quality}}\",\"{{Param.Scene}}\"] [template]\n".into(),
        ),
        (
            "a removed parameter the old values give is the template's",
            &[
                ("  - {name: Quality, type: STRING, default: low}\n", ""),
                ("args: [\"{{Param.Quality}}\"]", "args: [q]"),
            ],
            &[("Quality", "high")],
            &[],
            "- parameters[Quality] [template]\n~ steps[Render]\n  resolvedSymTab[Param.Quality]: {\"type\":\"string\",\"value\":\"high\"} -> null [template]\n  ~ script\n    ~ onRun\n      args: [\"{{Param.Quality}}\"] -> [\"q\"] [template]\n".into(),
        ),
        (
            "an added step is the template's",
            &[("steps:\n", "steps:\n  - name: Encode\n    script: {actions: {onRun: {command: encode}}}\n")],
            &[],
            &[],
            "+ steps[Encode] [template]\n".into(),
        ),
        (
            "an old value the new template rejects makes every job change both",
            &[("default: low}", "default: low, allowedValues: [low, mid]}")],
            &[("Quality", "high")],
            &[("Quality", "mid")],
            "~ parameterDefinitions[Quality]\n  allowedValues: null -> [\"low\",\"mid\"] [template]\n~ parameters[Quality]\n  value: \"high\" -> \"mid\" [both]\n~ steps[Render]\n  resolvedSymTab[Param.Quality]: {\"type\":\"string\",\"value\":\"high\"} -> {\"type\":\"string\",\"value\":\"mid\"} [both]\ncross: Validation error: Parameter 'Quality': value 'high' is not in allowed values\n".into(),
        ),
        (
            "a task parameter reorder is the template's",
            &[(
                "        - {name: Frame, type: INT, range: \"{{Param.Frames}}\"}\n        - {name: Eye, type: STRING, range: [l, r]}\n",
                "        - {name: Eye, type: STRING, range: [l, r]}\n        - {name: Frame, type: INT, range: \"{{Param.Frames}}\"}\n",
            )],
            &[],
            &[("Frames", "1-4")],
            "~ parameters[Frames]\n  value: \"1-3\" -> \"1-4\" [parameters]\n~ steps[Render]\n  ~ parameterSpace\n    taskParameterDefinitions.order: [\"Frame\",\"Eye\"] -> [\"Eye\",\"Frame\"] [template]\n    ~ taskParameterDefinitions[Frame]\n      range: \"1-3\" -> \"1-4\" [parameters]\n".into(),
        ),
        (
            "an old value from the old default stays the old value when the new template drops the default",
            &[("{name: Frames, type: STRING, default: \"1-3\"}", "{name: Frames, type: STRING}")],
            &[],
            &[("Frames", "1-4")],
            "~ parameterDefinitions[Frames]\n  default: \"1-3\" -> null [template]\n~ parameters[Frames]\n  value: \"1-3\" -> \"1-4\" [parameters]\n~ steps[Render]\n  ~ parameterSpace\n    ~ taskParameterDefinitions[Frame]\n      range: \"1-3\" -> \"1-4\" [parameters]\n".into(),
        ),
        (
            "a step the template adds is the template's even when it reads a changed parameter",
            &[("steps:\n", "steps:\n  - name: Encode\n    script: {actions: {onRun: {command: encode, args: [\"{{Param.Quality}}\"]}}}\n")],
            &[],
            &[("Quality", "high")],
            format!("~ parameters[Quality]\n  value: \"low\" -> \"high\" [parameters]\n~ steps[Render]\n  {SYM_LOW_HIGH} [parameters]\n+ steps[Encode] [template]\n"),
        ),
        (
            "a step added when the cross build fails is both",
            &[
                ("default: low}", "default: low, allowedValues: [low, mid]}"),
                ("steps:\n", "steps:\n  - name: Encode\n    script: {actions: {onRun: {command: encode}}}\n"),
            ],
            &[("Quality", "high")],
            &[("Quality", "mid")],
            "~ parameterDefinitions[Quality]\n  allowedValues: null -> [\"low\",\"mid\"] [template]\n~ parameters[Quality]\n  value: \"high\" -> \"mid\" [both]\n~ steps[Render]\n  resolvedSymTab[Param.Quality]: {\"type\":\"string\",\"value\":\"high\"} -> {\"type\":\"string\",\"value\":\"mid\"} [both]\n+ steps[Encode] [both]\ncross: Validation error: Parameter 'Quality': value 'high' is not in allowed values\n".into(),
        ),
        (
            "a step let changes the step without changing the job",
            &[("  - name: Render\n", "  - name: Render\n    let: [\"x = 1\"]\n")],
            &[],
            &[],
            "~ steps[Render]\n  let: null -> [\"x = 1\"] [template]\n".into(),
        ),
    ];
    for (name, edits, old_params, new_params, want) in cases {
        let got = diff(BASE, &edited(BASE, edits), old_params, new_params).unwrap();
        assert_eq!(&got, want, "{name}");
    }
}

/// Each case: an old and a new definition of `P`, and the change the
/// template layer reports for it. The job holds only the type and value.
#[test]
fn definition_fields() {
    let cases = [
        (
            "type: STRING, default: x, description: a",
            "type: STRING, default: x, description: b",
            "description: \"a\" -> \"b\"",
        ),
        (
            "type: STRING, default: a",
            "type: STRING, default: b",
            "default: \"a\" -> \"b\" [template]\n~ parameters[P]\n  value: \"a\" -> \"b\"",
        ),
        (
            "type: INT, default: 1, allowedValues: [1, 2]",
            "type: INT, default: 1, allowedValues: [1, 3]",
            "allowedValues: [1,2] -> [1,3]",
        ),
        (
            "type: FLOAT, default: 1, allowedValues: [1, 2]",
            "type: FLOAT, default: 1, allowedValues: [1, 2.5]",
            "allowedValues: [1.0,2.0] -> [1.0,2.5]",
        ),
        // A duplicate FLOAT value is the same set, as it is for STRING and INT.
        (
            "type: FLOAT, default: 1, allowedValues: [1, 2]",
            "type: FLOAT, default: 1, allowedValues: [1, 2, 2]",
            "",
        ),
        // Validation compares FLOAT values with `==`, so -0.0 and 0.0 are one value.
        (
            "type: FLOAT, default: 0, allowedValues: [0.0, -0.0]",
            "type: FLOAT, default: 0, allowedValues: [0.0]",
            "",
        ),
        (
            "type: \"LIST[FLOAT]\", default: [0], item: {allowedValues: [0.0, -0.0]}",
            "type: \"LIST[FLOAT]\", default: [0], item: {allowedValues: [0.0]}",
            "",
        ),
        // allowedValues are a set; order only changes how a submitter lists them.
        (
            "type: STRING, default: a, allowedValues: [a, b]",
            "type: STRING, default: a, allowedValues: [b, a]",
            "",
        ),
        (
            "type: INT, default: 1, minValue: 0",
            "type: INT, default: 1, minValue: 1",
            "minValue: 0 -> 1",
        ),
        (
            "type: FLOAT, default: 1, maxValue: 2",
            "type: FLOAT, default: 1, maxValue: 3",
            "maxValue: 2.0 -> 3.0",
        ),
        (
            "type: STRING, default: a, minLength: 1",
            "type: STRING, default: a, minLength: 0",
            "minLength: 1 -> 0",
        ),
        (
            "type: STRING, default: a, maxLength: 3",
            "type: STRING, default: a, maxLength: 4",
            "maxLength: 3 -> 4",
        ),
        (
            "type: \"LIST[STRING]\", default: [a], item: {allowedValues: [a]}",
            "type: \"LIST[STRING]\", default: [a], item: {allowedValues: [a, b]}",
            "item.allowedValues: [\"a\"] -> [\"a\",\"b\"]",
        ),
        (
            "type: \"LIST[INT]\", default: [1], item: {minValue: 0}",
            "type: \"LIST[INT]\", default: [1], item: {minValue: 1}",
            "item.minValue: 0 -> 1",
        ),
        (
            "type: \"LIST[FLOAT]\", default: [1], item: {maxValue: 2}",
            "type: \"LIST[FLOAT]\", default: [1], item: {maxValue: 3}",
            "item.maxValue: 2.0 -> 3.0",
        ),
        (
            "type: \"LIST[STRING]\", default: [a], item: {minLength: 1}",
            "type: \"LIST[STRING]\", default: [a], item: {minLength: 0}",
            "item.minLength: 1 -> 0",
        ),
        (
            "type: \"LIST[STRING]\", default: [a], item: {maxLength: 1}",
            "type: \"LIST[STRING]\", default: [a], item: {maxLength: 2}",
            "item.maxLength: 1 -> 2",
        ),
        (
            "type: \"LIST[LIST[INT]]\", default: [[1]], item: {item: {allowedValues: [1]}}",
            "type: \"LIST[LIST[INT]]\", default: [[1]], item: {item: {allowedValues: [1, 2]}}",
            "item.item.allowedValues: [1] -> [1,2]",
        ),
        (
            "type: \"LIST[LIST[INT]]\", default: [[1]], item: {item: {minValue: 0}}",
            "type: \"LIST[LIST[INT]]\", default: [[1]], item: {item: {minValue: 1}}",
            "item.item.minValue: 0 -> 1",
        ),
        (
            "type: \"LIST[LIST[INT]]\", default: [[1]], item: {item: {maxValue: 2}}",
            "type: \"LIST[LIST[INT]]\", default: [[1]], item: {item: {maxValue: 3}}",
            "item.item.maxValue: 2 -> 3",
        ),
        (
            "type: PATH, default: a, objectType: FILE",
            "type: PATH, default: a, objectType: DIRECTORY",
            "objectType: \"FILE\" -> \"DIRECTORY\"",
        ),
        (
            "type: PATH, default: a, dataFlow: IN",
            "type: PATH, default: a, dataFlow: OUT",
            "dataFlow: \"IN\" -> \"OUT\"",
        ),
        (
            "type: STRING, default: a, userInterface: {label: A}",
            "type: STRING, default: a, userInterface: {label: B}",
            "",
        ),
    ];
    let yaml = |def: &str| {
        format!(
            "specificationVersion: jobtemplate-2023-09\nextensions: [EXPR]\nname: Demo\n\
             parameterDefinitions:\n  - {{name: P, {def}}}\n\
             steps:\n  - name: S\n    script: {{actions: {{onRun: {{command: run}}}}}}\n"
        )
    };
    for (old, new, want) in cases {
        let want = if want.is_empty() {
            String::new()
        } else {
            format!("~ parameterDefinitions[P]\n  {want} [template]\n")
        };
        assert_eq!(
            diff(&yaml(old), &yaml(new), &[], &[]).unwrap(),
            want,
            "{old} -> {new}"
        );
    }
}

#[test]
fn a_step_let_joins_the_step_node_the_job_diff_made() {
    let old = "specificationVersion: jobtemplate-2023-09\nextensions: [EXPR]\nname: Demo\n\
               steps:\n  - name: S\n    script: {actions: {onRun: {command: a}}}\n";
    let new = edited(
        old,
        &[
            ("  - name: S\n", "  - name: S\n    let: [\"x = 1\"]\n"),
            ("command: a", "command: b"),
        ],
    );
    assert_eq!(
        diff(old, &new, &[], &[]).unwrap(),
        "~ steps[S]\n  let: null -> [\"x = 1\"] [template]\n  ~ script\n    ~ onRun\n      \
         command: \"a\" -> \"b\" [template]\n"
    );
}

/// Every step's `let` change is reported, in step order, when many steps
/// change.
#[test]
fn every_step_let_change_is_reported() {
    let n = 2000;
    let yaml = |x: u32| {
        let mut y = "specificationVersion: jobtemplate-2023-09\nextensions: [EXPR]\nname: Demo\n\
                     steps:\n"
            .to_string();
        for i in 0..n {
            y.push_str(&format!(
                "  - name: S{i}\n    let: [\"x = {x}\"]\n    \
                 script: {{actions: {{onRun: {{command: c}}}}}}\n"
            ));
        }
        y
    };
    let want: String = (0..n)
        .map(|i| format!("~ steps[S{i}]\n  let: [\"x = 1\"] -> [\"x = 2\"] [template]\n"))
        .collect();
    assert_eq!(diff(&yaml(1), &yaml(2), &[], &[]).unwrap(), want);
}

/// A retyped parameter the old side took from its default builds the cross
/// job as the same value given would: the old job's value when it coerces to
/// the new type, else its string form.
#[test]
fn a_retyped_parameter_from_its_default_builds_the_cross_job() {
    let yaml = |definition: &str, command: &str| {
        format!(
            "specificationVersion: jobtemplate-2023-09\nextensions: [EXPR]\nname: Demo\n\
             parameterDefinitions:\n  - {{name: Q, {definition}}}\n\
             steps:\n  - name: S\n    script: {{actions: {{onRun: {{command: {command}}}}}}}\n"
        )
    };
    // The old definition, the value it defaults to, the new type and value.
    let cases = [
        ("type: RANGE_EXPR, default: \"1-3\"", "1-3", "STRING", "q"),
        (
            "type: \"LIST[INT]\", default: [1]",
            "[1]",
            "\"LIST[STRING]\"",
            "[\"q\"]",
        ),
        (
            "type: \"LIST[BOOL]\", default: [true]",
            "[true]",
            "\"LIST[STRING]\"",
            "[\"q\"]",
        ),
    ];
    for (definition, value, new_type, new_value) in cases {
        let old = yaml(definition, "c");
        let new = yaml(&format!("type: {new_type}"), "d");
        let new_params = [("Q", new_value)];
        let from_default = diff(&old, &new, &[], &new_params).unwrap();
        let given = diff(&old, &new, &[("Q", value)], &new_params).unwrap();
        assert!(
            from_default.contains("command: \"c\" -> \"d\" [template]")
                && !from_default.contains("cross:"),
            "{definition}:\n{from_default}"
        );
        assert_eq!(from_default, given, "{definition}");
    }
    // The string form alone would refuse this: "2.0" is not an INT, 2.0 is.
    let old = yaml("type: FLOAT, default: \"2.0\"", "c");
    let new = yaml("type: INT", "d");
    let shown = diff(&old, &new, &[], &[("Q", "5")]).unwrap();
    assert!(
        shown.contains("command: \"c\" -> \"d\" [template]") && !shown.contains("cross:"),
        "{shown}"
    );
}

#[test]
fn a_side_that_does_not_build_is_named() {
    let bad = edited(BASE, &[("default: low}", "allowedValues: [low]}")]);
    let q = [("Quality", "high")];
    let old = diff(&bad, BASE, &q, &q).unwrap_err();
    let new = diff(BASE, &bad, &q, &q).unwrap_err();
    assert_eq!(
        old.to_string(),
        "old job: Validation error: Parameter 'Quality': value 'high' is not in allowed values"
    );
    assert_eq!(
        new.to_string(),
        "new job: Validation error: Parameter 'Quality': value 'high' is not in allowed values"
    );
}

#[test]
fn origin_displays_lowercase() {
    let shown: Vec<String> = [Origin::Template, Origin::Parameters, Origin::Both]
        .iter()
        .map(ToString::to_string)
        .collect();
    assert_eq!(shown, ["template", "parameters", "both"]);
}

#[test]
fn a_job_name_edited_to_the_value_it_resolved_to_is_no_change() {
    let old = edited(
        BASE,
        &[
            ("name: Demo\n", "name: \"{{Param.Shot}}\"\n"),
            (
                "parameterDefinitions:\n",
                "parameterDefinitions:\n  - {name: Shot, type: STRING, default: Shot010}\n",
            ),
        ],
    );
    let new = edited(&old, &[("name: \"{{Param.Shot}}\"\n", "name: Shot010\n")]);
    assert_eq!(diff(&old, &new, &[], &[]).unwrap(), "");
}

/// An environment template's parameter definitions are compared as merged
/// with the job template's.
#[test]
fn an_environment_template_definition_change_is_the_templates() {
    let env = |def: &str| {
        environment(&format!(
            "specificationVersion: environment-2023-09\nparameterDefinitions:\n  \
             - {{name: Pool, type: STRING, {def}}}\nenvironment:\n  name: Env\n  \
             variables: {{A: b}}\n"
        ))
    };
    let old = [env("default: a, description: one")];
    let new = [env("default: b, description: two")];
    assert_eq!(
        diff_with_environments((BASE, &old), (BASE, &new), &[], &[]).unwrap(),
        "~ parameterDefinitions[Pool]\n  description: \"one\" -> \"two\" [template]\n  \
         default: \"a\" -> \"b\" [template]\n~ parameters[Pool]\n  value: \"a\" -> \"b\" [template]\n"
    );
    // The job template's definition comes last, so its description wins.
    let job = edited(
        BASE,
        &[(
            "parameterDefinitions:\n",
            "parameterDefinitions:\n  - {name: Pool, type: STRING, description: job}\n",
        )],
    );
    assert_eq!(
        diff_with_environments((&job, &old), (&job, &new), &[], &[]).unwrap(),
        "~ parameterDefinitions[Pool]\n  default: \"a\" -> \"b\" [template]\n~ parameters[Pool]\n  \
         value: \"a\" -> \"b\" [template]\n"
    );
}

#[test]
fn a_working_directory_change_is_the_templates() {
    let t = template(&edited(
        BASE,
        &[(
            "parameterDefinitions:\n",
            "parameterDefinitions:\n  - {name: Out, type: PATH}\n",
        )],
    ));
    let v = values(&[("Out", "out")]);
    let ctx = t.default_validation_context();
    let input = |cwd| TemplateInput {
        template: &t,
        environments: &[],
        parameters: &v,
        path_options: PathParameterOptions {
            job_template_dir: "/jobs",
            current_working_dir: cwd,
            path_format: openjd_expr::path_mapping::PathFormat::Posix,
            allow_template_dir_walk_up: false,
            allow_uri_path_values: true,
        },
        ctx: &ctx,
    };
    let d = diff_templates(&input("/a"), &input("/b")).unwrap();
    assert_eq!(
        d.tree.to_string(),
        "~ parameters[Out]\n  value: \"/a/out\" -> \"/b/out\" [template]\n"
    );
}
