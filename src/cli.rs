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
    Analyze,
    /// Import valid PLUs into DIGIweb
    Import(ImportArgs),
    /// Test DIGIweb authentication and connectivity
    TestConnection,
    /// Verify import readiness without writing PLUs
    Verify,
}

#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct ImportArgs {
    /// Import only the first N valid normalized PLUs
    #[arg(long, value_parser = parse_positive_usize, conflicts_with = "test")]
    pub limit: Option<usize>,
    /// Convenience alias for --limit 1
    #[arg(long)]
    pub test: bool,
    /// Continue submitting later selected PLUs after a failure or unknown final status
    #[arg(long)]
    pub continue_on_error: bool,
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
    },
    Import {
        limit: Option<usize>,
        continue_on_error: bool,
        test_mode: bool,
        legacy_used: bool,
        defaulted_from_no_command: bool,
    },
    TestConnection,
    Verify,
}

impl EffectiveCommand {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Analyze { .. } => "analyze",
            Self::Import { .. } => "import",
            Self::TestConnection => "test-connection",
            Self::Verify => "verify",
        }
    }

    pub fn uses_legacy_config(&self) -> bool {
        match self {
            Self::Analyze { legacy_used } => *legacy_used,
            Self::Import { legacy_used, .. } => *legacy_used,
            Self::TestConnection | Self::Verify => false,
        }
    }
}

pub fn effective_command(cli: &Cli, config: &AppConfig) -> EffectiveCommand {
    match &cli.command {
        Some(CliCommand::Analyze) => EffectiveCommand::Analyze { legacy_used: false },
        Some(CliCommand::Import(args)) => EffectiveCommand::Import {
            limit: if args.test { Some(1) } else { args.limit },
            continue_on_error: args.continue_on_error,
            test_mode: args.test,
            legacy_used: false,
            defaulted_from_no_command: false,
        },
        Some(CliCommand::TestConnection) => EffectiveCommand::TestConnection,
        Some(CliCommand::Verify) => EffectiveCommand::Verify,
        None => legacy_effective_command(config),
    }
}

fn legacy_effective_command(config: &AppConfig) -> EffectiveCommand {
    if config.import.dry_run_inspect_only {
        EffectiveCommand::Analyze { legacy_used: true }
    } else {
        EffectiveCommand::Import {
            limit: if config.import.send_only_first_plu {
                Some(1)
            } else {
                None
            },
            continue_on_error: config.import.continue_after_record_failure,
            test_mode: false,
            legacy_used: true,
            defaulted_from_no_command: true,
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
                legacy_used: true,
                defaulted_from_no_command: true
            }
        );
    }

    #[test]
    fn commands_parse() {
        assert!(matches!(
            parse(&["to-digi-rs", "analyze"]).command,
            Some(CliCommand::Analyze)
        ));
        assert!(matches!(
            parse(&["to-digi-rs", "import"]).command,
            Some(CliCommand::Import(_))
        ));
        assert!(matches!(
            parse(&["to-digi-rs", "test-connection"]).command,
            Some(CliCommand::TestConnection)
        ));
        assert!(matches!(
            parse(&["to-digi-rs", "verify"]).command,
            Some(CliCommand::Verify)
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
                legacy_used: false,
                defaulted_from_no_command: false
            }
        );
        assert!(Cli::try_parse_from(["to-digi-rs", "import", "--test", "--limit", "1"]).is_err());
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
        assert!(help.contains("test-connection"));
        assert!(help.contains("verify"));
        assert_eq!(Cli::command().get_version(), Some("0.4.0"));
    }

    #[test]
    fn legacy_config_maps_to_command_when_no_cli_command_is_supplied() {
        let mut config = AppConfig::default();
        config.import.dry_run_inspect_only = true;
        assert_eq!(
            effective_command(&parse(&["to-digi-rs"]), &config),
            EffectiveCommand::Analyze { legacy_used: true }
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
                legacy_used: true,
                defaulted_from_no_command: true
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
                legacy_used: false,
                defaulted_from_no_command: false
            }
        );
    }
}
