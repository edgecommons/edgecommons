//! The command surface (DESIGN-cli §4).
//!
//! Noun–verb throughout. The old flat surface (`create-component` beside `list-components`)
//! is gone, and there are deliberately **no aliases**: the CLI was never published, so the
//! rename is free exactly once, and `validate` vs `deployment validate` would otherwise be
//! a permanent trap.

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};

#[derive(Debug, Parser)]
#[command(
    name = "edgecommons",
    version,
    about = "Scaffold, validate, and deploy EdgeCommons components.",
    propagate_version = true
)]
pub struct Cli {
    /// Emit machine-readable JSON instead of human output.
    #[arg(long, global = true)]
    pub json: bool,

    /// Suppress non-essential output.
    #[arg(short, long, global = true, conflicts_with = "verbose")]
    pub quiet: bool,

    /// Increase verbosity (repeatable).
    #[arg(short, long, global = true, action = clap::ArgAction::Count)]
    pub verbose: u8,

    /// Never emit colored output.
    #[arg(long, global = true)]
    pub no_color: bool,

    /// Never prompt; a missing required input becomes a usage error instead of a question.
    #[arg(long, global = true)]
    pub yes: bool,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Work with a component: scaffold, validate, upgrade, version, package, release.
    #[command(subcommand)]
    Component(ComponentCmd),

    /// Inspect the component templates this CLI can generate.
    #[command(subcommand)]
    Template(TemplateCmd),

    /// Query the EdgeCommons ecosystem registry.
    #[command(subcommand)]
    Registry(RegistryCmd),

    /// Model-to-artifact deployment: validate, lock, render, plan, diff, release.
    #[command(subcommand)]
    Deployment(DeploymentCmd),

    /// The Deployment Studio server — a shell around the same kernel.
    #[command(subcommand)]
    Studio(StudioCmd),

    /// Check the external tools needed for the platforms you target.
    Doctor(DoctorArgs),

    /// Generate a shell completion script.
    Completions {
        #[arg(value_enum)]
        shell: Shell,
    },
}

#[derive(Debug, Subcommand)]
pub enum ComponentCmd {
    /// Scaffold a new component (language × kind × platforms).
    New(NewArgs),
    /// Validate a component's config and artifacts.
    Validate(ValidateArgs),
    /// Move a component to a given *edgecommons library* version.
    Upgrade(UpgradeArgs),
    /// Set the *component's own* version across its manifests.
    Version(VersionArgs),
    /// Build deployable artifacts for the selected platform(s).
    Package(PackageArgs),
    /// Build artifacts, compute digests, and emit a release descriptor.
    ///
    /// This verb never tags, uploads, or publishes: the CLI produces, the runner publishes
    /// (D-CLI-10). A release cut from a laptop holding credentials has no provenance.
    Release(ReleaseArgs),
}

#[derive(Debug, Args)]
pub struct NewArgs {
    /// Fully-qualified component name, e.g. com.example.MyComponent.
    #[arg(short, long)]
    pub name: Option<String>,

    /// Implementation language.
    #[arg(short, long, value_enum)]
    pub language: Option<Language>,

    /// Component archetype.
    #[arg(short, long, value_enum, default_value_t = Kind::Service)]
    pub kind: Kind,

    /// Short description.
    #[arg(short, long)]
    pub description: Option<String>,

    /// Where to create the component.
    #[arg(short, long, default_value = ".")]
    pub path: PathBuf,

    /// Target platforms; controls which artifact packs are emitted.
    #[arg(long, value_delimiter = ',', value_enum)]
    pub platforms: Vec<Platform>,

    /// How the component depends on the edgecommons library.
    #[arg(long, value_enum, default_value_t = DepSource::Local)]
    pub dep_source: DepSource,

    /// Path to a local edgecommons library checkout (for `--dep-source local`).
    #[arg(long)]
    pub library_path: Option<PathBuf>,

    /// Component author.
    #[arg(short, long)]
    pub author: Option<String>,

    /// S3 bucket for Greengrass component artifacts. Only used when the GREENGRASS pack is emitted.
    #[arg(short, long)]
    pub bucket: Option<String>,

    /// AWS region for Greengrass publishing. Only used when the GREENGRASS pack is emitted.
    #[arg(short, long, default_value = "us-east-1")]
    pub region: String,

    /// Overwrite a non-empty target directory.
    #[arg(short, long)]
    pub force: bool,

    /// Use a template from a local directory instead of the embedded one.
    #[arg(long, conflicts_with = "template_git")]
    pub template_dir: Option<PathBuf>,

    /// Clone a template from a git URL. The only network access `component new` ever makes.
    #[arg(long)]
    pub template_git: Option<String>,
}

#[derive(Debug, Args)]
pub struct ValidateArgs {
    /// The component project directory.
    #[arg(short, long, default_value = ".")]
    pub path: PathBuf,

    /// Validate this config file specifically (default: every config the project ships).
    #[arg(short, long)]
    pub config: Option<PathBuf>,

    /// The platform this config is destined for.
    ///
    /// Some rules are only decidable with it — a transport or config source that is illegal on
    /// one platform is perfectly legal on another — so they are skipped when it is absent
    /// rather than guessed at.
    #[arg(long, value_enum)]
    pub platform: Option<Platform>,
}

#[derive(Debug, Args)]
pub struct UpgradeArgs {
    #[arg(short, long, default_value = ".")]
    pub path: PathBuf,

    /// Target edgecommons library version.
    #[arg(short, long)]
    pub to: String,

    /// Show what would change without writing.
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(Debug, Args)]
pub struct VersionArgs {
    #[arg(short, long, default_value = ".")]
    pub path: PathBuf,

    /// The component's new version.
    #[arg(short, long)]
    pub to: String,

    #[arg(long)]
    pub dry_run: bool,
}

#[derive(Debug, Args)]
pub struct PackageArgs {
    #[arg(short, long, default_value = ".")]
    pub path: PathBuf,

    /// Platforms to build artifacts for.
    #[arg(long, value_delimiter = ',', value_enum)]
    pub platforms: Vec<Platform>,

    /// Publish the built artifact (Greengrass: `gdk component publish`).
    #[arg(long)]
    pub publish: bool,
}

#[derive(Debug, Args)]
pub struct ReleaseArgs {
    #[arg(short, long, default_value = ".")]
    pub path: PathBuf,

    /// Where to write the release descriptor.
    #[arg(short, long, default_value = "release.json")]
    pub out: PathBuf,
}

#[derive(Debug, Subcommand)]
pub enum TemplateCmd {
    /// List the language × kind matrix.
    List,
    /// Show one template's manifest: platforms, tokens, and emitted files.
    Show {
        /// Template id, e.g. `rust/service`.
        id: String,
    },
}

#[derive(Debug, Subcommand)]
pub enum RegistryCmd {
    /// List the components in the ecosystem catalog.
    List(RegistryListArgs),
    /// Show one catalog entry.
    Show {
        name: String,
        /// Registry URL or a local components.json path.
        #[arg(long, env = "EDGECOMMONS_REGISTRY_URL")]
        source: Option<String>,
    },
    /// List the published releases of a component.
    Versions {
        name: String,
        /// Registry URL or a local components.json path.
        #[arg(long, env = "EDGECOMMONS_REGISTRY_URL")]
        source: Option<String>,
    },
}

#[derive(Debug, Args)]
pub struct RegistryListArgs {
    /// Registry URL or a local components.json path.
    #[arg(long, env = "EDGECOMMONS_REGISTRY_URL")]
    pub source: Option<String>,

    #[arg(long, value_enum)]
    pub language: Option<Language>,

    /// Filter by catalog category.
    #[arg(long, value_enum)]
    pub category: Option<Category>,
}

#[derive(Debug, Subcommand)]
pub enum DeploymentCmd {
    /// Validate the deployment model and every rendered effective config.
    Validate { definition: PathBuf },
    /// Resolve pinned versions to digests. The only verb that touches the network.
    Lock { definition: PathBuf },
    /// Render the model to native artifacts plus the normalized plan.
    Render {
        definition: PathBuf,
        #[arg(long)]
        env: String,
        #[arg(long, value_enum)]
        target: Platform,
    },
    /// Emit the normalized plan JSON.
    Plan {
        definition: PathBuf,
        #[arg(long)]
        env: String,
        #[arg(long, value_enum)]
        target: Platform,
    },
    /// Diff this render against a release ref, grouped by consequence.
    Diff {
        definition: PathBuf,
        #[arg(long)]
        against: String,
    },
    /// Promote one release stream.
    Release {
        definition: PathBuf,
        /// Which stream to promote. The two are independently reconciled (REVIEW #2).
        #[arg(long, value_enum)]
        stream: Stream,
    },
}

#[derive(Debug, Subcommand)]
pub enum StudioCmd {
    /// Serve the Studio UI over the same kernel the CLI uses.
    Serve {
        #[arg(long, default_value = ".")]
        repo: String,
        #[arg(long, default_value = "127.0.0.1:8787")]
        bind: String,
    },
}

#[derive(Debug, Args)]
pub struct DoctorArgs {
    /// Only check what these platforms need. Defaults to all.
    #[arg(long, value_delimiter = ',', value_enum)]
    pub platforms: Vec<Platform>,

    /// Only check what this language needs. Defaults to all.
    #[arg(short, long, value_enum)]
    pub language: Option<Language>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
#[value(rename_all = "UPPER")]
pub enum Language {
    Java,
    Python,
    Rust,
    Typescript,
}

/// The component archetype (D-CLI-4). Mirrors the registry's own category vocabulary,
/// so a scaffolded component and its catalog entry speak the same word.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
#[value(rename_all = "kebab-case")]
pub enum Kind {
    Service,
    ProtocolAdapter,
    Processor,
    Sink,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, ValueEnum)]
#[value(rename_all = "UPPER")]
pub enum Platform {
    Greengrass,
    Host,
    Kubernetes,
}

impl Platform {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Greengrass => "GREENGRASS",
            Self::Host => "HOST",
            Self::Kubernetes => "KUBERNETES",
        }
    }

    #[must_use]
    pub fn all() -> Vec<Self> {
        vec![Self::Greengrass, Self::Host, Self::Kubernetes]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
#[value(rename_all = "lowercase")]
pub enum DepSource {
    Local,
    Registry,
}

/// The registry's full category enum. The Python CLI advertised three of these six.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
#[value(rename_all = "lowercase")]
pub enum Category {
    Adapter,
    Processor,
    Sink,
    Bridge,
    Console,
    Service,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
#[value(rename_all = "lowercase")]
pub enum Stream {
    Artifact,
    Config,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
#[value(rename_all = "lowercase")]
pub enum Shell {
    Bash,
    Zsh,
    Fish,
    Powershell,
    Elvish,
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn the_command_surface_is_internally_consistent() {
        Cli::command().debug_assert();
    }

    #[test]
    fn noun_verb_parses() {
        let cli = Cli::try_parse_from([
            "edgecommons",
            "component",
            "new",
            "-n",
            "com.example.Foo",
            "-l",
            "RUST",
        ])
        .unwrap();
        match cli.command {
            Command::Component(ComponentCmd::New(a)) => {
                assert_eq!(a.name.as_deref(), Some("com.example.Foo"));
                assert_eq!(a.language, Some(Language::Rust));
                // The default archetype is the plain baseline.
                assert_eq!(a.kind, Kind::Service);
            }
            other => panic!("wrong command: {other:?}"),
        }
    }

    #[test]
    fn the_archetype_axis_is_reachable() {
        let cli = Cli::try_parse_from([
            "edgecommons",
            "component",
            "new",
            "-n",
            "com.example.Modbus",
            "-l",
            "PYTHON",
            "-k",
            "protocol-adapter",
        ])
        .unwrap();
        match cli.command {
            Command::Component(ComponentCmd::New(a)) => assert_eq!(a.kind, Kind::ProtocolAdapter),
            other => panic!("wrong command: {other:?}"),
        }
    }

    #[test]
    fn the_old_flat_verbs_are_gone() {
        // A clean break: no aliases. `create-component` must not resolve.
        assert!(Cli::try_parse_from(["edgecommons", "create-component", "-n", "x"]).is_err());
        assert!(Cli::try_parse_from(["edgecommons", "list-components"]).is_err());
        assert!(Cli::try_parse_from(["edgecommons", "list-templates"]).is_err());
    }

    #[test]
    fn registry_exposes_all_six_categories() {
        for c in [
            "adapter",
            "processor",
            "sink",
            "bridge",
            "console",
            "service",
        ] {
            assert!(
                Cli::try_parse_from(["edgecommons", "registry", "list", "--category", c]).is_ok(),
                "category {c} should be accepted"
            );
        }
    }

    #[test]
    fn deployment_release_takes_a_stream_not_an_atomic_apply() {
        let cli = Cli::try_parse_from([
            "edgecommons",
            "deployment",
            "release",
            "def.yaml",
            "--stream",
            "config",
        ])
        .unwrap();
        match cli.command {
            Command::Deployment(DeploymentCmd::Release { stream, .. }) => {
                assert_eq!(stream, Stream::Config);
            }
            other => panic!("wrong command: {other:?}"),
        }
    }

    #[test]
    fn json_is_global() {
        let cli = Cli::try_parse_from(["edgecommons", "--json", "template", "list"]).unwrap();
        assert!(cli.json);
    }
}
