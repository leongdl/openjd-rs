// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// Copyright by contributors to this project.
// SPDX-License-Identifier: (Apache-2.0 OR MIT)

//! Template-level diff: build two jobs from templates and parameter sets,
//! diff them, and tag every change with where it came from.
//!
//! **Status: experimental prototype.** The API may change without notice.
//!
//! [`diff_templates`] takes two [`TemplateInput`]s, builds each into a
//! [`Job`] with the same decode, parameter preprocessing and `create_job`
//! calls the CLI uses, and runs [`diff_jobs`] on the pair. On top of that
//! job layer it adds a *template layer* for what a built job cannot show
//! (parameter definitions, step-level `let` text, `extensions`,
//! `specificationVersion`). Each [`Change`] carries an [`Origin`] (template,
//! parameters or both) and a [`Stage`] (the earliest OpenJD stage it shows
//! at). Every step gets a [`StepSummary`] of its task change, from
//! [`task_delta`].
//!
//! See `specs/model/template-diff.md`.

use std::collections::HashSet;
use std::fmt;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use indexmap::IndexMap;
use openjd_expr::path_mapping::PathFormat;
use openjd_expr::value::Float64;
use openjd_expr::{ExprType, ExprValue, RangeExpr};
use serde::{Deserialize, Serialize, Serializer};
use serde_json::{json, Value};

use crate::error::ModelError;
use crate::job::apply::{plan, ApplyPlan};
use crate::job::diff::{diff_jobs, diff_steps, DiffNode, DiffOptions, DiffStatus, DiffTree};
use crate::job::task_delta::{task_delta, TaskBox, TaskDelta};
use crate::job::TaskParameter;
use crate::job::{Environment, Job, JobParameter, Step, StepParameterSpace, TaskParamRange};
use crate::step_param_space::StepParameterSpaceIterator;
use crate::template::parse::{
    decode_environment_template, decode_job_template, document_string_to_object, DocumentType,
};
use crate::template::{EnvironmentTemplate, JobParameterDefinition, JobTemplate};
use crate::types::{CallerLimits, JobParameterInputValues, JobParameterType, ModelExtension};
use crate::{create_job, preprocess_job_parameters, PathParameterOptions};

/// Longest [`Shown::text`] in characters before it is cut with `…`.
const MAX_TEXT_CHARS: usize = 120;

// ─── inputs ─────────────────────────────────────────────────────────

/// Where a template comes from.
#[derive(Debug, Clone)]
pub enum TemplateSource {
    /// A file. JSON when the extension is `.json`, YAML otherwise.
    Path(PathBuf),
    /// Template text. JSON when it starts with `{`, YAML otherwise.
    Text(String),
    /// An already parsed document.
    Value(serde_json::Value),
    /// An already decoded and validated job template. It is not decoded
    /// again, so the extension allowlist is not applied to it.
    Decoded(Box<JobTemplate>),
}

impl TemplateSource {
    /// [`Path`](Self::Path) when `s` names an existing file, otherwise
    /// [`Text`](Self::Text).
    #[must_use]
    pub fn guess(s: &str) -> Self {
        if Path::new(s).is_file() {
            Self::Path(PathBuf::from(s))
        } else {
            Self::Text(s.to_string())
        }
    }
}

/// One side of a template diff: everything `create_job` needs.
///
/// Built with [`TemplateInput::new`] and the builder methods. The defaults
/// are the ones `openjd check` uses: every supported extension, default
/// [`CallerLimits`], the template file's directory for relative PATH
/// defaults (the process working directory for other sources), and the
/// process working directory for relative PATH values.
#[derive(Debug, Clone)]
pub struct TemplateInput {
    source: TemplateSource,
    parameters: JobParameterInputValues,
    environments: Vec<TemplateSource>,
    extensions: Vec<String>,
    limits: CallerLimits,
    template_dir: Option<PathBuf>,
    cwd: Option<PathBuf>,
}

impl TemplateInput {
    /// A side with no parameters and the defaults described on the type.
    #[must_use]
    pub fn new(source: TemplateSource) -> Self {
        Self {
            source,
            parameters: JobParameterInputValues::new(),
            environments: Vec::new(),
            extensions: ModelExtension::ALL
                .iter()
                .map(|e| e.as_str().to_string())
                .collect(),
            limits: CallerLimits::default(),
            template_dir: None,
            cwd: None,
        }
    }

    /// Set one job parameter, as a string to be coerced to its declared
    /// type. A later value for the same name wins.
    #[must_use]
    pub fn param(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.parameters
            .insert(name.into(), ExprValue::String(value.into()));
        self
    }

    /// Set several job parameters. A later value for the same name wins.
    #[must_use]
    pub fn params(mut self, values: impl IntoIterator<Item = (String, ExprValue)>) -> Self {
        self.parameters.extend(values);
        self
    }

    /// Add an environment template. Its parameter definitions are merged
    /// with the job template's, as `preprocess_job_parameters` does.
    #[must_use]
    pub fn environment(mut self, source: TemplateSource) -> Self {
        self.environments.push(source);
        self
    }

    /// Allow exactly these extensions (names such as `EXPR`).
    #[must_use]
    pub fn extensions(mut self, names: &[&str]) -> Self {
        self.extensions = names.iter().map(|s| s.to_string()).collect();
        self
    }

    /// Allow no extensions.
    #[must_use]
    pub fn no_extensions(mut self) -> Self {
        self.extensions.clear();
        self
    }

    /// Caller limits for decode and job creation.
    #[must_use]
    pub fn limits(mut self, limits: CallerLimits) -> Self {
        self.limits = limits;
        self
    }

    /// Directory that relative PATH defaults are joined to. It must be
    /// absolute.
    #[must_use]
    pub fn template_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.template_dir = Some(dir.into());
        self
    }

    /// Directory that relative PATH values are joined to.
    #[must_use]
    pub fn cwd(mut self, dir: impl Into<PathBuf>) -> Self {
        self.cwd = Some(dir.into());
        self
    }

    /// The job parameter values set so far.
    #[must_use]
    pub fn parameters(&self) -> &JobParameterInputValues {
        &self.parameters
    }

    /// Decode the template and build its job with this side's parameters.
    pub fn build(&self) -> Result<Job, ModelError> {
        let loaded = load(self)?;
        build(&loaded, &self.parameters, &self.limits)
    }
}

/// A side after decoding: the templates and the resolved directories.
struct Loaded {
    template: JobTemplate,
    environments: Vec<EnvironmentTemplate>,
    /// Everything except parameters and limits that decides the built job,
    /// for an equality check. `None` when the template was given decoded.
    fingerprint: Option<Value>,
    template_dir: String,
    cwd: String,
}

fn read_document(source: &TemplateSource, limits: &CallerLimits) -> Result<Value, ModelError> {
    match source {
        TemplateSource::Path(p) => {
            let text = std::fs::read_to_string(p).map_err(|e| {
                ModelError::DecodeValidation(format!("cannot read '{}': {e}", p.display()))
            })?;
            let kind = if p.extension().and_then(|e| e.to_str()) == Some("json") {
                DocumentType::Json
            } else {
                DocumentType::Yaml
            };
            document_string_to_object(&text, kind, limits)
        }
        TemplateSource::Text(t) => {
            let kind = if t.trim_start().starts_with('{') {
                DocumentType::Json
            } else {
                DocumentType::Yaml
            };
            document_string_to_object(t, kind, limits)
        }
        TemplateSource::Value(v) => Ok(v.clone()),
        TemplateSource::Decoded(_) => Err(ModelError::DecodeValidation(
            "a decoded job template has no document".into(),
        )),
    }
}

/// Windows `canonicalize` returns `\\?\C:\...`, which PATH handling rejects.
fn strip_extended_prefix(p: &Path) -> PathBuf {
    let s = p.to_string_lossy();
    match s.strip_prefix(r"\\?\") {
        Some(rest) => PathBuf::from(rest),
        None => p.to_path_buf(),
    }
}

fn path_string(p: &Path, what: &str) -> Result<String, ModelError> {
    p.to_str().map(str::to_string).ok_or_else(|| {
        ModelError::DecodeValidation(format!("{what} '{}' is not valid UTF-8", p.display()))
    })
}

fn load(input: &TemplateInput) -> Result<Loaded, ModelError> {
    let allow: Vec<&str> = input.extensions.iter().map(String::as_str).collect();
    let (template, document) = match &input.source {
        TemplateSource::Decoded(t) => ((**t).clone(), None),
        other => {
            let doc = read_document(other, &input.limits)?;
            let t = decode_job_template(doc.clone(), Some(&allow), &input.limits)?;
            (t, Some(doc))
        }
    };
    let mut environments = Vec::new();
    let mut env_docs = Vec::new();
    for source in &input.environments {
        if matches!(source, TemplateSource::Decoded(_)) {
            return Err(ModelError::DecodeValidation(
                "an environment template cannot be given as a decoded job template".into(),
            ));
        }
        let doc = read_document(source, &input.limits)?;
        environments.push(decode_environment_template(
            doc.clone(),
            Some(&allow),
            &input.limits,
        )?);
        env_docs.push(doc);
    }
    let process_cwd = || {
        std::env::current_dir().map_err(|e| {
            ModelError::DecodeValidation(format!("cannot read the current directory: {e}"))
        })
    };
    let cwd = match &input.cwd {
        Some(c) => c.clone(),
        None => strip_extended_prefix(&process_cwd()?),
    };
    let template_dir = match (&input.template_dir, &input.source) {
        (Some(d), _) => d.clone(),
        (None, TemplateSource::Path(p)) => {
            let full = std::fs::canonicalize(p).map_err(|e| {
                ModelError::DecodeValidation(format!("cannot resolve '{}': {e}", p.display()))
            })?;
            strip_extended_prefix(full.parent().unwrap_or(Path::new(".")))
        }
        (None, _) => cwd.clone(),
    };
    let template_dir = path_string(&template_dir, "template directory")?;
    let cwd = path_string(&cwd, "working directory")?;
    let fingerprint = document.map(|doc| {
        json!({
            "template": doc,
            "environments": env_docs,
            "extensions": input.extensions,
            "templateDir": template_dir,
            "cwd": cwd,
        })
    });
    Ok(Loaded {
        template,
        environments,
        fingerprint,
        template_dir,
        cwd,
    })
}

fn build(
    loaded: &Loaded,
    params: &JobParameterInputValues,
    limits: &CallerLimits,
) -> Result<Job, ModelError> {
    let values = preprocess_job_parameters(
        &loaded.template,
        params,
        &loaded.environments,
        &PathParameterOptions {
            job_template_dir: &loaded.template_dir,
            current_working_dir: &loaded.cwd,
            path_format: PathFormat::host(),
            allow_template_dir_walk_up: false,
            allow_uri_path_values: true,
        },
    )?;
    let ctx = loaded
        .template
        .default_validation_context()
        .with_caller_limits(limits.clone());
    create_job(&loaded.template, &values, &ctx)
}

/// Name the side that failed, keeping the error's variant where it holds a
/// message.
fn on_side(side: &str, e: ModelError) -> ModelError {
    match e {
        ModelError::DecodeValidation(m) => {
            ModelError::DecodeValidation(format!("{side} template: {m}"))
        }
        ModelError::Compatibility(m) => ModelError::Compatibility(format!("{side} template: {m}")),
        ModelError::UnsupportedSchema(m) => {
            ModelError::UnsupportedSchema(format!("{side} template: {m}"))
        }
        other => ModelError::DecodeValidation(format!("{side} template: {other}")),
    }
}

// ─── result types ───────────────────────────────────────────────────

/// Which comparison a [`Change`] comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Layer {
    /// The decoded templates, before anything is resolved.
    Template,
    /// The built jobs ([`diff_jobs`]).
    Job,
}

/// Where a change came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Origin {
    /// The template changed; the parameter values alone would not change it.
    Template,
    /// The parameter values changed; the template alone would not change it.
    Parameters,
    /// Both, or the split could not be computed (see [`TemplateDiff::notes`]).
    Both,
    /// The inputs were two built jobs ([`diff_built_jobs`]), so the cause is
    /// not known.
    Job,
}

/// The earliest generic OpenJD stage at which a change shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Stage {
    /// Template text only: definitions, step-level `let`, extensions.
    Template,
    /// Preprocessed job parameter values.
    Parameters,
    /// The built job.
    Job,
    /// A format string that only a worker can resolve: it references a
    /// `Task.` or `Session.` symbol.
    RunTime,
}

/// A value shown in a change: the JSON form and a rendering for people.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Shown {
    /// The value as JSON, in full.
    pub raw: Value,
    /// A compact rendering: ranges as `1-100`, typed values bare, symbol
    /// tables as `name=value` pairs. Cut at 120 characters with `…`.
    pub text: String,
}

impl Shown {
    fn new(raw: Value, text: String) -> Self {
        Self {
            raw,
            text: truncate(text),
        }
    }

    fn generic(raw: Value) -> Self {
        let text = render(&raw);
        Self::new(raw, text)
    }
}

impl fmt::Display for Shown {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.text)
    }
}

/// One changed field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Change {
    /// The comparison it comes from.
    pub layer: Layer,
    /// [`DiffTree`] path of the node, e.g.
    /// `steps[Render].parameterSpace.taskParameterDefinitions[Eye]`. Empty
    /// for the root. Template-layer paths are `parameterDefinitions[P]`,
    /// `steps[S]` and the root.
    pub path: String,
    /// The field, as in [`FieldChange::field`](crate::job::diff::FieldChange::field).
    /// Empty when the whole node was added, removed or left unexpanded.
    pub field: String,
    /// The old value; `None` when the node or field was added.
    pub before: Option<Shown>,
    /// The new value; `None` when the node or field was removed.
    pub after: Option<Shown>,
    /// Where it came from.
    pub origin: Origin,
    /// The earliest stage it shows at.
    pub stage: Stage,
}

/// How a step's tasks change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum TaskSummary {
    /// The same tasks in the same order.
    Unchanged {
        /// Number of tasks.
        count: u64,
    },
    /// An exact product decomposition ([`TaskDelta::Product`]).
    Product {
        /// Tasks on both sides.
        kept: BoxSummary,
        /// Tasks only on the new side, one box per product term.
        added: Vec<BoxSummary>,
        /// Tasks only on the old side.
        removed: Vec<BoxSummary>,
    },
    /// No per-task mapping ([`TaskDelta::Fallback`]): every task is replaced.
    Replaced {
        /// Old number of tasks.
        old_count: u64,
        /// New number of tasks.
        new_count: u64,
        /// Why there is no mapping.
        reason: String,
    },
    /// A new step.
    Added {
        /// Number of tasks.
        count: u64,
    },
    /// A removed step.
    Removed {
        /// Number of tasks.
        count: u64,
    },
}

/// One product box of tasks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BoxSummary {
    /// Number of tasks in the box.
    pub count: u64,
    /// Each dimension's name and its values, readable (`1-100`, `left, right`).
    pub dims: Vec<(String, String)>,
}

/// One step of either job.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StepSummary {
    /// Step name, the identity both sides are matched by.
    pub name: String,
    /// How the two sides of the step relate, with the caller's options.
    #[serde(serialize_with = "ser_status")]
    pub status: DiffStatus,
    /// Position in the old job's steps.
    pub old_index: Option<usize>,
    /// Position in the new job's steps.
    pub new_index: Option<usize>,
    /// The task change.
    pub tasks: TaskSummary,
}

/// Counts over a [`TemplateDiff`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiffStat {
    /// Steps on both sides that differ.
    pub steps_changed: u64,
    /// Steps only in the new job.
    pub steps_added: u64,
    /// Steps only in the old job.
    pub steps_removed: u64,
    /// Tasks that stay, in every step.
    pub tasks_kept: u64,
    /// Tasks appended to steps, including all tasks of added steps.
    pub tasks_added: u64,
    /// Tasks deleted from steps, including all tasks of removed steps.
    pub tasks_removed: u64,
    /// New tasks written by replacing every task of a step.
    pub tasks_replaced: u64,
}

/// The result of a template diff.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TemplateDiff {
    /// Every changed field of both layers, sorted by path, field and layer.
    pub changes: Vec<Change>,
    /// Every step of either job: old order, then new-only steps.
    pub steps: Vec<StepSummary>,
    /// The job layer as [`diff_jobs`] returned it.
    #[serde(serialize_with = "ser_tree")]
    pub job: DiffTree,
    /// The old built job.
    pub old_job: Job,
    /// The new built job.
    pub new_job: Job,
    /// Anything the diff could not decide, e.g. why origins are `Both`.
    pub notes: Vec<String>,
}

impl TemplateDiff {
    /// Whether anything differs.
    #[must_use]
    pub fn is_changed(&self) -> bool {
        !self.changes.is_empty() || self.steps.iter().any(|s| s.status != DiffStatus::Unchanged)
    }

    /// Changes at `prefix` or below it. `steps[Render]` matches
    /// `steps[Render]` and `steps[Render].script`, not `steps[Render2]`.
    /// The empty prefix matches everything.
    pub fn changes_at<'a>(&'a self, prefix: &'a str) -> impl Iterator<Item = &'a Change> + 'a {
        self.changes.iter().filter(move |c| under(&c.path, prefix))
    }

    /// The summary of the step named `name`.
    #[must_use]
    pub fn step(&self, name: &str) -> Option<&StepSummary> {
        self.steps.iter().find(|s| s.name == name)
    }

    /// The apply plan for the two jobs ([`job::apply::plan`](crate::job::apply::plan)).
    pub fn plan(&self) -> Result<ApplyPlan<'_>, ModelError> {
        plan(&self.old_job, &self.new_job)
    }

    /// Step and task counts.
    #[must_use]
    pub fn stat(&self) -> DiffStat {
        let mut s = DiffStat::default();
        for step in &self.steps {
            match step.status {
                DiffStatus::Modified => s.steps_changed += 1,
                DiffStatus::Added => s.steps_added += 1,
                DiffStatus::Removed => s.steps_removed += 1,
                DiffStatus::Unchanged => {}
            }
            match &step.tasks {
                TaskSummary::Unchanged { count } => s.tasks_kept += count,
                TaskSummary::Product {
                    kept,
                    added,
                    removed,
                } => {
                    s.tasks_kept += kept.count;
                    s.tasks_added += added.iter().map(|b| b.count).sum::<u64>();
                    s.tasks_removed += removed.iter().map(|b| b.count).sum::<u64>();
                }
                TaskSummary::Replaced { new_count, .. } => s.tasks_replaced += new_count,
                TaskSummary::Added { count } => s.tasks_added += count,
                TaskSummary::Removed { count } => s.tasks_removed += count,
            }
        }
        s
    }
}

fn under(path: &str, prefix: &str) -> bool {
    prefix.is_empty()
        || path == prefix
        || path
            .strip_prefix(prefix)
            .is_some_and(|rest| rest.starts_with('.'))
}

// ─── entry points ───────────────────────────────────────────────────

/// Diff two templates, each with its own parameters and options.
///
/// Errors name the side that failed: `old template: …` or `new template: …`.
pub fn diff_templates(
    old: &TemplateInput,
    new: &TemplateInput,
    options: &DiffOptions,
) -> Result<TemplateDiff, ModelError> {
    let lo = load(old).map_err(|e| on_side("old", e))?;
    let ln = load(new).map_err(|e| on_side("new", e))?;
    let old_job = build(&lo, &old.parameters, &old.limits).map_err(|e| on_side("old", e))?;
    let new_job = build(&ln, &new.parameters, &new.limits).map_err(|e| on_side("new", e))?;
    let template = template_layer(&lo.template, &ln.template);
    let same_template = matches!((&lo.fingerprint, &ln.fingerprint), (Some(a), Some(b)) if a == b);
    let mut notes = Vec::new();
    let rule = if old.parameters == new.parameters {
        OriginRule::All(Origin::Template)
    } else if same_template {
        OriginRule::All(Origin::Parameters)
    } else {
        // `mid` is the new template with the old parameters.
        match build(&ln, &old.parameters, &new.limits) {
            Ok(mid) => OriginRule::Split {
                template: change_keys(&diff_jobs(&old_job, &mid, options)),
                parameters: change_keys(&diff_jobs(&mid, &new_job, options)),
            },
            Err(e) => {
                notes.push(format!(
                    "origin: the new template does not build with the old parameters, so \
                     job-layer changes are marked both: {e}"
                ));
                OriginRule::All(Origin::Both)
            }
        }
    };
    Ok(assemble(old_job, new_job, template, &rule, notes, options))
}

/// Diff one template under two parameter sets. `old` and `new` are laid
/// over the template input's own parameters; a later value wins.
///
/// Errors name the build that failed: `old template: …` or
/// `new template: …`, or `template: …` when the template itself does not
/// decode.
pub fn diff_parameter_sets(
    template: &TemplateInput,
    old: &JobParameterInputValues,
    new: &JobParameterInputValues,
    options: &DiffOptions,
) -> Result<TemplateDiff, ModelError> {
    let loaded = load(template).map_err(|e| on_side("the", e))?;
    let overlay = |side: &JobParameterInputValues| {
        let mut p = template.parameters.clone();
        p.extend(side.iter().map(|(k, v)| (k.clone(), v.clone())));
        p
    };
    let (old_params, new_params) = (overlay(old), overlay(new));
    let old_job = build(&loaded, &old_params, &template.limits).map_err(|e| on_side("old", e))?;
    let new_job = build(&loaded, &new_params, &template.limits).map_err(|e| on_side("new", e))?;
    let rule = OriginRule::All(Origin::Parameters);
    Ok(assemble(
        old_job,
        new_job,
        Vec::new(),
        &rule,
        Vec::new(),
        options,
    ))
}

/// Diff two jobs that are already built. There is no template layer, and
/// every change has origin [`Origin::Job`].
#[must_use]
pub fn diff_built_jobs(old: Job, new: Job, options: &DiffOptions) -> TemplateDiff {
    assemble(
        old,
        new,
        Vec::new(),
        &OriginRule::All(Origin::Job),
        Vec::new(),
        options,
    )
}

// ─── assembly ───────────────────────────────────────────────────────

enum OriginRule {
    All(Origin),
    Split {
        template: HashSet<(String, String)>,
        parameters: HashSet<(String, String)>,
    },
}

impl OriginRule {
    fn classify(&self, path: &str, field: &str) -> Origin {
        match self {
            Self::All(o) => *o,
            Self::Split {
                template,
                parameters,
            } => {
                let key = (path.to_string(), field.to_string());
                match (template.contains(&key), parameters.contains(&key)) {
                    (true, false) => Origin::Template,
                    (false, true) => Origin::Parameters,
                    _ => Origin::Both,
                }
            }
        }
    }
}

/// The `(path, field)` of every change in a job tree.
fn change_keys(tree: &DiffTree) -> HashSet<(String, String)> {
    let mut keys = HashSet::new();
    for (id, node) in tree.changed() {
        let path = tree.path(id);
        if node.status == DiffStatus::Modified && node.expanded {
            for fc in &node.field_changes {
                keys.insert((path.clone(), fc.field.clone()));
            }
        } else {
            keys.insert((path, String::new()));
        }
    }
    keys
}

fn assemble(
    mut old_job: Job,
    mut new_job: Job,
    template: Vec<Change>,
    rule: &OriginRule,
    mut notes: Vec<String>,
    options: &DiffOptions,
) -> TemplateDiff {
    // `create_job` fills `parameters` from a `HashMap`, so their order
    // varies from run to run. Parameter order is not a change, so sort it to
    // make the tree and the result deterministic.
    old_job.parameters.sort_keys();
    new_job.parameters.sort_keys();
    let tree = diff_jobs(&old_job, &new_job, options);
    let (old_doc, new_doc) = (to_json(&old_job), to_json(&new_job));
    let mut changes = template;
    for (id, node) in tree.changed() {
        let path = tree.path(id);
        let segments = segments(&tree, id);
        let mut push = |field: String, before: Option<Shown>, after: Option<Shown>| {
            let origin = rule.classify(&path, &field);
            let stage = job_stage(&path, &field, before.as_ref(), after.as_ref());
            changes.push(Change {
                layer: Layer::Job,
                path: path.clone(),
                field,
                before,
                after,
                origin,
                stage,
            });
        };
        let side = |doc: &Value, job: &Job| {
            lookup(doc, &segments).map(|v| shown_node(node, "", v.clone(), job))
        };
        match node.status {
            DiffStatus::Added => push(String::new(), None, side(&new_doc, &new_job)),
            DiffStatus::Removed => push(String::new(), side(&old_doc, &old_job), None),
            DiffStatus::Modified if !node.expanded => push(
                String::new(),
                side(&old_doc, &old_job),
                side(&new_doc, &new_job),
            ),
            DiffStatus::Modified => {
                for fc in &node.field_changes {
                    let shown = |v: &Value, job: &Job| {
                        (!v.is_null()).then(|| shown_node(node, &fc.field, v.clone(), job))
                    };
                    push(
                        fc.field.clone(),
                        shown(&fc.before, &old_job),
                        shown(&fc.after, &new_job),
                    );
                }
            }
            DiffStatus::Unchanged => {}
        }
    }
    changes.sort_by(|a, b| (&a.path, &a.field, a.layer).cmp(&(&b.path, &b.field, b.layer)));
    for c in &mut changes {
        disambiguate(c);
    }
    let steps = step_summaries(&old_job, &new_job, options, &mut notes);
    TemplateDiff {
        changes,
        steps,
        job: tree,
        old_job,
        new_job,
        notes,
    }
}

/// When both sides render the same text but differ (`"3"` and `3`), show
/// both as JSON so the line does not read `3 -> 3`.
fn disambiguate(c: &mut Change) {
    if let (Some(b), Some(a)) = (&mut c.before, &mut c.after) {
        if b.text == a.text && b.raw != a.raw {
            b.text = truncate(b.raw.to_string());
            a.text = truncate(a.raw.to_string());
        }
    }
}

fn to_json<T: Serialize + ?Sized>(v: &T) -> Value {
    serde_json::to_value(v).unwrap_or_else(|e| Value::String(format!("<not serializable: {e}>")))
}

/// Path segments of a node, root first.
fn segments(tree: &DiffTree, id: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = tree.node(id);
    while let Some(n) = cur {
        if !n.segment.is_empty() {
            out.push(n.segment.clone());
        }
        cur = n.parent.and_then(|p| tree.node(p));
    }
    out.reverse();
    out
}

/// Follow diff segments through a serialized job: `name[key]` selects a
/// map entry, or the element of a list whose `name` is `key`.
fn lookup<'v>(doc: &'v Value, segments: &[String]) -> Option<&'v Value> {
    segments
        .iter()
        .try_fold(doc, |v, seg| match seg.split_once('[') {
            None => v.get(seg),
            Some((coll, rest)) => {
                let key = rest.strip_suffix(']')?;
                match v.get(coll)? {
                    Value::Object(m) => m.get(key),
                    Value::Array(a) => a
                        .iter()
                        .find(|e| e.get("name").and_then(Value::as_str) == Some(key)),
                    _ => None,
                }
            }
        })
}

fn job_stage(path: &str, field: &str, before: Option<&Shown>, after: Option<&Shown>) -> Stage {
    if path.starts_with("parameters[") {
        return Stage::Parameters;
    }
    let is_let = field == "let";
    let format_field = is_let
        || field.starts_with("variables.")
        || matches!(
            field,
            "command" | "args" | "timeout" | "cancelation" | "data" | "filename"
        );
    let run_time = |s: Option<&Shown>| s.is_some_and(|s| refs_run_time(&s.raw, is_let));
    if format_field && (run_time(before) || run_time(after)) {
        Stage::RunTime
    } else {
        Stage::Job
    }
}

/// Whether any string in `v` references a `Task.` or `Session.` symbol: in
/// a `{{ }}` interpolation, or anywhere in a `let` expression.
fn refs_run_time(v: &Value, is_let: bool) -> bool {
    match v {
        Value::String(s) => {
            (is_let || s.contains("{{")) && (s.contains("Task.") || s.contains("Session."))
        }
        Value::Array(a) => a.iter().any(|e| refs_run_time(e, is_let)),
        Value::Object(m) => m.values().any(|e| refs_run_time(e, is_let)),
        _ => false,
    }
}

// ─── rendering ──────────────────────────────────────────────────────

fn truncate(s: String) -> String {
    if s.chars().count() <= MAX_TEXT_CHARS {
        return s;
    }
    let mut out: String = s.chars().take(MAX_TEXT_CHARS - 1).collect();
    out.push('…');
    out
}

/// A job-layer value, rendered by what the node holds.
fn shown_node(node: &DiffNode, field: &str, raw: Value, job: &Job) -> Shown {
    use crate::job::diff::DiffNodeKind;
    let text = match (node.kind, field) {
        (DiffNodeKind::TaskParameter, "definition" | "") => {
            TaskParameter::deserialize(&raw).map_or_else(|_| render(&raw), |p| range_text(&p))
        }
        (DiffNodeKind::JobParameter, "value" | "") => node
            .key
            .as_ref()
            .and_then(|k| job.parameters.get(k))
            .map_or_else(|| render(&raw), |p| p.value.to_display_string()),
        (DiffNodeKind::JobParameter, "type") => node
            .key
            .as_ref()
            .and_then(|k| job.parameters.get(k))
            .map_or_else(|| render(&raw), |p| p.param_type.as_spec_str().to_string()),
        _ => render(&raw),
    };
    Shown::new(raw, text)
}

/// A task parameter's range: `1-100`, `1-9:2`, `[1, 3]` or `[left, right]`.
fn range_text(p: &TaskParameter) -> String {
    let ints = |r: &TaskParamRange<i64>| match r {
        TaskParamRange::RangeExpr(e) => e.to_string(),
        TaskParamRange::List(v) => list_text(v.iter().map(i64::to_string)),
    };
    match p {
        TaskParameter::Int { range, chunks } => match chunks {
            Some(c) => format!("{} chunks of {}", ints(range), c.default_task_count),
            None => ints(range),
        },
        TaskParameter::ChunkInt { range, chunks } => {
            format!("{} chunks of {}", ints(range), chunks.default_task_count)
        }
        TaskParameter::Float { range } => list_text(range.iter().map(Float64::to_display_string)),
        TaskParameter::String { range } | TaskParameter::Path { range } => {
            list_text(range.iter().cloned())
        }
    }
}

fn list_text(items: impl Iterator<Item = String>) -> String {
    format!("[{}]", items.collect::<Vec<_>>().join(", "))
}

/// Generic rendering: strings bare, lists as `[a, b]`, symbol tables as
/// `name=value` pairs, objects as `{key: value}`.
fn render(v: &Value) -> String {
    match v {
        Value::Null => "null".into(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => s.clone(),
        Value::Array(a) if !a.is_empty() && a.iter().all(is_symbol) => a
            .iter()
            .map(|e| {
                format!(
                    "{}={}",
                    e.get("name").map_or_else(String::new, render),
                    e.get("value").map_or_else(String::new, render)
                )
            })
            .collect::<Vec<_>>()
            .join(", "),
        Value::Array(a) => list_text(a.iter().map(render)),
        Value::Object(m) => format!(
            "{{{}}}",
            m.iter()
                .map(|(k, v)| format!("{k}: {}", render(v)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

/// A symbol table entry: `{name, type, value}`.
fn is_symbol(v: &Value) -> bool {
    v.as_object().is_some_and(|m| {
        m.len() == 3 && m.contains_key("name") && m.contains_key("type") && m.contains_key("value")
    })
}

// ─── template layer ─────────────────────────────────────────────────

fn template_change(
    path: String,
    field: &str,
    before: Option<Value>,
    after: Option<Value>,
) -> Change {
    Change {
        layer: Layer::Template,
        path,
        field: field.to_string(),
        before: before.map(Shown::generic),
        after: after.map(Shown::generic),
        origin: Origin::Template,
        stage: Stage::Template,
    }
}

fn template_layer(a: &JobTemplate, b: &JobTemplate) -> Vec<Change> {
    let mut out = Vec::new();
    if a.specification_version != b.specification_version {
        out.push(template_change(
            String::new(),
            "specificationVersion",
            Some(json!(a.specification_version)),
            Some(json!(b.specification_version)),
        ));
    }
    let exts = |t: &JobTemplate| {
        t.extensions
            .as_ref()
            .map(|v| json!(v.iter().map(|e| e.as_str()).collect::<Vec<_>>()))
    };
    if exts(a) != exts(b) {
        out.push(template_change(
            String::new(),
            "extensions",
            exts(a),
            exts(b),
        ));
    }

    let defs = |t: &'_ JobTemplate| -> IndexMap<String, Vec<(String, Value)>> {
        t.parameter_definitions_list()
            .iter()
            .map(|d| (d.name().to_string(), definition_record(d)))
            .collect()
    };
    let (da, db) = (defs(a), defs(b));
    for (name, ra) in &da {
        let path = format!("parameterDefinitions[{name}]");
        let Some(rb) = db.get(name) else {
            out.push(template_change(path, "", Some(record_json(ra)), None));
            continue;
        };
        let fields = ra.iter().map(|(k, _)| k).chain(
            rb.iter()
                .map(|(k, _)| k)
                .filter(|k| !ra.iter().any(|(x, _)| x == *k)),
        );
        for field in fields {
            let get =
                |r: &[(String, Value)]| r.iter().find(|(k, _)| k == field).map(|(_, v)| v.clone());
            let (va, vb) = (get(ra), get(rb));
            if va != vb {
                out.push(template_change(path.clone(), field, va, vb));
            }
        }
    }
    for (name, rb) in db.iter().filter(|(n, _)| !da.contains_key(*n)) {
        out.push(template_change(
            format!("parameterDefinitions[{name}]"),
            "",
            None,
            Some(record_json(rb)),
        ));
    }

    for sa in &a.steps {
        let Some(sb) = b.steps.iter().find(|s| s.name == sa.name) else {
            continue;
        };
        if sa.let_bindings != sb.let_bindings {
            out.push(template_change(
                format!("steps[{}]", sa.name),
                "let",
                sa.let_bindings.as_ref().map(|v| json!(v)),
                sb.let_bindings.as_ref().map(|v| json!(v)),
            ));
        }
    }
    out
}

fn record_json(r: &[(String, Value)]) -> Value {
    Value::Object(r.iter().cloned().collect())
}

/// Builds a parameter definition's comparable fields, in a fixed order.
#[derive(Default)]
struct Record(Vec<(String, Value)>);

impl Record {
    fn put(&mut self, field: &str, v: Option<Value>) {
        if let Some(v) = v {
            self.0.push((field.to_string(), v));
        }
    }

    fn ui(&mut self, control: &Option<String>, label: &Option<String>, group: &Option<String>) {
        self.put("userInterface.control", control.as_ref().map(|v| json!(v)));
        self.put("userInterface.label", label.as_ref().map(|v| json!(v)));
        self.put("userInterface.groupLabel", group.as_ref().map(|v| json!(v)));
    }
}

fn int_json(v: &crate::template::FlexInt) -> Value {
    json!(v.0)
}

fn float_json(v: &crate::template::FlexFloat) -> Value {
    json!(v.0)
}

fn filters_json(f: &[crate::template::FileFilter]) -> Value {
    Value::Array(f.iter().map(filter_json).collect())
}

fn filter_json(f: &crate::template::FileFilter) -> Value {
    json!({"label": f.label, "patterns": f.patterns})
}

/// Every field of a parameter definition that can differ, flattened:
/// `userInterface.label`, `item.minValue`, `item.item.maxValue`.
fn definition_record(d: &JobParameterDefinition) -> Vec<(String, Value)> {
    use JobParameterDefinition as D;
    let mut r = Record::default();
    r.put("type", Some(json!(d.type_name())));
    r.put("description", d.description().map(|s| json!(s)));
    let ints = |v: &[crate::template::FlexInt]| json!(v.iter().map(|i| i.0).collect::<Vec<_>>());
    let floats =
        |v: &[crate::template::FlexFloat]| json!(v.iter().map(|f| f.0).collect::<Vec<_>>());
    match d {
        D::STRING(p) => {
            r.put("default", p.default.as_ref().map(|v| json!(v)));
            r.put("allowedValues", p.allowed_values.as_ref().map(|v| json!(v)));
            r.put("minLength", p.min_length.map(|v| json!(v)));
            r.put("maxLength", p.max_length.map(|v| json!(v)));
            if let Some(u) = &p.user_interface {
                r.ui(&u.control, &u.label, &u.group_label);
            }
        }
        D::PATH(p) => {
            r.put("default", p.default.as_ref().map(|v| json!(v)));
            r.put("allowedValues", p.allowed_values.as_ref().map(|v| json!(v)));
            r.put("minLength", p.min_length.map(|v| json!(v)));
            r.put("maxLength", p.max_length.map(|v| json!(v)));
            r.put("objectType", p.object_type.map(|v| to_json(&v)));
            r.put("dataFlow", p.data_flow.map(|v| to_json(&v)));
            if let Some(u) = &p.user_interface {
                r.ui(&u.control, &u.label, &u.group_label);
                r.put(
                    "userInterface.fileFilters",
                    u.file_filters.as_deref().map(filters_json),
                );
                r.put(
                    "userInterface.fileFilterDefault",
                    u.file_filter_default.as_ref().map(filter_json),
                );
            }
        }
        D::INT(p) => {
            r.put("default", p.default.as_ref().map(int_json));
            r.put("allowedValues", p.allowed_values.as_ref().map(|v| ints(v)));
            r.put("minValue", p.min_value.as_ref().map(int_json));
            r.put("maxValue", p.max_value.as_ref().map(int_json));
            if let Some(u) = &p.user_interface {
                r.ui(&u.control, &u.label, &u.group_label);
                r.put(
                    "userInterface.singleStepDelta",
                    u.single_step_delta.as_ref().map(int_json),
                );
            }
        }
        D::FLOAT(p) => {
            r.put("default", p.default.as_ref().map(float_json));
            r.put(
                "allowedValues",
                p.allowed_values.as_ref().map(|v| floats(v)),
            );
            r.put("minValue", p.min_value.as_ref().map(float_json));
            r.put("maxValue", p.max_value.as_ref().map(float_json));
            if let Some(u) = &p.user_interface {
                r.ui(&u.control, &u.label, &u.group_label);
                r.put("userInterface.decimals", u.decimals.as_ref().map(int_json));
                r.put(
                    "userInterface.singleStepDelta",
                    u.single_step_delta.as_ref().map(float_json),
                );
            }
        }
        D::BOOL(p) => {
            r.put("default", p.default.as_ref().map(|v| json!(v.0)));
            if let Some(u) = &p.user_interface {
                r.ui(&u.control, &u.label, &u.group_label);
            }
        }
        D::RANGE_EXPR(p) => {
            r.put("default", p.default.as_ref().map(|v| json!(v)));
            r.put("minLength", p.min_length.map(|v| json!(v)));
            r.put("maxLength", p.max_length.map(|v| json!(v)));
            if let Some(u) = &p.user_interface {
                r.ui(&u.control, &u.label, &u.group_label);
            }
        }
        D::LIST_STRING(p) => {
            r.put("default", p.default.as_ref().map(|v| json!(v)));
            r.put("minLength", p.min_length.map(|v| json!(v)));
            r.put("maxLength", p.max_length.map(|v| json!(v)));
            if let Some(i) = &p.item {
                r.put(
                    "item.allowedValues",
                    i.allowed_values.as_ref().map(|v| json!(v)),
                );
                r.put("item.minLength", i.min_length.map(|v| json!(v)));
                r.put("item.maxLength", i.max_length.map(|v| json!(v)));
            }
            if let Some(u) = &p.user_interface {
                r.ui(&u.control, &u.label, &u.group_label);
            }
        }
        D::LIST_PATH(p) => {
            r.put("default", p.default.as_ref().map(|v| json!(v)));
            r.put("minLength", p.min_length.map(|v| json!(v)));
            r.put("maxLength", p.max_length.map(|v| json!(v)));
            r.put("objectType", p.object_type.map(|v| to_json(&v)));
            r.put("dataFlow", p.data_flow.map(|v| to_json(&v)));
            if let Some(i) = &p.item {
                r.put(
                    "item.allowedValues",
                    i.allowed_values.as_ref().map(|v| json!(v)),
                );
                r.put("item.minLength", i.min_length.map(|v| json!(v)));
                r.put("item.maxLength", i.max_length.map(|v| json!(v)));
            }
            if let Some(u) = &p.user_interface {
                r.ui(&u.control, &u.label, &u.group_label);
                r.put(
                    "userInterface.fileFilters",
                    u.file_filters.as_deref().map(filters_json),
                );
                r.put(
                    "userInterface.fileFilterDefault",
                    u.file_filter_default.as_ref().map(filter_json),
                );
            }
        }
        D::LIST_INT(p) => {
            r.put("default", p.default.as_deref().map(ints));
            r.put("minLength", p.min_length.map(|v| json!(v)));
            r.put("maxLength", p.max_length.map(|v| json!(v)));
            if let Some(i) = &p.item {
                r.put("item.allowedValues", i.allowed_values.as_deref().map(ints));
                r.put("item.minValue", i.min_value.as_ref().map(int_json));
                r.put("item.maxValue", i.max_value.as_ref().map(int_json));
            }
            if let Some(u) = &p.user_interface {
                r.ui(&u.control, &u.label, &u.group_label);
                r.put(
                    "userInterface.singleStepDelta",
                    u.single_step_delta.as_ref().map(int_json),
                );
            }
        }
        D::LIST_FLOAT(p) => {
            r.put("default", p.default.as_deref().map(floats));
            r.put("minLength", p.min_length.map(|v| json!(v)));
            r.put("maxLength", p.max_length.map(|v| json!(v)));
            if let Some(i) = &p.item {
                r.put(
                    "item.allowedValues",
                    i.allowed_values.as_deref().map(floats),
                );
                r.put("item.minValue", i.min_value.as_ref().map(float_json));
                r.put("item.maxValue", i.max_value.as_ref().map(float_json));
            }
            if let Some(u) = &p.user_interface {
                r.ui(&u.control, &u.label, &u.group_label);
                r.put("userInterface.decimals", u.decimals.as_ref().map(int_json));
                r.put(
                    "userInterface.singleStepDelta",
                    u.single_step_delta.as_ref().map(float_json),
                );
            }
        }
        D::LIST_BOOL(p) => {
            r.put(
                "default",
                p.default
                    .as_ref()
                    .map(|v| json!(v.iter().map(|b| b.0).collect::<Vec<_>>())),
            );
            r.put("minLength", p.min_length.map(|v| json!(v)));
            r.put("maxLength", p.max_length.map(|v| json!(v)));
            if let Some(u) = &p.user_interface {
                r.ui(&u.control, &u.label, &u.group_label);
            }
        }
        D::LIST_LIST_INT(p) => {
            r.put(
                "default",
                p.default.as_ref().map(|v| {
                    json!(v
                        .iter()
                        .map(|l| l.iter().map(|i| i.0).collect::<Vec<_>>())
                        .collect::<Vec<_>>())
                }),
            );
            r.put("minLength", p.min_length.map(|v| json!(v)));
            r.put("maxLength", p.max_length.map(|v| json!(v)));
            if let Some(i) = &p.item {
                r.put("item.minLength", i.min_length.map(|v| json!(v)));
                r.put("item.maxLength", i.max_length.map(|v| json!(v)));
                if let Some(ii) = &i.item {
                    r.put(
                        "item.item.allowedValues",
                        ii.allowed_values.as_deref().map(ints),
                    );
                    r.put("item.item.minValue", ii.min_value.as_ref().map(int_json));
                    r.put("item.item.maxValue", ii.max_value.as_ref().map(int_json));
                }
            }
            if let Some(u) = &p.user_interface {
                r.ui(&u.control, &u.label, &u.group_label);
            }
        }
    }
    r.0
}

// ─── steps and tasks ────────────────────────────────────────────────

fn task_count(step: &Step, notes: &mut Vec<String>) -> u64 {
    let Some(space) = &step.parameter_space else {
        return 1;
    };
    match StepParameterSpaceIterator::new(space) {
        Ok(it) => it.len() as u64,
        Err(e) => {
            notes.push(format!("steps[{}]: cannot count tasks: {e}", step.name));
            0
        }
    }
}

fn box_summary(b: &TaskBox) -> BoxSummary {
    BoxSummary {
        count: b.len() as u64,
        dims: b
            .space()
            .task_parameter_definitions
            .iter()
            .map(|(name, p)| (name.clone(), values_text(p)))
            .collect(),
    }
}

/// A box dimension's values: integer runs as range expressions (`1-100`,
/// `1-9:2`, `4`), other values comma-separated.
fn values_text(p: &TaskParameter) -> String {
    match p {
        TaskParameter::Int { range, .. } | TaskParameter::ChunkInt { range, .. } => match range {
            TaskParamRange::RangeExpr(e) => e.to_string(),
            TaskParamRange::List(v) => int_runs(v),
        },
        TaskParameter::Float { range } => range
            .iter()
            .map(Float64::to_display_string)
            .collect::<Vec<_>>()
            .join(", "),
        TaskParameter::String { range } | TaskParameter::Path { range } => range.join(", "),
    }
}

/// `[1,2,3,7,9,11]` -> `1-3,7-11:2`.
fn int_runs(v: &[i64]) -> String {
    let mut parts = Vec::new();
    let mut i = 0;
    while i < v.len() {
        let step = v.get(i + 1).and_then(|n| n.checked_sub(v[i]));
        let mut j = i;
        if let Some(d) = step.filter(|d| *d > 0) {
            while j + 1 < v.len() && v[j + 1].checked_sub(v[j]) == Some(d) {
                j += 1;
            }
            // A step other than 1 needs three values to be a run.
            if d != 1 && j - i < 2 {
                j = i;
            }
            if j > i {
                parts.push(if d == 1 {
                    format!("{}-{}", v[i], v[j])
                } else {
                    format!("{}-{}:{d}", v[i], v[j])
                });
                i = j + 1;
                continue;
            }
        }
        parts.push(v[i].to_string());
        i += 1;
    }
    parts.join(",")
}

fn space_of(s: &Step) -> Option<&StepParameterSpace> {
    s.parameter_space.as_ref()
}

fn step_summaries(
    old: &Job,
    new: &Job,
    options: &DiffOptions,
    notes: &mut Vec<String>,
) -> Vec<StepSummary> {
    let status_only = options.clone().with_max_depth(0);
    let mut names: Vec<&str> = old.steps.iter().map(|s| s.name.as_str()).collect();
    names.extend(
        new.steps
            .iter()
            .map(|s| s.name.as_str())
            .filter(|n| !old.steps.iter().any(|s| s.name == *n)),
    );
    let mut out = Vec::new();
    for name in names {
        let a = old.steps.iter().position(|s| s.name == name);
        let b = new.steps.iter().position(|s| s.name == name);
        let (status, tasks) = match (a.map(|i| &old.steps[i]), b.map(|j| &new.steps[j])) {
            (Some(x), Some(y)) => {
                let status = if diff_steps(x, y, &status_only).is_changed() {
                    DiffStatus::Modified
                } else {
                    DiffStatus::Unchanged
                };
                let tasks = match task_delta(space_of(x), space_of(y)) {
                    TaskDelta::Unchanged => TaskSummary::Unchanged {
                        count: task_count(y, notes),
                    },
                    TaskDelta::Product(d) => TaskSummary::Product {
                        kept: box_summary(&d.kept),
                        added: d.added.iter().map(box_summary).collect(),
                        removed: d.removed.iter().map(box_summary).collect(),
                    },
                    TaskDelta::Fallback { reason } => TaskSummary::Replaced {
                        old_count: task_count(x, notes),
                        new_count: task_count(y, notes),
                        reason,
                    },
                };
                (status, tasks)
            }
            (Some(x), None) => (
                DiffStatus::Removed,
                TaskSummary::Removed {
                    count: task_count(x, notes),
                },
            ),
            (None, Some(y)) => (
                DiffStatus::Added,
                TaskSummary::Added {
                    count: task_count(y, notes),
                },
            ),
            (None, None) => continue,
        };
        out.push(StepSummary {
            name: name.to_string(),
            status,
            old_index: a,
            new_index: b,
            tasks,
        });
    }
    out
}

// ─── built-job reader ───────────────────────────────────────────────

/// The serde form of a [`Job`], read field by field.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BuiltJob {
    name: String,
    description: Option<String>,
    extensions: Option<Vec<String>>,
    parameters: Option<IndexMap<String, Value>>,
    steps: Vec<Step>,
    job_environments: Option<Vec<Environment>>,
}

/// Read a [`Job`] back from the JSON or YAML that `openjd build` prints
/// (the job's serde form). JSON when the text starts with `{`, YAML
/// otherwise. Unknown fields are rejected.
///
/// It takes text, not a `serde_json::Value`, because a `Value` object
/// sorts its keys, which would reorder task parameter definitions and so
/// change task order.
pub fn job_from_str(text: &str) -> Result<Job, ModelError> {
    let err = |m: String| ModelError::DecodeValidation(format!("not a built job: {m}"));
    let raw: BuiltJob = if text.trim_start().starts_with('{') {
        serde_json::from_str(text).map_err(|e| err(e.to_string()))?
    } else {
        serde_saphyr::from_str(text).map_err(|e| err(e.to_string()))?
    };
    let extensions = raw
        .extensions
        .map(|v| {
            v.iter()
                .map(|e| ModelExtension::from_str(e))
                .collect::<Result<Vec<_>, _>>()
        })
        .transpose()
        .map_err(|e| err(format!("extensions: {e}")))?;
    let mut parameters = IndexMap::new();
    for (k, v) in raw.parameters.unwrap_or_default() {
        let p = parameter_from_value(&v).map_err(|e| err(format!("parameters[{k}]: {e}")))?;
        parameters.insert(k, p);
    }
    Ok(Job {
        name: raw.name,
        description: raw.description,
        extensions,
        parameters,
        steps: raw.steps,
        job_environments: raw.job_environments,
    })
}

fn parameter_from_value(v: &Value) -> Result<JobParameter, String> {
    let name = v
        .get("name")
        .and_then(Value::as_str)
        .ok_or("'name' must be a string")?
        .to_string();
    let type_name = v
        .get("paramType")
        .and_then(Value::as_str)
        .ok_or("'paramType' must be a string")?;
    let param_type = match type_name {
        "String" => JobParameterType::String,
        "Int" => JobParameterType::Int,
        "Float" => JobParameterType::Float,
        "Path" => JobParameterType::Path,
        "Bool" => JobParameterType::Bool,
        "RangeExpr" => JobParameterType::RangeExpr,
        "ListString" => JobParameterType::ListString,
        "ListInt" => JobParameterType::ListInt,
        "ListFloat" => JobParameterType::ListFloat,
        "ListPath" => JobParameterType::ListPath,
        "ListBool" => JobParameterType::ListBool,
        "ListListInt" => JobParameterType::ListListInt,
        other => return Err(format!("unknown paramType '{other}'")),
    };
    let elem_hint = match param_type {
        JobParameterType::ListBool => ExprType::BOOL,
        JobParameterType::ListInt => ExprType::INT,
        JobParameterType::ListFloat => ExprType::FLOAT,
        JobParameterType::ListPath => ExprType::PATH,
        JobParameterType::ListListInt => ExprType::parse("list[int]")?,
        _ => ExprType::STRING,
    };
    let value = expr_value(v.get("value").ok_or("missing 'value'")?, &elem_hint)?;
    Ok(JobParameter {
        name,
        param_type,
        value,
    })
}

/// An `ExprValue` from its derived serde form, e.g. `{"String": "x"}` or
/// `{"ListString": [["a"], 48]}`. `hint` types an empty list.
fn expr_value(v: &Value, hint: &ExprType) -> Result<ExprValue, String> {
    if v.as_str() == Some("Null") {
        return Ok(ExprValue::Null);
    }
    let bad = || format!("unrecognized value {v}");
    let Some((tag, inner)) = v
        .as_object()
        .filter(|m| m.len() == 1)
        .and_then(|m| m.iter().next())
    else {
        return Err(bad());
    };
    // Lists serialize as `[elements, ...cached fields]`.
    let elements = || -> Result<&Vec<Value>, String> {
        match inner {
            Value::Array(a) => match a.first() {
                Some(Value::Array(e))
                    if tag != "ListBool" && tag != "ListInt" && tag != "ListFloat" =>
                {
                    Ok(e)
                }
                _ => Ok(a),
            },
            _ => Err(bad()),
        }
    };
    let list = |items: Vec<ExprValue>, elem: ExprType| {
        ExprValue::make_list(items, elem).map_err(|e| e.to_string())
    };
    let float = |x: &Value| Float64::deserialize(x).map_err(|e| e.to_string());
    let string = |x: &Value| x.as_str().map(str::to_string).ok_or_else(bad);
    match tag.as_str() {
        "Bool" => inner.as_bool().map(ExprValue::Bool).ok_or_else(bad),
        "Int" => inner.as_i64().map(ExprValue::Int).ok_or_else(bad),
        "Float" => float(inner).map(ExprValue::Float),
        "String" => string(inner).map(ExprValue::String),
        "Path" => {
            let value = inner.get("value").ok_or_else(bad).and_then(string)?;
            let format = PathFormat::deserialize(inner.get("format").ok_or_else(bad)?)
                .map_err(|e| e.to_string())?;
            Ok(ExprValue::new_path(value, format))
        }
        "RangeExpr" => RangeExpr::deserialize(inner)
            .map(ExprValue::RangeExpr)
            .map_err(|e| e.to_string()),
        "ListBool" => list(
            elements()?
                .iter()
                .map(|e| e.as_bool().map(ExprValue::Bool).ok_or_else(bad))
                .collect::<Result<_, _>>()?,
            ExprType::BOOL,
        ),
        "ListInt" => list(
            elements()?
                .iter()
                .map(|e| e.as_i64().map(ExprValue::Int).ok_or_else(bad))
                .collect::<Result<_, _>>()?,
            ExprType::INT,
        ),
        "ListFloat" => list(
            elements()?
                .iter()
                .map(|e| float(e).map(ExprValue::Float))
                .collect::<Result<_, _>>()?,
            ExprType::FLOAT,
        ),
        "ListString" => list(
            elements()?
                .iter()
                .map(|e| string(e).map(ExprValue::String))
                .collect::<Result<_, _>>()?,
            ExprType::STRING,
        ),
        "ListPath" => {
            let format = match inner.get(1) {
                Some(f) => PathFormat::deserialize(f).map_err(|e| e.to_string())?,
                None => return Err(bad()),
            };
            list(
                elements()?
                    .iter()
                    .map(|e| string(e).map(|s| ExprValue::new_path(s, format)))
                    .collect::<Result<_, _>>()?,
                ExprType::PATH,
            )
        }
        "ListList" => {
            let inner_hint = hint.list_element_type().cloned().unwrap_or(ExprType::INT);
            list(
                elements()?
                    .iter()
                    .map(|e| expr_value(e, &inner_hint))
                    .collect::<Result<_, _>>()?,
                hint.clone(),
            )
        }
        _ => Err(bad()),
    }
}

// ─── serde helpers ──────────────────────────────────────────────────

fn status_str(s: DiffStatus) -> &'static str {
    match s {
        DiffStatus::Unchanged => "unchanged",
        DiffStatus::Added => "added",
        DiffStatus::Removed => "removed",
        DiffStatus::Modified => "modified",
    }
}

fn ser_status<S: Serializer>(s: &DiffStatus, ser: S) -> Result<S::Ok, S::Error> {
    ser.serialize_str(status_str(*s))
}

/// The tree as its changed nodes: `{path, status, expanded, fieldChanges}`.
fn ser_tree<S: Serializer>(tree: &DiffTree, ser: S) -> Result<S::Ok, S::Error> {
    let nodes: Vec<Value> = tree
        .changed()
        .map(|(id, n)| {
            json!({
                "path": tree.path(id),
                "status": status_str(n.status),
                "expanded": n.expanded,
                "fieldChanges": n.field_changes.iter().map(|c| json!({
                    "field": c.field, "before": c.before, "after": c.after,
                })).collect::<Vec<_>>(),
            })
        })
        .collect();
    nodes.serialize(ser)
}

// ─── Display ────────────────────────────────────────────────────────

impl fmt::Display for Layer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Template => "template",
            Self::Job => "job",
        })
    }
}

impl fmt::Display for Origin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Template => "template",
            Self::Parameters => "parameters",
            Self::Both => "both",
            Self::Job => "job",
        })
    }
}

impl fmt::Display for Stage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Template => "template",
            Self::Parameters => "parameters",
            Self::Job => "job",
            Self::RunTime => "run time",
        })
    }
}

impl Change {
    fn mark(&self) -> char {
        match (&self.before, &self.after) {
            (None, Some(_)) => '+',
            (Some(_), None) => '-',
            _ => '~',
        }
    }

    fn value_text(&self) -> String {
        match (&self.before, &self.after) {
            (Some(b), Some(a)) => format!("{b} -> {a}"),
            (None, Some(a)) => a.text.clone(),
            (Some(b), None) => b.text.clone(),
            (None, None) => String::new(),
        }
    }

    /// `{mark} {label}: {values}  [{origin}]`.
    fn line(&self, label: &str) -> String {
        let values = self.value_text();
        if values.is_empty() {
            format!("{} {label}  [{}]", self.mark(), self.origin)
        } else {
            format!("{} {label}: {values}  [{}]", self.mark(), self.origin)
        }
    }

    /// The label in the job-level part of a [`TemplateDiff`] listing.
    fn job_label(&self) -> String {
        let keyed = |prefix: &str| {
            self.path
                .strip_prefix(prefix)
                .and_then(|r| r.strip_suffix(']'))
        };
        if let Some(name) = keyed("parameters[") {
            return match self.field.as_str() {
                "" | "value" => format!("parameter {name}"),
                f => format!("parameter {name}.{f}"),
            };
        }
        if let Some(name) = keyed("parameterDefinitions[") {
            return match self.field.as_str() {
                "" => format!("parameter definition {name}"),
                f => format!("parameter definition {name}.{f}"),
            };
        }
        join_label(&self.path, &self.field)
    }

    /// The label under a step, relative to `steps[S]`.
    fn step_label(&self, step_path: &str) -> String {
        let rel = self
            .path
            .strip_prefix(step_path)
            .map(|r| r.trim_start_matches('.'))
            .unwrap_or(&self.path);
        if let Some(name) = rel
            .strip_prefix("parameterSpace.taskParameterDefinitions[")
            .and_then(|r| r.strip_suffix(']'))
        {
            if matches!(self.field.as_str(), "" | "definition") {
                return name.to_string();
            }
        }
        join_label(rel, &self.field)
    }
}

fn join_label(path: &str, field: &str) -> String {
    match (path.is_empty(), field.is_empty()) {
        (true, true) => "<root>".into(),
        (true, false) => field.to_string(),
        (false, true) => path.to_string(),
        (false, false) => format!("{path}.{field}"),
    }
}

/// `~ steps[Render].script.onRun.args: [a] -> [b]  [template]`.
impl fmt::Display for Change {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.line(&join_label(&self.path, &self.field)))
    }
}

/// `Frame 1-100 × Eye left`.
impl fmt::Display for BoxSummary {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.dims.is_empty() {
            return f.write_str("(no parameters)");
        }
        let dims: Vec<String> = self.dims.iter().map(|(n, v)| format!("{n} {v}")).collect();
        f.write_str(&dims.join(" × "))
    }
}

impl TaskSummary {
    /// Old and new task counts.
    fn counts(&self) -> (u64, u64) {
        match self {
            Self::Unchanged { count } => (*count, *count),
            Self::Product {
                kept,
                added,
                removed,
            } => (
                kept.count + removed.iter().map(|b| b.count).sum::<u64>(),
                kept.count + added.iter().map(|b| b.count).sum::<u64>(),
            ),
            Self::Replaced {
                old_count,
                new_count,
                ..
            } => (*old_count, *new_count),
            Self::Added { count } => (0, *count),
            Self::Removed { count } => (*count, 0),
        }
    }
}

/// `tasks: 100 -> 400`, then one `keep`, `add` or `drop` line per box.
impl fmt::Display for TaskSummary {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (old, new) = self.counts();
        match self {
            Self::Unchanged { count } => write!(f, "tasks: {count} unchanged"),
            Self::Added { count } => write!(f, "tasks: {count} added"),
            Self::Removed { count } => write!(f, "tasks: {count} removed"),
            Self::Replaced { reason, .. } => {
                write!(f, "tasks: {old} -> {new}, all replaced ({reason})")
            }
            Self::Product {
                kept,
                added,
                removed,
            } => {
                write!(f, "tasks: {old} -> {new}")?;
                let boxes = std::iter::once(("keep", kept))
                    .filter(|(_, b)| b.count > 0)
                    .chain(added.iter().map(|b| ("add", b)))
                    .chain(removed.iter().map(|b| ("drop", b)));
                for (verb, b) in boxes {
                    write!(f, "\n  {verb:<4} {:<5} {b}", b.count)?;
                }
                Ok(())
            }
        }
    }
}

impl StepSummary {
    fn mark(&self) -> char {
        match self.status {
            DiffStatus::Added => '+',
            DiffStatus::Removed => '-',
            _ => '~',
        }
    }

    /// One line for `openjd diff --stat`:
    /// `~ Render  tasks 100 -> 400  (+300 -0)`.
    #[must_use]
    pub fn stat_line(&self) -> String {
        let (old, new) = self.tasks.counts();
        let delta = match &self.tasks {
            TaskSummary::Replaced { .. } => "replaced".to_string(),
            TaskSummary::Product { added, removed, .. } => format!(
                "+{} -{}",
                added.iter().map(|b| b.count).sum::<u64>(),
                removed.iter().map(|b| b.count).sum::<u64>()
            ),
            _ => format!("+{} -{}", new.saturating_sub(old), old.saturating_sub(new)),
        };
        format!(
            "{} {}  tasks {old} -> {new}  ({delta})",
            self.mark(),
            self.name
        )
    }
}

/// `~ step Render`, then the task summary indented by four spaces.
impl fmt::Display for StepSummary {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} step {}", self.mark(), self.name)?;
        for line in self.tasks.to_string().lines() {
            write!(f, "\n    {line}")?;
        }
        Ok(())
    }
}

/// `1 step changed. Tasks: 100 kept, 300 added, 0 removed.`
impl fmt::Display for DiffStat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = if self.steps_changed == 1 { "" } else { "s" };
        write!(f, "{} step{s} changed", self.steps_changed)?;
        if self.steps_added > 0 {
            write!(f, ", {} added", self.steps_added)?;
        }
        if self.steps_removed > 0 {
            write!(f, ", {} removed", self.steps_removed)?;
        }
        write!(
            f,
            ". Tasks: {} kept, {} added, {} removed",
            self.tasks_kept, self.tasks_added, self.tasks_removed
        )?;
        if self.tasks_replaced > 0 {
            write!(f, ", {} replaced", self.tasks_replaced)?;
        }
        f.write_str(".")
    }
}

/// Job-level changes, then each changed step with its changes and task
/// summary, then the [`DiffStat`] line. `no changes` when nothing differs.
impl fmt::Display for TemplateDiff {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if !self.is_changed() {
            return f.write_str("no changes");
        }
        let mut lines: Vec<String> = Vec::new();
        let push = |lines: &mut Vec<String>, line: String| {
            // Both layers report `extensions`; print a repeated line once.
            if lines.last() != Some(&line) {
                lines.push(line);
            }
        };
        for c in self
            .changes
            .iter()
            .filter(|c| !c.path.starts_with("steps["))
        {
            push(&mut lines, c.line(&c.job_label()));
        }
        for s in &self.steps {
            let step_path = format!("steps[{}]", s.name);
            let changes: Vec<&Change> = self
                .changes_at(&step_path)
                .filter(|c| !(c.path == step_path && c.field.is_empty()))
                .collect();
            if s.status == DiffStatus::Unchanged && changes.is_empty() {
                continue;
            }
            lines.push(format!("{} step {}", s.mark(), s.name));
            for c in changes {
                push(
                    &mut lines,
                    format!("    {}", c.line(&c.step_label(&step_path))),
                );
            }
            for line in s.tasks.to_string().lines() {
                lines.push(format!("    {line}"));
            }
        }
        lines.push(String::new());
        lines.push(self.stat().to_string());
        f.write_str(&lines.join("\n"))
    }
}
