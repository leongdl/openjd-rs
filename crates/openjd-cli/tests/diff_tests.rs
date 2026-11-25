// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// Copyright by contributors to this project.
// SPDX-License-Identifier: (Apache-2.0 OR MIT)

//! Tests of `openjd diff`, including the curated examples in
//! `examples/diff/` and the examples its `--help` ends with.

use std::path::{Path, PathBuf};
use std::process::Command;

const EXAMPLE_DIR: &str = "examples/diff";

/// Set to rewrite `examples/diff/README.md` with each command's real output.
const UPDATE: &str = "OPENJD_UPDATE_DIFF_EXAMPLES";

const TEMPLATE: &str = r#"
specificationVersion: jobtemplate-2023-09
name: Job
parameterDefinitions:
  - {name: Frames, type: STRING, default: "1-3"}
  - {name: Label, type: STRING, default: a}
steps:
  - name: Render
    parameterSpace:
      taskParameterDefinitions:
        - {name: Frame, type: INT, range: "{{Param.Frames}}"}
    script:
      actions:
        onRun: {command: render, args: ["{{Task.Param.Frame}}"]}
  - name: Label
    script:
      actions:
        onRun: {command: echo, args: ["{{Param.Label}}"]}
"#;

fn openjd(dir: &Path, args: &[&str]) -> (i32, String, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_openjd"))
        .current_dir(dir)
        .arg("diff")
        .args(args)
        .output()
        .expect("failed to execute openjd");
    (
        output.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n"),
        String::from_utf8_lossy(&output.stderr).replace("\r\n", "\n"),
    )
}

/// A temporary directory holding `job.yaml` (`TEMPLATE`) and `files`.
fn dir_with(files: &[(&str, &str)]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("job.yaml"), TEMPLATE).unwrap();
    for (name, content) in files {
        std::fs::write(dir.path().join(name), content).unwrap();
    }
    dir
}

#[test]
fn one_template_under_two_parameter_sets() {
    let dir = dir_with(&[]);
    let (code, stdout, stderr) = openjd(
        dir.path(),
        &[
            "job.yaml",
            "--old-job-param",
            "Label=x",
            "--new-job-param",
            "Label=y",
        ],
    );
    assert_eq!((code, stderr.as_str()), (0, ""));
    assert_eq!(
        stdout,
        "job.yaml: old parameters -> new parameters\n\
         ~ parameters[Label]\n  value: \"x\" -> \"y\"\n\
         ~ steps[Label]\n  resolvedSymTab[Param.Label]: \
         {\"type\":\"string\",\"value\":\"x\"} -> {\"type\":\"string\",\"value\":\"y\"}\n\
         1 step changed\n"
    );
}

#[test]
fn shared_parameters_apply_to_both_sides_and_side_parameters_win() {
    let dir = dir_with(&[]);
    // The side flag comes first on the command line and still wins over -p.
    let (code, stdout, stderr) = openjd(
        dir.path(),
        &[
            "job.yaml",
            "--new-job-param",
            "Label=y",
            "-p",
            "Label=x",
            "-p",
            "Frames=1-2",
        ],
    );
    assert_eq!((code, stderr.as_str()), (0, ""));
    assert_eq!(
        stdout,
        "job.yaml: old parameters -> new parameters\n\
         ~ parameters[Label]\n  value: \"x\" -> \"y\"\n\
         ~ steps[Label]\n  resolvedSymTab[Param.Label]: \
         {\"type\":\"string\",\"value\":\"x\"} -> {\"type\":\"string\",\"value\":\"y\"}\n\
         1 step changed\n"
    );
}

#[test]
fn two_templates() {
    let new = TEMPLATE.replace("command: echo", "command: print");
    let dir = dir_with(&[("new.yaml", &new)]);
    let (code, stdout, stderr) = openjd(dir.path(), &["job.yaml", "new.yaml", "-p", "Label=z"]);
    assert_eq!((code, stderr.as_str()), (0, ""));
    assert_eq!(
        stdout,
        "job.yaml -> new.yaml\n\
         ~ steps[Label]\n  ~ script\n    ~ onRun\n      command: \"echo\" -> \"print\"\n\
         1 step changed\n"
    );
}

#[test]
fn changes_outside_steps_count_zero_steps() {
    let new = TEMPLATE.replace("name: Job", "name: Renamed");
    let dir = dir_with(&[("new.yaml", &new)]);
    let (code, stdout, stderr) = openjd(dir.path(), &["job.yaml", "new.yaml"]);
    assert_eq!((code, stderr.as_str()), (0, ""));
    assert_eq!(
        stdout,
        "job.yaml -> new.yaml\nname: \"Job\" -> \"Renamed\"\n0 steps changed\n"
    );
}

#[test]
fn unchanged() {
    let dir = dir_with(&[]);
    let (code, stdout, stderr) = openjd(dir.path(), &["job.yaml", "job.yaml"]);
    assert_eq!((code, stderr.as_str()), (0, ""));
    assert_eq!(stdout, "job.yaml -> job.yaml\nno changes\n");
}

#[test]
fn exit_code_is_3_only_when_the_jobs_differ_and_it_is_asked_for() {
    let dir = dir_with(&[]);
    let changed = ["job.yaml", "--new-job-param", "Label=y"];
    assert_eq!(openjd(dir.path(), &changed).0, 0);
    let (code, stdout, stderr) = openjd(dir.path(), &[&changed[..], &["--exit-code"]].concat());
    assert_eq!((code, stderr.as_str()), (3, ""));
    assert!(stdout.ends_with("1 step changed\n"), "{stdout}");
    let (code, stdout, _) = openjd(dir.path(), &["job.yaml", "--exit-code"]);
    assert_eq!(code, 0);
    assert!(stdout.ends_with("no changes\n"), "{stdout}");
}

#[test]
fn a_bad_parameter_is_an_error_naming_its_side() {
    let dir = dir_with(&[]);
    let (code, stdout, stderr) = openjd(
        dir.path(),
        &["job.yaml", "--new-job-param", "Nope=1", "--exit-code"],
    );
    assert_eq!((code, stdout.as_str()), (1, ""));
    assert_eq!(
        stderr,
        "ERROR: new job: Validation error: Job parameter values provided for parameters that are not defined \
         in the template: Nope\n"
    );
    let (code, _, stderr) = openjd(dir.path(), &["job.yaml", "-p", "Label"]);
    assert_eq!(code, 1);
    assert_eq!(
        stderr,
        "ERROR: old job: Job parameter string ('Label') not formatted correctly. It must be \
         key=value pairs, inline JSON, or a path to a JSON or YAML document prefixed with \
         'file://'.\n"
    );
}

#[test]
fn a_missing_template_is_an_error_naming_its_side() {
    let dir = dir_with(&[]);
    let (code, stdout, stderr) = openjd(dir.path(), &["job.yaml", "nope.yaml"]);
    assert_eq!((code, stdout.as_str()), (1, ""));
    assert_eq!(stderr, "ERROR: new job: 'nope.yaml' does not exist.\n");
}

#[test]
fn environment_parameters_apply_to_both_sides() {
    let env = "specificationVersion: environment-2023-09\n\
               parameterDefinitions:\n  - {name: EnvParam, type: STRING, default: d}\n\
               environment:\n  name: E\n  variables: {V: \"{{Param.EnvParam}}\"}\n";
    let dir = dir_with(&[("env.yaml", env)]);
    let args = [
        "job.yaml",
        "--old-job-param",
        "EnvParam=a",
        "--new-job-param",
        "EnvParam=b",
    ];
    let (code, _, stderr) = openjd(dir.path(), &args);
    assert_eq!(code, 1, "EnvParam is defined only by the environment");
    assert!(stderr.starts_with("ERROR: old job: "), "{stderr}");
    let (code, stdout, stderr) = openjd(
        dir.path(),
        &[&args[..], &["--environment", "env.yaml"]].concat(),
    );
    assert_eq!((code, stderr.as_str()), (0, ""));
    assert_eq!(
        stdout,
        "job.yaml: old parameters -> new parameters\n\
         ~ parameters[EnvParam]\n  value: \"a\" -> \"b\"\n\
         0 steps changed\n"
    );
}

#[test]
fn help_marks_the_command_unstable() {
    let output = Command::new(env!("CARGO_BIN_EXE_openjd"))
        .arg("--help")
        .output()
        .unwrap();
    let help = String::from_utf8_lossy(&output.stdout);
    let line = help
        .lines()
        .find(|l| l.trim_start().starts_with("diff "))
        .unwrap_or_else(|| panic!("no diff line in:\n{help}"));
    assert!(line.ends_with("(unstable)"), "{line}");
}

fn example_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(EXAMPLE_DIR)
}

/// The `$ openjd diff ...` commands of the README's `console` blocks.
fn readme_commands(readme: &str) -> Vec<String> {
    let mut in_console = false;
    let mut commands = Vec::new();
    for line in readme.lines() {
        if line.starts_with("```") {
            in_console = line == "```console";
        } else if in_console {
            if let Some(command) = line.strip_prefix("$ ") {
                commands.push(command.to_string());
            }
        }
    }
    commands
}

/// `command`'s arguments after `openjd diff`. Commands use no shell quoting.
fn diff_args(command: &str) -> Vec<&str> {
    let words: Vec<&str> = command.split_whitespace().collect();
    assert_eq!(words[..2], ["openjd", "diff"], "{command}");
    words[2..].to_vec()
}

/// Run `command` from the example directory. Only `--exit-code` may exit 3.
fn run_example(command: &str) -> String {
    let args = diff_args(command);
    let (code, stdout, stderr) = openjd(&example_dir(), &args);
    let expected = if args.contains(&"--exit-code") {
        [0, 3]
    } else {
        [0, 0]
    };
    assert!(
        expected.contains(&code),
        "{command} exited {code}: {stderr}"
    );
    assert_eq!(stderr, "", "{command}");
    stdout
}

/// The README with each console block's output replaced by its command's
/// output, and the files the commands name.
fn regenerate_readme(readme: &str) -> (String, Vec<String>) {
    let mut out = String::new();
    let mut used = Vec::new();
    let mut in_console = false;
    let mut skipping = false;
    for line in readme.lines() {
        if line.starts_with("```") {
            in_console = line == "```console";
            skipping = false;
        } else if in_console {
            if let Some(command) = line.strip_prefix("$ ") {
                out.push_str(line);
                out.push('\n');
                out.push_str(&run_example(command));
                for arg in diff_args(command) {
                    used.push(arg.trim_start_matches("file://").to_string());
                }
                skipping = true;
                continue;
            }
            if skipping {
                continue;
            }
        }
        out.push_str(line);
        out.push('\n');
    }
    (out, used)
}

#[test]
fn curated_examples_readme_is_up_to_date() {
    let path = example_dir().join("README.md");
    let current = std::fs::read_to_string(&path)
        .unwrap()
        .replace("\r\n", "\n");
    let (updated, used) = regenerate_readme(&current);
    let unused: Vec<String> = std::fs::read_dir(example_dir())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name != "README.md" && !used.contains(name))
        .collect();
    assert!(
        unused.is_empty(),
        "files in {EXAMPLE_DIR} the README does not use: {unused:?}"
    );
    if std::env::var_os(UPDATE).is_some() {
        std::fs::write(&path, updated).unwrap();
        return;
    }
    assert!(
        updated == current,
        "{EXAMPLE_DIR}/README.md shows output its commands do not print. Fix it, or rewrite \
         it with:\n  {UPDATE}=1 cargo test -p openjd-cli --test diff_tests\n\n\
         --- README:\n{current}\n--- with the real output:\n{updated}"
    );
}

#[test]
fn help_examples_run_and_are_in_the_readme() {
    let output = Command::new(env!("CARGO_BIN_EXE_openjd"))
        .args(["diff", "--help"])
        .output()
        .unwrap();
    let help = String::from_utf8_lossy(&output.stdout);
    let examples: Vec<&str> = help
        .lines()
        .filter_map(|l| {
            l.strip_prefix("  ")
                .filter(|c| c.starts_with("openjd diff "))
        })
        .collect();
    assert!(examples.len() >= 3, "{help}");
    let readme = std::fs::read_to_string(example_dir().join("README.md")).unwrap();
    let readme_commands = readme_commands(&readme);
    for example in examples {
        assert!(
            readme_commands.iter().any(|c| c == example),
            "the --help example `{example}` is not a README command"
        );
        run_example(example);
    }
}
