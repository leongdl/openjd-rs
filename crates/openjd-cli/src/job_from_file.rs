// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// Copyright by contributors to this project.
// SPDX-License-Identifier: (Apache-2.0 OR MIT)

//! Job creation from a template file and `-p` arguments, shared by
//! `summary` and `diff`.

use std::path::{Path, PathBuf};

use openjd_model::job::template_diff::TemplateInput;
use openjd_model::job::Job;
use openjd_model::template::parse;
use openjd_model::template::{EnvironmentTemplate, JobTemplate};
use openjd_model::{JobParameterInputValues, PathParameterOptions, ValidationContext};

use crate::common::{caller_limits, document_type, parse_extensions, read_input_file};

/// A job template file read with its environment templates and `-p`
/// arguments: everything that builds its job.
pub struct TemplateFile {
    pub template: JobTemplate,
    environments: Vec<EnvironmentTemplate>,
    parameters: JobParameterInputValues,
    template_dir: String,
    current_working_dir: String,
    ctx: ValidationContext,
}

impl TemplateFile {
    /// Decode the job template at `path`, as `summary` and `diff` do:
    /// `parameters` are `-p` arguments, `extensions` is the `--extensions`
    /// argument, and `environments` are environment template files whose
    /// parameter definitions the parameters are checked against.
    pub fn load(
        path: &Path,
        parameters: &[String],
        extensions: &Option<String>,
        environments: &[PathBuf],
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let content = read_input_file(path)?;
        let template_value =
            parse::document_string_to_object(&content, document_type(path), &caller_limits())?;

        let exts = parse_extensions(extensions)?;
        let supported_exts: Vec<&str> = exts.iter().map(|s| s.as_str()).collect();

        let template =
            parse::decode_job_template(template_value, Some(&supported_exts), &caller_limits())?;

        let mut env_templates = Vec::new();
        for env_path in environments {
            let env_content = std::fs::read_to_string(env_path)?;
            let env_value = parse::document_string_to_object(
                &env_content,
                document_type(env_path),
                &caller_limits(),
            )?;
            env_templates.push(parse::decode_environment_template(
                env_value,
                Some(&supported_exts),
                &caller_limits(),
            )?);
        }

        let parameters = crate::run::parse_cli_parameters(
            parameters,
            &crate::run::float_parameter_names(&template, &env_templates),
        )?;
        let template_dir = crate::run::strip_extended_prefix(
            std::fs::canonicalize(path)?
                .parent()
                .unwrap_or_else(|| Path::new(".")),
        );
        let current_working_dir = crate::run::strip_extended_prefix(&std::env::current_dir()?);

        // Derive the context from the template itself — `create_job`
        // requires the context's revision/extensions to cover the
        // template's — and carry the CLI's caller limits so job creation
        // enforces the same caps the decode ran under.
        let ctx = template
            .default_validation_context()
            .with_caller_limits(caller_limits());
        Ok(Self {
            template,
            environments: env_templates,
            parameters,
            template_dir: template_dir.to_str().unwrap_or(".").to_string(),
            current_working_dir: current_working_dir.to_str().unwrap_or(".").to_string(),
            ctx,
        })
    }

    /// The input that builds this file's job.
    pub fn input(&self) -> TemplateInput<'_> {
        TemplateInput {
            template: &self.template,
            environments: &self.environments,
            parameters: &self.parameters,
            path_options: PathParameterOptions {
                job_template_dir: &self.template_dir,
                current_working_dir: &self.current_working_dir,
                path_format: openjd_expr::path_mapping::PathFormat::host(),
                allow_template_dir_walk_up: false,
                allow_uri_path_values: true,
            },
            ctx: &self.ctx,
        }
    }
}

/// Decode the job template at `path` and create its job; see
/// [`TemplateFile::load`].
pub fn create_job_from_file(
    path: &Path,
    parameters: &[String],
    extensions: &Option<String>,
    environments: &[PathBuf],
) -> Result<(JobTemplate, Job), Box<dyn std::error::Error>> {
    let file = TemplateFile::load(path, parameters, extensions, environments)?;
    let job = file.input().build()?;
    Ok((file.template, job))
}
