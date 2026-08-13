use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};

use crate::config::AppConfig;
use crate::diagnostics::DiagnosticCategory;

#[derive(Debug, Clone, Parser)]
#[command(author, version, about = "One-shot DIGIweb PLU importer")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<CliCommand>,
}

#[derive(Debug, Clone, Subcommand)]
pub enum CliCommand {
    /// Analyze plu.mdb without contacting DIGIweb
    Analyze(AnalyzeArgs),
    /// Record operator-confirmed DIGIweb references in config.toml
    Confirm(ConfirmArgs),
    /// Discover raw MDB structure and data quality without profile or network access
    Discover(DiscoverArgs),
    /// Diagnose exact invalid/skipped PLUs without contacting DIGIweb
    Diagnose(DiagnoseArgs),
    /// Check deployment readiness without importing PLUs
    Doctor(DoctorArgs),
    /// Build payloads and manifests without any PLU API writes
    DryRun(DryRunArgs),
    /// Initialize a deployment directory with launchers and templates
    Init(InitArgs),
    /// Import valid PLUs into DIGIweb
    Import(ImportArgs),
    /// Pull the configured Docker image when using generated launchers
    Pull,
    /// Profile utility commands
    Profile(ProfileArgs),
    /// Resume a previous import manifest
    Resume(ResumeArgs),
    /// Audit source-to-DIGIweb field mapping without submitting PLUs
    MapAudit(MapAuditArgs),
    /// Preview profile-driven sanitization without contacting DIGIweb
    Sanitize(SanitizeArgs),
    /// Test DIGIweb authentication and connectivity
    TestConnection,
    /// Verify import readiness without writing PLUs
    Verify(VerifyArgs),
    /// Print application version
    Version,
}

#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct AnalyzeArgs {
    /// Apply a fill-only sanitization profile before analysis
    #[arg(long, value_name = "PROFILE", conflicts_with_all = ["profile", "raw"])]
    pub sanitize_profile: Option<PathBuf>,
    /// Apply a built-in profile such as starsky
    #[arg(long, value_name = "NAME", conflicts_with_all = ["sanitize_profile", "raw"])]
    pub profile: Option<String>,
    /// Analyze the raw source without any default deployment profile
    #[arg(long, conflicts_with_all = ["sanitize_profile", "profile"])]
    pub raw: bool,
}

#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct DiscoverArgs {
    /// Include detailed phase timing in the report
    #[arg(long)]
    pub timings: bool,
}

#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct DiagnoseArgs {
    /// Show only records that would not be imported cleanly
    #[arg(long)]
    pub invalid_only: bool,
    /// Diagnose one PLU number
    #[arg(long)]
    pub plu: Option<u64>,
    /// Limit output to a diagnostic category
    #[arg(long, value_parser = parse_diagnostic_category)]
    pub category: Option<DiagnosticCategory>,
    /// Apply a fill-only sanitization profile before diagnostics
    #[arg(long, value_name = "PROFILE", conflicts_with = "profile")]
    pub sanitize_profile: Option<PathBuf>,
    /// Apply a built-in profile such as bigway before diagnostics
    #[arg(long, value_name = "NAME", conflicts_with = "sanitize_profile")]
    pub profile: Option<String>,
}

#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct ConfirmArgs {
    /// Reference kind to confirm from the effective source/profile combination
    #[arg(value_enum)]
    pub target: ConfirmTarget,
    /// Apply a fill-only sanitization profile before collecting references
    #[arg(long, value_name = "PROFILE", conflicts_with = "profile")]
    pub sanitize_profile: Option<PathBuf>,
    /// Apply a built-in profile such as bigway before collecting references
    #[arg(long, value_name = "NAME", conflicts_with = "sanitize_profile")]
    pub profile: Option<String>,
    /// Preview the config changes without writing config.toml
    #[arg(long)]
    pub dry_run: bool,
    /// Confirm non-interactively; required for unattended writes
    #[arg(long)]
    pub yes: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ConfirmTarget {
    Departments,
    Groups,
    LabelFormats,
    All,
}

#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct DryRunArgs {
    /// Select only the first N valid normalized PLUs
    #[arg(long, value_parser = parse_positive_usize, conflicts_with_all = ["test", "plu"])]
    pub limit: Option<usize>,
    /// Convenience alias for --limit 1
    #[arg(long, conflicts_with = "plu")]
    pub test: bool,
    /// Select exactly one PLU number after normalization and validation
    #[arg(long, value_parser = parse_positive_u64, conflicts_with_all = ["limit", "test"])]
    pub plu: Option<u64>,
    /// Apply a fill-only sanitization profile before dry-run payload building
    #[arg(long, value_name = "PROFILE", conflicts_with = "profile")]
    pub sanitize_profile: Option<PathBuf>,
    /// Apply a built-in profile such as starsky before dry-run payload building
    #[arg(long, value_name = "NAME", conflicts_with = "sanitize_profile")]
    pub profile: Option<String>,
}

#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct MapAuditArgs {
    /// Number of payload samples to include
    #[arg(long, default_value_t = crate::mapping_audit::default_sample_limit())]
    pub sample: usize,
    /// Audit one PLU number
    #[arg(long)]
    pub plu: Option<u64>,
    /// Include detailed phase timing in the report
    #[arg(long)]
    pub timings: bool,
}

#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct DoctorArgs {
    /// Pull the selected image while checking launcher readiness
    #[arg(long)]
    pub pull: bool,
    /// Internal marker used by generated launchers after host-side Docker checks
    #[arg(long, hide = true)]
    pub inside_container: bool,
    /// Apply a built-in profile such as starsky during readiness checks
    #[arg(long, value_name = "NAME", conflicts_with = "sanitize_profile")]
    pub profile: Option<String>,
    /// Apply an external sanitization profile during readiness checks
    #[arg(long, value_name = "PROFILE")]
    pub sanitize_profile: Option<PathBuf>,
}

#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct InitArgs {
    /// Refresh known generated files, preserving customer files and creating backups
    #[arg(long)]
    pub refresh_generated_files: bool,
}

#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct ImportArgs {
    /// Import only the first N valid normalized PLUs
    #[arg(long, value_parser = parse_positive_usize, conflicts_with_all = ["test", "resume", "plu"])]
    pub limit: Option<usize>,
    /// Convenience alias for --limit 1
    #[arg(long, conflicts_with_all = ["resume", "plu"])]
    pub test: bool,
    /// Import exactly one PLU number after normalization, validation, and readiness checks
    #[arg(long, value_parser = parse_positive_u64, conflicts_with_all = ["limit", "test", "resume"])]
    pub plu: Option<u64>,
    /// Alias for the offline dry-run command; no PLU writes are performed
    #[arg(long, conflicts_with = "resume")]
    pub dry_run: bool,
    /// Continue submitting later selected PLUs after a failure or unknown final status
    #[arg(long)]
    pub continue_on_error: bool,
    /// Resume a previous import manifest instead of selecting PLUs from CLI flags
    #[arg(long, value_name = "MANIFEST")]
    pub resume: Option<PathBuf>,
    /// Retry only confirmed FAILED records during --resume
    #[arg(long, requires = "resume")]
    pub retry_failed: bool,
    /// Apply a fill-only sanitization profile before import
    #[arg(long, value_name = "PROFILE", conflicts_with_all = ["resume", "profile"])]
    pub sanitize_profile: Option<PathBuf>,
    /// Apply a built-in profile such as starsky before import
    #[arg(long, value_name = "NAME", conflicts_with_all = ["resume", "sanitize_profile"])]
    pub profile: Option<String>,
}

#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct ProfileArgs {
    #[command(subcommand)]
    pub command: ProfileCommand,
}

#[derive(Debug, Clone, Subcommand, PartialEq, Eq)]
pub enum ProfileCommand {
    /// Suggest a draft profile from deterministic source findings
    Suggest(ProfileSuggestArgs),
}

#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct ProfileSuggestArgs {
    /// Draft profile name. The file is written as profiles/<name>.draft.toml.
    #[arg(long)]
    pub name: String,
}

#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct ResumeArgs {
    /// Manifest produced by a previous import
    pub manifest: PathBuf,
    /// Retry only confirmed FAILED records
    #[arg(long)]
    pub retry_failed: bool,
    /// Continue submitting later selected PLUs after a failure or unknown final status
    #[arg(long)]
    pub continue_on_error: bool,
}

#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct SanitizeArgs {
    /// Built-in profile name such as starsky, or a profile path for compatibility
    #[arg(long, value_name = "PROFILE")]
    pub profile: Option<String>,
    /// External TOML sanitization profile to preview
    #[arg(long, value_name = "PROFILE", conflicts_with = "profile")]
    pub sanitize_profile: Option<PathBuf>,
    /// Explicit alias; sanitize is always a dry run
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct VerifyArgs {
    /// Apply a fill-only sanitization profile before readiness verification
    #[arg(long, value_name = "PROFILE", conflicts_with = "profile")]
    pub sanitize_profile: Option<PathBuf>,
    /// Apply a built-in profile such as starsky before readiness verification
    #[arg(long, value_name = "NAME", conflicts_with = "sanitize_profile")]
    pub profile: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProfileSelection {
    External(PathBuf),
    BuiltIn(String),
}

impl ProfileSelection {
    pub fn from_cli_profile(value: &str) -> Self {
        if matches!(value, "bigway" | "starsky") {
            Self::BuiltIn(value.to_string())
        } else {
            Self::External(PathBuf::from(value))
        }
    }

    pub fn display(&self) -> String {
        match self {
            Self::External(path) => path.display().to_string(),
            Self::BuiltIn(name) => format!("built-in:{name}"),
        }
    }
}

fn parse_positive_usize(value: &str) -> Result<usize, String> {
    let parsed = value
        .parse::<usize>()
        .map_err(|err| format!("invalid positive integer: {err}"))?;
    if parsed == 0 {
        Err("--limit must be greater than zero".to_string())
    } else {
        Ok(parsed)
    }
}

fn parse_positive_u64(value: &str) -> Result<u64, String> {
    let parsed = value
        .parse::<u64>()
        .map_err(|err| format!("invalid positive integer: {err}"))?;
    if parsed == 0 {
        Err("--plu must be greater than zero".to_string())
    } else {
        Ok(parsed)
    }
}

fn parse_diagnostic_category(value: &str) -> Result<DiagnosticCategory, String> {
    DiagnosticCategory::parse(value)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EffectiveCommand {
    Analyze {
        legacy_used: bool,
        sanitize_profile: Option<ProfileSelection>,
        raw: bool,
    },
    Discover {
        timings: bool,
    },
    Diagnose {
        invalid_only: bool,
        plu: Option<u64>,
        category: Option<DiagnosticCategory>,
        sanitize_profile: Option<ProfileSelection>,
    },
    Confirm {
        target: ConfirmTarget,
        sanitize_profile: Option<ProfileSelection>,
        dry_run: bool,
        yes: bool,
    },
    Doctor {
        pull: bool,
        inside_container: bool,
        sanitize_profile: Option<ProfileSelection>,
    },
    Init {
        refresh_generated_files: bool,
    },
    Import {
        limit: Option<usize>,
        plu: Option<u64>,
        continue_on_error: bool,
        test_mode: bool,
        resume: Option<PathBuf>,
        retry_failed: bool,
        legacy_used: bool,
        defaulted_from_no_command: bool,
        sanitize_profile: Option<ProfileSelection>,
    },
    DryRun {
        limit: Option<usize>,
        plu: Option<u64>,
        test_mode: bool,
        sanitize_profile: Option<ProfileSelection>,
    },
    Pull,
    ProfileSuggest {
        name: String,
    },
    MapAudit {
        sample: usize,
        plu: Option<u64>,
        timings: bool,
    },
    Sanitize {
        profile: ProfileSelection,
        dry_run: bool,
    },
    TestConnection,
    Verify {
        sanitize_profile: Option<ProfileSelection>,
    },
    Version,
}

impl EffectiveCommand {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Analyze { .. } => "analyze",
            Self::Confirm { .. } => "confirm",
            Self::Discover { .. } => "discover",
            Self::Diagnose { .. } => "diagnose",
            Self::Doctor { .. } => "doctor",
            Self::DryRun { .. } => "dry-run",
            Self::Init { .. } => "init",
            Self::Import { .. } => "import",
            Self::Pull => "pull",
            Self::ProfileSuggest { .. } => "profile suggest",
            Self::MapAudit { .. } => "map-audit",
            Self::Sanitize { .. } => "sanitize",
            Self::TestConnection => "test-connection",
            Self::Verify { .. } => "verify",
            Self::Version => "version",
        }
    }

    pub fn uses_legacy_config(&self) -> bool {
        match self {
            Self::Analyze { legacy_used, .. } => *legacy_used,
            Self::Import { legacy_used, .. } => *legacy_used,
            Self::Confirm { .. }
            | Self::Discover { .. }
            | Self::Diagnose { .. }
            | Self::Doctor { .. }
            | Self::DryRun { .. }
            | Self::Init { .. }
            | Self::MapAudit { .. }
            | Self::ProfileSuggest { .. }
            | Self::Pull
            | Self::Sanitize { .. }
            | Self::TestConnection
            | Self::Verify { .. }
            | Self::Version => false,
        }
    }
}

pub fn effective_command(cli: &Cli, config: &AppConfig) -> EffectiveCommand {
    match &cli.command {
        Some(CliCommand::Analyze(args)) => EffectiveCommand::Analyze {
            legacy_used: false,
            sanitize_profile: resolve_optional_profile(
                args.sanitize_profile.clone(),
                args.profile.clone(),
                args.raw,
                config,
            ),
            raw: args.raw,
        },
        Some(CliCommand::Discover(args)) => EffectiveCommand::Discover {
            timings: args.timings,
        },
        Some(CliCommand::Diagnose(args)) => EffectiveCommand::Diagnose {
            invalid_only: args.invalid_only,
            plu: args.plu,
            category: args.category,
            sanitize_profile: resolve_optional_profile(
                args.sanitize_profile.clone(),
                args.profile.clone(),
                false,
                config,
            ),
        },
        Some(CliCommand::Confirm(args)) => EffectiveCommand::Confirm {
            target: args.target,
            sanitize_profile: resolve_optional_profile(
                args.sanitize_profile.clone(),
                args.profile.clone(),
                false,
                config,
            ),
            dry_run: args.dry_run,
            yes: args.yes,
        },
        Some(CliCommand::Doctor(args)) => EffectiveCommand::Doctor {
            pull: args.pull,
            inside_container: args.inside_container,
            sanitize_profile: resolve_optional_profile(
                args.sanitize_profile.clone(),
                args.profile.clone(),
                false,
                config,
            ),
        },
        Some(CliCommand::Init(args)) => EffectiveCommand::Init {
            refresh_generated_files: args.refresh_generated_files,
        },
        Some(CliCommand::Import(args)) if args.dry_run => EffectiveCommand::DryRun {
            limit: if args.test { Some(1) } else { args.limit },
            plu: args.plu,
            test_mode: args.test,
            sanitize_profile: resolve_explicit_profile(
                args.sanitize_profile.clone(),
                args.profile.clone(),
            ),
        },
        Some(CliCommand::Import(args)) => EffectiveCommand::Import {
            limit: if args.test { Some(1) } else { args.limit },
            plu: args.plu,
            continue_on_error: args.continue_on_error,
            test_mode: args.test,
            resume: args.resume.clone(),
            retry_failed: args.retry_failed,
            legacy_used: false,
            defaulted_from_no_command: false,
            sanitize_profile: resolve_optional_profile(
                args.sanitize_profile.clone(),
                args.profile.clone(),
                false,
                config,
            ),
        },
        Some(CliCommand::DryRun(args)) => EffectiveCommand::DryRun {
            limit: if args.test { Some(1) } else { args.limit },
            plu: args.plu,
            test_mode: args.test,
            sanitize_profile: resolve_explicit_profile(
                args.sanitize_profile.clone(),
                args.profile.clone(),
            ),
        },
        Some(CliCommand::Pull) => EffectiveCommand::Pull,
        Some(CliCommand::Profile(args)) => match &args.command {
            ProfileCommand::Suggest(suggest) => EffectiveCommand::ProfileSuggest {
                name: suggest.name.clone(),
            },
        },
        Some(CliCommand::MapAudit(args)) => EffectiveCommand::MapAudit {
            sample: args.sample,
            plu: args.plu,
            timings: args.timings,
        },
        Some(CliCommand::Resume(args)) => EffectiveCommand::Import {
            limit: None,
            plu: None,
            continue_on_error: args.continue_on_error,
            test_mode: false,
            resume: Some(args.manifest.clone()),
            retry_failed: args.retry_failed,
            legacy_used: false,
            defaulted_from_no_command: false,
            sanitize_profile: None,
        },
        Some(CliCommand::Sanitize(args)) => EffectiveCommand::Sanitize {
            profile: resolve_required_profile(
                args.sanitize_profile.clone(),
                args.profile.clone(),
                config,
            ),
            dry_run: args.dry_run,
        },
        Some(CliCommand::TestConnection) => EffectiveCommand::TestConnection,
        Some(CliCommand::Verify(args)) => EffectiveCommand::Verify {
            sanitize_profile: resolve_optional_profile(
                args.sanitize_profile.clone(),
                args.profile.clone(),
                false,
                config,
            ),
        },
        Some(CliCommand::Version) => EffectiveCommand::Version,
        None => legacy_effective_command(config),
    }
}

fn resolve_optional_profile(
    external: Option<PathBuf>,
    builtin: Option<String>,
    raw: bool,
    config: &AppConfig,
) -> Option<ProfileSelection> {
    if raw {
        return None;
    }
    external
        .map(ProfileSelection::External)
        .or_else(|| builtin.as_deref().map(ProfileSelection::from_cli_profile))
        .or_else(|| default_profile(config))
}

fn resolve_required_profile(
    external: Option<PathBuf>,
    builtin_or_path: Option<String>,
    config: &AppConfig,
) -> ProfileSelection {
    external
        .map(ProfileSelection::External)
        .or_else(|| {
            builtin_or_path
                .as_deref()
                .map(ProfileSelection::from_cli_profile)
        })
        .or_else(|| default_profile(config))
        .unwrap_or_else(|| ProfileSelection::BuiltIn("starsky".to_string()))
}

fn resolve_explicit_profile(
    external: Option<PathBuf>,
    builtin_or_path: Option<String>,
) -> Option<ProfileSelection> {
    external.map(ProfileSelection::External).or_else(|| {
        builtin_or_path
            .as_deref()
            .map(ProfileSelection::from_cli_profile)
    })
}

fn default_profile(config: &AppConfig) -> Option<ProfileSelection> {
    let default = config.profiles.default.trim();
    if default.is_empty() || default.eq_ignore_ascii_case("none") {
        None
    } else {
        Some(ProfileSelection::from_cli_profile(default))
    }
}

fn legacy_effective_command(config: &AppConfig) -> EffectiveCommand {
    if config.import.dry_run_inspect_only {
        EffectiveCommand::Analyze {
            legacy_used: true,
            sanitize_profile: None,
            raw: true,
        }
    } else {
        EffectiveCommand::Import {
            limit: if config.import.send_only_first_plu {
                Some(1)
            } else {
                None
            },
            plu: None,
            continue_on_error: config.import.continue_after_record_failure,
            test_mode: false,
            resume: None,
            retry_failed: false,
            legacy_used: true,
            defaulted_from_no_command: true,
            sanitize_profile: default_profile(config),
        }
    }
}

#[cfg(test)]
mod tests {
    use clap::{CommandFactory, Parser};

    use super::*;

    fn parse(args: &[&str]) -> Cli {
        Cli::try_parse_from(args).expect("parse")
    }

    #[test]
    fn no_command_defaults_to_import_effectively() {
        let config = AppConfig::default();
        let command = effective_command(&parse(&["to-digi-rs"]), &config);

        assert_eq!(
            command,
            EffectiveCommand::Import {
                limit: None,
                plu: None,
                continue_on_error: false,
                test_mode: false,
                resume: None,
                retry_failed: false,
                legacy_used: true,
                defaulted_from_no_command: true,
                sanitize_profile: None,
            }
        );
    }

    #[test]
    fn commands_parse() {
        assert!(matches!(
            parse(&["to-digi-rs", "analyze"]).command,
            Some(CliCommand::Analyze(_))
        ));
        assert!(matches!(
            parse(&["to-digi-rs", "confirm", "all"]).command,
            Some(CliCommand::Confirm(_))
        ));
        assert!(matches!(
            parse(&["to-digi-rs", "discover"]).command,
            Some(CliCommand::Discover(_))
        ));
        assert!(matches!(
            parse(&["to-digi-rs", "diagnose", "--invalid-only"]).command,
            Some(CliCommand::Diagnose(_))
        ));
        assert!(matches!(
            parse(&["to-digi-rs", "doctor"]).command,
            Some(CliCommand::Doctor(_))
        ));
        assert!(matches!(
            parse(&["to-digi-rs", "dry-run"]).command,
            Some(CliCommand::DryRun(_))
        ));
        assert!(matches!(
            parse(&["to-digi-rs", "init"]).command,
            Some(CliCommand::Init(_))
        ));
        assert!(matches!(
            parse(&["to-digi-rs", "import"]).command,
            Some(CliCommand::Import(_))
        ));
        assert!(matches!(
            parse(&["to-digi-rs", "pull"]).command,
            Some(CliCommand::Pull)
        ));
        assert!(matches!(
            parse(&["to-digi-rs", "map-audit"]).command,
            Some(CliCommand::MapAudit(_))
        ));
        assert!(matches!(
            parse(&["to-digi-rs", "profile", "suggest", "--name", "bigway"]).command,
            Some(CliCommand::Profile(_))
        ));
        assert!(matches!(
            parse(&["to-digi-rs", "resume", "output/run/import-results.json"]).command,
            Some(CliCommand::Resume(_))
        ));
        assert!(matches!(
            parse(&["to-digi-rs", "sanitize", "--profile", "starsky"]).command,
            Some(CliCommand::Sanitize(_))
        ));
        assert!(matches!(
            parse(&["to-digi-rs", "test-connection"]).command,
            Some(CliCommand::TestConnection)
        ));
        assert!(matches!(
            parse(&["to-digi-rs", "verify"]).command,
            Some(CliCommand::Verify(_))
        ));
        assert!(matches!(
            parse(&["to-digi-rs", "version"]).command,
            Some(CliCommand::Version)
        ));
    }

    #[test]
    fn import_limit_parses_and_zero_fails() {
        let Cli {
            command: Some(CliCommand::Import(args)),
        } = parse(&["to-digi-rs", "import", "--limit", "2"])
        else {
            panic!("expected import");
        };
        assert_eq!(args.limit, Some(2));

        assert!(Cli::try_parse_from(["to-digi-rs", "import", "--limit", "0"]).is_err());
    }

    #[test]
    fn test_alias_maps_to_limit_one_and_conflicts_with_limit() {
        let config = AppConfig::default();
        let cli = parse(&["to-digi-rs", "import", "--test"]);
        assert_eq!(
            effective_command(&cli, &config),
            EffectiveCommand::Import {
                limit: Some(1),
                plu: None,
                continue_on_error: false,
                test_mode: true,
                resume: None,
                retry_failed: false,
                legacy_used: false,
                defaulted_from_no_command: false,
                sanitize_profile: None,
            }
        );
        assert!(Cli::try_parse_from(["to-digi-rs", "import", "--test", "--limit", "1"]).is_err());
        assert!(Cli::try_parse_from(["to-digi-rs", "import", "--test", "--plu", "721"]).is_err());
        assert!(Cli::try_parse_from(["to-digi-rs", "dry-run", "--test", "--plu", "721"]).is_err());
        assert!(
            Cli::try_parse_from(["to-digi-rs", "dry-run", "--limit", "1", "--plu", "721"]).is_err()
        );
    }

    #[test]
    fn plu_selector_parses_and_conflicts_with_other_selectors() {
        let config = AppConfig::default();
        let cli = parse(&["to-digi-rs", "import", "--plu", "721"]);
        assert_eq!(
            effective_command(&cli, &config),
            EffectiveCommand::Import {
                limit: None,
                plu: Some(721),
                continue_on_error: false,
                test_mode: false,
                resume: None,
                retry_failed: false,
                legacy_used: false,
                defaulted_from_no_command: false,
                sanitize_profile: None,
            }
        );
        let cli = parse(&["to-digi-rs", "dry-run", "--plu", "721"]);
        assert_eq!(
            effective_command(&cli, &config),
            EffectiveCommand::DryRun {
                limit: None,
                plu: Some(721),
                test_mode: false,
                sanitize_profile: None,
            }
        );
        assert!(Cli::try_parse_from(["to-digi-rs", "import", "--plu", "0"]).is_err());
        assert!(
            Cli::try_parse_from(["to-digi-rs", "import", "--plu", "721", "--limit", "1"]).is_err()
        );
        assert!(Cli::try_parse_from(["to-digi-rs", "import", "--plu", "721", "--test"]).is_err());
        assert!(
            Cli::try_parse_from([
                "to-digi-rs",
                "import",
                "--resume",
                "import-results.json",
                "--plu",
                "721"
            ])
            .is_err()
        );
    }

    #[test]
    fn resume_parses_and_conflicts_with_selection_flags() {
        let config = AppConfig::default();
        let cli = parse(&["to-digi-rs", "import", "--resume", "import-results.json"]);

        assert_eq!(
            effective_command(&cli, &config),
            EffectiveCommand::Import {
                limit: None,
                plu: None,
                continue_on_error: false,
                test_mode: false,
                resume: Some(PathBuf::from("import-results.json")),
                retry_failed: false,
                legacy_used: false,
                defaulted_from_no_command: false,
                sanitize_profile: None,
            }
        );
        assert!(
            Cli::try_parse_from([
                "to-digi-rs",
                "import",
                "--resume",
                "import-results.json",
                "--limit",
                "1"
            ])
            .is_err()
        );
        assert!(
            Cli::try_parse_from([
                "to-digi-rs",
                "import",
                "--resume",
                "import-results.json",
                "--test"
            ])
            .is_err()
        );
        assert!(
            Cli::try_parse_from([
                "to-digi-rs",
                "import",
                "--resume",
                "import-results.json",
                "--sanitize-profile",
                "profiles/starsky.toml"
            ])
            .is_err()
        );
    }

    #[test]
    fn retry_failed_requires_resume_and_continue_on_error_is_allowed() {
        assert!(Cli::try_parse_from(["to-digi-rs", "import", "--retry-failed"]).is_err());

        let Cli {
            command: Some(CliCommand::Import(args)),
        } = parse(&[
            "to-digi-rs",
            "import",
            "--resume",
            "import-results.json",
            "--retry-failed",
            "--continue-on-error",
        ])
        else {
            panic!("expected import");
        };

        assert_eq!(args.resume, Some(PathBuf::from("import-results.json")));
        assert!(args.retry_failed);
        assert!(args.continue_on_error);
    }

    #[test]
    fn continue_on_error_is_honored() {
        let config = AppConfig::default();
        let cli = parse(&["to-digi-rs", "import", "--continue-on-error"]);

        assert!(matches!(
            effective_command(&cli, &config),
            EffectiveCommand::Import {
                continue_on_error: true,
                ..
            }
        ));
    }

    #[test]
    fn unknown_command_fails_cleanly() {
        assert!(Cli::try_parse_from(["to-digi-rs", "wat"]).is_err());
    }

    #[test]
    fn help_includes_all_commands_and_version_is_current() {
        let help = Cli::command().render_long_help().to_string();
        assert!(help.contains("analyze"));
        assert!(help.contains("confirm"));
        assert!(help.contains("discover"));
        assert!(help.contains("diagnose"));
        assert!(help.contains("doctor"));
        assert!(help.contains("dry-run"));
        assert!(help.contains("init"));
        assert!(help.contains("import"));
        assert!(help.contains("pull"));
        assert!(help.contains("map-audit"));
        assert!(help.contains("profile"));
        assert!(help.contains("resume"));
        assert!(help.contains("sanitize"));
        assert!(help.contains("test-connection"));
        assert!(help.contains("verify"));
        assert!(help.contains("version"));
        let mut command = CliCommand::augment_subcommands(clap::Command::new("to-digi-rs"));
        let import_help = command
            .find_subcommand_mut("import")
            .expect("import command")
            .render_long_help()
            .to_string();
        assert!(import_help.contains("--resume"));
        assert!(import_help.contains("--retry-failed"));
        assert!(import_help.contains("--sanitize-profile"));
        assert_eq!(Cli::command().get_version(), Some("0.9.0"));
    }

    #[test]
    fn legacy_config_maps_to_command_when_no_cli_command_is_supplied() {
        let mut config = AppConfig::default();
        config.import.dry_run_inspect_only = true;
        assert_eq!(
            effective_command(&parse(&["to-digi-rs"]), &config),
            EffectiveCommand::Analyze {
                legacy_used: true,
                sanitize_profile: None,
                raw: true,
            }
        );

        config.import.dry_run_inspect_only = false;
        config.import.send_only_first_plu = true;
        config.import.continue_after_record_failure = true;
        assert_eq!(
            effective_command(&parse(&["to-digi-rs"]), &config),
            EffectiveCommand::Import {
                limit: Some(1),
                plu: None,
                continue_on_error: true,
                test_mode: false,
                resume: None,
                retry_failed: false,
                legacy_used: true,
                defaulted_from_no_command: true,
                sanitize_profile: None,
            }
        );
    }

    #[test]
    fn explicit_cli_overrides_legacy_config() {
        let mut config = AppConfig::default();
        config.import.send_only_first_plu = true;
        config.import.continue_after_record_failure = true;
        let cli = parse(&["to-digi-rs", "import", "--limit", "2"]);

        assert_eq!(
            effective_command(&cli, &config),
            EffectiveCommand::Import {
                limit: Some(2),
                plu: None,
                continue_on_error: false,
                test_mode: false,
                resume: None,
                retry_failed: false,
                legacy_used: false,
                defaulted_from_no_command: false,
                sanitize_profile: None,
            }
        );
    }

    #[test]
    fn sanitize_profile_options_parse_for_operational_commands() {
        assert!(matches!(
            effective_command(
                &parse(&[
                    "to-digi-rs",
                    "analyze",
                    "--sanitize-profile",
                    "profiles/starsky.toml"
                ]),
                &AppConfig::default()
            ),
            EffectiveCommand::Analyze {
                sanitize_profile: Some(_),
                ..
            }
        ));
        assert!(matches!(
            effective_command(
                &parse(&[
                    "to-digi-rs",
                    "verify",
                    "--sanitize-profile",
                    "profiles/starsky.toml"
                ]),
                &AppConfig::default()
            ),
            EffectiveCommand::Verify {
                sanitize_profile: Some(_)
            }
        ));
        assert!(matches!(
            effective_command(
                &parse(&[
                    "to-digi-rs",
                    "import",
                    "--sanitize-profile",
                    "profiles/starsky.toml",
                    "--limit",
                    "1"
                ]),
                &AppConfig::default()
            ),
            EffectiveCommand::Import {
                sanitize_profile: Some(_),
                limit: Some(1),
                plu: None,
                ..
            }
        ));
    }

    #[test]
    fn built_in_profile_options_parse_for_operational_commands() {
        for profile in ["starsky", "bigway"] {
            assert!(matches!(
                effective_command(
                    &parse(&["to-digi-rs", "analyze", "--profile", profile]),
                    &AppConfig::default()
                ),
                EffectiveCommand::Analyze {
                    sanitize_profile: Some(ProfileSelection::BuiltIn(name)),
                    ..
                } if name == profile
            ));
            assert!(matches!(
                effective_command(
                    &parse(&["to-digi-rs", "import", "--profile", profile]),
                    &AppConfig::default()
                ),
                EffectiveCommand::Import {
                    sanitize_profile: Some(ProfileSelection::BuiltIn(name)),
                    ..
                } if name == profile
            ));
            assert!(matches!(
                effective_command(
                    &parse(&["to-digi-rs", "diagnose", "--profile", profile, "--plu", "1"]),
                    &AppConfig::default()
                ),
                EffectiveCommand::Diagnose {
                    sanitize_profile: Some(ProfileSelection::BuiltIn(name)),
                    plu: Some(1),
                    ..
                } if name == profile
            ));
            assert!(matches!(
                effective_command(
                    &parse(&["to-digi-rs", "confirm", "all", "--profile", profile, "--yes"]),
                    &AppConfig::default()
                ),
                EffectiveCommand::Confirm {
                    sanitize_profile: Some(ProfileSelection::BuiltIn(name)),
                    target: ConfirmTarget::All,
                    yes: true,
                    ..
                } if name == profile
            ));
        }
    }

    #[test]
    fn confirm_targets_parse_with_safety_flags() {
        for (target, expected) in [
            ("departments", ConfirmTarget::Departments),
            ("groups", ConfirmTarget::Groups),
            ("label-formats", ConfirmTarget::LabelFormats),
            ("all", ConfirmTarget::All),
        ] {
            assert!(matches!(
                effective_command(
                    &parse(&["to-digi-rs", "confirm", target, "--dry-run"]),
                    &AppConfig::default()
                ),
                EffectiveCommand::Confirm {
                    target,
                    dry_run: true,
                    yes: false,
                    ..
                } if target == expected
            ));
        }
    }

    #[test]
    fn raw_analysis_suppresses_default_profile() {
        let mut config = AppConfig::default();
        config.profiles.default = "starsky".to_string();

        assert_eq!(
            effective_command(&parse(&["to-digi-rs", "analyze", "--raw"]), &config),
            EffectiveCommand::Analyze {
                legacy_used: false,
                sanitize_profile: None,
                raw: true,
            }
        );
    }

    #[test]
    fn offline_diagnostics_parse_effectively_without_profiles() {
        assert_eq!(
            effective_command(
                &parse(&["to-digi-rs", "discover", "--timings"]),
                &AppConfig::default()
            ),
            EffectiveCommand::Discover { timings: true }
        );
        assert_eq!(
            effective_command(
                &parse(&["to-digi-rs", "map-audit", "--sample", "2", "--plu", "42"]),
                &AppConfig::default()
            ),
            EffectiveCommand::MapAudit {
                sample: 2,
                plu: Some(42),
                timings: false,
            }
        );
        assert_eq!(
            effective_command(
                &parse(&["to-digi-rs", "profile", "suggest", "--name", "bigway"]),
                &AppConfig::default()
            ),
            EffectiveCommand::ProfileSuggest {
                name: "bigway".to_string(),
            }
        );
        assert_eq!(
            effective_command(
                &parse(&["to-digi-rs", "diagnose", "--category", "duplicate-barcode"]),
                &AppConfig::default()
            ),
            EffectiveCommand::Diagnose {
                invalid_only: false,
                plu: None,
                category: Some(DiagnosticCategory::DuplicateBarcode),
                sanitize_profile: None,
            }
        );
        assert_eq!(
            effective_command(
                &parse(&["to-digi-rs", "dry-run", "--limit", "2"]),
                &AppConfig::default()
            ),
            EffectiveCommand::DryRun {
                limit: Some(2),
                plu: None,
                test_mode: false,
                sanitize_profile: None,
            }
        );
        assert_eq!(
            effective_command(
                &parse(&["to-digi-rs", "import", "--dry-run", "--test"]),
                &AppConfig::default()
            ),
            EffectiveCommand::DryRun {
                limit: Some(1),
                plu: None,
                test_mode: true,
                sanitize_profile: None,
            }
        );
    }
}
