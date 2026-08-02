//! Verb implementations. Each returns an [`ec_diag::Outcome`]: either it ran (yielding a
//! report of findings) or it could not run (a [`ec_diag::Fatal`]).
//!
//! Every declared `deployment` verb is now built. Should a future verb ship declared-but-unbuilt,
//! report it as [`ec_diag::Fatal::NotImplemented`] — its own exit code — so CI can tell "this build
//! cannot do that yet" from "you invoked it wrong", and word the message without naming an internal
//! phase or roadmap item.

pub mod component;
pub mod deployment;
pub mod doctor;
pub mod draft;
pub mod registry;
pub mod release;
