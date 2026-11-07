// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// Copyright by contributors to this project.
// SPDX-License-Identifier: (Apache-2.0 OR MIT)

//! `job_diff`: diff two OpenJD templates and preview how the diff applies.
//!
//! ```text
//! cargo run -p openjd-model --example job_diff -- OLD.yaml NEW.yaml [options]
//!
//!   --max-depth N        do not expand nodes at depth N or deeper
//!   --with-symtab        also compare resolvedSymTab
//!   --param K=V          job parameter for both sides (repeatable)
//!   --old-param K=V      job parameter for the old side only
//!   --new-param K=V      job parameter for the new side only
//!   --plan               print the step/task apply plan (job templates only)
//! ```
//!
//! Both files must be job templates, or both environment templates.
//! Sample inputs are in `examples/job_diff/`.

use std::collections::HashMap;
use std::path::Path;
use std::process::ExitCode;

use openjd_model::job::diff::DiffNodeId;
use openjd_model::job::diff::{diff_environments, diff_jobs, DiffOptions, DiffStatus, DiffTree};
use openjd_model::job::ResolvedChunks;
use openjd_model::job::{Job, Step, StepParameterSpace, TaskParameter};
use openjd_model::{
    convert_environment, create_job, decode_template, preprocess_job_parameters, CallerLimits,
    DecodedTemplate, JobParameterInputValues, PathParameterOptions, StepDependencyGraph,
    StepParameterSpaceIterator, TaskParameterSet,
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
    let mut with_symtab = false;
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
            "--with-symtab" => {
                options = options.with_resolved_symtab(true);
                with_symtab = true;
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

const USAGE: &str = "usage: job_diff OLD NEW [--max-depth N] [--with-symtab] \
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

fn step_node(tree: &DiffTree, name: &str) -> Option<DiffNodeId> {
    tree.child(0, &format!("steps[{name}]"))
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
    // Removes: dependents first (reverse topo of the OLD graph).
    for &i in topo(old)?.iter().rev() {
        let s = &old.steps[i];
        if let Some(node) = step_node(tree, &s.name).and_then(|id| tree.node(id)) {
            if node.status == DiffStatus::Removed {
                op(format!(
                    "step       REMOVE {:<8} (old#{i}; store: stepId = lookup(job, name), cascade tasks)",
                    s.name
                ));
            }
        }
    }
    // Adds and modifies: origins first (topo of the NEW graph).
    for &j in &topo(new)? {
        let s = &new.steps[j];
        let Some(id) = step_node(tree, &s.name) else {
            continue;
        };
        let Some(node) = tree.node(id) else { continue };
        match node.status {
            DiffStatus::Unchanged => {
                println!("     step       ----   {:<8} unchanged, no write", s.name)
            }
            DiffStatus::Added => {
                op(format!(
                    "step       ADD    {:<8} (new#{j}; store: stepId = generate_id(\"step-\"), save name -> id)",
                    s.name
                ));
                for e in s.step_environments.iter().flatten() {
                    op(format!("  stepEnv  ADD    {}.{}", s.name, e.name));
                }
                task_ops(&mut op, &s.name, None, s)?;
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
                    "step       MODIFY {:<8} (old#{} -> new#{j}; store: stepId = lookup(job, name)) [{}]",
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
                    task_ops(&mut op, &s.name, old_step, s)?;
                }
            }
            DiffStatus::Removed => {}
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

/// Stable identity of a task: its parameter values, sorted by name.
fn canonical(t: &TaskParameterSet) -> String {
    let mut kv: Vec<(String, String)> = t
        .iter()
        .map(|(k, v)| {
            (
                k.clone(),
                serde_json::to_string(&v.value).unwrap_or_else(|e| format!("<{e}>")),
            )
        })
        .collect();
    kv.sort();
    if kv.is_empty() {
        return "(no params)".into();
    }
    kv.into_iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join(",")
}

fn tasks(space: Option<&StepParameterSpace>) -> Result<Vec<TaskParameterSet>, String> {
    match space {
        None => Ok(vec![TaskParameterSet::new()]),
        Some(s) => Ok(StepParameterSpaceIterator::new(s)
            .map_err(|e| e.to_string())?
            .collect()),
    }
}

fn chunks(space: Option<&StepParameterSpace>) -> Vec<(&str, Option<&ResolvedChunks>)> {
    space
        .iter()
        .flat_map(|s| s.task_parameter_definitions.iter())
        .map(|(k, p)| {
            let c = match p {
                TaskParameter::Int { chunks, .. } => chunks.as_ref(),
                TaskParameter::ChunkInt { chunks, .. } => Some(chunks),
                _ => None,
            };
            (k.as_str(), c)
        })
        .collect()
}

/// Value-based task delta. Existing tasks keep their store index (DDB
/// `sk = INDEX#n`); added tasks are appended after the old maximum.
fn task_ops(
    op: &mut impl FnMut(String),
    step: &str,
    old: Option<&Step>,
    new: &Step,
) -> Result<(), String> {
    let (os, ns) = (
        old.and_then(|s| s.parameter_space.as_ref()),
        new.parameter_space.as_ref(),
    );
    if old.is_some()
        && (os.and_then(|s| s.combination.as_ref()) != ns.and_then(|s| s.combination.as_ref())
            || chunks(os) != chunks(ns))
    {
        op(format!(
            "  tasks    REPLACE_ALL {step} (combination or chunking changed: no per-task mapping)"
        ));
        return Ok(());
    }
    let old_tasks = match old {
        Some(_) => tasks(os)?,
        None => Vec::new(),
    };
    let mut old_index: HashMap<String, usize> = old_tasks
        .iter()
        .enumerate()
        .map(|(i, t)| (canonical(t), i))
        .collect();
    let (mut keep, mut shifted, mut added) = (0usize, 0usize, Vec::new());
    for (j, t) in tasks(ns)?.iter().enumerate() {
        match old_index.remove(&canonical(t)) {
            Some(i) => {
                keep += 1;
                if i != j {
                    shifted += 1;
                }
            }
            None => added.push((j, canonical(t))),
        }
    }
    let mut removed: Vec<(usize, String)> = old_index.into_iter().map(|(k, i)| (i, k)).collect();
    removed.sort();
    op(format!(
        "  tasks    {step}: keep {keep} ({shifted} at a different iterator position), add {}, remove {}",
        added.len(),
        removed.len()
    ));
    let next = old_tasks.len();
    for (k, (j, t)) in added.iter().enumerate().take(5) {
        op(format!(
            "  task     ADD    {step} {t:<14} new iter#{j:<3} -> store INDEX#{}",
            next + k
        ));
    }
    if added.len() > 5 {
        println!("     ... {} more adds", added.len() - 5);
    }
    for (i, t) in removed.iter().take(5) {
        op(format!("  task     REMOVE {step} {t:<14} store INDEX#{i}"));
    }
    if removed.len() > 5 {
        println!("     ... {} more removes", removed.len() - 5);
    }
    Ok(())
}
