//! Verb implementations. Each returns an [`ec_diag::Outcome`]: either it ran (yielding a
//! report of findings) or it could not run (a [`ec_diag::Fatal`]).

pub mod component;
pub mod deployment;
pub mod doctor;
pub mod registry;
pub mod release;

use ec_diag::Fatal;

/// A verb that is declared in the surface but not built in this binary.
///
/// Declared-but-unbuilt is reported honestly with its own exit code
/// ([`ec_diag::ExitCode::NotImplemented`]) rather than as a crash or a usage error, so CI can
/// tell "this build cannot do that yet" from "you invoked it wrong".
///
/// The message names no internal phase, roadmap item, or design document: those are ours, not
/// the user's, and a tool that talks about its own backlog to the person trying to use it is
/// leaking its plumbing.
pub fn not_implemented(verb: &str) -> Fatal {
    Fatal::NotImplemented(format!(
        "`edgecommons {verb}` is not available in this build."
    ))
}
