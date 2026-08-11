mod analysis;
mod cli;
mod config;
mod deployment;
mod diagnostics;
mod digiweb;
mod discovery;
mod error;
mod import;
mod logging;
mod mapping_audit;
mod models;
mod profile_suggestion;
mod recovery;
mod sanitization;
mod selection;
mod source;
mod validation;

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use analysis::model::ReferenceTableSnapshot;
use analysis::{
    AnalysisInput, collect_analysis, render_console_summary, write_json_report, write_text_report,
};
use clap::Parser;
use cli::{Cli, CliCommand, EffectiveCommand, ProfileSelection, effective_command};
use config::{AppConfig, client_secret_log_message, load_client_secret};
use deployment::{run_doctor, run_init};
use diagnostics::{
    DiagnosticCategory, DiagnosticTiming, DiagnosticsInput, build_diagnostics_report,
    build_dry_run_manifest, filter_diagnostics, render_diagnostics_console, render_dry_run_console,
    write_diagnostics_reports, write_dry_run_outputs,
};
use digiweb::auth::authenticate;
use digiweb::client::DigiwebClient;
use digiweb::payload::DigiwebPluPayload;
use digiweb::preflight::{
    AuthenticationReadinessStatus, ReadinessResult, ReferenceConfirmationStatus,
    ReferenceReadiness, collect_required_references, evaluate_reference_readiness,
};
use discovery::{DiscoveryInput, PhaseTiming, render_discovery_console, write_discovery_reports};
use error::AppError;
use import::runner::{ImportRunOptions, run_import};
use logging::{AuditLogger, FinalImportLog};
use mapping_audit::{MappingAuditInput, render_mapping_console, write_mapping_reports};
use models::plu::Plu;
use profile_suggestion::{
    ProfileSuggestionInput, render_profile_suggestion_console, suggest_profile,
    write_profile_suggestion,
};
use recovery::validator::target_identity;
use recovery::{DEFAULT_MANIFEST_PATH, SourceIdentity, sha256_file};
use sanitization::{
    SanitizationIntegration, SanitizationProfile, SanitizationReportInput, apply_profile,
    load_profile_from_safe_path, validate_profile_path, write_sanitization_reports,
};
use selection::{SelectionCriteria, select_eligible_plus, selection_error};
use serde::Serialize;
use source::SourceDataset;
use source::mapping::{normalize_dataset, validate_source_schema};
use source::mdb_tools::MdbTools;
use source::schema::MdbSchema;
use source::{FIXED_SOURCE_FILE, VerifiedSourceFile};
use validation::issue::Severity;
use validation::validator::{ValidationReport, valid_plu_candidates, validate_plus};

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let exit_code = match AuditLogger::create(Path::new("logs.txt")) {
        Ok(mut logger) => run(&cli, &mut logger).await,
        Err(err) => {
            eprintln!("failed to create logs.txt: {err}");
            4
        }
    };
    std::process::exit(exit_code);
}

async fn run(cli: &Cli, logger: &mut AuditLogger) -> i32 {
    match run_inner(cli, logger).await {
        Ok(code) => code,
        Err(err) => {
            let _ = logger.error(err.to_string());
            let _ = logger.final_failure(err.stage(), &err.to_string(), true);
            eprintln!("Result: FAIL");
            eprintln!("Stage: {}", err.stage());
            eprintln!("Reason: {err}");
            eprintln!("Exit code: {}", err.exit_code());
            err.exit_code()
        }
    }
}

async fn run_inner(cli: &Cli, logger: &mut AuditLogger) -> Result<i32, AppError> {
    let config_path = Path::new("config.toml");
    let config_exists = config_path.exists();
    let config_not_required_for_dispatch = matches!(
        cli.command,
        Some(CliCommand::Discover(_))
            | Some(CliCommand::Diagnose(_))
            | Some(CliCommand::DryRun(_))
            | Some(CliCommand::Init(_))
            | Some(CliCommand::MapAudit(_))
            | Some(CliCommand::Profile(_))
            | Some(CliCommand::Pull)
            | Some(CliCommand::Version)
    );
    let config = if config_not_required_for_dispatch {
        AppConfig::default()
    } else {
        AppConfig::load(config_path)?
    };
    let command = effective_command(cli, &config);
    if matches!(
        command,
        EffectiveCommand::Analyze { .. }
            | EffectiveCommand::Discover { .. }
            | EffectiveCommand::Diagnose { .. }
            | EffectiveCommand::DryRun { .. }
            | EffectiveCommand::MapAudit { .. }
            | EffectiveCommand::ProfileSuggest { .. }
            | EffectiveCommand::Sanitize { .. }
    ) && !config_exists
    {
        logger.line("config.toml not found; using built-in offline mapping defaults.")?;
    }
    if !matches!(
        command,
        EffectiveCommand::Analyze { .. }
            | EffectiveCommand::Discover { .. }
            | EffectiveCommand::Diagnose { .. }
            | EffectiveCommand::DryRun { .. }
            | EffectiveCommand::Doctor { .. }
            | EffectiveCommand::MapAudit { .. }
            | EffectiveCommand::ProfileSuggest { .. }
            | EffectiveCommand::Sanitize { .. }
            | EffectiveCommand::Init { .. }
            | EffectiveCommand::Pull
            | EffectiveCommand::Version
    ) {
        if !config_exists {
            return Err(AppError::Config(format!(
                "config.toml is required for the '{}' command",
                command.name()
            )));
        }
        config.validate_startup()?;
    }
    log_command(&command, logger)?;
    if !matches!(
        command,
        EffectiveCommand::Analyze { .. }
            | EffectiveCommand::Discover { .. }
            | EffectiveCommand::Diagnose { .. }
            | EffectiveCommand::DryRun { .. }
            | EffectiveCommand::MapAudit { .. }
            | EffectiveCommand::ProfileSuggest { .. }
            | EffectiveCommand::Sanitize { .. }
            | EffectiveCommand::Init { .. }
            | EffectiveCommand::Pull
            | EffectiveCommand::Version
    ) {
        logger.kv("DIGIweb target URL", &config.digiweb.base_url)?;
        if config.digiweb.allow_invalid_certificates {
            logger.warning("TLS certificate validation is disabled.")?;
        }

        if config.digiweb.log_credentials_for_testing {
            logger.warning("Testing credential logging is enabled. Only the Client ID is written; client secrets are never logged.")?;
            logger.kv("DIGIweb Client ID", &config.digiweb.client_id)?;
        }
    }

    if command.uses_legacy_config() || config.deprecated_command_selector_flags_present() {
        logger.warning("Legacy [import] command-selector flags are deprecated and will be removed in a future release. Use CLI commands and flags instead.")?;
    }

    match command {
        EffectiveCommand::Analyze {
            sanitize_profile, ..
        } => run_analyze(&config, logger, sanitize_profile.as_ref()),
        EffectiveCommand::Discover { timings } => run_discover(&config, logger, timings),
        EffectiveCommand::Diagnose {
            invalid_only,
            plu,
            category,
        } => run_diagnose(&config, logger, invalid_only, plu, category),
        EffectiveCommand::Doctor {
            pull,
            inside_container,
            sanitize_profile,
        } => run_doctor(
            &config,
            pull,
            inside_container,
            sanitize_profile.as_ref(),
            logger,
        ),
        EffectiveCommand::Init {
            refresh_generated_files,
        } => run_init(refresh_generated_files, logger),
        EffectiveCommand::Import {
            limit,
            plu,
            continue_on_error,
            test_mode,
            resume,
            retry_failed,
            sanitize_profile,
            ..
        } => {
            if config.import.dry_run_inspect_only {
                return Err(AppError::Config(
                    "CONFIGURATION CONFLICT\n\nconfig.toml contains:\ndry_run_inspect_only = true\n\nThe requested command performs live writes:\n./to-digi import\n\nNo PLUs were submitted.\n\nUse:\n./to-digi dry-run\n\nor explicitly migrate/remove the deprecated setting after reviewing the configuration."
                        .to_string(),
                ));
            }
            run_import_command(
                &config,
                limit,
                plu,
                continue_on_error,
                test_mode,
                resume.as_deref(),
                retry_failed,
                sanitize_profile.as_ref(),
                logger,
            )
            .await
        }
        EffectiveCommand::DryRun {
            limit,
            plu,
            test_mode,
            sanitize_profile,
        } => run_dry_run(
            &config,
            logger,
            limit,
            plu,
            test_mode,
            sanitize_profile.as_ref(),
        ),
        EffectiveCommand::Pull => {
            println!("Pull is handled by the generated ./to-digi launcher.");
            println!(
                "For direct Docker use, run: docker pull {}",
                deployment::default_image_reference()
            );
            logger.line("Pull command invoked inside importer; no Docker resources modified.")?;
            Ok(0)
        }
        EffectiveCommand::ProfileSuggest { name } => run_profile_suggest(&config, logger, &name),
        EffectiveCommand::MapAudit {
            sample,
            plu,
            timings,
        } => run_map_audit(&config, logger, sample, plu, timings),
        EffectiveCommand::Sanitize { profile, .. } => run_sanitize(&config, logger, &profile),
        EffectiveCommand::TestConnection => run_test_connection(&config, logger).await,
        EffectiveCommand::Verify { sanitize_profile } => {
            run_verify(&config, logger, sanitize_profile.as_ref()).await
        }
        EffectiveCommand::Version => {
            println!("to-digi-rs {}", env!("CARGO_PKG_VERSION"));
            logger.kv("Application version", env!("CARGO_PKG_VERSION"))?;
            Ok(0)
        }
    }
}

fn log_command(command: &EffectiveCommand, logger: &mut AuditLogger) -> Result<(), AppError> {
    logger.kv("Command", command.name())?;
    match command {
        EffectiveCommand::Analyze {
            sanitize_profile,
            raw,
            ..
        } => {
            logger.kv("Network access permitted", "no")?;
            logger.kv("Authentication attempted", "NO")?;
            logger.kv("DIGIweb API requests attempted", "NO")?;
            logger.kv("Source database modified", "NO")?;
            if let Some(path) = sanitize_profile {
                logger.kv("Sanitization profile", &path.display())?;
            }
            if *raw {
                logger.kv("Raw analysis requested", "yes")?;
            }
        }
        EffectiveCommand::Discover { timings } => {
            logger.kv("Network access permitted", "no")?;
            logger.kv("Authentication attempted", "NO")?;
            logger.kv("DIGIweb API requests attempted", "NO")?;
            logger.kv("Source database modified", "NO")?;
            logger.kv("Profile applied", "NO")?;
            logger.kv(
                "Detailed timings requested",
                if *timings { "yes" } else { "no" },
            )?;
        }
        EffectiveCommand::Diagnose {
            invalid_only,
            plu,
            category,
        } => {
            logger.kv("Network access permitted", "no")?;
            logger.kv("Authentication attempted", "NO")?;
            logger.kv("DIGIweb API requests attempted", "NO")?;
            logger.kv("Source database modified", "NO")?;
            logger.kv("Invalid only", if *invalid_only { "yes" } else { "no" })?;
            if let Some(plu) = plu {
                logger.kv("PLU filter", &plu.to_string())?;
            }
            if let Some(category) = category {
                logger.kv("Diagnostic category", category.as_str())?;
            }
        }
        EffectiveCommand::Doctor {
            pull,
            inside_container,
            sanitize_profile,
        } => {
            logger.kv("PLU write permitted", "no")?;
            logger.kv("Docker pull requested", if *pull { "yes" } else { "no" })?;
            logger.kv(
                "Inside container",
                if *inside_container { "yes" } else { "no" },
            )?;
            if let Some(profile) = sanitize_profile {
                logger.kv("Sanitization profile", &profile.display())?;
            }
        }
        EffectiveCommand::Init {
            refresh_generated_files,
        } => {
            logger.kv("PLU write permitted", "no")?;
            logger.kv(
                "Refresh generated files",
                if *refresh_generated_files {
                    "yes"
                } else {
                    "no"
                },
            )?;
        }
        EffectiveCommand::Import {
            limit,
            plu,
            continue_on_error,
            test_mode,
            resume,
            retry_failed,
            sanitize_profile,
            legacy_used,
            defaulted_from_no_command,
        } => {
            if *defaulted_from_no_command {
                logger.line("No command supplied; defaulting to import.")?;
            }
            if *test_mode {
                logger.line("Test mode enabled: equivalent to --limit 1.")?;
            }
            let import_limit = if resume.is_some() {
                "manifest-controlled".to_string()
            } else {
                limit
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "none".to_string())
            };
            logger.kv("Import limit", &import_limit)?;
            if let Some(plu) = plu {
                logger.kv("Requested PLU", &plu.to_string())?;
            }
            if let Some(path) = resume {
                logger.kv("Resume manifest", &path.display().to_string())?;
                logger.kv(
                    "Retry confirmed failed records",
                    if *retry_failed { "true" } else { "false" },
                )?;
            }
            if let Some(path) = sanitize_profile {
                logger.kv("Sanitization profile", &path.display())?;
            }
            logger.kv(
                "Continue on error",
                if *continue_on_error { "true" } else { "false" },
            )?;
            logger.kv(
                "Legacy configuration used",
                if *legacy_used { "yes" } else { "no" },
            )?;
        }
        EffectiveCommand::DryRun {
            limit,
            plu,
            test_mode,
            sanitize_profile,
        } => {
            logger.kv("Network access permitted", "no")?;
            logger.kv("Authentication attempted", "NO")?;
            logger.kv("DIGIweb API requests attempted", "NO")?;
            logger.kv("Source database modified", "NO")?;
            logger.kv("API write requests", "0")?;
            if *test_mode {
                logger.line("Dry-run test mode enabled: equivalent to --limit 1.")?;
            }
            logger.kv(
                "Dry-run limit",
                &limit
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "none".to_string()),
            )?;
            if let Some(plu) = plu {
                logger.kv("Requested PLU", &plu.to_string())?;
            }
            if let Some(profile) = sanitize_profile {
                logger.kv("Sanitization profile", &profile.display())?;
            }
        }
        EffectiveCommand::Pull => {
            logger.kv("PLU write permitted", "no")?;
            logger.kv("Docker pull command", "launcher-handled")?;
        }
        EffectiveCommand::ProfileSuggest { name } => {
            logger.kv("Network access permitted", "no")?;
            logger.kv("Authentication attempted", "NO")?;
            logger.kv("DIGIweb API requests attempted", "NO")?;
            logger.kv("Source database modified", "NO")?;
            logger.kv("Profile draft name", name)?;
            logger.kv("Profile applied", "NO")?;
        }
        EffectiveCommand::MapAudit {
            sample,
            plu,
            timings,
        } => {
            logger.kv("Network access permitted", "no")?;
            logger.kv("Authentication attempted", "NO")?;
            logger.kv("DIGIweb API requests attempted", "NO")?;
            logger.kv("Source database modified", "NO")?;
            logger.kv("Payload samples requested", &sample.to_string())?;
            if let Some(plu) = plu {
                logger.kv("PLU filter", &plu.to_string())?;
            }
            logger.kv(
                "Detailed timings requested",
                if *timings { "yes" } else { "no" },
            )?;
        }
        EffectiveCommand::Sanitize { profile, dry_run } => {
            logger.kv("Network access permitted", "no")?;
            logger.kv("Authentication attempted", "NO")?;
            logger.kv("DIGIweb API requests attempted", "NO")?;
            logger.kv("Source database modified", "NO")?;
            logger.kv("Sanitization profile", &profile.display())?;
            logger.kv("Dry run", if *dry_run { "true" } else { "implicit" })?;
        }
        EffectiveCommand::TestConnection => {
            logger.kv("PLU write permitted", "no")?;
        }
        EffectiveCommand::Verify { sanitize_profile } => {
            logger.kv("PLU write permitted", "no")?;
            if let Some(path) = sanitize_profile {
                logger.kv("Sanitization profile", &path.display())?;
            }
        }
        EffectiveCommand::Version => {
            logger.kv("PLU write permitted", "no")?;
        }
    }
    Ok(())
}

fn read_source_context(
    config: &AppConfig,
    logger: &mut AuditLogger,
    sanitization_profile: Option<SanitizationProfile>,
    detailed_plu_logging: bool,
) -> Result<SourceContext, AppError> {
    let source_path = Path::new(FIXED_SOURCE_FILE);
    logger.kv("Path checked for source file", "./plu.mdb")?;
    logger.line("The application will not scan for alternate MDB files.")?;

    MdbTools::verify_required_commands()?;
    logger.kv("mdbtools verification", "SUCCESS")?;

    let source_file = VerifiedSourceFile::verify(source_path)?;
    logger.kv(
        "Source file opened",
        &format!("{} read-only", source_file.path().display()),
    )?;
    logger.line("Confirmation: only plu.mdb was opened.")?;

    let source_file_size_bytes = std::fs::metadata(source_file.path())
        .map_err(|err| AppError::InvalidSourceFile {
            path: source_file.path().to_path_buf(),
            message: err.to_string(),
        })?
        .len();
    let source_identity = SourceIdentity {
        filename: FIXED_SOURCE_FILE.to_string(),
        size_bytes: source_file_size_bytes,
        sha256: sha256_file(Path::new(FIXED_SOURCE_FILE))?,
    };
    let (mut schema, dataset) =
        MdbTools::read_dataset(source_file.path(), &config.mapping, logger)?;
    let reference_tables = read_reference_tables(source_file.path(), &mut schema, logger)?;
    validate_source_schema(&schema, &config.mapping)?;
    logger.kv(
        "Number of PLUs discovered",
        &dataset.plu_rows.len().to_string(),
    )?;
    logger.kv(
        "Number of ingredient records discovered",
        &dataset.ingredient_rows.len().to_string(),
    )?;
    logger.kv(
        "Number of nutrition records discovered",
        &dataset.nutrition_rows.len().to_string(),
    )?;

    let raw_normalization_report =
        normalize_dataset(&dataset, &config.mapping, config.digiweb.store_number)?;
    let raw_validation_report = validate_plus(&raw_normalization_report.plus);
    let raw_valid_plus =
        valid_plu_candidates(&raw_normalization_report.plus, &raw_validation_report);
    let raw_valid_count = raw_valid_plus.len();
    let raw_valid_numbers = raw_valid_plus
        .iter()
        .map(|plu| plu.plu_number)
        .collect::<Vec<_>>();
    let raw_before_invalid = counted_invalid_rows(
        &raw_normalization_report,
        &raw_validation_report,
        raw_valid_count,
    );
    drop(raw_valid_plus);
    let (effective_dataset, normalization_report, mut sanitization) =
        if let Some(profile) = sanitization_profile {
            logger.line(format!("Sanitization profile: {}", profile.profile_name))?;
            logger.line("Profile-provided values will be applied in memory.")?;
            logger.line("Source MDB will not be modified.")?;
            let sanitized = apply_profile(&dataset, &profile)?;
            for record in &sanitized.report.records {
                for field in &record.fields {
                    if field.field == "selling_date_term" {
                        logger.line(format!(
                            "PLU {} selling-date term corrected by profile.",
                            record.plu_number
                        ))?;
                        logger.kv("Original value", &field.original_value)?;
                        logger.kv("Sanitized value", &field.sanitized_value)?;
                        logger.kv("Reason", selling_date_reason_for_log(field.reason))?;
                    }
                }
            }
            let normalization_report = normalize_dataset(
                &sanitized.dataset,
                &config.mapping,
                config.digiweb.store_number,
            )?;
            let validation_report = validate_plus(&normalization_report.plus);
            let valid_plus = valid_plu_candidates(&normalization_report.plus, &validation_report);
            let after_invalid =
                counted_invalid_rows(&normalization_report, &validation_report, valid_plus.len());
            let after_numbers = valid_plus
                .iter()
                .map(|plu| plu.plu_number)
                .collect::<Vec<_>>();
            let mut integration = sanitization::report::integration_from_parts(
                profile,
                sanitized.report,
                raw_valid_count,
                raw_before_invalid,
                valid_plus.len(),
                after_invalid,
                &raw_valid_numbers,
                &after_numbers,
            )?;
            integration.still_invalid_plus =
                still_invalid_plu_numbers(&normalization_report, &validation_report, &valid_plus);
            logger.kv(
                "PLUs recovered by sanitization",
                &integration.recovered_plus.to_string(),
            )?;
            logger.kv(
                "Existing nonempty values changed",
                &integration
                    .engine_report
                    .nonempty_values_changed
                    .to_string(),
            )?;
            (sanitized.dataset, normalization_report, Some(integration))
        } else {
            (dataset, raw_normalization_report, None)
        };
    let placeholder_ignored = normalization_report
        .row_issues
        .iter()
        .filter(|issue| is_empty_placeholder_issue(issue))
        .count();
    let invalid_source_rows = normalization_report
        .row_issues
        .iter()
        .filter(|issue| !is_empty_placeholder_issue(issue))
        .count();
    for issue in &normalization_report.row_issues {
        let plu = issue
            .plu_number
            .map(|value| value.to_string())
            .unwrap_or_else(|| "unknown".to_string());
        if is_empty_placeholder_issue(issue) {
            logger.line(format!(
                "PLU {plu} ignored as empty placeholder: {}",
                issue.message
            ))?;
        } else {
            logger.line(format!("PLU {plu} skipped: {}", issue.message))?;
        }
    }
    logger.kv(
        "Unmatched PluIng rows",
        &normalization_report.orphan_pluing_rows.to_string(),
    )?;
    logger.kv(
        "PLUs using explicit group references",
        &normalization_report.explicit_group_references.to_string(),
    )?;
    logger.kv(
        "PLUs defaulted to group 997",
        &normalization_report.defaulted_group_references.to_string(),
    )?;
    logger.kv(
        "PLUs with invalid group values",
        &normalization_report.invalid_group_values.to_string(),
    )?;
    let orphan_pluing_rows = normalization_report.orphan_pluing_rows;
    let explicit_group_references = normalization_report.explicit_group_references;
    let defaulted_group_references = normalization_report.defaulted_group_references;
    let invalid_group_values = normalization_report.invalid_group_values;
    let plus = normalization_report.plus;
    logger.kv("Normalized PLU records", &plus.len().to_string())?;
    if detailed_plu_logging {
        for plu in &plus {
            logger.line(format!("PLU: {}", plu.plu_number))?;
            logger.kv(
                "Raw department",
                &format!("{:?}", plu.source_department.as_deref().unwrap_or("")),
            )?;
            logger.kv(
                "Normalized department reference",
                &plu.department_number
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "missing".to_string()),
            )?;
            logger.kv(
                "Raw Main Group Code",
                &format!("{:?}", plu.source_group.as_deref().unwrap_or("")),
            )?;
            logger.kv(
                "Normalized group reference",
                &plu.group_number
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "missing".to_string()),
            )?;
            logger.kv(
                "Group default applied",
                if plu.group_default_applied {
                    "yes"
                } else {
                    "no"
                },
            )?;
            logger.kv(
                "Raw Barcode Format",
                &format!("{:?}", plu.source_barcode_format.as_deref().unwrap_or("")),
            )?;
            logger.kv(
                "Raw Barcode",
                &format!("{:?}", plu.source_barcode.as_deref().unwrap_or("")),
            )?;
            logger.kv(
                "Raw Flag Data",
                &format!("{:?}", plu.source_flag_data.as_deref().unwrap_or("")),
            )?;
            logger.kv(
                "Derived DIGIweb barcode type",
                plu.barcode_type.as_deref().unwrap_or("missing"),
            )?;
            logger.kv(
                "Derived DIGIweb barcode reference",
                plu.barcode_ref_no.as_deref().unwrap_or("missing"),
            )?;
            logger.kv(
                "Derived barcode data",
                &format!("{:?}", plu.barcode.as_deref().unwrap_or("")),
            )?;
            if let Some(group) = plu.group_number {
                logger.line(format!(
                    "PLU {} group reference: {} - local validation passed",
                    plu.plu_number, group
                ))?;
                logger.line(format!("Source Main Group Code: {}", group))?;
                logger.line(format!("DIGIweb group reference number: {}", group))?;
                logger.line("Server existence: UNVERIFIED during local source normalization")?;
                logger.line("Internal UUID: not resolved during preflight")?;
                logger.kv("Group validation", "accepted as positive integer")?;
            }
        }
    } else {
        logger.kv(
            "Per-PLU normalization detail",
            "omitted; use diagnose --plu for record-level details",
        )?;
    }
    let required_references = collect_required_references(&plus);
    for reference in &required_references {
        logger.line(format!(
            "Required DIGIweb reference: department {} + group {} used by {} PLUs; examples: {} => {}",
            reference.department_number,
            reference.group_number,
            reference.source_plu_numbers.len(),
            format_limited_u64s(&reference.source_plu_numbers, 8),
            reference.status.as_str()
        ))?;
    }
    if !required_references.is_empty() {
        logger.line("DIGIweb group preflight lookup is not configured in this version; PLU submission will rely on the supported PLU API response for final group resolution.")?;
    }

    let validation_report = validate_plus(&plus);
    logger.kv(
        "Validation errors",
        &validation_report.error_count().to_string(),
    )?;
    logger.kv(
        "Validation warnings",
        &validation_report.warning_count().to_string(),
    )?;
    for issue in &validation_report.issues {
        let plu = issue
            .plu_number
            .map(|value| value.to_string())
            .unwrap_or_else(|| "n/a".to_string());
        logger.line(format!(
            "{}: PLU {} {}: {}",
            issue.severity.as_str(),
            plu,
            issue.field,
            issue.message
        ))?;
    }
    if validation_report
        .issues
        .iter()
        .any(|issue| issue.severity == Severity::Error && issue.plu_number.is_none())
    {
        return Err(AppError::Validation(validation_report.error_count()));
    }
    let valid_plus = valid_plu_candidates(&plus, &validation_report);
    let validation_skipped = plus.len().saturating_sub(valid_plus.len());
    let invalid_source_rows = invalid_source_rows + validation_skipped;
    for plu in &plus {
        if !valid_plus
            .iter()
            .any(|candidate| candidate.plu_number == plu.plu_number)
        {
            logger.line(format!("PLU {} skipped: validation errors", plu.plu_number))?;
        }
    }
    logger.kv("Valid PLUs available", &valid_plus.len().to_string())?;
    if let Some(sanitization) = &mut sanitization {
        sanitization.after_valid = valid_plus.len();
        sanitization.after_invalid = invalid_source_rows;
        sanitization.still_invalid_plus = still_invalid_plu_numbers_from_issues(
            &normalization_report.row_issues,
            &validation_report,
            &valid_plus,
        );
    }
    logger.kv(
        "Empty placeholder PLUs ignored",
        &placeholder_ignored.to_string(),
    )?;
    logger.kv(
        "PLUs skipped due to validation error",
        &validation_skipped.to_string(),
    )?;
    Ok(SourceContext {
        schema,
        dataset: effective_dataset,
        plus,
        valid_plus,
        validation_report,
        source_identity,
        sanitization,
        placeholder_ignored,
        invalid_source_rows,
        validation_skipped,
        orphan_pluing_rows,
        explicit_group_references,
        defaulted_group_references,
        invalid_group_values,
        row_issues: normalization_report.row_issues,
        source_file_size_bytes,
        source_is_symlink: false,
        source_opened_read_only: true,
        reference_tables,
        nutrition_fallback_to_pluing: config.mapping.nutrition_table.trim().is_empty()
            || config.mapping.nutrition_table == config.mapping.ingredient_table,
        nutrition_source_table: if config.mapping.nutrition_table.trim().is_empty() {
            config.mapping.ingredient_table.clone()
        } else {
            config.mapping.nutrition_table.clone()
        },
    })
}

fn run_analyze(
    config: &AppConfig,
    logger: &mut AuditLogger,
    sanitize_profile_path: Option<&ProfileSelection>,
) -> Result<i32, AppError> {
    println!("Starting analysis...");
    println!("Outputs will be written to analysis-report.txt, analysis-report.json, and logs.txt");
    logger.line("ANALYSIS ONLY")?;
    logger.line("Network access permitted: NO")?;
    logger.line("Authentication attempted: NO")?;
    logger.line("DIGIweb API requests attempted: NO")?;
    logger.line("Source database modified: NO")?;
    let profile = load_optional_sanitization_profile(sanitize_profile_path)?;
    let source = read_source_context(config, logger, profile, true)?;
    let report = build_analysis_report(&source);
    write_text_report(Path::new("analysis-report.txt"), &report)?;
    write_json_report(Path::new("analysis-report.json"), &report)?;
    logger.kv("Text analysis report", "analysis-report.txt")?;
    logger.kv("JSON analysis report", "analysis-report.json")?;
    logger.kv("Analysis status", report.analysis_status.as_text())?;
    logger.kv("Analysis warnings", &report.warnings.len().to_string())?;
    logger.kv(
        "Analysis blocking errors",
        &report.blocking_errors.len().to_string(),
    )?;
    logger.final_import_summary(FinalImportLog {
        status: report.analysis_status.as_text(),
        source_discovered: source.dataset.plu_rows.len(),
        placeholders_ignored: source.placeholder_ignored,
        invalid_source_rows: source.invalid_source_rows,
        validation_skipped: source.validation_skipped,
        skipped_duplicate_barcode: skipped_duplicate_barcode_count(&source),
        normalized: source.plus.len(),
        valid: source.valid_plus.len(),
        selected: 0,
        submitted: 0,
        succeeded: 0,
        failed: 0,
        unknown: 0,
        not_attempted: 0,
        intentionally_skipped_by_limit: 0,
        successful_plu_numbers: &[],
        failed_plu_numbers: &[],
        unknown_plu_numbers: &[],
        dry_run: true,
    })?;
    print!(
        "{}",
        render_console_summary(&report, "./analysis-report.txt", "./analysis-report.json")
    );
    println!("Final status: {}", report.analysis_status.as_text());
    Ok(report.analysis_status.exit_code())
}

fn run_discover(
    config: &AppConfig,
    logger: &mut AuditLogger,
    _timings_requested: bool,
) -> Result<i32, AppError> {
    println!("Starting customer MDB discovery...");
    println!(
        "Outputs will be written to discovery-report.txt, discovery-report.json, and logs.txt"
    );
    logger.line("CUSTOMER MDB DISCOVERY ONLY")?;
    logger.line("Network access permitted: NO")?;
    logger.line("Authentication attempted: NO")?;
    logger.line("DIGIweb API requests attempted: NO")?;
    logger.line("Source database modified: NO")?;
    logger.line("Sanitization profile applied: NO")?;
    let started_at = chrono::Local::now();
    let total_started = Instant::now();
    let source_started = Instant::now();
    let source = read_source_context(config, logger, None, true)?;
    let source_timing = PhaseTiming::from_duration("MDB source read", source_started.elapsed());
    let report_started = Instant::now();
    let finished_at = chrono::Local::now();
    let mut timings = vec![source_timing];
    timings.push(PhaseTiming::from_duration(
        "Total before report write",
        total_started.elapsed(),
    ));
    let mut report = discovery::build_discovery_report(DiscoveryInput {
        command: "discover",
        source_path: FIXED_SOURCE_FILE,
        source_sha256: &source.source_identity.sha256,
        started_at,
        finished_at,
        dataset: &source.dataset,
        valid_plus: &source.valid_plus,
        all_normalized_plus: &source.plus,
        row_issues: &source.row_issues,
        validation_report: &source.validation_report,
        placeholder_ignored: source.placeholder_ignored,
        reference_tables: &source.reference_tables,
        timings,
    });
    report.timings.push(PhaseTiming::from_duration(
        "Report generation",
        report_started.elapsed(),
    ));
    write_discovery_reports(
        Path::new("discovery-report.txt"),
        Path::new("discovery-report.json"),
        &report,
    )?;
    logger.kv("Text discovery report", "discovery-report.txt")?;
    logger.kv("JSON discovery report", "discovery-report.json")?;
    logger.line("Authentication attempted: NO")?;
    logger.line("DIGIweb API requests attempted: NO")?;
    logger.line("Source database modified: NO")?;
    logger.line("PLUs submitted: 0")?;
    logger.final_import_summary(FinalImportLog {
        status: "SUCCESS",
        source_discovered: source.dataset.plu_rows.len(),
        placeholders_ignored: source.placeholder_ignored,
        invalid_source_rows: source.invalid_source_rows,
        validation_skipped: source.validation_skipped,
        skipped_duplicate_barcode: skipped_duplicate_barcode_count(&source),
        normalized: source.plus.len(),
        valid: source.valid_plus.len(),
        selected: 0,
        submitted: 0,
        succeeded: 0,
        failed: 0,
        unknown: 0,
        not_attempted: 0,
        intentionally_skipped_by_limit: 0,
        successful_plu_numbers: &[],
        failed_plu_numbers: &[],
        unknown_plu_numbers: &[],
        dry_run: true,
    })?;
    print!("{}", render_discovery_console(&report));
    println!("Final status: SUCCESS");
    Ok(0)
}

fn run_map_audit(
    config: &AppConfig,
    logger: &mut AuditLogger,
    sample: usize,
    plu: Option<u64>,
    _timings_requested: bool,
) -> Result<i32, AppError> {
    println!("Starting source-to-DIGIweb mapping audit...");
    println!("Outputs will be written to mapping-report.txt, mapping-report.json, and logs.txt");
    logger.line("SOURCE TO DIGIWEB MAPPING AUDIT ONLY")?;
    logger.line("Network access permitted: NO")?;
    logger.line("Authentication attempted: NO")?;
    logger.line("DIGIweb API requests attempted: NO")?;
    logger.line("Source database modified: NO")?;
    let started_at = chrono::Local::now();
    let total_started = Instant::now();
    let source_started = Instant::now();
    let source = read_source_context(config, logger, None, true)?;
    let finished_at = chrono::Local::now();
    let timings = vec![
        PhaseTiming::from_duration("MDB source read", source_started.elapsed()),
        PhaseTiming::from_duration("Total before report write", total_started.elapsed()),
    ];
    let report = mapping_audit::build_mapping_audit_report(MappingAuditInput {
        source_path: FIXED_SOURCE_FILE,
        source_sha256: &source.source_identity.sha256,
        started_at,
        finished_at,
        dataset: &source.dataset,
        valid_plus: &source.valid_plus,
        config: &config.digiweb,
        sample_limit: sample,
        target_plu: plu,
        timings,
    })?;
    write_mapping_reports(
        Path::new("mapping-report.txt"),
        Path::new("mapping-report.json"),
        &report,
    )?;
    logger.kv("Text mapping report", "mapping-report.txt")?;
    logger.kv("JSON mapping report", "mapping-report.json")?;
    logger.kv("Mapping audit status", report.status.as_text())?;
    logger.line("Authentication attempted: NO")?;
    logger.line("DIGIweb API requests attempted: NO")?;
    logger.line("Source database modified: NO")?;
    logger.line("PLUs submitted: 0")?;
    logger.final_import_summary(FinalImportLog {
        status: report.status.as_text(),
        source_discovered: source.dataset.plu_rows.len(),
        placeholders_ignored: source.placeholder_ignored,
        invalid_source_rows: source.invalid_source_rows,
        validation_skipped: source.validation_skipped,
        skipped_duplicate_barcode: skipped_duplicate_barcode_count(&source),
        normalized: source.plus.len(),
        valid: source.valid_plus.len(),
        selected: report.selected_plu_count,
        submitted: 0,
        succeeded: 0,
        failed: 0,
        unknown: 0,
        not_attempted: 0,
        intentionally_skipped_by_limit: 0,
        successful_plu_numbers: &[],
        failed_plu_numbers: &[],
        unknown_plu_numbers: &[],
        dry_run: true,
    })?;
    print!("{}", render_mapping_console(&report));
    println!("Final status: {}", report.status.as_text());
    Ok(report.status.exit_code())
}

fn run_profile_suggest(
    config: &AppConfig,
    logger: &mut AuditLogger,
    name: &str,
) -> Result<i32, AppError> {
    println!("Starting profile suggestion...");
    println!(
        "Outputs will be written to profiles/<name>.draft.toml, profile-recommendations.txt, and logs.txt"
    );
    logger.line("PROFILE SUGGESTION ONLY")?;
    logger.line("Network access permitted: NO")?;
    logger.line("Authentication attempted: NO")?;
    logger.line("DIGIweb API requests attempted: NO")?;
    logger.line("Source database modified: NO")?;
    logger.line("Sanitization profile applied: NO")?;
    let started_at = chrono::Local::now();
    let total_started = Instant::now();
    let source_started = Instant::now();
    let source = read_source_context(config, logger, None, true)?;
    let finished_at = chrono::Local::now();
    let timings = vec![
        PhaseTiming::from_duration("MDB source read", source_started.elapsed()),
        PhaseTiming::from_duration("Total before report write", total_started.elapsed()),
    ];
    let report = suggest_profile(ProfileSuggestionInput {
        profile_name: name,
        source_path: FIXED_SOURCE_FILE,
        source_sha256: &source.source_identity.sha256,
        started_at,
        finished_at,
        dataset: &source.dataset,
        timings,
    })?;
    write_profile_suggestion(&report)?;
    logger.kv("Draft profile", &report.draft_path)?;
    logger.kv("Profile recommendations", &report.recommendations_path)?;
    logger.line("Authentication attempted: NO")?;
    logger.line("DIGIweb API requests attempted: NO")?;
    logger.line("Source database modified: NO")?;
    logger.line("PLUs submitted: 0")?;
    logger.final_import_summary(FinalImportLog {
        status: "SUCCESS",
        source_discovered: source.dataset.plu_rows.len(),
        placeholders_ignored: source.placeholder_ignored,
        invalid_source_rows: source.invalid_source_rows,
        validation_skipped: source.validation_skipped,
        skipped_duplicate_barcode: skipped_duplicate_barcode_count(&source),
        normalized: source.plus.len(),
        valid: source.valid_plus.len(),
        selected: 0,
        submitted: 0,
        succeeded: 0,
        failed: 0,
        unknown: 0,
        not_attempted: 0,
        intentionally_skipped_by_limit: 0,
        successful_plu_numbers: &[],
        failed_plu_numbers: &[],
        unknown_plu_numbers: &[],
        dry_run: true,
    })?;
    print!("{}", render_profile_suggestion_console(&report));
    println!("Final status: SUCCESS");
    Ok(0)
}

fn run_diagnose(
    config: &AppConfig,
    logger: &mut AuditLogger,
    invalid_only: bool,
    plu: Option<u64>,
    category: Option<DiagnosticCategory>,
) -> Result<i32, AppError> {
    println!("Starting PLU diagnostics...");
    println!(
        "Outputs will be written to diagnostics-report.txt, diagnostics-report.json, and logs.txt"
    );
    logger.line("PLU DIAGNOSTICS ONLY")?;
    logger.line("Network access permitted: NO")?;
    logger.line("Authentication attempted: NO")?;
    logger.line("DIGIweb API requests attempted: NO")?;
    logger.line("Source database modified: NO")?;
    let started_at = chrono::Local::now();
    let source_started = Instant::now();
    let source = read_source_context(config, logger, None, true)?;
    let finished_at = chrono::Local::now();
    let report = build_diagnostics_report(DiagnosticsInput {
        source_path: FIXED_SOURCE_FILE,
        source_sha256: &source.source_identity.sha256,
        started_at,
        finished_at,
        dataset: &source.dataset,
        all_normalized_plus: &source.plus,
        valid_plus: &source.valid_plus,
        row_issues: &source.row_issues,
        validation_report: &source.validation_report,
        timings: vec![DiagnosticTiming::from_duration(
            "MDB source read and normalization",
            source_started.elapsed(),
        )],
    });
    let report = filter_diagnostics(&report, invalid_only, plu, category);
    write_diagnostics_reports(
        Path::new("diagnostics-report.txt"),
        Path::new("diagnostics-report.json"),
        &report,
    )?;
    logger.kv("Text diagnostics report", "diagnostics-report.txt")?;
    logger.kv("JSON diagnostics report", "diagnostics-report.json")?;
    logger.line("Authentication attempted: NO")?;
    logger.line("DIGIweb API requests attempted: NO")?;
    logger.line("Source database modified: NO")?;
    logger.line("PLUs submitted: 0")?;
    log_duplicate_barcode_section(logger, &report.duplicate_barcode_groups)?;
    logger.final_import_summary(FinalImportLog {
        status: "DIAGNOSTICS_COMPLETE",
        source_discovered: source.dataset.plu_rows.len(),
        placeholders_ignored: source.placeholder_ignored,
        invalid_source_rows: source.invalid_source_rows,
        validation_skipped: source.validation_skipped,
        skipped_duplicate_barcode: report.summary.skipped_duplicate_barcode,
        normalized: source.plus.len(),
        valid: source.valid_plus.len(),
        selected: 0,
        submitted: 0,
        succeeded: 0,
        failed: 0,
        unknown: 0,
        not_attempted: 0,
        intentionally_skipped_by_limit: 0,
        successful_plu_numbers: &[],
        failed_plu_numbers: &[],
        unknown_plu_numbers: &[],
        dry_run: true,
    })?;
    print!("{}", render_diagnostics_console(&report));
    println!("Final status: DIAGNOSTICS_COMPLETE");
    Ok(0)
}

fn run_dry_run(
    config: &AppConfig,
    logger: &mut AuditLogger,
    limit: Option<usize>,
    requested_plu: Option<u64>,
    test_mode: bool,
    sanitize_profile_path: Option<&ProfileSelection>,
) -> Result<i32, AppError> {
    println!("DRY RUN");
    println!("NO API WRITES");
    println!("Outputs will be written to dry-run-report.txt, dry-run-manifest.json, and logs.txt");
    logger.line("DRY RUN")?;
    logger.line("NO API WRITES")?;
    logger.line("Network access permitted: NO")?;
    logger.line("Authentication attempted: NO")?;
    logger.line("DIGIweb API requests attempted: NO")?;
    logger.line("API write requests: 0")?;
    logger.line("DIGIweb modified: NO")?;
    logger.line("Source database modified: NO")?;
    let profile = load_optional_sanitization_profile(sanitize_profile_path)?;
    let started = Instant::now();
    let source = read_source_context(config, logger, profile, true)?;
    let selection = SelectionCriteria {
        limit,
        requested_plu,
        test_mode,
    };
    let selected_plus = select_eligible_plus(
        &source.plus,
        &source.valid_plus,
        &source.row_issues,
        &source.validation_report,
        selection,
    )
    .map_err(|failure| {
        let message = failure.message();
        let _ = logger.error(&message);
        selection_error(&failure)
    })?;
    let diagnostics = build_diagnostics_report(DiagnosticsInput {
        source_path: FIXED_SOURCE_FILE,
        source_sha256: &source.source_identity.sha256,
        started_at: chrono::Local::now(),
        finished_at: chrono::Local::now(),
        dataset: &source.dataset,
        all_normalized_plus: &source.plus,
        valid_plus: &source.valid_plus,
        row_issues: &source.row_issues,
        validation_report: &source.validation_report,
        timings: vec![DiagnosticTiming::from_duration(
            "Dry-run source read and normalization",
            started.elapsed(),
        )],
    });
    let manifest = build_dry_run_manifest(
        FIXED_SOURCE_FILE,
        &source.source_identity.sha256,
        &source.dataset,
        &source.plus,
        &source.valid_plus,
        &diagnostics,
        &config.digiweb,
        selection,
    )?;
    write_dry_run_payload_previews(config, &selected_plus)?;
    write_dry_run_outputs(
        Path::new("dry-run-report.txt"),
        Path::new("dry-run-manifest.json"),
        &manifest,
    )?;
    logger.kv("Dry-run report", "dry-run-report.txt")?;
    logger.kv("Dry-run manifest", "dry-run-manifest.json")?;
    logger.kv(
        "Skipped duplicate barcode",
        &manifest
            .summary
            .source_validation_findings
            .duplicate_barcode_plus
            .to_string(),
    )?;
    logger.line("API write requests: 0")?;
    logger.line("DIGIweb modified: NO")?;
    logger.line("PLUs submitted: 0")?;
    log_duplicate_barcode_section(logger, &diagnostics.duplicate_barcode_groups)?;
    logger.final_import_summary(FinalImportLog {
        status: if dry_run_selected_success(&manifest) {
            "DRY_RUN_SUCCESS"
        } else {
            "DRY_RUN_COMPLETED_WITH_ISSUES"
        },
        source_discovered: source.dataset.plu_rows.len(),
        placeholders_ignored: source.placeholder_ignored,
        invalid_source_rows: source.invalid_source_rows,
        validation_skipped: source.validation_skipped,
        skipped_duplicate_barcode: manifest
            .summary
            .source_validation_findings
            .duplicate_barcode_plus,
        normalized: source.plus.len(),
        valid: source.valid_plus.len(),
        selected: manifest.summary.selected,
        submitted: 0,
        succeeded: 0,
        failed: 0,
        unknown: 0,
        not_attempted: 0,
        intentionally_skipped_by_limit: 0,
        successful_plu_numbers: &[],
        failed_plu_numbers: &[],
        unknown_plu_numbers: &[],
        dry_run: true,
    })?;
    print!("{}", render_dry_run_console(&manifest));
    let exit_code = if dry_run_selected_success(&manifest) {
        0
    } else {
        1
    };
    println!(
        "Final status: {}",
        if exit_code == 0 {
            "DRY_RUN_SUCCESS"
        } else {
            "DRY_RUN_COMPLETED_WITH_ISSUES"
        }
    );
    Ok(exit_code)
}

fn dry_run_selected_success(manifest: &diagnostics::DryRunManifest) -> bool {
    manifest.summary.selection.selected_payload_failures == 0
        && manifest.summary.selection.selected_invalid_skips == 0
        && manifest.summary.selection.selected_duplicate_skips == 0
        && manifest.summary.selection.would_submit == manifest.summary.selection.selected
}

fn write_dry_run_payload_previews(config: &AppConfig, selected: &[&Plu]) -> Result<(), AppError> {
    if !config.import.write_payload_preview {
        return Ok(());
    }
    let preview_dir = Path::new("payload-previews");
    if preview_dir.exists() {
        std::fs::remove_dir_all(preview_dir).map_err(|err| {
            AppError::Internal(format!("failed to clean payload preview directory: {err}"))
        })?;
    }
    std::fs::create_dir_all(preview_dir).map_err(|err| {
        AppError::Internal(format!("failed to create payload preview directory: {err}"))
    })?;
    for plu in selected {
        let payload = DigiwebPluPayload::from_plu(plu, &config.digiweb)?;
        let json = serde_json::to_string_pretty(&payload)
            .map_err(|err| AppError::Internal(format!("payload serialization failed: {err}")))?;
        std::fs::write(
            preview_dir.join(format!("plu-{}.json", plu.plu_number)),
            json,
        )
        .map_err(|err| AppError::Internal(format!("failed to write payload preview: {err}")))?;
    }
    Ok(())
}

fn log_duplicate_barcode_section(
    logger: &mut AuditLogger,
    groups: &[diagnostics::DuplicateBarcodeGroup],
) -> Result<(), AppError> {
    if groups.is_empty() {
        return Ok(());
    }
    logger.line("SKIPPED - DUPLICATE BARCODE")?;
    for group in groups {
        logger.line(format!("Barcode: {}", group.effective_barcode))?;
        logger.line(format!("Kept PLU: {}", group.canonical_plu))?;
        logger.line(format!("Skipped PLUs: {:?}", group.skipped_plus))?;
        logger.line("Customer action: Assign unique valid barcodes to the skipped PLUs and add/reimport them manually.")?;
    }
    Ok(())
}

fn run_sanitize(
    config: &AppConfig,
    logger: &mut AuditLogger,
    profile_selection: &ProfileSelection,
) -> Result<i32, AppError> {
    println!("Starting sanitization preview...");
    println!(
        "Outputs will be written to sanitization-report.txt, sanitization-report.json, and logs.txt"
    );
    logger.line("SANITIZATION PREVIEW")?;
    logger.line("Network access permitted: NO")?;
    logger.line("Authentication attempted: NO")?;
    logger.line("DIGIweb API requests attempted: NO")?;
    logger.line("Source database modified: NO")?;
    let profile = load_sanitization_profile(profile_selection)?;
    let source = read_source_context(config, logger, Some(profile), true)?;
    let sanitization = source.sanitization.as_ref().ok_or_else(|| {
        AppError::Internal("sanitize command did not produce a sanitization report".to_string())
    })?;
    let report = sanitization::report::build_report(SanitizationReportInput {
        source: source.source_identity.clone(),
        profile: &sanitization.profile,
        profile_sha256: &sanitization.profile_sha256,
        engine_report: &sanitization.engine_report,
        before_valid: sanitization.before_valid,
        before_invalid: sanitization.before_invalid,
        after_valid: sanitization.after_valid,
        after_invalid: sanitization.after_invalid,
        recovered_plus: sanitization.recovered_plus,
        still_invalid_plus: &sanitization.still_invalid_plus,
    });
    write_sanitization_reports(
        Path::new("sanitization-report.txt"),
        Path::new("sanitization-report.json"),
        Path::new("sanitization-profile.snapshot.toml"),
        &report,
        &sanitization.normalized_profile_toml,
    )?;
    logger.kv("Text sanitization report", "sanitization-report.txt")?;
    logger.kv("JSON sanitization report", "sanitization-report.json")?;
    logger.kv(
        "Sanitization profile snapshot",
        "sanitization-profile.snapshot.toml",
    )?;
    print_sanitization_summary(&report);
    let exit_code = if report.summary.still_invalid_plus == 0 {
        0
    } else {
        1
    };
    println!(
        "Final status: {}",
        if exit_code == 0 {
            "PASS"
        } else {
            "PASS_WITH_WARNINGS"
        }
    );
    Ok(exit_code)
}

fn load_optional_sanitization_profile(
    path: Option<&ProfileSelection>,
) -> Result<Option<SanitizationProfile>, AppError> {
    path.map(load_sanitization_profile).transpose()
}

fn profile_for_import_or_resume(
    resume_manifest: Option<&Path>,
    sanitize_profile_path: Option<&ProfileSelection>,
) -> Result<Option<SanitizationProfile>, AppError> {
    if let Some(path) = resume_manifest {
        if sanitize_profile_path.is_some() {
            return Err(AppError::Config(
                "--resume cannot be combined with --sanitize-profile; the manifest controls the original sanitization profile.".to_string(),
            ));
        }
        let manifest = recovery::load_manifest(path)?;
        if !manifest.sanitization.enabled {
            return Ok(None);
        }
        let snapshot_name = manifest
            .sanitization
            .profile_snapshot
            .as_deref()
            .unwrap_or("");
        let snapshot_path = path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(snapshot_name);
        validate_profile_path(&snapshot_path).map_err(|_| {
            AppError::Config(
                "The sanitization profile recorded by this manifest is missing or has changed.\n\nResume was cancelled before authentication or API submission."
                    .to_string(),
            )
        })?;
        let snapshot_contents = std::fs::read_to_string(&snapshot_path).map_err(|_| {
            AppError::Config(
                "The sanitization profile recorded by this manifest is missing or has changed.\n\nResume was cancelled before authentication or API submission."
                    .to_string(),
            )
        })?;
        let actual_hash = sanitization::report::sha256_text(&snapshot_contents);
        if Some(actual_hash) != manifest.sanitization.profile_sha256 {
            return Err(AppError::Config(
                "The sanitization profile recorded by this manifest is missing or has changed.\n\nResume was cancelled before authentication or API submission."
                    .to_string(),
            ));
        }
        let profile: SanitizationProfile = toml::from_str(&snapshot_contents).map_err(|_| {
            AppError::Config(
                "The sanitization profile recorded by this manifest is missing or has changed.\n\nResume was cancelled before authentication or API submission."
                    .to_string(),
            )
        })?;
        profile.validate()?;
        Ok(Some(profile))
    } else {
        load_optional_sanitization_profile(sanitize_profile_path)
    }
}

fn load_sanitization_profile(
    selection: &ProfileSelection,
) -> Result<SanitizationProfile, AppError> {
    match selection {
        ProfileSelection::External(path) => load_profile_from_safe_path(path),
        ProfileSelection::BuiltIn(name) => load_builtin_profile(name),
    }
}

fn load_builtin_profile(name: &str) -> Result<SanitizationProfile, AppError> {
    let contents = match name {
        "starsky" => include_str!("../profiles/starsky.toml"),
        other => {
            return Err(AppError::Config(format!(
                "unknown built-in sanitization profile '{other}'"
            )));
        }
    };
    let profile: SanitizationProfile = toml::from_str(contents)
        .map_err(|err| AppError::Config(format!("invalid built-in profile '{name}': {err}")))?;
    profile.validate()?;
    Ok(profile)
}

fn print_sanitization_summary(report: &sanitization::SanitizationReport) {
    println!("SANITIZATION ANALYSIS COMPLETE");
    println!();
    println!("Profile: {}", report.profile.name);
    println!("Source PLUs: {}", report.summary.source_plus);
    println!();
    println!("PLUs requiring changes: {}", report.summary.changed_plus);
    println!("PLUs unchanged: {}", report.summary.unchanged_plus);
    println!("Placeholder PLUs: {}", report.summary.placeholder_plus);
    println!();
    println!("Proposed changes:");
    for (field, summary) in &report.field_summaries {
        println!(
            "- {}: {} changed, {} empty defaults, {} invalid nonempty corrected, {} valid preserved",
            field,
            summary.changed,
            summary.empty_defaulted,
            summary.invalid_nonempty_corrected,
            summary.valid_preserved
        );
    }
    for (field, count) in &report.field_changes {
        if !report.field_summaries.contains_key(field) {
            println!("- {}: {} PLUs", field, count);
        }
    }
    println!();
    println!("Before sanitization:");
    println!("- Valid PLUs: {}", report.summary.before_valid_plus);
    println!("- Invalid PLUs: {}", report.summary.before_invalid_plus);
    println!();
    println!("After sanitization:");
    println!("- Valid PLUs: {}", report.summary.after_valid_plus);
    println!("- Still invalid: {}", report.summary.still_invalid_plus);
    println!();
    println!("Recovered by profile: {}", report.summary.recovered_plus);
    println!(
        "Existing nonempty values changed: {}",
        report.summary.nonempty_values_changed
    );
    println!();
    println!("plu.mdb modified: NO");
    println!("Authentication attempted: NO");
    println!("DIGIweb API requests attempted: NO");
    println!();
    println!("Text sanitization report:");
    println!("sanitization-report.txt");
    println!("JSON sanitization report:");
    println!("sanitization-report.json");
    println!("Profile snapshot:");
    println!("sanitization-profile.snapshot.toml");
}

fn counted_invalid_rows(
    normalization_report: &source::mapping::NormalizationReport,
    validation_report: &ValidationReport,
    valid_count: usize,
) -> usize {
    let placeholder_ignored = normalization_report
        .row_issues
        .iter()
        .filter(|issue| is_empty_placeholder_issue(issue))
        .count();
    let invalid_source_rows = normalization_report
        .row_issues
        .len()
        .saturating_sub(placeholder_ignored);
    let validation_skipped = normalization_report.plus.len().saturating_sub(valid_count);
    counted_invalid_rows_from_counts(invalid_source_rows, validation_skipped)
        + validation_report
            .issues
            .iter()
            .filter(|issue| issue.severity == Severity::Error && issue.plu_number.is_none())
            .count()
}

fn counted_invalid_rows_from_counts(
    invalid_source_rows: usize,
    validation_skipped: usize,
) -> usize {
    invalid_source_rows + validation_skipped
}

fn still_invalid_plu_numbers(
    normalization_report: &source::mapping::NormalizationReport,
    validation_report: &ValidationReport,
    valid_plus: &[Plu],
) -> Vec<u64> {
    still_invalid_plu_numbers_from_issues(
        &normalization_report.row_issues,
        validation_report,
        valid_plus,
    )
}

fn still_invalid_plu_numbers_from_issues(
    row_issues: &[validation::issue::ValidationIssue],
    validation_report: &ValidationReport,
    valid_plus: &[Plu],
) -> Vec<u64> {
    let valid = valid_plus
        .iter()
        .map(|plu| plu.plu_number)
        .collect::<std::collections::BTreeSet<_>>();
    let mut values = row_issues
        .iter()
        .filter_map(|issue| issue.plu_number)
        .chain(
            validation_report
                .issues
                .iter()
                .filter_map(|issue| issue.plu_number),
        )
        .filter(|plu| !valid.contains(plu))
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    values.sort_unstable();
    values
}

async fn run_import_command(
    config: &AppConfig,
    limit: Option<usize>,
    requested_plu: Option<u64>,
    continue_on_error: bool,
    test_mode: bool,
    resume_manifest: Option<&Path>,
    retry_failed: bool,
    sanitize_profile_path: Option<&ProfileSelection>,
    logger: &mut AuditLogger,
) -> Result<i32, AppError> {
    println!("Starting import...");
    println!("Outputs will be written under output/ when launched with ./to-digi.");
    let profile = profile_for_import_or_resume(resume_manifest, sanitize_profile_path)?;
    let source = read_source_context(config, logger, profile, true)?;
    let selection = SelectionCriteria {
        limit,
        requested_plu,
        test_mode,
    };
    if resume_manifest.is_none() {
        select_eligible_plus(
            &source.plus,
            &source.valid_plus,
            &source.row_issues,
            &source.validation_report,
            selection,
        )
        .map_err(|failure| {
            let message = failure.message();
            let _ = logger.error(&message);
            selection_error(&failure)
        })?;
    }
    if source.valid_plus.is_empty() {
        logger.final_failure("validation", "no valid PLUs are available to send", true)?;
        return Ok(2);
    }
    if source
        .validation_report
        .issues
        .iter()
        .any(|issue| issue.severity == Severity::Warning)
    {
        logger.line("Validation warnings are present; continuing because no blocking validation errors were found.")?;
    }

    let target_identity = target_identity(config);
    let manifest_path = manifest_path_from_environment(resume_manifest)?;
    if resume_manifest.is_none() {
        logger.kv("Import manifest", &manifest_path.display().to_string())?;
    }

    let summary = run_import(
        config.clone(),
        &source.valid_plus,
        source.source_identity.clone(),
        target_identity,
        &manifest_path,
        resume_manifest,
        source.sanitization.clone(),
        ImportRunOptions {
            limit,
            requested_plu,
            continue_after_record_failure: continue_on_error,
            test_mode,
            retry_failed,
        },
        logger,
    )
    .await?;
    for record in &summary.records {
        if matches!(
            record.final_status,
            digiweb::status::ProcessingStatus::SubmittedStatusUnknown
                | digiweb::status::ProcessingStatus::UnknownOrTimeout
        ) {
            logger.line(format!(
                "UNKNOWN RECORD: PLU {} started={} request_id={} http_result={} status={} duration_ms={} message={}",
                record.plu_number,
                record.started_at.to_rfc3339(),
                record.api_request_id.as_deref().unwrap_or("n/a"),
                record.http_result,
                record.final_status.as_str(),
                record.duration_ms,
                record.failure_message.as_deref().unwrap_or("n/a")
            ))?;
        } else if record.final_status != digiweb::status::ProcessingStatus::Success {
            logger.line(format!(
                "FAILED RECORD: PLU {} started={} request_id={} http_result={} status={} duration_ms={} message={}",
                record.plu_number,
                record.started_at.to_rfc3339(),
                record.api_request_id.as_deref().unwrap_or("n/a"),
                record.http_result,
                record.final_status.as_str(),
                record.duration_ms,
                record.failure_message.as_deref().unwrap_or("n/a")
            ))?;
        }
    }
    let final_status = summary.final_status();
    let successful_plu_numbers = summary
        .records
        .iter()
        .filter(|record| record.final_status == digiweb::status::ProcessingStatus::Success)
        .map(|record| record.plu_number)
        .collect::<Vec<_>>();
    let failed_plu_numbers = summary
        .records
        .iter()
        .filter(|record| record.final_status == digiweb::status::ProcessingStatus::Fail)
        .map(|record| record.plu_number)
        .collect::<Vec<_>>();
    let unknown_plu_numbers = summary
        .records
        .iter()
        .filter(|record| {
            matches!(
                record.final_status,
                digiweb::status::ProcessingStatus::SubmittedStatusUnknown
                    | digiweb::status::ProcessingStatus::UnknownOrTimeout
            )
        })
        .map(|record| record.plu_number)
        .collect::<Vec<_>>();
    logger.final_import_summary(FinalImportLog {
        status: final_status.as_str(),
        source_discovered: source.dataset.plu_rows.len(),
        placeholders_ignored: source.placeholder_ignored,
        invalid_source_rows: source.invalid_source_rows,
        validation_skipped: source.validation_skipped,
        skipped_duplicate_barcode: skipped_duplicate_barcode_count(&source),
        normalized: source.plus.len(),
        valid: summary.discovered,
        selected: summary.selected,
        submitted: summary.submitted,
        succeeded: summary.succeeded,
        failed: summary.failed,
        unknown: summary.unknown,
        not_attempted: summary.not_attempted_after_stop,
        intentionally_skipped_by_limit: summary.intentionally_skipped_by_limit,
        successful_plu_numbers: &successful_plu_numbers,
        failed_plu_numbers: &failed_plu_numbers,
        unknown_plu_numbers: &unknown_plu_numbers,
        dry_run: false,
    })?;
    println!("Final status: {}", final_status.as_str());
    Ok(final_status.exit_code())
}

async fn run_test_connection(
    config: &AppConfig,
    logger: &mut AuditLogger,
) -> Result<i32, AppError> {
    println!("Testing DIGIweb connection...");
    println!("Target: {}", config.digiweb.base_url);
    println!("Authentication attempt started");
    println!("Outputs will be written to logs.txt");
    validate_connection_urls(config)?;
    let client_secret = load_client_secret(config)?;
    logger.kv(
        "Client secret",
        client_secret_log_message(config, environment_secret_present()),
    )?;
    let started = Instant::now();
    let client = DigiwebClient::new(config.clone())?;
    match authenticate(client.http(), config, &client_secret).await {
        Ok(_) => {
            println!("Authentication: SUCCESS");
        }
        Err(err) => {
            println!("Authentication: FAILED");
            println!("Reason: {}", err);
            println!("Result: FAIL");
            return Err(err);
        }
    }
    logger.line("DIGIweb connection test: SUCCESS")?;
    logger.kv("Base URL reachable", "yes")?;
    logger.kv("Authentication successful", "yes")?;
    logger.line("No PLU data was submitted.")?;
    logger.kv("Elapsed ms", &started.elapsed().as_millis().to_string())?;
    logger.flush()?;
    println!("DIGIweb connectivity: SUCCESS");
    println!("Result: PASS");
    Ok(0)
}

async fn run_verify(
    config: &AppConfig,
    logger: &mut AuditLogger,
    sanitize_profile_path: Option<&ProfileSelection>,
) -> Result<i32, AppError> {
    let started_at = chrono::Local::now();
    println!("Starting import readiness verification...");
    println!("Outputs will be written to logs.txt, verify-report.txt, and verify-report.json");
    logger.line("Verify scope: import-readiness verification only; no source-versus-DIGIweb post-import comparison is attempted.")?;
    let profile = load_optional_sanitization_profile(sanitize_profile_path)?;
    let source = read_source_context(config, logger, profile, false)?;
    for plu in &source.valid_plus {
        DigiwebPluPayload::from_plu(plu, &config.digiweb)?;
    }
    let excluded_plu_numbers = excluded_plu_numbers(&source);
    let excluded_count = excluded_plu_numbers.len();
    let reference_readiness =
        evaluate_reference_readiness(&source.valid_plus, &config.verification, excluded_count)?;
    logger.kv("Source PLUs", &source.dataset.plu_rows.len().to_string())?;
    logger.kv("Eligible PLUs", &source.valid_plus.len().to_string())?;
    logger.kv(
        "Customer-action-required / excluded",
        &excluded_count.to_string(),
    )?;
    logger.kv("Eligible payload validation", "PASSED")?;
    logger.kv(
        "Source contains excluded issues",
        if excluded_count > 0 { "YES" } else { "NO" },
    )?;
    validate_connection_urls(config)?;
    let client_secret = load_client_secret(config)?;
    logger.kv(
        "Client secret",
        client_secret_log_message(config, environment_secret_present()),
    )?;
    let client = DigiwebClient::new(config.clone())?;
    let auth_result = authenticate(client.http(), config, &client_secret).await;
    let authentication = if auth_result.is_ok() {
        AuthenticationReadinessStatus::Passed
    } else {
        AuthenticationReadinessStatus::Failed
    };
    logger.kv("DIGIweb authentication", authentication.as_str())?;
    if let Some(sanitization) = &source.sanitization {
        logger.kv("Sanitization profile", &sanitization.profile.profile_name)?;
        logger.kv("Sanitization profile hash", &sanitization.profile_sha256)?;
        logger.kv(
            "PLUs recovered by sanitization",
            &sanitization.recovered_plus.to_string(),
        )?;
        logger.kv(
            "PLUs still invalid after sanitization",
            &sanitization.after_invalid.to_string(),
        )?;
    }
    log_reference_readiness(logger, &reference_readiness)?;
    logger.kv("Write operation attempted", "NO")?;
    logger.kv("PLU write requests", "0")?;
    let finished_at = chrono::Local::now();
    let readiness_result = final_readiness_result(authentication, &reference_readiness);
    let verify_report = build_verify_report(VerifyReportInput {
        source: &source,
        config,
        authentication,
        reference_readiness: &reference_readiness,
        readiness_result,
        excluded_plu_numbers: &excluded_plu_numbers,
        started_at,
        finished_at,
    });
    write_verify_reports(&verify_report)?;
    logger.kv("Verify report", "verify-report.txt")?;
    logger.kv("Verify JSON report", "verify-report.json")?;

    if let Err(err) = auth_result {
        logger.kv("IMPORT READINESS", readiness_result.as_str())?;
        logger.flush()?;
        return Err(err);
    }

    if readiness_result == ReadinessResult::NotReady {
        logger.line("VERIFY RESULT: NOT READY")?;
        logger.line("Import readiness cannot be confirmed safely until every required Department, Group, and effective Label Format is manually confirmed in [verification].")?;
        logger.kv("IMPORT READINESS", "NOT_READY_UNVERIFIED_REFERENCE")?;
        logger.flush()?;
        println!("VERIFY RESULT: NOT READY");
        print_unverified_references(&reference_readiness);
        println!("Final status: NOT_READY_UNVERIFIED_REFERENCE");
        return Ok(1);
    }
    logger.line(format!("VERIFY RESULT: {}", readiness_result.as_str()))?;
    logger.kv("IMPORT READINESS", readiness_result.as_str())?;
    logger.flush()?;
    println!("VERIFY RESULT: {}", readiness_result.as_str());
    println!("Final status: {}", readiness_result.as_str());
    Ok(0)
}

struct VerifyReportInput<'a> {
    source: &'a SourceContext,
    config: &'a AppConfig,
    authentication: AuthenticationReadinessStatus,
    reference_readiness: &'a ReferenceReadiness,
    readiness_result: ReadinessResult,
    excluded_plu_numbers: &'a [u64],
    started_at: chrono::DateTime<chrono::Local>,
    finished_at: chrono::DateTime<chrono::Local>,
}

#[derive(Debug, Clone, Serialize)]
struct VerifyReport {
    schema_version: u32,
    application_version: String,
    command: String,
    generated_at: String,
    started_at: String,
    finished_at: String,
    source_path: String,
    source_sha256: String,
    target_url: String,
    store_number: u32,
    authentication: String,
    source_plu_count: usize,
    eligible_plu_count: usize,
    excluded_customer_action_count: usize,
    excluded_customer_action_plus: Vec<u64>,
    references: VerifyReferenceSection,
    unverified_reference_count: usize,
    stale_confirmations: Vec<digiweb::preflight::StaleConfirmation>,
    readiness: String,
    safety: VerifySafety,
}

#[derive(Debug, Clone, Serialize)]
struct VerifyReferenceSection {
    departments: Vec<digiweb::preflight::DepartmentReadiness>,
    groups: Vec<digiweb::preflight::GroupReadiness>,
    label_formats: Vec<digiweb::preflight::LabelFormatReadiness>,
}

#[derive(Debug, Clone, Serialize)]
struct VerifySafety {
    write_operation_attempted: bool,
    plu_write_requests: usize,
    source_database_modified: bool,
}

fn build_verify_report(input: VerifyReportInput<'_>) -> VerifyReport {
    VerifyReport {
        schema_version: 1,
        application_version: env!("CARGO_PKG_VERSION").to_string(),
        command: "verify".to_string(),
        generated_at: chrono::Local::now().to_rfc3339(),
        started_at: input.started_at.to_rfc3339(),
        finished_at: input.finished_at.to_rfc3339(),
        source_path: FIXED_SOURCE_FILE.to_string(),
        source_sha256: input.source.source_identity.sha256.clone(),
        target_url: input.config.digiweb.base_url.clone(),
        store_number: input.config.digiweb.store_number,
        authentication: input.authentication.as_str().to_string(),
        source_plu_count: input.source.dataset.plu_rows.len(),
        eligible_plu_count: input.source.valid_plus.len(),
        excluded_customer_action_count: input.excluded_plu_numbers.len(),
        excluded_customer_action_plus: input.excluded_plu_numbers.to_vec(),
        references: VerifyReferenceSection {
            departments: input.reference_readiness.departments.clone(),
            groups: input.reference_readiness.groups.clone(),
            label_formats: input.reference_readiness.label_formats.clone(),
        },
        unverified_reference_count: input.reference_readiness.unverified_reference_count,
        stale_confirmations: input.reference_readiness.stale_confirmations.clone(),
        readiness: input.readiness_result.as_str().to_string(),
        safety: VerifySafety {
            write_operation_attempted: false,
            plu_write_requests: 0,
            source_database_modified: false,
        },
    }
}

fn write_verify_reports(report: &VerifyReport) -> Result<(), AppError> {
    fs::write("verify-report.txt", render_verify_report_text(report))
        .map_err(|err| AppError::Internal(format!("failed to write verify report: {err}")))?;
    let json = serde_json::to_string_pretty(report)
        .map_err(|err| AppError::Internal(format!("verify JSON serialization failed: {err}")))?;
    fs::write("verify-report.json", json)
        .map_err(|err| AppError::Internal(format!("failed to write verify JSON: {err}")))?;
    Ok(())
}

fn render_verify_report_text(report: &VerifyReport) -> String {
    let mut out = String::new();
    push_line(&mut out, "VERIFY READINESS REPORT");
    push_line(&mut out, "");
    push_kv(&mut out, "Application version", &report.application_version);
    push_kv(&mut out, "Command", &report.command);
    push_kv(&mut out, "Source path", &report.source_path);
    push_kv(&mut out, "Source SHA-256", &report.source_sha256);
    push_kv(&mut out, "Target URL", &report.target_url);
    push_kv(&mut out, "Store number", &report.store_number.to_string());
    push_kv(&mut out, "Authentication", &report.authentication);
    push_kv(
        &mut out,
        "Source PLUs",
        &report.source_plu_count.to_string(),
    );
    push_kv(
        &mut out,
        "Eligible PLUs",
        &report.eligible_plu_count.to_string(),
    );
    push_kv(
        &mut out,
        "Customer-action-required / excluded",
        &report.excluded_customer_action_count.to_string(),
    );
    push_kv(
        &mut out,
        "Excluded PLUs",
        &join_u64s(&report.excluded_customer_action_plus),
    );
    push_line(&mut out, "");
    push_line(&mut out, "DEPARTMENTS");
    for department in &report.references.departments {
        push_line(
            &mut out,
            format!(
                "Department {} | Status: {} | Used by: {} PLUs",
                department.number,
                department.status.as_str(),
                department.source_plu_numbers.len()
            ),
        );
        push_line(
            &mut out,
            format!("PLUs: {}", join_u64s(&department.source_plu_numbers)),
        );
    }
    push_line(&mut out, "");
    push_line(&mut out, "GROUPS");
    for group in &report.references.groups {
        push_line(
            &mut out,
            format!(
                "Department {} / Group {} | Status: {} | Used by: {} PLUs",
                group.department,
                group.number,
                group.status.as_str(),
                group.source_plu_numbers.len()
            ),
        );
        push_line(
            &mut out,
            format!("PLUs: {}", join_u64s(&group.source_plu_numbers)),
        );
    }
    push_line(&mut out, "");
    push_line(&mut out, "EFFECTIVE LABEL FORMATS");
    for label_format in &report.references.label_formats {
        push_line(
            &mut out,
            format!(
                "Label Format {} | Status: {} | Used by: {} PLUs",
                label_format.number,
                label_format.status.as_str(),
                label_format.source_plu_numbers.len()
            ),
        );
        push_line(
            &mut out,
            format!("PLUs: {}", join_u64s(&label_format.source_plu_numbers)),
        );
    }
    if !report.stale_confirmations.is_empty() {
        push_line(&mut out, "");
        push_line(&mut out, "UNUSED / STALE CONFIRMATIONS");
        for warning in &report.stale_confirmations {
            push_line(&mut out, &warning.message);
        }
    }
    push_line(&mut out, "");
    push_kv(
        &mut out,
        "Unverified reference count",
        &report.unverified_reference_count.to_string(),
    );
    push_kv(&mut out, "Readiness", &report.readiness);
    push_line(&mut out, "");
    push_line(&mut out, "SAFETY");
    push_kv(
        &mut out,
        "Write operation attempted",
        if report.safety.write_operation_attempted {
            "YES"
        } else {
            "NO"
        },
    );
    push_kv(
        &mut out,
        "PLU write requests",
        &report.safety.plu_write_requests.to_string(),
    );
    push_kv(
        &mut out,
        "Source database modified",
        if report.safety.source_database_modified {
            "YES"
        } else {
            "NO"
        },
    );
    push_kv(&mut out, "Started", &report.started_at);
    push_kv(&mut out, "Finished", &report.finished_at);
    out
}

fn log_reference_readiness(
    logger: &mut AuditLogger,
    readiness: &ReferenceReadiness,
) -> Result<(), AppError> {
    logger.kv(
        "DIGIweb department/group existence",
        reference_group_summary(readiness),
    )?;
    logger.kv(
        "DIGIweb label format existence",
        reference_label_summary(readiness),
    )?;
    logger.line("Required reference readiness:")?;
    for department in &readiness.departments {
        logger.line(format!(
            "Department {}                  {} | Used by: {} PLUs | Examples: {}",
            department.number,
            department.status.as_str(),
            department.source_plu_numbers.len(),
            format_limited_u64s(&department.source_plu_numbers, 8)
        ))?;
    }
    for group in &readiness.groups {
        logger.line(format!(
            "Department {} / Group {}      {} | Used by: {} PLUs | Examples: {}",
            group.department,
            group.number,
            group.status.as_str(),
            group.source_plu_numbers.len(),
            format_limited_u64s(&group.source_plu_numbers, 8)
        ))?;
    }
    for label_format in &readiness.label_formats {
        logger.line(format!(
            "Label Format {}                {} | Used by: {} PLUs | Examples: {}",
            label_format.number,
            label_format.status.as_str(),
            label_format.source_plu_numbers.len(),
            format_limited_u64s(&label_format.source_plu_numbers, 8)
        ))?;
    }
    for warning in &readiness.stale_confirmations {
        logger.warning(&warning.message)?;
    }
    Ok(())
}

fn reference_group_summary(readiness: &ReferenceReadiness) -> &'static str {
    let unverified_departments = readiness
        .departments
        .iter()
        .any(|reference| reference.status == ReferenceConfirmationStatus::Unverified);
    let unverified_groups = readiness
        .groups
        .iter()
        .any(|reference| reference.status == ReferenceConfirmationStatus::Unverified);
    if unverified_departments || unverified_groups {
        "UNVERIFIED"
    } else {
        "MANUALLY_CONFIRMED"
    }
}

fn reference_label_summary(readiness: &ReferenceReadiness) -> &'static str {
    if readiness
        .label_formats
        .iter()
        .any(|reference| reference.status == ReferenceConfirmationStatus::Unverified)
    {
        "UNVERIFIED"
    } else {
        "MANUALLY_CONFIRMED"
    }
}

fn final_readiness_result(
    authentication: AuthenticationReadinessStatus,
    references: &ReferenceReadiness,
) -> ReadinessResult {
    if authentication != AuthenticationReadinessStatus::Passed || !references.is_ready() {
        ReadinessResult::NotReady
    } else {
        references.result
    }
}

fn print_unverified_references(readiness: &ReferenceReadiness) {
    println!();
    println!("Unverified required references:");
    for department in readiness
        .departments
        .iter()
        .filter(|reference| reference.status == ReferenceConfirmationStatus::Unverified)
    {
        println!(
            "- Department {} | Used by: {} PLUs | Examples: {}",
            department.number,
            department.source_plu_numbers.len(),
            format_limited_u64s(&department.source_plu_numbers, 8)
        );
    }
    for group in readiness
        .groups
        .iter()
        .filter(|reference| reference.status == ReferenceConfirmationStatus::Unverified)
    {
        println!(
            "- Department {} / Group {} | Used by: {} PLUs | Examples: {}",
            group.department,
            group.number,
            group.source_plu_numbers.len(),
            format_limited_u64s(&group.source_plu_numbers, 8)
        );
    }
    for label_format in readiness
        .label_formats
        .iter()
        .filter(|reference| reference.status == ReferenceConfirmationStatus::Unverified)
    {
        println!(
            "- Label Format {} | Used by: {} PLUs | Examples: {}",
            label_format.number,
            label_format.source_plu_numbers.len(),
            format_limited_u64s(&label_format.source_plu_numbers, 8)
        );
    }
}

fn excluded_plu_numbers(source: &SourceContext) -> Vec<u64> {
    let valid = source
        .valid_plus
        .iter()
        .map(|plu| plu.plu_number)
        .collect::<std::collections::BTreeSet<_>>();
    let mut excluded = source
        .row_issues
        .iter()
        .filter_map(|issue| issue.plu_number)
        .chain(
            source
                .validation_report
                .issues
                .iter()
                .filter_map(|issue| issue.plu_number),
        )
        .filter(|plu| !valid.contains(plu))
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    excluded.sort_unstable();
    excluded
}

fn join_u64s(values: &[u64]) -> String {
    if values.is_empty() {
        "none".to_string()
    } else {
        values
            .iter()
            .map(u64::to_string)
            .collect::<Vec<_>>()
            .join(", ")
    }
}

fn push_line(out: &mut String, value: impl AsRef<str>) {
    out.push_str(value.as_ref());
    out.push('\n');
}

fn push_kv(out: &mut String, key: &str, value: &str) {
    push_line(out, format!("{key}: {value}"));
}

fn validate_connection_urls(config: &AppConfig) -> Result<(), AppError> {
    reqwest::Url::parse(&config.digiweb.base_url)
        .map_err(|err| AppError::Config(format!("invalid digiweb.base_url: {err}")))?;
    let token_url = config.token_url()?;
    reqwest::Url::parse(&token_url)
        .map_err(|err| AppError::Config(format!("invalid digiweb.token_url: {err}")))?;
    Ok(())
}

fn environment_secret_present() -> bool {
    std::env::var("TO_DIGI_RS_CLIENT_SECRET")
        .ok()
        .filter(|value| !value.is_empty())
        .is_some()
        || std::env::var("DIGIWEB_CLIENT_SECRET")
            .ok()
            .filter(|value| !value.is_empty())
            .is_some()
        || std::env::var("TO_DIGI_RS_CLIENT_SECRET_FILE")
            .ok()
            .filter(|value| !value.is_empty())
            .is_some()
}

fn format_limited_u64s(values: &[u64], limit: usize) -> String {
    if values.is_empty() {
        return "none".to_string();
    }
    let shown = values
        .iter()
        .take(limit)
        .map(|value| value.to_string())
        .collect::<Vec<_>>()
        .join(", ");
    if values.len() > limit {
        format!("{shown}, ...")
    } else {
        shown
    }
}

fn manifest_path_from_environment(resume_manifest: Option<&Path>) -> Result<PathBuf, AppError> {
    if let Some(path) = resume_manifest {
        return Ok(path.to_path_buf());
    }
    std::env::var("TO_DIGI_RS_IMPORT_MANIFEST_PATH")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .map(PathBuf::from)
        .or_else(|| Some(PathBuf::from(DEFAULT_MANIFEST_PATH)))
        .ok_or_else(|| AppError::Config("import manifest path could not be resolved".to_string()))
}

struct SourceContext {
    schema: MdbSchema,
    dataset: SourceDataset,
    plus: Vec<Plu>,
    valid_plus: Vec<Plu>,
    validation_report: ValidationReport,
    source_identity: SourceIdentity,
    sanitization: Option<SanitizationIntegration>,
    placeholder_ignored: usize,
    invalid_source_rows: usize,
    validation_skipped: usize,
    orphan_pluing_rows: usize,
    explicit_group_references: usize,
    defaulted_group_references: usize,
    invalid_group_values: usize,
    row_issues: Vec<validation::issue::ValidationIssue>,
    source_file_size_bytes: u64,
    source_is_symlink: bool,
    source_opened_read_only: bool,
    reference_tables: Vec<ReferenceTableSnapshot>,
    nutrition_fallback_to_pluing: bool,
    nutrition_source_table: String,
}

fn build_analysis_report(source: &SourceContext) -> analysis::model::AnalysisReport {
    collect_analysis(AnalysisInput {
        source_filename: FIXED_SOURCE_FILE,
        source_file_size_bytes: source.source_file_size_bytes,
        source_is_symlink: source.source_is_symlink,
        source_opened_read_only: source.source_opened_read_only,
        mdb_tables: &source.schema.tables,
        dataset: &source.dataset,
        valid_plus: &source.valid_plus,
        all_normalized_plus: &source.plus,
        row_issues: &source.row_issues,
        validation_report: &source.validation_report,
        placeholder_ignored: source.placeholder_ignored,
        invalid_source_rows: source.invalid_source_rows,
        validation_skipped: source.validation_skipped,
        orphan_pluing_rows: source.orphan_pluing_rows,
        explicit_group_references: source.explicit_group_references,
        defaulted_group_references: source.defaulted_group_references,
        invalid_group_values: source.invalid_group_values,
        reference_tables: &source.reference_tables,
        nutrition_fallback_to_pluing: source.nutrition_fallback_to_pluing,
        nutrition_source_table: &source.nutrition_source_table,
        sanitization: source.sanitization.as_ref(),
    })
}

fn skipped_duplicate_barcode_count(source: &SourceContext) -> usize {
    source
        .validation_report
        .issues
        .iter()
        .filter(|issue| {
            issue.severity == Severity::Error
                && issue.field == "barcode"
                && issue.message.contains("duplicate barcode")
        })
        .count()
}

fn read_reference_tables(
    source_path: &Path,
    schema: &mut MdbSchema,
    logger: &mut AuditLogger,
) -> Result<Vec<ReferenceTableSnapshot>, AppError> {
    let mut tables = Vec::new();
    for table_name in ["Department", "Maingroup"] {
        if schema.has_table(table_name) {
            let (columns, rows) = MdbTools::export_table(source_path, table_name)?;
            schema.set_columns(table_name, columns.clone());
            logger.kv(
                &format!("Rows in source reference table {table_name}"),
                &rows.len().to_string(),
            )?;
            tables.push(ReferenceTableSnapshot {
                name: table_name.to_string(),
                present: true,
                row_count: rows.len(),
                columns,
                rows,
            });
        } else {
            tables.push(ReferenceTableSnapshot {
                name: table_name.to_string(),
                present: false,
                row_count: 0,
                columns: Vec::new(),
                rows: Vec::new(),
            });
        }
    }
    Ok(tables)
}

fn is_empty_placeholder_issue(issue: &validation::issue::ValidationIssue) -> bool {
    issue.plu_number == Some(0) && issue.message.contains("missing product name")
}

fn selling_date_reason_for_log(
    reason: sanitization::engine::SanitizedChangeReason,
) -> &'static str {
    match reason {
        sanitization::engine::SanitizedChangeReason::OutsideAllowedRange => {
            "outside supported range 1..999"
        }
        sanitization::engine::SanitizedChangeReason::MalformedValue => "malformed numeric value",
        sanitization::engine::SanitizedChangeReason::EmptyDefaulted => "empty or unspecified value",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use digiweb::preflight::{
        DepartmentReadiness, GroupReadiness, LabelFormatReadiness, StaleConfirmation,
    };

    fn report_with_many_plu_dependencies() -> VerifyReport {
        let plus = (1..=12).collect::<Vec<_>>();
        VerifyReport {
            schema_version: 1,
            application_version: "test".to_string(),
            command: "verify".to_string(),
            generated_at: "2026-08-11T00:00:00-04:00".to_string(),
            started_at: "2026-08-11T00:00:00-04:00".to_string(),
            finished_at: "2026-08-11T00:00:01-04:00".to_string(),
            source_path: "plu.mdb".to_string(),
            source_sha256: "abc".to_string(),
            target_url: "https://example.invalid".to_string(),
            store_number: 1,
            authentication: "PASSED".to_string(),
            source_plu_count: 596,
            eligible_plu_count: 592,
            excluded_customer_action_count: 4,
            excluded_customer_action_plus: vec![21, 22, 700, 9317],
            references: VerifyReferenceSection {
                departments: vec![DepartmentReadiness {
                    number: 2,
                    status: ReferenceConfirmationStatus::ManuallyConfirmed,
                    source_plu_numbers: plus.clone(),
                }],
                groups: vec![GroupReadiness {
                    department: 2,
                    number: 997,
                    status: ReferenceConfirmationStatus::ManuallyConfirmed,
                    source_plu_numbers: plus.clone(),
                }],
                label_formats: vec![LabelFormatReadiness {
                    number: 6,
                    status: ReferenceConfirmationStatus::ManuallyConfirmed,
                    source_plu_numbers: plus,
                }],
            },
            unverified_reference_count: 0,
            stale_confirmations: vec![StaleConfirmation {
                reference_type: "label_format".to_string(),
                reference: "99".to_string(),
                message: "Configured confirmation not required by this MDB: Label Format 99"
                    .to_string(),
            }],
            readiness: "READY_WITH_SKIPS".to_string(),
            safety: VerifySafety {
                write_operation_attempted: false,
                plu_write_requests: 0,
                source_database_modified: false,
            },
        }
    }

    #[test]
    fn verify_report_text_and_json_include_readiness_and_safety() {
        let report = report_with_many_plu_dependencies();

        let text = render_verify_report_text(&report);
        let json = serde_json::to_string(&report).expect("json");

        assert!(text.contains("Readiness: READY_WITH_SKIPS"));
        assert!(text.contains("Customer-action-required / excluded: 4"));
        assert!(text.contains("Write operation attempted: NO"));
        assert!(text.contains("PLU write requests: 0"));
        assert!(json.contains("\"readiness\":\"READY_WITH_SKIPS\""));
        assert!(json.contains("\"write_operation_attempted\":false"));
    }

    #[test]
    fn verify_report_text_keeps_full_dependency_lists() {
        let report = report_with_many_plu_dependencies();
        let text = render_verify_report_text(&report);

        assert!(text.contains("PLUs: 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12"));
        assert!(text.contains("Configured confirmation not required by this MDB: Label Format 99"));
    }

    #[test]
    fn bounded_dependency_examples_are_concise() {
        let values = (1..=12).collect::<Vec<_>>();

        assert_eq!(
            format_limited_u64s(&values, 8),
            "1, 2, 3, 4, 5, 6, 7, 8, ..."
        );
    }

    #[test]
    fn authentication_failure_blocks_readiness() {
        let readiness = ReferenceReadiness {
            departments: Vec::new(),
            groups: Vec::new(),
            label_formats: Vec::new(),
            stale_confirmations: Vec::new(),
            unverified_reference_count: 0,
            result: ReadinessResult::Ready,
        };

        assert_eq!(
            final_readiness_result(AuthenticationReadinessStatus::Failed, &readiness),
            ReadinessResult::NotReady
        );
    }

    #[test]
    fn preflight_wording_does_not_claim_group_uuid_resolution() {
        let misleading = ["Internal DIGIweb group UUID:", " resolved by DIGIweb"].concat();
        assert!(!include_str!("main.rs").contains(&misleading));
    }
}
