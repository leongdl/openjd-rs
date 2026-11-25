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

/// What a `Modify` [`ApplyOp`] changes inside its target. A system that
/// supports only some changes checks each op's kinds against the ones it
/// supports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
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
    /// A step environment added.
    EnvAdd,
    /// A step environment removed.
    EnvRemove,
    /// An environment's `name` or `description`.
    EnvMetadata,
    /// An environment's `variables`.
    EnvVariables,
    /// An environment's script.
    EnvScript,
    /// An environment's resolved symbol table: a parameter it reads changed.
    EnvSymbols,
    /// A step's `name` or `description`.
    StepMetadata,
    /// A step's script.
    StepScript,
    /// A step's host requirements.
    StepHostRequirements,
    /// A step's dependencies.
    StepDependencies,
    /// A step's resolved symbol table: a parameter it reads changed.
    StepSymbols,
    /// A step's parameter space, which replaces all of its tasks.
    StepParameterSpace,
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
    /// The step with this name, including its environments and tasks.
    Step(String),
}

impl fmt::Display for OpTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OpTarget::Job => write!(f, "job"),
            OpTarget::Parameter(name) => write!(f, "parameter {name}"),
            OpTarget::JobEnvironment(name) => write!(f, "job environment {name}"),
            OpTarget::Step(name) => write!(f, "step {name}"),
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
}

impl fmt::Display for OpAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            OpAction::Add => "add",
            OpAction::Modify => "modify",
            OpAction::Remove => "remove",
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
    /// What it changes inside the target: never empty for a `Modify` of a
    /// job, job environment or step, and empty otherwise.
    pub kinds: BTreeSet<MutationKind>,
}

impl fmt::Display for ApplyOp {
    /// Writes `{target}: {action}`, then ` [{kinds}]` when there are kinds,
    /// comma-separated.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.target, self.action)?;
        if !self.kinds.is_empty() {
            let kinds: Vec<_> = self.kinds.iter().map(ToString::to_string).collect();
            write!(f, " [{}]", kinds.join(", "))?;
        }
        Ok(())
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
/// `create_job` never produces such a job. [`ModelError::DecodeValidation`]
/// when the diff names a field or child that no [`MutationKind`] maps.
pub fn plan(old: &Job, new: &Job) -> Result<ApplyPlan, ModelError> {
    let new_order = StepDependencyGraph::new(new)?.topo_sorted()?;
    let old_order = StepDependencyGraph::new(old)?.topo_sorted()?;
    let tree = diff_jobs(old, new);
    let root = tree.root();
    let mut ops = Vec::new();
    let mut push = |target, action, kinds| {
        ops.push(ApplyOp {
            target,
            action,
            kinds,
        })
    };

    if !root.field_changes.is_empty() {
        let kinds = root
            .field_changes
            .iter()
            .map(|c| job_kind(&c.field))
            .collect::<Result<_, _>>()?;
        push(OpTarget::Job, OpAction::Modify, kinds);
    }

    for (name, node) in elements(root, "parameters") {
        push(
            OpTarget::Parameter(name.into()),
            action(node.status),
            BTreeSet::new(),
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
        // An added or removed node is not expanded, so it has no kinds.
        push(
            OpTarget::JobEnvironment(name.into()),
            action(node.status),
            env_kinds(node)?,
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
        let target = OpTarget::Step(step.name.clone());
        match node.status {
            DiffStatus::Added => push(target, OpAction::Add, BTreeSet::new()),
            DiffStatus::Modified => push(target, OpAction::Modify, step_kinds(node)?),
            DiffStatus::Unchanged | DiffStatus::Removed => {}
        }
    }
    for step in old_order.iter().rev().filter_map(|&i| old.steps.get(i)) {
        if steps
            .get(step.name.as_str())
            .is_some_and(|n| n.status == DiffStatus::Removed)
        {
            push(
                OpTarget::Step(step.name.clone()),
                OpAction::Remove,
                BTreeSet::new(),
            );
        }
    }
    Ok(ApplyPlan { ops })
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

// The *_kind functions match the fields and children `specs/model/job-diff.md`
// "Tree shape" lists. Any other name is an error, so a field the diff gains
// fails the plan rather than going without a kind.

/// `variables` for both `variables` and `variables[K]`.
fn collection(name: &str) -> &str {
    name.split_once('[').map_or(name, |(c, _)| c)
}

fn unmapped(node: &str, name: &str) -> ModelError {
    ModelError::DecodeValidation(format!(
        "the job diff names {node} `{name}`, which no mutation kind maps"
    ))
}

pub(crate) fn job_kind(field: &str) -> Result<MutationKind, ModelError> {
    Ok(match field {
        "name" | "description" => MutationKind::JobMetadata,
        "extensions" => MutationKind::JobExtensions,
        "steps.order" => MutationKind::StepReorder,
        "jobEnvironments.order" => MutationKind::EnvReorder,
        _ => return Err(unmapped("job field", field)),
    })
}

pub(crate) fn env_kind(field: &str) -> Result<MutationKind, ModelError> {
    Ok(match collection(field) {
        "name" | "description" => MutationKind::EnvMetadata,
        "variables" => MutationKind::EnvVariables,
        "resolvedSymTab" => MutationKind::EnvSymbols,
        _ => return Err(unmapped("environment field", field)),
    })
}

pub(crate) fn step_kind(field: &str) -> Result<MutationKind, ModelError> {
    Ok(match collection(field) {
        "name" | "description" => MutationKind::StepMetadata,
        "dependencies" => MutationKind::StepDependencies,
        "resolvedSymTab" => MutationKind::StepSymbols,
        "stepEnvironments.order" => MutationKind::EnvReorder,
        _ => return Err(unmapped("step field", field)),
    })
}

/// The kinds of a changed environment node.
fn env_kinds(node: &DiffNode) -> Result<BTreeSet<MutationKind>, ModelError> {
    let fields = node.field_changes.iter().map(|c| env_kind(&c.field));
    let children = changed(node).map(|c| match c.segment.as_str() {
        "script" => Ok(MutationKind::EnvScript),
        s => Err(unmapped("environment child", s)),
    });
    fields.chain(children).collect()
}

/// The kinds of a modified step node.
fn step_kinds(node: &DiffNode) -> Result<BTreeSet<MutationKind>, ModelError> {
    let mut kinds = node
        .field_changes
        .iter()
        .map(|c| step_kind(&c.field))
        .collect::<Result<BTreeSet<_>, _>>()?;
    for child in changed(node) {
        match (collection(&child.segment), child.status) {
            ("script", _) => kinds.insert(MutationKind::StepScript),
            ("hostRequirements", _) => kinds.insert(MutationKind::StepHostRequirements),
            ("parameterSpace", _) => kinds.insert(MutationKind::StepParameterSpace),
            ("stepEnvironments", DiffStatus::Added) => kinds.insert(MutationKind::EnvAdd),
            ("stepEnvironments", DiffStatus::Removed) => kinds.insert(MutationKind::EnvRemove),
            ("stepEnvironments", _) => {
                kinds.extend(env_kinds(child)?);
                true
            }
            _ => return Err(unmapped("step child", &child.segment)),
        };
    }
    Ok(kinds)
}

fn changed(node: &DiffNode) -> impl Iterator<Item = &DiffNode> {
    node.children.iter().filter(|c| c.is_changed())
}
