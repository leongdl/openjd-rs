// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// Copyright by contributors to this project.
// SPDX-License-Identifier: (Apache-2.0 OR MIT)

//! `openjd diff` command — diff two templates, one template under two
//! parameter sets, or two built jobs.

use std::io::Read;
use std::path::Path;

use clap::Args;
use openjd_model::job::apply::{Action, ApplyPlan, Op, TaskChange};
use openjd_model::job::diff::DiffOptions;
use openjd_model::job::template_diff::{
    diff_built_jobs, diff_parameter_sets, diff_templates, job_from_str, Change, StepSummary,
    TemplateDiff, TemplateInput, TemplateSource,
};
use openjd_model::job::Job;
use serde::Serialize;

/// Exit code of `--exit-code` when the inputs differ.
const EXIT_DIFFERENT: i32 = 4;

const EXAMPLES: &str = "\
Examples:
  openjd diff render_v1.yaml render_v2.yaml -p Scene=/proj/shot010.blend
  openjd diff render.yaml --old-param Frames=1-100 --new-param Frames=1-200
  openjd diff render.yaml --old-param file://old.json --new-param file://new.json --tasks
  openjd build render.yaml -p Frames=1-100 > old.json
  openjd diff --jobs old.json new.json --plan
  openjd diff old.yaml - --stat < new.yaml
  openjd diff old.yaml new.yaml --output json --exit-code";

#[derive(Args)]
#[command(after_help = EXAMPLES)]
pub struct DiffArgs {
    /// Old template file, or `-` for stdin
    pub old: String,

    /// New template file, or `-` for stdin. Omit to diff OLD against itself
    /// under --old-param and --new-param
    pub new: Option<String>,

    /// Job parameter for both sides (Key=Value, file://path, or inline JSON)
    #[arg(short = 'p', long = "job-param", alias = "parameter")]
    pub parameters: Vec<String>,

    /// Job parameter for the old side only; wins over --job-param
    #[arg(long = "old-param")]
    pub old_params: Vec<String>,

    /// Job parameter for the new side only; wins over --job-param
    #[arg(long = "new-param")]
    pub new_params: Vec<String>,

    /// Environment template file, for both sides
    #[arg(long = "environment", alias = "env")]
    pub environments: Vec<String>,

    /// Extensions to support (comma-separated). Empty string disables all
    #[arg(long = "extensions")]
    pub extensions: Option<String>,

    /// OLD and NEW are built jobs (the output of `openjd build`), not templates
    #[arg(long, conflicts_with_all = ["parameters", "old_params", "new_params", "environments", "extensions"])]
    pub jobs: bool,

    /// Show only this step and the job-level changes
    #[arg(long)]
    pub step: Option<String>,

    /// Show only the task change of each step
    #[arg(long, conflicts_with = "stat")]
    pub tasks: bool,

    /// Show one line per changed step and the totals
    #[arg(long)]
    pub stat: bool,

    /// Also print the apply plan and the mutation kinds it needs
    #[arg(long)]
    pub plan: bool,

    /// Do not expand the job diff below this depth
    #[arg(long)]
    pub depth: Option<usize>,

    /// Do not compare resolved symbol tables (hides parameter changes in steps)
    #[arg(long)]
    pub ignore_symbols: bool,

    /// How to format the output
    #[arg(long = "output", value_parser = ["human-readable", "json", "yaml"], default_value = "human-readable")]
    pub output: String,

    /// With --output json or yaml, also include the two jobs and the job diff tree
    #[arg(long)]
    pub verbose: bool,

    /// Exit 0 when there are no differences and 4 when there are (errors exit 1)
    #[arg(long)]
    pub exit_code: bool,
}

/// Run `openjd diff`; returns the process exit code.
pub fn execute(args: DiffArgs) -> Result<i32, Box<dyn std::error::Error>> {
    let mut options = DiffOptions::default().with_resolved_symtab(!args.ignore_symbols);
    if let Some(n) = args.depth {
        options = options.with_max_depth(n);
    }
    let mut stdin_used = false;
    let mut diff = if args.jobs {
        let new = args
            .new
            .as_deref()
            .ok_or("--jobs needs both OLD and NEW.")?;
        let old = read_job(&args.old, &mut stdin_used)?;
        let new = read_job(new, &mut stdin_used)?;
        diff_built_jobs(old, new, &options)
    } else {
        let side = |extra: &[String]| {
            let all: Vec<String> = args.parameters.iter().chain(extra).cloned().collect();
            crate::run::parse_cli_parameters(&all)
        };
        let (old_params, new_params) = (side(&args.old_params)?, side(&args.new_params)?);
        let base = |arg: &str,
                    stdin_used: &mut bool|
         -> Result<TemplateInput, Box<dyn std::error::Error>> {
            let exts = crate::common::parse_extensions(&args.extensions)?;
            let exts: Vec<&str> = exts.iter().map(String::as_str).collect();
            let mut input = TemplateInput::new(read_template(arg, stdin_used)?)
                .extensions(&exts)
                .limits(crate::common::caller_limits());
            for env in &args.environments {
                input = input.environment(read_template(env, stdin_used)?);
            }
            Ok(input)
        };
        match args.new.as_deref() {
            None => diff_parameter_sets(
                &base(&args.old, &mut stdin_used)?,
                &old_params,
                &new_params,
                &options,
            )?,
            Some(new) => {
                let old = base(&args.old, &mut stdin_used)?.params(old_params);
                let new = base(new, &mut stdin_used)?.params(new_params);
                diff_templates(&old, &new, &options)?
            }
        }
    };
    if let Some(step) = &args.step {
        keep_step(&mut diff, step)?;
    }
    let plan = if args.plan { Some(diff.plan()?) } else { None };
    match args.output.as_str() {
        "json" | "yaml" => print_document(&diff, plan.as_ref(), &args)?,
        _ => print_human(&diff, plan.as_ref(), &args),
    }
    Ok(if args.exit_code && diff.is_changed() {
        EXIT_DIFFERENT
    } else {
        0
    })
}

/// Read stdin once; a second `-` is an error.
fn read_stdin(stdin_used: &mut bool) -> Result<String, Box<dyn std::error::Error>> {
    if *stdin_used {
        return Err("Only one input can be read from stdin ('-').".into());
    }
    *stdin_used = true;
    let mut text = String::new();
    std::io::stdin().read_to_string(&mut text)?;
    Ok(text)
}

fn read_template(
    arg: &str,
    stdin_used: &mut bool,
) -> Result<TemplateSource, Box<dyn std::error::Error>> {
    if arg == "-" {
        return Ok(TemplateSource::Text(read_stdin(stdin_used)?));
    }
    // Read once here for the CLI's usual "does not exist" message; the
    // library reads the file again so its directory sets relative PATH defaults.
    crate::common::read_input_file(Path::new(arg))?;
    Ok(TemplateSource::Path(arg.into()))
}

fn read_job(arg: &str, stdin_used: &mut bool) -> Result<Job, Box<dyn std::error::Error>> {
    let text = if arg == "-" {
        read_stdin(stdin_used)?
    } else {
        crate::common::read_input_file(Path::new(arg))?
    };
    job_from_str(&text).map_err(|e| format!("'{arg}': {e}").into())
}

/// Keep the changes and summary of one step, and the job-level changes.
fn keep_step(diff: &mut TemplateDiff, step: &str) -> Result<(), String> {
    if diff.step(step).is_none() {
        return Err(format!("Step '{step}' does not exist in either job."));
    }
    let prefix = format!("steps[{step}]");
    let keep: Vec<Change> = diff
        .changes
        .iter()
        .filter(|c| {
            !c.path.starts_with("steps[")
                || c.path == prefix
                || c.path.starts_with(&format!("{prefix}."))
        })
        .cloned()
        .collect();
    diff.changes = keep;
    diff.steps.retain(|s| s.name == step);
    Ok(())
}

fn shown_steps(diff: &TemplateDiff) -> impl Iterator<Item = &StepSummary> {
    diff.steps.iter().filter(|s| {
        s.status != openjd_model::job::diff::DiffStatus::Unchanged
            || diff
                .changes_at(&format!("steps[{}]", s.name))
                .next()
                .is_some()
    })
}

fn print_human(diff: &TemplateDiff, plan: Option<&ApplyPlan<'_>>, args: &DiffArgs) {
    if args.tasks || args.stat {
        let lines: Vec<String> = shown_steps(diff)
            .map(|s| {
                if args.stat {
                    s.stat_line()
                } else {
                    s.to_string()
                }
            })
            .collect();
        if lines.is_empty() {
            println!("no changes");
        } else {
            println!("{}", lines.join("\n"));
        }
        if args.stat && !lines.is_empty() {
            println!("{}", diff.stat());
        }
    } else {
        println!("{diff}");
    }
    if let Some(plan) = plan {
        println!("\napply plan:");
        let ops = plan_ops(diff, plan);
        if ops.is_empty() {
            println!("  nothing to apply");
        }
        for (n, op) in ops.iter().enumerate() {
            println!("  {}. {}  [{}]", n + 1, op.action, op.kinds.join(", "));
        }
        let kinds: Vec<String> = plan.kinds().iter().map(|k| format!("{k:?}")).collect();
        println!("kinds: {}", kinds.join(", "));
    }
}

/// One op of the plan, for printing.
#[derive(Serialize)]
struct PlanOp {
    action: String,
    kinds: Vec<String>,
}

#[derive(Serialize)]
struct PlanDoc {
    ops: Vec<PlanOp>,
    kinds: Vec<String>,
}

fn plan_ops(diff: &TemplateDiff, plan: &ApplyPlan<'_>) -> Vec<PlanOp> {
    let step_name = |i: usize, removed: bool| {
        let job = if removed {
            &diff.old_job
        } else {
            &diff.new_job
        };
        job.steps.get(i).map_or("?", |s| s.name.as_str())
    };
    plan.ops
        .iter()
        .map(|op| {
            let action = match &op.action {
                Action::Job(_) => "job modify".to_string(),
                Action::Parameter(o) => {
                    format!("parameter {} {}", verb(o), op_name(o, |p| &p.name))
                }
                Action::Environment(_, o) => {
                    format!("job environment {} {}", verb(o), op_name(o, |e| &e.name))
                }
                Action::Step(i, o) => format!(
                    "step {} {}",
                    verb(o),
                    step_name(*i, matches!(o, Op::Remove { .. }))
                ),
                Action::Tasks { step, change, .. } => {
                    format!(
                        "tasks of {}: {}",
                        step_name(*step, false),
                        task_change(change)
                    )
                }
                _ => "other".to_string(),
            };
            PlanOp {
                action,
                kinds: op.kinds.iter().map(|k| format!("{k:?}")).collect(),
            }
        })
        .collect()
}

fn verb<T>(op: &Op<'_, T>) -> &'static str {
    match op {
        Op::Add { .. } => "add",
        Op::Modify { .. } => "modify",
        Op::Remove { .. } => "remove",
    }
}

fn op_name<'a, T>(op: &Op<'a, T>, name: impl Fn(&'a T) -> &'a String) -> &'a str {
    match *op {
        Op::Add { new } | Op::Modify { new, .. } => name(new),
        Op::Remove { old } => name(old),
    }
}

fn task_change(change: &TaskChange) -> String {
    match change {
        TaskChange::Boxes(d) => {
            let added: usize = d.added.iter().map(|b| b.len()).sum();
            let removed: usize = d.removed.iter().map(|b| b.len()).sum();
            let start = d.old.len();
            let at = if added > 0 {
                format!(
                    " at index {start}..{}",
                    start.saturating_add(added).saturating_sub(1)
                )
            } else {
                String::new()
            };
            format!("keep {}, add {added}{at}, drop {removed}", d.kept.len())
        }
        TaskChange::All => "all".to_string(),
        TaskChange::Replace => "replace all".to_string(),
    }
}

/// The `--output json|yaml` document: the diff's serde form without the jobs
/// and the tree, unless `--verbose`.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Brief<'a> {
    changes: &'a [Change],
    steps: &'a [StepSummary],
    notes: &'a [String],
    #[serde(skip_serializing_if = "Option::is_none")]
    plan: Option<PlanDoc>,
}

#[derive(Serialize)]
struct Full<'a> {
    #[serde(flatten)]
    diff: &'a TemplateDiff,
    #[serde(skip_serializing_if = "Option::is_none")]
    plan: Option<PlanDoc>,
}

fn print_document(
    diff: &TemplateDiff,
    plan: Option<&ApplyPlan<'_>>,
    args: &DiffArgs,
) -> Result<(), Box<dyn std::error::Error>> {
    let plan = plan.map(|p| PlanDoc {
        ops: plan_ops(diff, p),
        kinds: p.kinds().iter().map(|k| format!("{k:?}")).collect(),
    });
    // Serialized directly, not through `serde_json::Value`, so the jobs keep
    // their map order (task parameter definition order is task order).
    let json = args.output == "json";
    let text = if args.verbose {
        let doc = Full { diff, plan };
        if json {
            serde_json::to_string_pretty(&doc)?
        } else {
            serde_saphyr::to_string(&doc)?
        }
    } else {
        let doc = Brief {
            changes: &diff.changes,
            steps: &diff.steps,
            notes: &diff.notes,
            plan,
        };
        if json {
            serde_json::to_string_pretty(&doc)?
        } else {
            serde_saphyr::to_string(&doc)?
        }
    };
    if json {
        println!("{text}");
    } else {
        print!("{text}");
    }
    Ok(())
}
