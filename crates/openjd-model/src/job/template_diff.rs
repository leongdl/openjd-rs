// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// Copyright by contributors to this project.
// SPDX-License-Identifier: (Apache-2.0 OR MIT)

//! A job diff of two templates and their parameter values, with the origin
//! of each change.
//!
//! **Status: unstable.** This module may change or be removed in any
//! release without notice. [`diff_templates`] builds both jobs, diffs them
//! with [`diff_jobs`], says of each change whether the template, the
//! parameter values or both caused it, and adds the template changes the
//! jobs cannot show. `specs/model/template-diff.md` gives the semantics.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

use openjd_expr::ExprValue;
use serde::Serialize;
use serde_json::Value;

use crate::error::ModelError;
use crate::job::create_job::parameters::coerce_to_job_parameter_type;
use crate::job::diff::{diff_jobs, DiffNode, DiffStatus, DiffTree, FieldChange, Origin};
use crate::job::Job;
use crate::template::{EnvironmentTemplate, JobTemplate};
use crate::types::{JobParameterInputValues, JobParameterType, ValidationContext};
use crate::PathParameterOptions;
use crate::{
    create_job, merge_job_parameter_definitions, preprocess_job_parameters,
    MergedParameterDefinition,
};

/// One side of a [`diff_templates`]: everything that builds its job.
pub struct TemplateInput<'a> {
    /// The job template.
    pub template: &'a JobTemplate,
    /// Environment templates whose parameter definitions join the template's.
    pub environments: &'a [EnvironmentTemplate],
    /// The parameter values given, before defaults are applied.
    pub parameters: &'a JobParameterInputValues,
    /// How PATH parameter values and defaults are resolved.
    pub path_options: PathParameterOptions<'a>,
    /// The context `create_job` runs under.
    pub ctx: &'a ValidationContext,
}

impl TemplateInput<'_> {
    /// Build the job: [`preprocess_job_parameters`], then [`create_job()`].
    ///
    /// # Errors
    ///
    /// The error of whichever of the two fails.
    pub fn build(&self) -> Result<Job, ModelError> {
        self.build_with(self.parameters)
    }

    fn build_with(&self, parameters: &JobParameterInputValues) -> Result<Job, ModelError> {
        let values = preprocess_job_parameters(
            self.template,
            parameters,
            self.environments,
            &self.path_options,
        )?;
        create_job(self.template, &values, self.ctx)
    }
}

/// The result of [`diff_templates`].
#[derive(Debug)]
#[non_exhaustive]
pub struct TemplateDiff {
    /// [`diff_jobs`] on the two jobs, with the origin of every change and the
    /// template changes the jobs cannot show.
    pub tree: DiffTree,
    /// The old side's job.
    pub old: Job,
    /// The new side's job.
    pub new: Job,
    /// Why every change the jobs show is [`Origin::Both`]: the new template
    /// with the old parameter values did not build. `None` when it built.
    pub cross_build_error: Option<ModelError>,
}

/// A side of a [`diff_templates`] that did not build.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum TemplateDiffError {
    /// The old side's job did not build.
    #[error("old job: {0}")]
    Old(ModelError),
    /// The new side's job did not build.
    #[error("new job: {0}")]
    New(ModelError),
}

/// Diff the jobs two template inputs build, and give each change its origin.
///
/// The origin comes from one more job, the new side with the old parameter
/// values; `specs/model/template-diff.md` gives the rule.
///
/// # Errors
///
/// [`TemplateDiffError`] names the side whose job did not build, the old side
/// first.
pub fn diff_templates(
    old: &TemplateInput<'_>,
    new: &TemplateInput<'_>,
) -> Result<TemplateDiff, TemplateDiffError> {
    let old_job = old.build().map_err(TemplateDiffError::Old)?;
    let new_job = new.build().map_err(TemplateDiffError::New)?;
    let old_definitions = merge_job_parameter_definitions(old.template, old.environments)
        .map_err(TemplateDiffError::Old)?;
    let new_definitions = merge_job_parameter_definitions(new.template, new.environments)
        .map_err(TemplateDiffError::New)?;
    let mut tree = diff_jobs(&old_job, &new_job);
    let cross = new.build_with(&cross_parameters(
        old,
        &old_job,
        &old_definitions,
        new,
        &new_definitions,
    ));
    let cross_build_error = match cross {
        Ok(mid) => {
            let template = keys(&diff_jobs(&old_job, &mid));
            let parameters = keys(&diff_jobs(&mid, &new_job));
            tag(tree.root_mut(), &mut Vec::new(), &|key| match (
                template.contains(key),
                parameters.contains(key),
            ) {
                (true, false) => Origin::Template,
                (false, true) => Origin::Parameters,
                _ => Origin::Both,
            });
            None
        }
        Err(e) => {
            tag(tree.root_mut(), &mut Vec::new(), &|_| Origin::Both);
            Some(e)
        }
    };
    template_layer(
        (old, &old_definitions),
        (new, &new_definitions),
        tree.root_mut(),
    );
    Ok(TemplateDiff {
        tree,
        old: old_job,
        new: new_job,
        cross_build_error,
    })
}

/// For each parameter the new side defines: the old value given; else none
/// when the new side has a default; else, for one the old side took from its
/// default, [`from_default`]; else the new value given.
fn cross_parameters(
    old: &TemplateInput<'_>,
    old_job: &Job,
    old_definitions: &[MergedParameterDefinition],
    new: &TemplateInput<'_>,
    new_definitions: &[MergedParameterDefinition],
) -> JobParameterInputValues {
    let mut values = JobParameterInputValues::new();
    for d in new_definitions {
        let old_default = old_definitions
            .iter()
            .find(|o| o.name == d.name)
            .and_then(|o| Some((o.param_type, o.default.as_deref()?)));
        let value = match (old.parameters.get(&d.name), &d.default) {
            (Some(v), _) => Some(v.clone()),
            (None, Some(_)) => None,
            (None, None) => match (old_job.parameters.get(&d.name), old_default) {
                (Some(p), Some(default)) => Some(from_default(&p.value, default, d.param_type)),
                _ => new.parameters.get(&d.name).cloned(),
            },
        };
        if let Some(v) = value {
            values.insert(d.name.clone(), v);
        }
    }
    values
}

/// The old side's input for a parameter it took from its default, under
/// `param_type`. Unretyped, the old job's value, so a PATH keeps the path it
/// resolved to. Retyped, the default as the model stores it (normalized, so
/// BOOL `yes` is `true`), else the old job's value when that coerces (FLOAT
/// `2.0` to INT).
fn from_default(
    value: &ExprValue,
    (old_type, default): (JobParameterType, &str),
    param_type: JobParameterType,
) -> ExprValue {
    let written = ExprValue::String(default.to_owned());
    let typed = old_type == param_type
        || (coerce_to_job_parameter_type(&written, param_type).is_err()
            && coerce_to_job_parameter_type(value, param_type).is_ok());
    if typed {
        value.clone()
    } else {
        written
    }
}

/// A change's identity: its node's segments from the root, and the field it
/// changes or the node's status.
type Key = (Vec<String>, Change);

#[derive(PartialEq, Eq, Hash)]
enum Change {
    Field(String),
    Added,
    Removed,
}

/// The key of every change in `tree`.
fn keys(tree: &DiffTree) -> HashSet<Key> {
    fn walk(node: &DiffNode, path: &mut Vec<String>, out: &mut HashSet<Key>) {
        for c in &node.field_changes {
            out.insert((path.clone(), Change::Field(c.field.clone())));
        }
        for child in &node.children {
            path.push(child.segment.clone());
            match child.status {
                DiffStatus::Added => {
                    out.insert((path.clone(), Change::Added));
                }
                DiffStatus::Removed => {
                    out.insert((path.clone(), Change::Removed));
                }
                DiffStatus::Modified | DiffStatus::Unchanged => walk(child, path, out),
            }
            path.pop();
        }
    }
    let mut out = HashSet::new();
    walk(tree.root(), &mut Vec::new(), &mut out);
    out
}

/// Set the origin of every field change below `node`, and of every added or
/// removed node, from its key.
fn tag(node: &mut DiffNode, path: &mut Vec<String>, origin: &dyn Fn(&Key) -> Origin) {
    for c in &mut node.field_changes {
        c.origin = Some(origin(&(path.clone(), Change::Field(c.field.clone()))));
    }
    for child in &mut node.children {
        path.push(child.segment.clone());
        let change = match child.status {
            DiffStatus::Added => Some(Change::Added),
            DiffStatus::Removed => Some(Change::Removed),
            DiffStatus::Modified | DiffStatus::Unchanged => None,
        };
        match change {
            Some(change) => {
                let key = (path.clone(), change);
                child.origin = Some(origin(&key));
            }
            None => tag(child, path, origin),
        }
        path.pop();
    }
}

/// One side of the template layer: its input and its merged parameter
/// definitions.
type Side<'s, 'a> = (&'s TemplateInput<'a>, &'s [MergedParameterDefinition]);

/// Add the changes the template makes that the jobs cannot show: each merged
/// parameter definition's fields other than its type, as a root child
/// `parameterDefinitions[NAME]`, and a step's `let`, on its `steps[NAME]`
/// node. Each has origin `Template`, and makes its node `Modified`.
fn template_layer(old: Side<'_, '_>, new: Side<'_, '_>, root: &mut DiffNode) {
    // Look-ups by name keep the layer linear in the definitions and steps.
    let new_definitions: HashMap<&str, &MergedParameterDefinition> =
        new.1.iter().map(|b| (b.name.as_str(), b)).collect();
    let new_steps: HashMap<&str, _> = new
        .0
        .template
        .steps
        .iter()
        .map(|b| (b.name.as_str(), b))
        .collect();
    let mut nodes = Vec::new();
    for a in old.1 {
        let Some(b) = new_definitions.get(a.name.as_str()) else {
            continue;
        };
        let mut node = DiffNode::new(
            format!("parameterDefinitions[{}]", a.name),
            DiffStatus::Modified,
        );
        let (x, y) = (
            definition_fields(a, description(old.0, &a.name)),
            definition_fields(b, description(new.0, &b.name)),
        );
        for ((field, x), (_, y)) in x.into_iter().zip(y) {
            push(&mut node, field, x, y);
        }
        if !node.field_changes.is_empty() {
            nodes.push(node);
        }
    }
    let mut changed = !nodes.is_empty();
    root.children.splice(0..0, nodes);
    let mut index: HashMap<String, usize> = root
        .children
        .iter()
        .enumerate()
        .map(|(i, c)| (c.segment.clone(), i))
        .collect();
    for a in &old.0.template.steps {
        let Some(b) = new_steps.get(a.name.as_str()) else {
            continue;
        };
        if a.let_bindings == b.let_bindings {
            continue;
        }
        // An unchanged job has no children, so the step's node may be missing.
        let segment = format!("steps[{}]", a.name);
        let i = *index.entry(segment).or_insert_with_key(|segment| {
            root.children
                .push(DiffNode::new(segment.clone(), DiffStatus::Unchanged));
            root.children.len() - 1
        });
        let Some(node) = root.children.get_mut(i) else {
            continue;
        };
        node.status = DiffStatus::Modified;
        push(
            node,
            "let",
            to_json(&a.let_bindings),
            to_json(&b.let_bindings),
        );
        changed = true;
    }
    if changed {
        root.status = DiffStatus::Modified;
    }
}

/// The description of parameter `name` from the last of the side's templates,
/// environments first, that gives one.
fn description<'a>(side: &TemplateInput<'a>, name: &str) -> Option<&'a str> {
    side.environments
        .iter()
        .filter_map(|e| e.parameter_definitions.as_deref())
        .chain(side.template.parameter_definitions.as_deref())
        .flatten()
        .rev()
        .filter(|d| d.name() == name)
        .find_map(|d| d.description())
}

fn push(node: &mut DiffNode, field: &str, before: Value, after: Value) {
    if before != after {
        node.field_changes.push(FieldChange {
            field: field.to_string(),
            before,
            after,
            origin: Some(Origin::Template),
        });
    }
}

/// A merged parameter definition's fields that the job does not carry, in a
/// fixed order, each `null` when unset: the job keeps only a parameter's
/// name, type and value. `userInterface` is left out, since it changes only
/// how a submitter presents the parameter.
fn definition_fields(
    d: &MergedParameterDefinition,
    description: Option<&str>,
) -> [(&'static str, Value); 17] {
    let either = |a: Value, b: Value| if a.is_null() { b } else { a };
    [
        ("description", to_json(&description)),
        ("default", to_json(&d.default)),
        (
            "allowedValues",
            either(
                either(
                    sorted(&d.allowed_values_int, Ord::cmp),
                    sorted(&d.allowed_values_float, f64::total_cmp),
                ),
                sorted(&d.allowed_values_str, Ord::cmp),
            ),
        ),
        (
            "minValue",
            either(to_json(&d.min_value_i64), to_json(&d.min_value_f64)),
        ),
        (
            "maxValue",
            either(to_json(&d.max_value_i64), to_json(&d.max_value_f64)),
        ),
        ("minLength", to_json(&d.min_length)),
        ("maxLength", to_json(&d.max_length)),
        (
            "item.allowedValues",
            either(
                either(
                    sorted(&d.item_allowed_values_int, Ord::cmp),
                    sorted(&d.item_allowed_values_float, f64::total_cmp),
                ),
                sorted(&d.item_allowed_values_str, Ord::cmp),
            ),
        ),
        (
            "item.minValue",
            either(
                to_json(&d.item_min_value_i64),
                to_json(&d.item_min_value_f64),
            ),
        ),
        (
            "item.maxValue",
            either(
                to_json(&d.item_max_value_i64),
                to_json(&d.item_max_value_f64),
            ),
        ),
        ("item.minLength", to_json(&d.item_min_length)),
        ("item.maxLength", to_json(&d.item_max_length)),
        (
            "item.item.allowedValues",
            sorted(&d.item_item_allowed_values_int, Ord::cmp),
        ),
        ("item.item.minValue", to_json(&d.item_item_min_value_i64)),
        ("item.item.maxValue", to_json(&d.item_item_max_value_i64)),
        ("objectType", to_json(&d.object_type)),
        ("dataFlow", to_json(&d.data_flow)),
    ]
}

/// A merged `allowedValues` set, sorted and without duplicates: merging keeps
/// it in no fixed order, and keeps a FLOAT list's duplicates. Duplicates are
/// found with `==`, as validation compares values, so -0.0 and 0.0 are one.
fn sorted<T: Clone + PartialEq + Serialize>(
    v: &Option<Vec<T>>,
    cmp: fn(&T, &T) -> Ordering,
) -> Value {
    to_json(&v.as_ref().map(|v| {
        let mut v = v.clone();
        v.sort_by(cmp);
        v.dedup();
        v
    }))
}

/// These values always serialize, so the fallback is unreachable.
fn to_json<T: Serialize + ?Sized>(v: &T) -> Value {
    serde_json::to_value(v).unwrap_or_else(|e| Value::String(format!("<unserializable: {e}>")))
}
