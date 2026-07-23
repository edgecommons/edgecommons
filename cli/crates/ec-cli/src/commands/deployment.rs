//! `edgecommons deployment …` — the kernel verbs over the local adapters (DESIGN-cli §8).
//!
//! `validate`, `render`, and `plan` run with no server and no network (RM-012); `release`
//! additionally reads local Git provenance through the adapter. `lock` is the one verb that
//! reaches the network (§8.7). `diff` is not built yet and reports NotImplemented.

use std::path::Path;

use ec_adapters::{LoadedWorkspace, RegistryTargets, describe_head, load_workspace};
use ec_deploy::ports::TargetsPort;
use ec_deploy::{
    Platform as KernelPlatform, lock as kernel_lock, release, render, validate as kernel_validate,
};
use ec_diag::{
    Diagnostic, EC4006_NO_RELEASE_INDEX, EC5001_DEPLOYMENT_SCHEMA, EC5002_DEPLOYMENT_SEMANTIC,
    EC5003_EFFECTIVE_CONFIG, EC5004_IDENTITY_DIVERGENCE, EC5005_CONFIG_INCOMPATIBLE_WITH_PIN,
    EC5006_COMPONENT_CONFIG_UNVALIDATED, EC5007_NO_LOCK, Fatal, Report,
};

use crate::cli::{Platform, Stream};

fn kernel_platform(p: Platform) -> KernelPlatform {
    match p {
        Platform::Greengrass => KernelPlatform::Greengrass,
        Platform::Host => KernelPlatform::Host,
        Platform::Kubernetes => KernelPlatform::Kubernetes,
    }
}

fn kernel_stream(s: Stream) -> ec_deploy::Stream {
    match s {
        Stream::Artifact => ec_deploy::Stream::Artifact,
        Stream::Config => ec_deploy::Stream::Config,
    }
}

fn load(definition: &Path) -> Result<LoadedWorkspace, Fatal> {
    load_workspace(definition).map_err(Fatal::Usage)
}

/// Stage one: the definition against its own embedded schema (offline by construction).
fn schema_stage(definition_text: &str, report: &mut Report) -> Result<(), Fatal> {
    let schema: serde_json::Value = serde_json::from_str(ec_deploy::DEFINITION_SCHEMA)
        .map_err(|e| Fatal::Internal(format!("embedded definition schema is invalid: {e}")))?;
    let doc: serde_json::Value = serde_yaml::from_str(definition_text)
        .map_err(|e| Fatal::Usage(format!("definition is not valid YAML: {e}")))?;
    let validator = jsonschema::validator_for(&schema).map_err(|e| {
        Fatal::Internal(format!("embedded definition schema does not compile: {e}"))
    })?;
    for error in validator.iter_errors(&doc) {
        report.push(
            Diagnostic::error(EC5001_DEPLOYMENT_SCHEMA, error.to_string())
                .with_pointer(error.instance_path.to_string()),
        );
    }
    Ok(())
}

/// Stage two: the semantic rules S-1..S-9.
fn semantic_stage(ws: &ec_deploy::workspace::Workspace, report: &mut Report) {
    let findings = kernel_validate::validate(ws, None);
    for e in findings.errors {
        report.push(Diagnostic::error(EC5002_DEPLOYMENT_SEMANTIC, e));
    }
    for w in findings.warnings {
        let code = if w.contains("thingName") {
            EC5004_IDENTITY_DIVERGENCE
        } else {
            EC5002_DEPLOYMENT_SEMANTIC
        };
        report.push(Diagnostic::warning(code, w));
    }
}

/// Stage four: the compatibility guard (DESIGN-cli §8.5.5) — derive compatibility, don't declare it.
///
/// The effective config's `component.global` is the component's *own* configuration, and the
/// canonical schema leaves it `additionalProperties: true` with zero declared properties, so today
/// it is validated by nothing. The lock carries the config schema published by the **exact pinned
/// version**, which turns "your floor says 0.3.0, so this is fine" into "`pipeline.window` is not
/// accepted by telemetry-processor 0.3.1".
///
/// Everything here degrades in the open. No lock, no locked entry, or a locked version that
/// publishes no schema each produce a **warning naming the reason** rather than a silent pass. When
/// components begin publishing schemas (RM-013), this same code path starts enforcing — no flag.
fn compatibility_stage(loaded: &LoadedWorkspace, report: &mut Report) {
    let pins = kernel_lock::pins_for(&loaded.workspace);
    if pins.is_empty() {
        return; // nothing pinned: a source-form definition has no version to be compatible with
    }
    let Some(lock) = &loaded.lock else {
        report.push(
            Diagnostic::warning(
                EC5007_NO_LOCK,
                format!(
                    "{} component version(s) are pinned but no lock is committed beside this \
                     definition, so no digest and no config schema has been resolved",
                    pins.len()
                ),
            )
            .with_help("run `edgecommons deployment lock` and commit the result"),
        );
        return;
    };
    for entry in lock.unverified() {
        report.push(Diagnostic::warning(
            EC4006_NO_RELEASE_INDEX,
            entry
                .unresolved
                .clone()
                .unwrap_or_else(|| format!("`{}` has no verified digest", entry.component)),
        ));
    }
    // Coverage is a property of the *pin*, so say it once per pinned version rather than once per
    // node × environment: the same gap repeated twenty times reads as twenty problems.
    for entry in &lock.components {
        if entry.config_schema.is_none() {
            report.push(
                Diagnostic::warning(
                    EC5006_COMPONENT_CONFIG_UNVALIDATED,
                    format!(
                        "{} {} publishes no config schema, so its `component.global` is validated \
                         by nothing",
                        entry.component, entry.version
                    ),
                )
                .with_help(
                    "the component repo must author and publish a per-version config schema \
                     (roadmap RM-013)",
                ),
            );
        }
    }

    for env in &loaded.workspace.definition.environments {
        let Ok(configs) = render::effective_configs(&loaded.workspace, &env.name) else {
            continue; // stage three already reported the render failure
        };
        for (node, component, config) in configs {
            let Some(locked) = lock.lookup(&component) else {
                continue; // not pinned, so nothing claims to be compatible
            };
            let Some(schema) = &locked.config_schema else {
                continue; // already reported once against the pin, above
            };
            let global = config
                .pointer("/component/global")
                .cloned()
                .unwrap_or_else(|| serde_json::json!({}));
            let Ok(validator) = jsonschema::validator_for(schema) else {
                report.push(Diagnostic::warning(
                    EC5006_COMPONENT_CONFIG_UNVALIDATED,
                    format!(
                        "the config schema locked for {component} {} does not compile, so its \
                         config is unvalidated",
                        locked.version
                    ),
                ));
                continue;
            };
            for error in validator.iter_errors(&global) {
                report.push(
                    Diagnostic::error(
                        EC5005_CONFIG_INCOMPATIBLE_WITH_PIN,
                        format!(
                            "{node}/{component}@{}: {error} is not accepted by {component} {}",
                            env.name, locked.version
                        ),
                    )
                    .with_pointer(format!("/component/global{}", error.instance_path)),
                );
            }
        }
    }
}

pub fn validate(definition: &Path) -> Result<Report, Fatal> {
    let loaded = load(definition)?;
    let mut report = Report::new();
    schema_stage(&loaded.definition_text, &mut report)?;
    semantic_stage(&loaded.workspace, &mut report);
    if report.error_count() > 0 {
        return Ok(report);
    }
    // Stage three: every rendered effective config against the strict runtime schema,
    // per environment (DESIGN-cli §8.1).
    for env in &loaded.workspace.definition.environments {
        let configs = render::effective_configs(&loaded.workspace, &env.name)
            .map_err(|e| Fatal::Usage(e.to_string()))?;
        for (node, component, config) in configs {
            let inner = ec_validate::schema::validate_envelope(
                &config,
                &format!("{node}/{component}@{}", env.name),
            );
            for d in inner.diagnostics {
                // Re-tag under the deployment family so CI can distinguish "your component
                // config is broken" surfaced through a deployment render.
                let mut d = d;
                d.code = EC5003_EFFECTIVE_CONFIG;
                report.push(d);
            }
        }
    }
    compatibility_stage(&loaded, &mut report);
    Ok(report)
}

pub fn render_cmd(
    definition: &Path,
    env: &str,
    target: Platform,
    quiet: bool,
) -> Result<Report, Fatal> {
    let loaded = load(definition)?;
    let mut report = Report::new();
    schema_stage(&loaded.definition_text, &mut report)?;
    semantic_stage(&loaded.workspace, &mut report);
    if report.error_count() > 0 {
        return Ok(report);
    }
    let output = run_render(&loaded, env, target)?;
    let out_root = loaded
        .root
        .join("render")
        .join(target.as_str().to_lowercase());
    for f in &output.files {
        let path = out_root.join(&f.path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| Fatal::Internal(format!("creating {}: {e}", parent.display())))?;
        }
        std::fs::write(&path, &f.text)
            .map_err(|e| Fatal::Internal(format!("writing {}: {e}", path.display())))?;
    }
    if !quiet {
        println!(
            "rendered {} files to {} (nothing committed; previews are ephemeral)",
            output.files.len(),
            out_root.display()
        );
    }
    Ok(report)
}

pub fn plan(definition: &Path, env: &str, target: Platform) -> Result<Report, Fatal> {
    let loaded = load(definition)?;
    let mut report = Report::new();
    schema_stage(&loaded.definition_text, &mut report)?;
    semantic_stage(&loaded.workspace, &mut report);
    if report.error_count() > 0 {
        return Ok(report);
    }
    let output = run_render(&loaded, env, target)?;
    let mut text =
        serde_json::to_string_pretty(&output.plan).map_err(|e| Fatal::Internal(e.to_string()))?;
    text.push('\n');
    print!("{text}");
    Ok(report)
}

pub fn release_cmd(definition: &Path, stream: Stream, quiet: bool) -> Result<Report, Fatal> {
    let loaded = load(definition)?;
    let mut report = Report::new();
    schema_stage(&loaded.definition_text, &mut report)?;
    semantic_stage(&loaded.workspace, &mut report);
    if report.error_count() > 0 {
        return Ok(report);
    }
    let def = &loaded.workspace.definition;
    let environment = match def.environments.as_slice() {
        [only] => only.name.clone(),
        envs => {
            return Err(Fatal::Usage(format!(
                "the definition declares {} environments; `deployment release` currently requires exactly one",
                envs.len()
            )));
        }
    };
    let target = KernelPlatform::from_family(&def.target_standard.family).ok_or_else(|| {
        Fatal::Usage(format!(
            "unknown targetStandard.family '{}'",
            def.target_standard.family
        ))
    })?;
    let commit = describe_head(&loaded.root).unwrap_or_else(|| "unknown".into());
    let warnings: Vec<String> = report
        .diagnostics
        .iter()
        .map(|d| d.message.clone())
        .collect();
    let output = release::build_release(
        &loaded.workspace,
        &environment,
        target,
        kernel_stream(stream),
        "initial",
        &commit,
        &warnings,
        report.error_count(),
    )
    .map_err(|e| match e {
        release::ReleaseError::Render(render::RenderError::TargetNotBuilt(p)) => {
            Fatal::NotImplemented(format!("the {p:?} renderer is not available in this build"))
        }
        other => Fatal::Usage(other.to_string()),
    })?;
    let release_dir = loaded.root.join("releases").join(&output.tag);
    for (rel, text) in &output.files {
        let path = release_dir.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| Fatal::Internal(format!("creating {}: {e}", parent.display())))?;
        }
        std::fs::write(&path, text)
            .map_err(|e| Fatal::Internal(format!("writing {}: {e}", path.display())))?;
    }
    if !quiet {
        println!(
            "release '{}' written under {} ({} files); the lock correlates both streams without fusing them",
            output.tag,
            release_dir.display(),
            output.files.len()
        );
    }
    Ok(report)
}

fn run_render(
    loaded: &LoadedWorkspace,
    env: &str,
    target: Platform,
) -> Result<render::RenderOutput, Fatal> {
    render::render_with_lock(
        &loaded.workspace,
        env,
        kernel_platform(target),
        "initial",
        loaded.lock.as_ref(),
    )
    .map_err(|e| match e {
        render::RenderError::TargetNotBuilt(p) => {
            Fatal::NotImplemented(format!("the {p:?} renderer is not available in this build"))
        }
        other => Fatal::Usage(other.to_string()),
    })
}

/// `deployment lock` — the one verb that reaches the network (DESIGN-cli §8.7).
///
/// Resolves every pinned component version against the registry and writes the result beside the
/// definition as `<stem>.lock`, so `validate`, `render`, and `plan` stay pure functions over files
/// already in Git. What cannot be verified today is recorded as unresolved **with its reason**,
/// and reported as a warning — never silently omitted.
pub fn lock(definition: &Path, source: Option<&str>, quiet: bool) -> Result<Report, Fatal> {
    let loaded = load(definition)?;
    let mut report = Report::new();
    schema_stage(&loaded.definition_text, &mut report)?;
    semantic_stage(&loaded.workspace, &mut report);
    if report.error_count() > 0 {
        return Ok(report);
    }

    let pins = kernel_lock::pins_for(&loaded.workspace);
    if pins.is_empty() {
        return Err(Fatal::Usage(
            "nothing to lock: no component in this definition pins an artifact.version \
             (a source-form artifact is a development shape with nothing to resolve)"
                .into(),
        ));
    }

    let targets = RegistryTargets::load(source).map_err(|e| Fatal::Environment(e.to_string()))?;
    let resolutions = pins
        .into_iter()
        .map(|pin| {
            let outcome = targets.resolve_pin(&pin).map_err(|e| e.to_string());
            (pin, outcome)
        })
        .collect();

    let definition_name = definition
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let lock_doc = kernel_lock::build_lock(&definition_name, resolutions);

    for entry in lock_doc.unverified() {
        report.push(
            Diagnostic::warning(
                EC4006_NO_RELEASE_INDEX,
                entry
                    .unresolved
                    .clone()
                    .unwrap_or_else(|| format!("`{}` is unresolved", entry.component)),
            )
            .with_help(
                "the pin is recorded and usable, but its digest is unverified — re-run \
                 `deployment lock` once the component publishes releases",
            ),
        );
    }

    let path = ec_adapters::lock_path_for(definition);
    let mut text =
        serde_json::to_string_pretty(&lock_doc).map_err(|e| Fatal::Internal(e.to_string()))?;
    text.push('\n');
    std::fs::write(&path, text)
        .map_err(|e| Fatal::Internal(format!("writing {}: {e}", path.display())))?;

    if !quiet {
        let verified = lock_doc.components.len() - lock_doc.unverified().len();
        println!(
            "locked {} component version(s) to {} ({verified} with a verified digest)",
            lock_doc.components.len(),
            path.display()
        );
    }
    Ok(report)
}
