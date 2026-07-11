//! Verb implementations. Each returns an [`ec_diag::Outcome`]: either it ran (yielding a
//! report of findings) or it could not run (a [`ec_diag::Fatal`]).

pub mod component;
pub mod doctor;

use ec_diag::Fatal;

/// A verb that is declared in the surface but not built in this phase.
///
/// Declared-but-unbuilt is reported honestly with its own exit code
/// ([`ec_diag::ExitCode::NotImplemented`]) rather than as a crash or a usage error, so CI can
/// tell "this build cannot do that yet" from "you invoked it wrong".
pub fn not_implemented(verb: &str, phase: &str, section: &str) -> Fatal {
    Fatal::NotImplemented(format!(
        "`edgecommons {verb}` is not implemented in this build ({phase}). \
         See docs/platform/DESIGN-cli.md {section}."
    ))
}
