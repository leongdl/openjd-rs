// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// Copyright by contributors to this project.
// SPDX-License-Identifier: (Apache-2.0 OR MIT)

//! Job creation from a template file and `-p` arguments, shared by
//! `summary` and `diff`.

use std::path::{Path, PathBuf};

use openjd_model::job::Job;
use openjd_model::template::parse;
use openjd_model::template::JobTemplate;

use crate::common::{caller_limits, document_type, parse_extensions, read_input_file};

/// Decode the job template at `path` and create its job, as `summary` and
/// `diff` do: `parameters` are `-p` arguments, `extensions` is the
/// `--extensions` argument, and `environments` are environment template
/// files whose parameter definitions the parameters are checked against.
pub fn create_job_from_file(
    path: &Path,
    parameters: &[String],
    extensions: &Option<String>,
    environments: &[PathBuf],
) -> Result<(JobTemplate, Job), Box<dyn std::error::Error>> {
    let content = read_input_file(path)?;
    let template_value =
        parse::document_string_to_object(&content, document_type(path), &caller_limits())?;

    let exts = parse_extensions(extensions)?;
    let supported_exts: Vec<&str> = exts.iter().map(|s| s.as_str()).collect();

    let job_template =
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

    let input_values = crate::run::parse_cli_parameters(
        parameters,
        &crate::run::float_parameter_names(&job_template, &env_templates),
    )?;
    let job_template_dir = crate::run::strip_extended_prefix(
        std::fs::canonicalize(path)?
            .parent()
            .unwrap_or_else(|| Path::new(".")),
    );
    let current_working_dir = crate::run::strip_extended_prefix(&std::env::current_dir()?);
    let param_values = openjd_model::preprocess_job_parameters(
        &job_template,
        &input_values,
        &env_templates,
        &openjd_model::PathParameterOptions {
            job_template_dir: job_template_dir.to_str().unwrap_or("."),
            current_working_dir: current_working_dir.to_str().unwrap_or("."),
            path_format: openjd_expr::path_mapping::PathFormat::host(),
            allow_template_dir_walk_up: false,
            allow_uri_path_values: true,
        },
    )?;

    // Derive the context from the template itself — `create_job`
    // requires the context's revision/extensions to cover the
    // template's — and carry the CLI's caller limits so job creation
    // enforces the same caps the decode ran under.
    let ctx = job_template
        .default_validation_context()
        .with_caller_limits(caller_limits());

    let job = openjd_model::create_job(&job_template, &param_values, &ctx)?;
    Ok((job_template, job))
}
