// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// Copyright by contributors to this project.
// SPDX-License-Identifier: (Apache-2.0 OR MIT)

//! `job_diff`: diff two OpenJD templates and preview how the diff applies.
//!
//! ```text
//! cargo run -p openjd-model --example job_diff -- OLD.yaml NEW.yaml [options]
//!
//!   --max-depth N        do not expand nodes at depth N or deeper
//!   --no-symtab          do not compare resolvedSymTab (hides parameter changes in steps)
//!   --param K=V          job parameter for both sides (repeatable)
//!   --old-param K=V      job parameter for the old side only
//!   --new-param K=V      job parameter for the new side only
//!   --plan               print the step/task apply plan (job templates only)
//! ```
//!
//! Both files must be job templates, or both environment templates.
//! Sample inputs are in `examples/job_diff/`.

use std::path::Path;
use std::process::ExitCode;

use openjd_model::job::diff::DiffNodeId;
use openjd_model::job::diff::{diff_environments, diff_jobs, DiffOptions, DiffStatus, DiffTree};
use openjd_model::job::task_delta::{task_delta, ProductDelta, TaskBox, TaskDelta};
use openjd_model::job::TaskParamRange;
use openjd_model::job::{Job, Step, StepParameterSpace, TaskParameter};
use openjd_model::{
    convert_environment, create_job, decode_template, preprocess_job_parameters, CallerLimits,
    DecodedTemplate, JobParameterInputValues, PathParameterOptions, StepDependencyGraph,
    StepParameterSpaceIterator,
};

struct Args {
    old: String,
    new: String,
    options: DiffOptions,
    old_params: Vec<(String, String)>,
    new_params: Vec<(String, String)>,
    plan: bool,
    with_symtab: bool,
}

fn parse_args() -> Result<Args, String> {
    let mut positional = Vec::new();
    let mut options = DiffOptions::default();
    let (mut old_params, mut new_params) = (Vec::new(), Vec::new());
    let mut plan = false;
    let mut with_symtab = true;
    let mut it = std::env::args().skip(1);
    let kv = |s: Option<String>, flag: &str| -> Result<(String, String), String> {
        let s = s.ok_or_else(|| format!("{flag} needs K=V"))?;
        let (k, v) = s
            .split_once('=')
            .ok_or_else(|| format!("{flag} needs K=V, got {s}"))?;
        Ok((k.to_string(), v.to_string()))
    };
    while let Some(a) = it.next() {
        match a.as_str() {
            "--max-depth" => {
                let n = it.next().ok_or("--max-depth needs N")?;
                let n = n.parse().map_err(|e| format!("--max-depth {n}: {e}"))?;
                options = options.with_max_depth(n);
            }
            "--no-symtab" => {
                options = options.with_resolved_symtab(false);
                with_symtab = false;
            }
            "--param" => {
                let p = kv(it.next(), "--param")?;
                old_params.push(p.clone());
                new_params.push(p);
            }
            "--old-param" => old_params.push(kv(it.next(), "--old-param")?),
            "--new-param" => new_params.push(kv(it.next(), "--new-param")?),
            "--plan" => plan = true,
            "-h" | "--help" => return Err(USAGE.into()),
            s if s.starts_with('-') => return Err(format!("unknown option {s}\n{USAGE}")),
            _ => positional.push(a),
        }
    }
    let [old, new]: [String; 2] = positional
        .try_into()
        .map_err(|_| format!("expected OLD and NEW\n{USAGE}"))?;
    Ok(Args {
        old,
        new,
        options,
        old_params,
        new_params,
        plan,
        with_symtab,
    })
}

const USAGE: &str = "usage: job_diff OLD NEW [--max-depth N] [--no-symtab] \
[--param K=V] [--old-param K=V] [--new-param K=V] [--plan]";

fn load(path: &str) -> Result<DecodedTemplate, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
    let v: serde_json::Value = serde_saphyr::from_str(&text).map_err(|e| format!("{path}: {e}"))?;
    decode_template(v, None, &CallerLimits::default()).map_err(|e| format!("{path}: {e}"))
}

fn instantiate(
    path: &str,
    jt: &openjd_model::template::JobTemplate,
    params: &[(String, String)],
) -> Result<Job, String> {
    let mut input = JobParameterInputValues::new();
    for (k, v) in params {
        input.insert(k.clone(), openjd_expr::ExprValue::String(v.clone()));
    }
    let dir = Path::new(path)
        .parent()
        .and_then(|p| std::fs::canonicalize(p).ok())
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| ".".into());
    let cwd = std::env::current_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| ".".into());
    let values = preprocess_job_parameters(
        jt,
        &input,
        &[],
        &PathParameterOptions {
            job_template_dir: &dir,
            current_working_dir: &cwd,
            path_format: openjd_expr::path_mapping::PathFormat::host(),
            allow_template_dir_walk_up: true,
            allow_uri_path_values: true,
        },
    )
    .map_err(|e| format!("{path}: {e}"))?;
    create_job(jt, &values, &jt.default_validation_context()).map_err(|e| format!("{path}: {e}"))
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let args = parse_args()?;
    match (load(&args.old)?, load(&args.new)?) {
        (DecodedTemplate::Job(a), DecodedTemplate::Job(b)) => {
            let old = instantiate(&args.old, &a, &args.old_params)?;
            let new = instantiate(&args.new, &b, &args.new_params)?;
            let tree = diff_jobs(&old, &new, &args.options);
            print_tree(&tree);
            print_steps(&tree);
            if args.plan {
                // The plan needs the full tree, whatever --max-depth said.
                let full = diff_jobs(
                    &old,
                    &new,
                    &DiffOptions::default().with_resolved_symtab(args.with_symtab),
                );
                print_plan(&full, &old, &new)?;
            }
        }
        (DecodedTemplate::Environment(a), DecodedTemplate::Environment(b)) => {
            let old = convert_environment(a.environment());
            let new = convert_environment(b.environment());
            print_tree(&diff_environments(&old, &new, &args.options));
        }
        _ => {
            return Err(
                "OLD and NEW must both be job templates or both environment templates".into(),
            )
        }
    }
    Ok(())
}

fn print_tree(tree: &DiffTree) {
    println!("== diff ({} nodes) ==", tree.nodes().len());
    if tree.is_changed() {
        print!("{tree}");
    } else {
        println!("no changes");
    }
}

/// Every step the diff saw, with its name and its position on each side.
fn print_steps(tree: &DiffTree) {
    let steps: Vec<_> = tree
        .root()
        .children
        .iter()
        .filter_map(|&c| tree.node(c))
        .filter(|n| n.segment.starts_with("steps["))
        .collect();
    if steps.is_empty() {
        return;
    }
    println!("\n== step identity ==");
    println!(
        "{:<10} {:<10} {:>5} {:>5}",
        "name", "status", "old#", "new#"
    );
    let idx = |i: Option<usize>| i.map_or("-".into(), |i| i.to_string());
    for n in steps {
        println!(
            "{:<10} {:<10} {:>5} {:>5}",
            n.key.as_deref().unwrap_or("?"),
            format!("{:?}", n.status),
            idx(n.old_index),
            idx(n.new_index)
        );
    }
}

fn print_plan(tree: &DiffTree, old: &Job, new: &Job) -> Result<(), String> {
    let topo = |j: &Job| -> Result<Vec<usize>, String> {
        StepDependencyGraph::new(j)
            .and_then(|g| g.topo_sorted())
            .map_err(|e| e.to_string())
    };
    println!("\n== apply plan ==");
    if !tree.is_changed() {
        println!("nothing to apply");
        return Ok(());
    }
    let mut n = 0;
    let mut op = |s: String| {
        n += 1;
        println!("{n:>3}. {s}");
    };
    for f in &tree.root().field_changes {
        op(format!("job        MODIFY {}", f.field));
    }
    for &c in &tree.root().children {
        let Some(node) = tree.node(c) else { continue };
        if node.is_changed() && !node.segment.starts_with("steps[") {
            let level = match node.segment.split('[').next().unwrap_or_default() {
                "parameters" => "param",
                "jobEnvironments" => "jobEnv",
                other => other,
            };
            op(format!(
                "{:<10} {:<6} {}",
                level,
                verb(node.status),
                node.key.as_deref().unwrap_or(&node.segment)
            ));
        }
    }
    // Steps by the int index threaded through the diff: a removed step's
    // `step_index` is into old.steps, every other step's into new.steps.
    let mut by_old: Vec<Option<DiffNodeId>> = vec![None; old.steps.len()];
    let mut by_new: Vec<Option<DiffNodeId>> = vec![None; new.steps.len()];
    for &c in &tree.root().children {
        let Some(node) = tree.node(c) else { continue };
        let Some(i) = node.step_index else { continue };
        let slot = if node.status == DiffStatus::Removed {
            by_old.get_mut(i)
        } else {
            by_new.get_mut(i)
        };
        if let Some(slot) = slot {
            *slot = Some(c);
        }
    }
    // Adds and modifies first, origins before dependents (topo of the NEW
    // graph): every edge a step gains points at a step that already exists.
    for &j in &topo(new)? {
        let s = &new.steps[j];
        let Some(id) = by_new.get(j).copied().flatten() else {
            continue;
        };
        let Some(node) = tree.node(id) else { continue };
        match node.status {
            DiffStatus::Unchanged => {
                println!(
                    "     step#{j:<2}    ----   {:<8} unchanged, no write",
                    s.name
                )
            }
            DiffStatus::Added => {
                op(format!(
                    "step#{j:<2}    ADD    {:<8} (store: stepId = generate_id(\"step-\"), save name -> id)",
                    s.name
                ));
                for e in s.step_environments.iter().flatten() {
                    op(format!("  stepEnv  ADD    step#{j}.{}", e.name));
                }
                task_ops(&mut op, j, None, s)?;
            }
            DiffStatus::Modified => {
                let mut what: Vec<String> =
                    node.field_changes.iter().map(|f| f.field.clone()).collect();
                what.extend(
                    node.children
                        .iter()
                        .filter_map(|&c| tree.node(c))
                        .filter(|c| c.is_changed())
                        .map(|c| c.segment.clone()),
                );
                op(format!(
                    "step#{j:<2}    MODIFY {:<8} (was old#{}; store: stepId = lookup(job, name)) [{}]",
                    s.name,
                    node.old_index.map_or("-".into(), |i| i.to_string()),
                    what.join(", ")
                ));
                let ps_changed = tree
                    .child(id, "parameterSpace")
                    .and_then(|c| tree.node(c))
                    .is_some_and(|c| c.is_changed());
                if ps_changed {
                    let old_step = node.old_index.and_then(|i| old.steps.get(i));
                    task_ops(&mut op, j, old_step, s)?;
                }
            }
            DiffStatus::Removed => {}
        }
    }
    // Removes last, dependents first (reverse topo of the OLD graph). By now
    // no remaining step depends on a removed one: the new job cannot name it.
    for &i in topo(old)?.iter().rev() {
        if by_old.get(i).copied().flatten().is_some() {
            op(format!(
                "step#{i:<2}    REMOVE {:<8} (old side; store: stepId = lookup(job, name), cascade tasks)",
                old.steps[i].name
            ));
        }
    }
    if let Some(f) = tree
        .root()
        .field_changes
        .iter()
        .find(|f| f.field == "steps.order")
    {
        println!(
            "     (steps.order {} -> {} is covered by the topo order above)",
            f.before, f.after
        );
    }
    Ok(())
}

fn verb(s: DiffStatus) -> &'static str {
    match s {
        DiffStatus::Added => "ADD",
        DiffStatus::Removed => "REMOVE",
        DiffStatus::Modified => "MODIFY",
        DiffStatus::Unchanged => "----",
    }
}

/// One line per box: `Frame∈{11..12} × Cam∈{B,C}  (4 tasks)`.
fn describe(b: &TaskBox) -> String {
    let dims: Vec<String> = b
        .space()
        .task_parameter_definitions
        .iter()
        .map(|(name, p)| {
            let vals: Vec<String> = match p {
                TaskParameter::Int {
                    range: TaskParamRange::List(v),
                    ..
                } => return format!("{name}∈{}", int_runs(v)),
                TaskParameter::String { range } | TaskParameter::Path { range } => range.clone(),
                TaskParameter::Float { range } => {
                    range.iter().map(|f| f.to_display_string()).collect()
                }
                other => vec![format!("{other:?}")],
            };
            format!("{name}∈{{{}}}", vals.join(","))
        })
        .collect();
    if dims.is_empty() {
        return "(no params)".into();
    }
    dims.join(" × ")
}

/// `[1,2,3,7,8]` -> `{1..3,7..8}`.
fn int_runs(v: &[i64]) -> String {
    let mut parts = Vec::new();
    let mut i = 0;
    while i < v.len() {
        let mut j = i;
        while j + 1 < v.len() && v[j + 1] == v[j] + 1 {
            j += 1;
        }
        parts.push(if i == j {
            v[i].to_string()
        } else {
            format!("{}..{}", v[i], v[j])
        });
        i = j + 1;
    }
    format!("{{{}}}", parts.join(","))
}

/// Task delta for one step. Existing tasks keep their store index (DDB
/// `sk = INDEX#n`); added tasks are appended after the old count.
fn task_ops(
    op: &mut impl FnMut(String),
    step: usize,
    old: Option<&Step>,
    new: &Step,
) -> Result<(), String> {
    let (os, ns) = (
        old.and_then(|s| s.parameter_space.as_ref()),
        new.parameter_space.as_ref(),
    );
    let count = |s: Option<&StepParameterSpace>| -> Result<usize, String> {
        s.map_or(Ok(1), |s| {
            StepParameterSpaceIterator::new(s)
                .map(|it| it.len())
                .map_err(|e| e.to_string())
        })
    };
    if old.is_none() {
        op(format!(
            "  tasks    step#{step}: add all {} -> store INDEX#0..",
            count(ns)?
        ));
        return Ok(());
    }
    match task_delta(os, ns) {
        TaskDelta::Unchanged => op(format!("  tasks    step#{step}: unchanged")),
        TaskDelta::Product(pd) => {
            let ProductDelta {
                old: full_old,
                kept,
                added,
                removed,
            } = *pd;
            op(format!(
                "  tasks    step#{step}: keep {}  [{}]",
                kept.len(),
                describe(&kept)
            ));
            let mut next = full_old.len();
            for b in &added {
                op(format!(
                    "  task     ADD    step#{step} {}  ({} tasks) -> store INDEX#{}..{}",
                    describe(b),
                    b.len(),
                    next,
                    next.saturating_add(b.len()).saturating_sub(1)
                ));
                next = next.saturating_add(b.len());
            }
            for b in &removed {
                let mut idx: Vec<usize> = b
                    .tasks()
                    .map_err(|e| e.to_string())?
                    .take(5)
                    .filter_map(|t| full_old.index_of(&t))
                    .collect();
                idx.sort_unstable();
                op(format!(
                    "  task     REMOVE step#{step} {}  ({} tasks) store INDEX#{:?}{}",
                    describe(b),
                    b.len(),
                    idx,
                    if b.len() > 5 { "..." } else { "" }
                ));
            }
        }
        TaskDelta::Fallback { reason } => {
            op(format!(
                "  tasks    step#{step}: REPLACE_ALL or enumerate by value ({reason}); {} old, {} new",
                count(os)?,
                count(ns)?
            ));
        }
    }
    Ok(())
}
