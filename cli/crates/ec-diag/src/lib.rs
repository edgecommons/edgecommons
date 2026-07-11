//! Diagnostics, exit codes, and output rendering for the `edgecommons` CLI.
//!
//! Every verb reports through the one [`Diagnostic`] model (DESIGN-cli §6.4), so
//! `component validate` and `deployment validate` differ only in *what they collect*,
//! never in how they report. Human and `--json` output are two renderers over the
//! same data, which is what keeps the JSON surface stable.

use std::fmt;
use std::path::{Path, PathBuf};

use serde::Serialize;

/// Process exit codes (DESIGN-cli §4.2).
///
/// `NotImplemented` is distinct from `Internal` on purpose: a verb that is declared
/// but not yet built is not a crash, and callers (CI especially) should be able to
/// tell the difference.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum ExitCode {
    /// Success.
    Ok = 0,
    /// The command ran and produced findings (validation failed, lint errors).
    Findings = 1,
    /// The command was invoked incorrectly.
    Usage = 2,
    /// A required external tool or environment prerequisite is missing.
    Environment = 3,
    /// An unexpected internal error.
    Internal = 4,
    /// The verb is declared but not implemented in this build.
    NotImplemented = 5,
}

impl ExitCode {
    #[must_use]
    pub fn as_i32(self) -> i32 {
        self as i32
    }
}

/// Diagnostic severity. Warnings never change the exit code; errors yield [`ExitCode::Findings`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warning,
}

impl fmt::Display for Severity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Error => write!(f, "error"),
            Self::Warning => write!(f, "warning"),
        }
    }
}

/// A stable diagnostic code.
///
/// The number ranges are the contract (DESIGN-cli §6.4):
/// `EC1xxx` schema · `EC2xxx` semantic · `EC3xxx` artifact · `EC4xxx` template ·
/// `EC5xxx` deployment. Codes are stable across releases so that CI can pin behavior
/// to a code rather than to a message string.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct Code(pub &'static str);

impl fmt::Display for Code {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

// --- Schema (EC1xxx) --------------------------------------------------------------
/// The config does not validate against the canonical edgecommons config schema.
pub const EC1001_SCHEMA: Code = Code("EC1001");
/// The config does not validate against the component's own `config.schema.json`.
pub const EC1002_COMPONENT_SCHEMA: Code = Code("EC1002");
/// The component publishes no config schema, so its own config is unvalidated.
pub const EC1003_NO_COMPONENT_SCHEMA: Code = Code("EC1003");

// --- Semantic (EC2xxx) ------------------------------------------------------------
/// `--transport IPC` is valid only on `--platform GREENGRASS`.
pub const EC2001_IPC_REQUIRES_GREENGRASS: Code = Code("EC2001");
/// A supervisord/HOST render requires `--platform HOST`.
pub const EC2002_SUPERVISORD_REQUIRES_HOST: Code = Code("EC2002");
/// A Kubernetes ConfigMap mount must not use `subPath`.
pub const EC2003_CONFIGMAP_SUBPATH: Code = Code("EC2003");
/// A hierarchical config lineage must be acyclic and ordered.
pub const EC2004_LINEAGE_CYCLE: Code = Code("EC2004");
/// Secret values are forbidden; only `secret://` references.
pub const EC2005_SECRET_VALUE: Code = Code("EC2005");
/// A raw publish to a reserved UNS class is rejected.
pub const EC2006_RESERVED_UNS_CLASS: Code = Code("EC2006");
/// A `CONFIG_COMPONENT` bootstrap loop.
pub const EC2007_CONFIG_BOOTSTRAP_LOOP: Code = Code("EC2007");
/// A UNS identity/topic token is invalid.
pub const EC2008_INVALID_UNS_TOKEN: Code = Code("EC2008");
/// The config source is not legal for the platform.
pub const EC2009_CONFIG_SOURCE_PLATFORM: Code = Code("EC2009");

// --- Artifact lint (EC3xxx) -------------------------------------------------------
/// The recipe uses the `{COMPONENT_NAME}` placeholder, which GDK does not substitute.
pub const EC3001_RECIPE_COMPONENT_NAME_PLACEHOLDER: Code = Code("EC3001");
/// An artifact `Permissions:` block is present; `CreateComponentVersion` rejects it.
pub const EC3002_RECIPE_PERMISSIONS_BLOCK: Code = Code("EC3002");
/// Unsubstituted `<<...>>` placeholders remain.
pub const EC3003_UNSUBSTITUTED_TOKEN: Code = Code("EC3003");
/// `RequiresPrivilege: true` runs the component as root.
pub const EC3004_REQUIRES_PRIVILEGE: Code = Code("EC3004");
/// The recipe is not valid YAML.
pub const EC3005_RECIPE_UNPARSABLE: Code = Code("EC3005");
/// `gdk-config.json` is missing or invalid.
pub const EC3006_GDK_CONFIG: Code = Code("EC3006");

// --- Template (EC4xxx) ------------------------------------------------------------
/// The template manifest is invalid.
pub const EC4001_MANIFEST_INVALID: Code = Code("EC4001");
/// The manifest references a file that is not in the template.
pub const EC4002_MANIFEST_MISSING_FILE: Code = Code("EC4002");
/// No template exists for the requested language/kind.
pub const EC4003_NO_SUCH_TEMPLATE: Code = Code("EC4003");

/// Where a diagnostic points: a line/column, or a JSON Pointer into a config document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Locus {
    /// A 1-indexed line, with an optional 1-indexed column.
    Line { line: usize, column: Option<usize> },
    /// An RFC 6901 JSON Pointer, e.g. `/component/global/pipeline`.
    Pointer(String),
}

impl fmt::Display for Locus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Line {
                line,
                column: Some(c),
            } => write!(f, "{line}:{c}"),
            Self::Line { line, column: None } => write!(f, "{line}"),
            Self::Pointer(p) if p.is_empty() => write!(f, "(root)"),
            Self::Pointer(p) => f.write_str(p),
        }
    }
}

/// One finding, from any layer of any verb.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Diagnostic {
    pub code: Code,
    pub severity: Severity,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub locus: Option<Locus>,
    pub message: String,
    /// Actionable guidance. Says what to *do*, not what went wrong.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub help: Option<String>,
}

impl Diagnostic {
    pub fn error(code: Code, message: impl Into<String>) -> Self {
        Self {
            code,
            severity: Severity::Error,
            file: None,
            locus: None,
            message: message.into(),
            help: None,
        }
    }

    pub fn warning(code: Code, message: impl Into<String>) -> Self {
        Self {
            code,
            severity: Severity::Warning,
            file: None,
            locus: None,
            message: message.into(),
            help: None,
        }
    }

    #[must_use]
    pub fn with_file(mut self, file: impl AsRef<Path>) -> Self {
        self.file = Some(file.as_ref().to_path_buf());
        self
    }

    #[must_use]
    pub fn with_line(mut self, line: usize) -> Self {
        self.locus = Some(Locus::Line { line, column: None });
        self
    }

    #[must_use]
    pub fn with_pointer(mut self, pointer: impl Into<String>) -> Self {
        self.locus = Some(Locus::Pointer(pointer.into()));
        self
    }

    #[must_use]
    pub fn with_help(mut self, help: impl Into<String>) -> Self {
        self.help = Some(help.into());
        self
    }

    #[must_use]
    pub fn is_error(&self) -> bool {
        self.severity == Severity::Error
    }
}

/// A collected set of findings, plus the exit code they imply.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub diagnostics: Vec<Diagnostic>,
}

impl Report {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, d: Diagnostic) {
        self.diagnostics.push(d);
    }

    pub fn extend(&mut self, ds: impl IntoIterator<Item = Diagnostic>) {
        self.diagnostics.extend(ds);
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.diagnostics.is_empty()
    }

    #[must_use]
    pub fn error_count(&self) -> usize {
        self.diagnostics.iter().filter(|d| d.is_error()).count()
    }

    #[must_use]
    pub fn warning_count(&self) -> usize {
        self.diagnostics.len() - self.error_count()
    }

    /// Errors mean findings; warnings alone still exit `0` (DESIGN-cli §6.4).
    #[must_use]
    pub fn exit_code(&self) -> ExitCode {
        if self.error_count() > 0 {
            ExitCode::Findings
        } else {
            ExitCode::Ok
        }
    }

    /// Render for a terminal.
    #[must_use]
    pub fn render_human(&self) -> String {
        let mut out = String::new();
        for d in &self.diagnostics {
            let mut head = format!("{}[{}]", d.severity, d.code);
            if let Some(file) = &d.file {
                head.push_str(&format!(" {}", file.display()));
                if let Some(locus) = &d.locus {
                    head.push_str(&format!(":{locus}"));
                }
            } else if let Some(locus) = &d.locus {
                head.push_str(&format!(" at {locus}"));
            }
            out.push_str(&format!("{head}\n  {}\n", d.message));
            if let Some(help) = &d.help {
                out.push_str(&format!("  help: {help}\n"));
            }
        }
        let (e, w) = (self.error_count(), self.warning_count());
        if e == 0 && w == 0 {
            out.push_str("OK — no findings.\n");
        } else {
            out.push_str(&format!("\n{e} error(s), {w} warning(s)\n"));
        }
        out
    }

    /// Render as a stable JSON object for `--json`.
    #[must_use]
    pub fn render_json(&self) -> String {
        serde_json::to_string_pretty(&serde_json::json!({
            "diagnostics": self.diagnostics,
            "errorCount": self.error_count(),
            "warningCount": self.warning_count(),
            "ok": self.error_count() == 0,
        }))
        .unwrap_or_else(|_| "{}".into())
    }
}

/// A fatal error: the verb could not run at all (as opposed to running and finding problems).
#[derive(Debug, thiserror::Error)]
pub enum Fatal {
    #[error("{0}")]
    Usage(String),
    #[error("{0}")]
    Environment(String),
    #[error("{0}")]
    NotImplemented(String),
    #[error("{0}")]
    Internal(String),
}

impl Fatal {
    #[must_use]
    pub fn exit_code(&self) -> ExitCode {
        match self {
            Self::Usage(_) => ExitCode::Usage,
            Self::Environment(_) => ExitCode::Environment,
            Self::NotImplemented(_) => ExitCode::NotImplemented,
            Self::Internal(_) => ExitCode::Internal,
        }
    }
}

impl From<std::io::Error> for Fatal {
    fn from(e: std::io::Error) -> Self {
        Self::Internal(e.to_string())
    }
}

/// The result of a verb: either it ran (yielding a report) or it could not run.
pub type Outcome = Result<Report, Fatal>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn warnings_alone_do_not_fail_the_build() {
        let mut r = Report::new();
        r.push(Diagnostic::warning(
            EC3004_REQUIRES_PRIVILEGE,
            "runs as root",
        ));
        assert_eq!(r.exit_code(), ExitCode::Ok);
        assert_eq!(r.warning_count(), 1);
        assert_eq!(r.error_count(), 0);
    }

    #[test]
    fn errors_yield_findings_exit_code() {
        let mut r = Report::new();
        r.push(Diagnostic::warning(
            EC3004_REQUIRES_PRIVILEGE,
            "runs as root",
        ));
        r.push(Diagnostic::error(EC1001_SCHEMA, "bad config"));
        assert_eq!(r.exit_code(), ExitCode::Findings);
        assert_eq!(r.error_count(), 1);
    }

    #[test]
    fn empty_report_is_ok() {
        let r = Report::new();
        assert_eq!(r.exit_code(), ExitCode::Ok);
        assert!(r.render_human().contains("no findings"));
    }

    #[test]
    fn json_render_is_stable_and_machine_readable() {
        let mut r = Report::new();
        r.push(
            Diagnostic::error(EC1002_COMPONENT_SCHEMA, "`pipeline.window` is not accepted")
                .with_file("config.json")
                .with_pointer("/component/global/pipeline")
                .with_help("remove the key, or deploy >= 0.4.0"),
        );
        let v: serde_json::Value = serde_json::from_str(&r.render_json()).unwrap();
        assert_eq!(v["ok"], false);
        assert_eq!(v["errorCount"], 1);
        assert_eq!(v["diagnostics"][0]["code"], "EC1002");
        assert_eq!(v["diagnostics"][0]["severity"], "error");
        assert_eq!(
            v["diagnostics"][0]["locus"]["pointer"],
            "/component/global/pipeline"
        );
        assert_eq!(
            v["diagnostics"][0]["help"],
            "remove the key, or deploy >= 0.4.0"
        );
    }

    #[test]
    fn human_render_shows_file_and_locus() {
        let mut r = Report::new();
        r.push(
            Diagnostic::error(EC3003_UNSUBSTITUTED_TOKEN, "leftover token")
                .with_file("recipe.yaml")
                .with_line(12),
        );
        let s = r.render_human();
        assert!(s.contains("error[EC3003]"), "{s}");
        assert!(s.contains("recipe.yaml:12"), "{s}");
    }

    #[test]
    fn fatal_maps_to_distinct_exit_codes() {
        assert_eq!(Fatal::Usage(String::new()).exit_code(), ExitCode::Usage);
        assert_eq!(
            Fatal::Environment(String::new()).exit_code(),
            ExitCode::Environment
        );
        assert_eq!(
            Fatal::NotImplemented(String::new()).exit_code(),
            ExitCode::NotImplemented
        );
        assert_eq!(
            Fatal::Internal(String::new()).exit_code(),
            ExitCode::Internal
        );
    }
}
