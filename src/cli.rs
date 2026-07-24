use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

use crate::config::AppConfig;

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
    /// Import valid PLUs into DIGIweb
    Import(ImportArgs),
    /// Preview profile-driven sanitization without contacting DIGIweb
    Sanitize(SanitizeArgs),
    /// Test DIGIweb authentication and connectivity
    TestConnection,
    /// Verify import readiness without writing PLUs
    Verify(VerifyArgs),
}

#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct AnalyzeArgs {
    /// Apply a fill-only sanitization profile before analysis
    #[arg(long, value_name = "PROFILE")]
    pub sanitize_profile: Option<PathBuf>,
}

#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct ImportArgs {
    /// Import only the first N valid normalized PLUs
    #[arg(long, value_parser = parse_positive_usize, conflicts_with_all = ["test", "resume"])]
    pub limit: Option<usize>,
    /// Convenience alias for --limit 1
    #[arg(long, conflicts_with = "resume")]
    pub test: bool,
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
    #[arg(long, value_name = "PROFILE", conflicts_with = "resume")]
    pub sanitize_profile: Option<PathBuf>,
}

#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct SanitizeArgs {
    /// TOML sanitization profile to preview
    #[arg(long, value_name = "PROFILE")]
    pub profile: PathBuf,
    /// Explicit alias; sanitize is always a dry run
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct VerifyArgs {
    /// Apply a fill-only sanitization profile before readiness verification
    #[arg(long, value_name = "PROFILE")]
    pub sanitize_profile: Option<PathBuf>,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EffectiveCommand {
    Analyze {
        legacy_used: bool,
        sanitize_profile: Option<PathBuf>,
    },
    Import {
        limit: Option<usize>,
        continue_on_error: bool,
        test_mode: bool,
        resume: Option<PathBuf>,
        retry_failed: bool,
        legacy_used: bool,
        defaulted_from_no_command: bool,
        sanitize_profile: Option<PathBuf>,
    },
    Sanitize {
        profile: PathBuf,
        dry_run: bool,
    },
    TestConnection,
    Verify {
        sanitize_profile: Option<PathBuf>,
    },
}

impl EffectiveCommand {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Analyze { .. } => "analyze",
            Self::Import { .. } => "import",
            Self::Sanitize { .. } => "sanitize",
            Self::TestConnection => "test-connection",
            Self::Verify { .. } => "verify",
        }
    }

    pub fn uses_legacy_config(&self) -> bool {
        match self {
            Self::Analyze { legacy_used, .. } => *legacy_used,
            Self::Import { legacy_used, .. } => *legacy_used,
            Self::Sanitize { .. } | Self::TestConnection | Self::Verify { .. } => false,
        }
    }
}

pub fn effective_command(cli: &Cli, config: &AppConfig) -> EffectiveCommand {
    match &cli.command {
        Some(CliCommand::Analyze(args)) => EffectiveCommand::Analyze {
            legacy_used: false,
            sanitize_profile: args.sanitize_profile.clone(),
        },
        Some(CliCommand::Import(args)) => EffectiveCommand::Import {
            limit: if args.test { Some(1) } else { args.limit },
            continue_on_error: args.continue_on_error,
            test_mode: args.test,
            resume: args.resume.clone(),
            retry_failed: args.retry_failed,
            legacy_used: false,
            defaulted_from_no_command: false,
            sanitize_profile: args.sanitize_profile.clone(),
        },
        Some(CliCommand::Sanitize(args)) => EffectiveCommand::Sanitize {
            profile: args.profile.clone(),
            dry_run: args.dry_run,
        },
        Some(CliCommand::TestConnection) => EffectiveCommand::TestConnection,
        Some(CliCommand::Verify(args)) => EffectiveCommand::Verify {
            sanitize_profile: args.sanitize_profile.clone(),
        },
        None => legacy_effective_command(config),
    }
}

fn legacy_effective_command(config: &AppConfig) -> EffectiveCommand {
    if config.import.dry_run_inspect_only {
        EffectiveCommand::Analyze {
            legacy_used: true,
            sanitize_profile: None,
        }
    } else {
        EffectiveCommand::Import {
            limit: if config.import.send_only_first_plu {
                Some(1)
            } else {
                None
            },
            continue_on_error: config.import.continue_after_record_failure,
            test_mode: false,
            resume: None,
            retry_failed: false,
            legacy_used: true,
            defaulted_from_no_command: true,
            sanitize_profile: None,
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
            parse(&["to-digi-rs", "import"]).command,
            Some(CliCommand::Import(_))
        ));
        assert!(matches!(
            parse(&[
                "to-digi-rs",
                "sanitize",
                "--profile",
                "profiles/starsky.toml"
            ])
            .command,
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
    }

    #[test]
    fn resume_parses_and_conflicts_with_selection_flags() {
        let config = AppConfig::default();
        let cli = parse(&["to-digi-rs", "import", "--resume", "import-results.json"]);

        assert_eq!(
            effective_command(&cli, &config),
            EffectiveCommand::Import {
                limit: None,
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
        assert!(help.contains("import"));
        assert!(help.contains("sanitize"));
        assert!(help.contains("test-connection"));
        assert!(help.contains("verify"));
        let mut command = CliCommand::augment_subcommands(clap::Command::new("to-digi-rs"));
        let import_help = command
            .find_subcommand_mut("import")
            .expect("import command")
            .render_long_help()
            .to_string();
        assert!(import_help.contains("--resume"));
        assert!(import_help.contains("--retry-failed"));
        assert!(import_help.contains("--sanitize-profile"));
        assert_eq!(Cli::command().get_version(), Some("0.8.0"));
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
            }
        );

        config.import.dry_run_inspect_only = false;
        config.import.send_only_first_plu = true;
        config.import.continue_after_record_failure = true;
        assert_eq!(
            effective_command(&parse(&["to-digi-rs"]), &config),
            EffectiveCommand::Import {
                limit: Some(1),
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
                ..
            }
        ));
    }
}
