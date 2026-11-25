// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// Copyright by contributors to this project.
// SPDX-License-Identifier: (Apache-2.0 OR MIT)

//! `openjd diff` command — compare the jobs created from two job templates,
//! or from one template under two parameter sets. Unstable.

use std::io::Write;
use std::path::{Path, PathBuf};

use clap::Args;
use openjd_model::job::diff::{diff_jobs, DiffTree};
use openjd_model::job::Job;

/// The exit code of `--exit-code` when the jobs differ. `openjd` exits 1 on
/// any error and clap exits 2 on a usage error, so a difference gets 3.
pub const EXIT_CHANGED: i32 = 3;

const EXAMPLES: &str = "\
Examples, run from crates/openjd-cli/examples/diff in the openjd-rs repository:
  openjd diff frames.yaml --old-job-param Frames=1-3 --new-job-param Frames=1-5
  openjd diff render_v1.yaml render_v2.yaml
  openjd diff render_v1.yaml render_v2.yaml --new-job-param file://shot.json
  openjd diff frames.yaml --new-job-param Frames=1-5 --exit-code

README.md in that directory walks through them with their output.";

#[derive(Args)]
#[command(after_help = EXAMPLES)]
pub struct DiffArgs {
    /// The old side's job template file. With no NEW, it is the new side's too
    pub old: PathBuf,

    /// The new side's job template file
    pub new: Option<PathBuf>,

    /// Job parameters for both sides (Key=Value, file://path, or inline JSON)
    #[arg(short = 'p', long = "job-param", alias = "parameter")]
    pub parameters: Vec<String>,

    /// Job parameters for the old side only, in the same forms as -p. Applied after -p, so they win
    #[arg(long = "old-job-param", alias = "old-parameter")]
    pub old_parameters: Vec<String>,

    /// Job parameters for the new side only, in the same forms as -p. Applied after -p, so they win
    #[arg(long = "new-job-param", alias = "new-parameter")]
    pub new_parameters: Vec<String>,

    /// Extensions to support (comma-separated). Empty string disables all.
    #[arg(long = "extensions")]
    pub extensions: Option<String>,

    /// Environment template files, for both sides
    #[arg(long = "environment", alias = "env")]
    pub environments: Vec<PathBuf>,

    /// Exit 0 when the jobs are equal and 3 when they differ. Errors exit 1
    #[arg(long)]
    pub exit_code: bool,
}

/// Run `openjd diff` and return the process exit code.
pub fn execute(args: DiffArgs) -> Result<i32, Box<dyn std::error::Error>> {
    let new_path = args.new.as_ref().unwrap_or(&args.old);
    let old =
        build_side(&args, &args.old, &args.old_parameters).map_err(|e| format!("old job: {e}"))?;
    let new =
        build_side(&args, new_path, &args.new_parameters).map_err(|e| format!("new job: {e}"))?;
    let tree = diff_jobs(&old, &new);
    let mut out = std::io::stdout().lock();
    out.write_all(render(&header(&args), &tree).as_bytes())?;
    out.flush()?;
    Ok(if args.exit_code && tree.is_changed() {
        EXIT_CHANGED
    } else {
        0
    })
}

/// Create one side's job from `path` with `-p` followed by that side's
/// parameters, so a side parameter wins over `-p`.
fn build_side(
    args: &DiffArgs,
    path: &Path,
    side_parameters: &[String],
) -> Result<Job, Box<dyn std::error::Error>> {
    let parameters: Vec<String> = args
        .parameters
        .iter()
        .chain(side_parameters)
        .cloned()
        .collect();
    let (_, job) = crate::job_from_file::create_job_from_file(
        path,
        &parameters,
        &args.extensions,
        &args.environments,
    )?;
    Ok(job)
}

fn header(args: &DiffArgs) -> String {
    match &args.new {
        Some(new) => format!("{} -> {}", args.old.display(), new.display()),
        None => format!("{}: old parameters -> new parameters", args.old.display()),
    }
}

/// The command's output: `header`, then the tree and a count of the steps it
/// changes, or `no changes`.
fn render(header: &str, tree: &DiffTree) -> String {
    if !tree.is_changed() {
        return format!("{header}\nno changes\n");
    }
    let steps = tree
        .root()
        .children
        .iter()
        .filter(|c| c.is_changed() && c.segment.starts_with("steps["))
        .count();
    let noun = if steps == 1 { "step" } else { "steps" };
    format!("{header}\n{tree}{steps} {noun} changed\n")
}
