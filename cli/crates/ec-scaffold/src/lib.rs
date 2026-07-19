//! Component scaffolding: embedded templates, manifest v2, and the generation pipeline.
//!
//! See DESIGN-cli §5. The two properties RM-012 calls non-negotiable are realized in
//! [`catalog`] (templates are compiled into the binary, so scaffolding works offline) and
//! [`generate`] (the conditional/platform-gated generation behavior is preserved as
//! behavior, not redesigned).

pub mod catalog;
pub mod generate;
pub mod licenses;
pub mod manifest;
pub mod upgrade;

pub use catalog::{Template, discover, find, matrix};
pub use generate::{DepSource, Inputs, generate_embedded};
pub use manifest::{Kind, Language, Manifest, Platform};
