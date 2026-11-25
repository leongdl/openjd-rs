// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// Copyright by contributors to this project.
// SPDX-License-Identifier: (Apache-2.0 OR MIT)

//! A plan for applying a job diff to a running job.
//!
//! **Status: unstable.** This module may change or be removed in any
//! release without notice. [`plan`] turns two jobs into an [`ApplyPlan`]: the
//! ordered operations a system performs to change a job from the old one to
//! the new one. `specs/model/job-apply.md` gives the semantics.

use std::collections::{BTreeSet, HashMap};
use std::fmt;

use crate::error::ModelError;
use crate::job::diff::{diff_jobs, DiffNode, DiffStatus};
use crate::job::step_dependency_graph::StepDependencyGraph;
use crate::job::Job;

/// What an [`ApplyOp`] changes. A system that supports only some changes
/// checks each op's kinds against the ones it supports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum MutationKind {
    /// The job's `name` or `description`.
    JobMetadata,
    /// The job's `extensions`.
    JobExtensions,
    /// The relative order of the steps both jobs have.
    StepReorder,
    /// The relative order of the job environments, or of a step's
    /// environments, that both sides have.
    EnvReorder,
    /// A job parameter added.
    ParameterAdd,
    /// A job parameter removed.
    ParameterRemove,
    /// A job parameter's type or value.
    ParameterValue,
    /// An environment added.
    EnvAdd,
    /// An environment removed.
    EnvRemove,
    /// An environment's `description`.
    EnvMetadata,
    /// An environment's `variables`.
    EnvVariables,
    /// An environment's script.
    EnvScript,
    /// An environment's resolved symbol table: a parameter it reads changed.
    EnvSymbols,
    /// A step added, with its environments.
    StepAdd,
    /// A step removed, with its environments and tasks.
    StepRemove,
    /// A step's `description`.
    StepMetadata,
    /// A step's script.
    StepScript,
    /// A step's host requirements.
    StepHostRequirements,
    /// A step's dependencies.
    StepDependencies,
    /// A step's resolved symbol table: a parameter it reads changed.
    StepSymbols,
    /// Every task of an added step created.
    TaskCreate,
    /// Every task of a step replaced by the tasks of its new parameter space.
    TaskReplace,
}

impl fmt::Display for MutationKind {
    /// Writes the variant name, such as `StepScript`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self, f)
    }
}

/// What an [`ApplyOp`] applies to, by name.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum OpTarget {
    /// The job's own fields.
    Job,
    /// The job parameter with this name.
    Parameter(String),
    /// The job environment with this name.
    JobEnvironment(String),
    /// The step with this name, including its environments.
    Step(String),
    /// The tasks of the step with this name.
    StepTasks(String),
}

impl fmt::Display for OpTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OpTarget::Job => write!(f, "job"),
            OpTarget::Parameter(name) => write!(f, "parameter {name}"),
            OpTarget::JobEnvironment(name) => write!(f, "job environment {name}"),
            OpTarget::Step(name) => write!(f, "step {name}"),
            OpTarget::StepTasks(name) => write!(f, "tasks of step {name}"),
        }
    }
}

/// How an [`ApplyOp`] changes its target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum OpAction {
    /// The target exists only in the new job: create it.
    Add,
    /// The target exists in both jobs and differs: update it.
    Modify,
    /// The target exists only in the old job: delete it.
    Remove,
    /// Replace every task of a step with the tasks of its new parameter space.
    Replace,
}

impl fmt::Display for OpAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            OpAction::Add => "add",
            OpAction::Modify => "modify",
            OpAction::Remove => "remove",
            OpAction::Replace => "replace",
        })
    }
}

/// One operation of an [`ApplyPlan`].
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ApplyOp {
    /// What the op applies to.
    pub target: OpTarget,
    /// How it changes the target.
    pub action: OpAction,
    /// What it changes; never empty.
    pub kinds: BTreeSet<MutationKind>,
}

impl fmt::Display for ApplyOp {
    /// Writes `{target}: {action} [{kinds}]`, the kinds comma-separated.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {} [", self.target, self.action)?;
        for (i, kind) in self.kinds.iter().enumerate() {
            if i > 0 {
                f.write_str(", ")?;
            }
            write!(f, "{kind}")?;
        }
        f.write_str("]")
    }
}

/// The ordered operations that change a job from one job to another.
///
/// `Display` writes one line per op, numbered from 1, in the format
/// `specs/model/job-apply.md` "Display" gives.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ApplyPlan {
    ops: Vec<ApplyOp>,
}

impl ApplyPlan {
    /// The operations, in the order a system applies them.
    #[must_use]
    pub fn ops(&self) -> &[ApplyOp] {
        &self.ops
    }

    /// Whether the plan has no operations.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.ops.is_empty()
    }
}

impl fmt::Display for ApplyPlan {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, op) in self.ops.iter().enumerate() {
            writeln!(f, "{}. {op}", i + 1)?;
        }
        Ok(())
    }
}

/// Plan the operations that change a running job from `old` to `new`.
///
/// # Errors
///
/// The error [`StepDependencyGraph`] returns when either job's steps have no
/// topological order: a dependency on an unknown step, or a cycle.
/// `create_job` never produces such a job.
pub fn plan(old: &Job, new: &Job) -> Result<ApplyPlan, ModelError> {
    let new_order = StepDependencyGraph::new(new)?.topo_sorted()?;
    let old_order = StepDependencyGraph::new(old)?.topo_sorted()?;
    let tree = diff_jobs(old, new);
    let root = tree.root();
    let mut ops = Vec::new();

    let kinds: BTreeSet<_> = root
        .field_changes
        .iter()
        .map(|c| job_kind(&c.field))
        .collect();
    push(&mut ops, OpTarget::Job, OpAction::Modify, kinds);

    for (name, node) in elements(root, "parameters") {
        let kind = match node.status {
            DiffStatus::Added => MutationKind::ParameterAdd,
            DiffStatus::Removed => MutationKind::ParameterRemove,
            DiffStatus::Unchanged | DiffStatus::Modified => MutationKind::ParameterValue,
        };
        push(
            &mut ops,
            OpTarget::Parameter(name.into()),
            action(node.status),
            BTreeSet::from([kind]),
        );
    }

    // Removes first, then adds and modifies in the new job's order, so a
    // store that inserts each added environment at its new index rebuilds the
    // new list.
    let envs = elements(root, "jobEnvironments");
    let new_index: HashMap<&str, usize> = new
        .job_environments
        .iter()
        .flatten()
        .enumerate()
        .map(|(i, e)| (e.name.as_str(), i))
        .collect();
    let (removed, mut kept): (Vec<_>, Vec<_>) = envs
        .into_iter()
        .partition(|(_, n)| n.status == DiffStatus::Removed);
    kept.sort_by_key(|(name, _)| new_index.get(name).copied());
    for (name, node) in removed.into_iter().chain(kept) {
        push(
            &mut ops,
            OpTarget::JobEnvironment(name.into()),
            action(node.status),
            env_kinds(node),
        );
    }

    // Adds and modifies in the new graph's topological order, so a step's
    // dependencies exist before it gains them; then removes in reverse order
    // of the old graph, once nothing in the new job depends on them.
    let steps: HashMap<&str, &DiffNode> = elements(root, "steps").into_iter().collect();
    for step in new_order.iter().filter_map(|&i| new.steps.get(i)) {
        let Some(node) = steps.get(step.name.as_str()) else {
            continue;
        };
        let target = || OpTarget::Step(step.name.clone());
        let tasks = || OpTarget::StepTasks(step.name.clone());
        match node.status {
            DiffStatus::Added => {
                push(
                    &mut ops,
                    target(),
                    OpAction::Add,
                    [MutationKind::StepAdd].into(),
                );
                push(
                    &mut ops,
                    tasks(),
                    OpAction::Add,
                    [MutationKind::TaskCreate].into(),
                );
            }
            DiffStatus::Modified => {
                let (kinds, replace_tasks) = step_kinds(node);
                push(&mut ops, target(), OpAction::Modify, kinds);
                if replace_tasks {
                    push(
                        &mut ops,
                        tasks(),
                        OpAction::Replace,
                        [MutationKind::TaskReplace].into(),
                    );
                }
            }
            DiffStatus::Unchanged | DiffStatus::Removed => {}
        }
    }
    for step in old_order.iter().rev().filter_map(|&i| old.steps.get(i)) {
        if steps
            .get(step.name.as_str())
            .is_some_and(|n| n.status == DiffStatus::Removed)
        {
            push(
                &mut ops,
                OpTarget::Step(step.name.clone()),
                OpAction::Remove,
                [MutationKind::StepRemove].into(),
            );
        }
    }
    Ok(ApplyPlan { ops })
}

/// Add an op unless it changes nothing.
fn push(ops: &mut Vec<ApplyOp>, target: OpTarget, action: OpAction, kinds: BTreeSet<MutationKind>) {
    if !kinds.is_empty() {
        ops.push(ApplyOp {
            target,
            action,
            kinds,
        });
    }
}

fn action(status: DiffStatus) -> OpAction {
    match status {
        DiffStatus::Added => OpAction::Add,
        DiffStatus::Removed => OpAction::Remove,
        DiffStatus::Unchanged | DiffStatus::Modified => OpAction::Modify,
    }
}

/// The changed children of `node` in `collection`, with the element names
/// their `collection[name]` segments give, in the diff's order.
fn elements<'a>(node: &'a DiffNode, collection: &str) -> Vec<(&'a str, &'a DiffNode)> {
    node.children
        .iter()
        .filter(|c| c.is_changed())
        .filter_map(|c| {
            let name = c
                .segment
                .strip_prefix(collection)?
                .strip_prefix('[')?
                .strip_suffix(']')?;
            Some((name, c))
        })
        .collect()
}

// Each *_kind maps a field the diff names; any other field maps to the
// node's metadata kind, so no change goes without a kind. Children are
// matched by the segments `specs/model/job-diff.md` "Tree shape" lists.

fn job_kind(field: &str) -> MutationKind {
    match field {
        "extensions" => MutationKind::JobExtensions,
        "steps.order" => MutationKind::StepReorder,
        "jobEnvironments.order" => MutationKind::EnvReorder,
        _ => MutationKind::JobMetadata,
    }
}

/// The kinds of a changed environment node.
fn env_kinds(node: &DiffNode) -> BTreeSet<MutationKind> {
    match node.status {
        DiffStatus::Added => return [MutationKind::EnvAdd].into(),
        DiffStatus::Removed => return [MutationKind::EnvRemove].into(),
        DiffStatus::Unchanged | DiffStatus::Modified => {}
    }
    let fields = node.field_changes.iter().map(|c| {
        if c.field.starts_with("variables") {
            MutationKind::EnvVariables
        } else if c.field.starts_with("resolvedSymTab") {
            MutationKind::EnvSymbols
        } else {
            MutationKind::EnvMetadata
        }
    });
    // An environment's only child is `script`.
    let children = node
        .children
        .iter()
        .filter(|c| c.is_changed())
        .map(|_| MutationKind::EnvScript);
    fields.chain(children).collect()
}

/// The kinds of a modified step node's own op, and whether its parameter
/// space changed, which replaces its tasks.
fn step_kinds(node: &DiffNode) -> (BTreeSet<MutationKind>, bool) {
    let mut kinds: BTreeSet<_> = node
        .field_changes
        .iter()
        .map(|c| match c.field.as_str() {
            "dependencies" => MutationKind::StepDependencies,
            "stepEnvironments.order" => MutationKind::EnvReorder,
            f if f.starts_with("resolvedSymTab") => MutationKind::StepSymbols,
            _ => MutationKind::StepMetadata,
        })
        .collect();
    let mut replace_tasks = false;
    for child in node.children.iter().filter(|c| c.is_changed()) {
        match child.segment.as_str() {
            "parameterSpace" => replace_tasks = true,
            "script" => {
                kinds.insert(MutationKind::StepScript);
            }
            "hostRequirements" => {
                kinds.insert(MutationKind::StepHostRequirements);
            }
            // The other children are `stepEnvironments[N]`.
            _ => kinds.extend(env_kinds(child)),
        }
    }
    (kinds, replace_tasks)
}
