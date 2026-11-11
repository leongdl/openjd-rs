// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// Copyright by contributors to this project.
// SPDX-License-Identifier: (Apache-2.0 OR MIT)

//! `openjd diff` and `openjd build`, run as a binary on the samples in
//! `crates/openjd-model/examples/template_diff/`.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

fn samples() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../openjd-model/examples/template_diff")
}

/// Run `openjd` in the samples directory, with `stdin` piped in.
fn openjd(args: &[&str], stdin: Option<&str>) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_openjd"))
        .args(args)
        .current_dir(samples())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut pipe = child.stdin.take().unwrap();
    pipe.write_all(stdin.unwrap_or("").as_bytes()).unwrap();
    drop(pipe);
    child.wait_with_output().unwrap()
}

fn stdout(o: &Output) -> String {
    String::from_utf8(o.stdout.clone()).unwrap()
}

fn stderr(o: &Output) -> String {
    String::from_utf8(o.stderr.clone()).unwrap()
}

fn assert_ok(o: &Output, expected: &str) {
    assert_eq!(stderr(o), "");
    assert_eq!(stdout(o), expected);
    assert_eq!(o.status.code(), Some(0));
}

const CASE3: &[&str] = &[
    "diff",
    "render_v1.yaml",
    "render_v2.yaml",
    "--new-param",
    "Frames=1-200",
    "-p",
    "Scene=/s",
];

const CASE3_OUT: &str = "\
~ parameter Frames: 1-100 -> 1-200  [parameters]
~ step Render
    ~ Eye: [left] -> [left, right]  [template]
    ~ Frame: 1-100 -> 1-200  [parameters]
    tasks: 100 -> 400
      keep 100   Frame 1-100 × Eye left
      add  200   Frame 101-200 × Eye left, right
      add  100   Frame 1-100 × Eye right

1 step changed. Tasks: 100 kept, 300 added, 0 removed.
";

fn with(base: &[&'static str], extra: &[&'static str]) -> Vec<&'static str> {
    base.iter().chain(extra).copied().collect()
}

#[test]
fn two_files() {
    let o = openjd(
        &["diff", "render_v1.yaml", "render_v2.yaml", "-p", "Scene=/s"],
        None,
    );
    assert_ok(
        &o,
        "\
~ step Render
    ~ Eye: [left] -> [left, right]  [template]
    tasks: 100 -> 200
      keep 100   Frame 1-100 × Eye left
      add  100   Frame 1-100 × Eye right

1 step changed. Tasks: 100 kept, 100 added, 0 removed.
",
    );
}

#[test]
fn one_file_with_old_and_new_params() {
    let o = openjd(
        &[
            "diff",
            "render_v1.yaml",
            "--old-param",
            "Frames=1-100",
            "--new-param",
            r#"{"Frames": "1-200"}"#,
            "-p",
            "Scene=/s",
        ],
        None,
    );
    assert_ok(
        &o,
        "\
~ parameter Frames: 1-100 -> 1-200  [parameters]
~ step Render
    ~ Frame: 1-100 -> 1-200  [parameters]
    tasks: 100 -> 200
      keep 100   Frame 1-100 × Eye left
      add  100   Frame 101-200 × Eye left

1 step changed. Tasks: 100 kept, 100 added, 0 removed.
",
    );
}

#[test]
fn both_changes() {
    assert_ok(&openjd(CASE3, None), CASE3_OUT);
}

#[test]
fn stat() {
    assert_ok(
        &openjd(&with(CASE3, &["--stat"]), None),
        "~ Render  tasks 100 -> 400  (+300 -0)\n1 step changed. Tasks: 100 kept, 300 added, 0 removed.\n",
    );
}

#[test]
fn tasks() {
    assert_ok(
        &openjd(&with(CASE3, &["--tasks"]), None),
        "\
~ step Render
    tasks: 100 -> 400
      keep 100   Frame 1-100 × Eye left
      add  200   Frame 101-200 × Eye left, right
      add  100   Frame 1-100 × Eye right
",
    );
}

#[test]
fn plan() {
    let o = openjd(&with(CASE3, &["--plan"]), None);
    assert_ok(
        &o,
        &format!(
            "{CASE3_OUT}
apply plan:
  1. parameter modify Frames  [ParameterValue]
  2. tasks of Render: keep 100, add 300 at index 100..399, drop 0  [TaskAppend]
kinds: ParameterValue, TaskAppend
"
        ),
    );
}

#[test]
fn step_filter() {
    let o = openjd(&with(CASE3, &["--step", "Render", "--stat"]), None);
    assert_ok(
        &o,
        "~ Render  tasks 100 -> 400  (+300 -0)\n1 step changed. Tasks: 100 kept, 300 added, 0 removed.\n",
    );
    let o = openjd(&with(CASE3, &["--step", "Nope"]), None);
    assert_eq!(
        stderr(&o),
        "ERROR: Step 'Nope' does not exist in either job.\n"
    );
    assert_eq!(o.status.code(), Some(1));
}

#[test]
fn jobs_round_trip_through_build() {
    let dir = tempfile::tempdir().unwrap();
    let old = openjd(&["build", "render_v1.yaml", "-p", "Scene=/s"], None);
    assert_eq!(old.status.code(), Some(0), "{}", stderr(&old));
    let new = openjd(
        &[
            "build",
            "render_v2.yaml",
            "-p",
            "Scene=/s",
            "-p",
            "Frames=1-200",
            "--output",
            "yaml",
        ],
        None,
    );
    assert_eq!(new.status.code(), Some(0), "{}", stderr(&new));
    let (a, b) = (dir.path().join("old.json"), dir.path().join("new.yaml"));
    std::fs::write(&a, &old.stdout).unwrap();
    std::fs::write(&b, &new.stdout).unwrap();
    let o = openjd(
        &["diff", "--jobs", a.to_str().unwrap(), b.to_str().unwrap()],
        None,
    );
    assert_ok(
        &o,
        &CASE3_OUT
            .replace("[parameters]", "[job]")
            .replace("[template]", "[job]"),
    );
    // A job diffed against itself, one side from stdin.
    let o = openjd(
        &["diff", "--jobs", a.to_str().unwrap(), "-"],
        Some(&stdout(&old)),
    );
    assert_ok(&o, "no changes\n");
}

#[test]
fn output_json_is_one_document() {
    let o = openjd(&with(CASE3, &["--output", "json", "--plan"]), None);
    assert_eq!(o.status.code(), Some(0));
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    let keys: Vec<&String> = v.as_object().unwrap().keys().collect();
    assert_eq!(keys, ["changes", "notes", "plan", "steps"]);
    assert_eq!(v["changes"].as_array().unwrap().len(), 3);
    assert_eq!(v["changes"][1]["origin"], "template");
    assert_eq!(
        v["plan"]["kinds"],
        serde_json::json!(["ParameterValue", "TaskAppend"])
    );

    let o = openjd(&with(CASE3, &["--output", "json", "--verbose"]), None);
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    let keys: Vec<&String> = v.as_object().unwrap().keys().collect();
    assert_eq!(
        keys,
        ["changes", "job", "newJob", "notes", "oldJob", "steps"]
    );

    let o = openjd(&with(CASE3, &["--output", "yaml"]), None);
    assert_eq!(o.status.code(), Some(0));
    assert!(stdout(&o).starts_with("changes:\n"), "{}", stdout(&o));
}

#[test]
fn exit_code() {
    let same = openjd(
        &[
            "diff",
            "render_v1.yaml",
            "render_v1.yaml",
            "-p",
            "Scene=/s",
            "--exit-code",
        ],
        None,
    );
    assert_eq!(
        (stdout(&same).as_str(), same.status.code()),
        ("no changes\n", Some(0))
    );
    let differ = openjd(&with(CASE3, &["--exit-code", "--stat"]), None);
    assert_eq!(differ.status.code(), Some(4));
    let error = openjd(
        &["diff", "render_v1.yaml", "render_v2.yaml", "--exit-code"],
        None,
    );
    assert_eq!(
        stderr(&error),
        "ERROR: Validation error: old template: Values missing for required job parameters: Scene\n"
    );
    assert_eq!(error.status.code(), Some(1));
    // Without the flag, differences still exit 0.
    assert_eq!(openjd(CASE3, None).status.code(), Some(0));
}

#[test]
fn stdin() {
    let v2 = std::fs::read_to_string(samples().join("render_v2.yaml")).unwrap();
    let o = openjd(
        &["diff", "render_v1.yaml", "-", "-p", "Scene=/s", "--stat"],
        Some(&v2),
    );
    assert_ok(
        &o,
        "~ Render  tasks 100 -> 200  (+100 -0)\n1 step changed. Tasks: 100 kept, 100 added, 0 removed.\n",
    );
    let o = openjd(&["diff", "-", "-"], Some(&v2));
    assert_eq!(
        stderr(&o),
        "ERROR: Only one input can be read from stdin ('-').\n"
    );
    assert_eq!(o.status.code(), Some(1));
}

#[test]
fn missing_file_names_the_path() {
    let o = openjd(&["diff", "nope.yaml", "render_v1.yaml"], None);
    assert_eq!(stderr(&o), "ERROR: 'nope.yaml' does not exist.\n");
    assert_eq!(stdout(&o), "");
    assert_eq!(o.status.code(), Some(1));
    let o = openjd(&["diff", "--jobs", "nope.json", "other.json"], None);
    assert_eq!(stderr(&o), "ERROR: 'nope.json' does not exist.\n");
    let o = openjd(&["diff", "--jobs", "render_v1.yaml"], None);
    assert_eq!(stderr(&o), "ERROR: --jobs needs both OLD and NEW.\n");
}

#[test]
fn build_errors_and_bad_jobs() {
    let o = openjd(&["build", "render_v1.yaml"], None);
    assert_eq!(
        stderr(&o),
        "ERROR: Validation error: Values missing for required job parameters: Scene\n"
    );
    assert_eq!(o.status.code(), Some(1));
    let o = openjd(
        &["diff", "--jobs", "-", "render_v2.yaml"],
        Some(r#"{"name": "x", "steps": [], "bogus": 1}"#),
    );
    assert_eq!(
        stderr(&o),
        "ERROR: '-': Validation error: not a built job: unknown field `bogus`, expected one of `name`, `description`, `extensions`, `parameters`, `steps`, `jobEnvironments` at line 1 column 34\n"
    );
}
