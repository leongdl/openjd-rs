// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// Copyright by contributors to this project.
// SPDX-License-Identifier: (Apache-2.0 OR MIT)

//! `openjd diff` command — compare the jobs created from two job templates,
//! or from one template under two parameter sets. Unstable.

use std::io::Write;
use std::path::{Path, PathBuf};

use clap::Args;
use openjd_model::job::apply::{plan, ApplyOp, ApplyPlan, OpTarget};
use openjd_model::job::task_delta::{TaskBox, TaskDelta};
use openjd_model::job::template_diff::{diff_templates, TemplateDiff};

use crate::job_from_file::TemplateFile;

/// The exit code of `--exit-code` when the diff has a change. `openjd` exits 1 on
/// any error and clap exits 2 on a usage error, so a difference gets 3.
const EXIT_CHANGED: i32 = 3;

const EXAMPLES: &str = "\
Examples:
  openjd diff job.yaml --old-job-param Frames=1-3 --new-job-param Frames=1-5
  openjd diff old.yaml new.yaml
  openjd diff old.yaml new.yaml --new-job-param file://params.json
  openjd diff job.yaml --new-job-param Frames=1-5 --exit-code
  openjd diff old.yaml new.yaml --plan";

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
    #[arg(long = "old-job-param")]
    pub old_parameters: Vec<String>,

    /// Job parameters for the new side only, in the same forms as -p. Applied after -p, so they win
    #[arg(long = "new-job-param")]
    pub new_parameters: Vec<String>,

    /// Extensions to support (comma-separated). Empty string disables all.
    #[arg(long = "extensions")]
    pub extensions: Option<String>,

    /// Environment template files, for both sides
    #[arg(long = "environment", alias = "env")]
    pub environments: Vec<PathBuf>,

    /// Exit 0 when the diff has no change and 3 when it has one. Errors exit 1
    #[arg(long)]
    pub exit_code: bool,

    /// After the diff, print the numbered operations that apply it to a running job
    #[arg(long)]
    pub plan: bool,
}

/// Run `openjd diff` and return the process exit code.
pub fn execute(args: DiffArgs) -> Result<i32, Box<dyn std::error::Error>> {
    let new_path = args.new.as_ref().unwrap_or(&args.old);
    let old =
        load_side(&args, &args.old, &args.old_parameters).map_err(|e| format!("old job: {e}"))?;
    let new =
        load_side(&args, new_path, &args.new_parameters).map_err(|e| format!("new job: {e}"))?;
    let diff = diff_templates(&old.input(), &new.input())?;
    let plan = plan(&diff.old, &diff.new)?;
    let mut out = std::io::stdout().lock();
    out.write_all(render(&header(&args), &diff, &plan, args.plan).as_bytes())?;
    out.flush()?;
    Ok(if args.exit_code && diff.tree.is_changed() {
        EXIT_CHANGED
    } else {
        0
    })
}

/// Read one side's template file with `-p` followed by that side's
/// parameters, so a side parameter wins over `-p`.
fn load_side(
    args: &DiffArgs,
    path: &Path,
    side_parameters: &[String],
) -> Result<TemplateFile, Box<dyn std::error::Error>> {
    let parameters: Vec<String> = args
        .parameters
        .iter()
        .chain(side_parameters)
        .cloned()
        .collect();
    TemplateFile::load(path, &parameters, &args.extensions, &args.environments)
}

fn header(args: &DiffArgs) -> String {
    match &args.new {
        Some(new) => format!("{} -> {}", args.old.display(), new.display()),
        None => format!("{}: old parameters -> new parameters", args.old.display()),
    }
}

/// The command's output: `header`, then the tree, a `tasks:` block when a
/// step's tasks change, a count of the steps it changes and, with
/// `show_plan`, `plan:` and the plan; or `no changes`.
fn render(header: &str, diff: &TemplateDiff, plan: &ApplyPlan, show_plan: bool) -> String {
    let tree = &diff.tree;
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
    let tasks: String = plan.ops().iter().filter_map(task_line).collect();
    let tasks = if tasks.is_empty() {
        tasks
    } else {
        format!("tasks:\n{tasks}")
    };
    let note = match &diff.cross_build_error {
        Some(e) => format!(
            "note: every change the jobs show is [both]: the new template does not build \
             with the old parameters: {e}\n"
        ),
        None => String::new(),
    };
    let plan = if show_plan {
        format!("plan:\n{plan}")
    } else {
        String::new()
    };
    format!("{header}\n{tree}{tasks}{steps} {noun} changed\n{note}{plan}")
}

/// `{step}: {summary}` for a step op whose tasks are added, removed,
/// reordered or replaced.
fn task_line(op: &ApplyOp) -> Option<String> {
    let (OpTarget::Step(name), Some(delta)) = (&op.target, &op.tasks) else {
        return None;
    };
    let summary = match delta {
        TaskDelta::Product(d) => {
            let count = |boxes: &[TaskBox]| boxes.iter().map(TaskBox::len).sum::<u64>();
            let counts = (!d.added.is_empty() || !d.removed.is_empty()).then(|| {
                format!(
                    "{} kept, {} added, {} removed",
                    d.kept.len(),
                    count(&d.added),
                    count(&d.removed)
                )
            });
            let order = d.reordered.then(|| "order changed".to_string());
            let parts: Vec<String> = counts.into_iter().chain(order).collect();
            if parts.is_empty() {
                return None;
            }
            parts.join(", ")
        }
        TaskDelta::Fallback { reason } => format!("all replaced ({reason})"),
        _ => return None,
    };
    Some(format!("{name}: {summary}\n"))
}
