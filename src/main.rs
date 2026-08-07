mod analysis;
mod cli;
mod config;
mod deployment;
mod digiweb;
mod error;
mod import;
mod logging;
mod models;
mod recovery;
mod sanitization;
mod source;
mod validation;

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
use digiweb::auth::authenticate;
use digiweb::client::DigiwebClient;
use digiweb::payload::DigiwebPluPayload;
use digiweb::preflight::collect_required_references;
use error::AppError;
use import::runner::{ImportRunOptions, run_import};
use logging::{AuditLogger, FinalImportLog};
use models::plu::Plu;
use recovery::validator::target_identity;
use recovery::{DEFAULT_MANIFEST_PATH, SourceIdentity, sha256_file};
use sanitization::{
    SanitizationIntegration, SanitizationProfile, SanitizationReportInput, apply_profile,
    load_profile_from_safe_path, validate_profile_path, write_sanitization_reports,
};
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
        Some(CliCommand::Init(_)) | Some(CliCommand::Pull) | Some(CliCommand::Version)
    );
    let config = if config_not_required_for_dispatch {
        AppConfig::default()
    } else {
        AppConfig::load(config_path)?
    };
    let command = effective_command(cli, &config);
    if matches!(
        command,
        EffectiveCommand::Analyze { .. } | EffectiveCommand::Sanitize { .. }
    ) && !config_exists
    {
        logger.line("config.toml not found; using built-in analysis mapping defaults.")?;
    }
    if !matches!(
        command,
        EffectiveCommand::Analyze { .. }
            | EffectiveCommand::Doctor { .. }
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
            continue_on_error,
            test_mode,
            resume,
            retry_failed,
            sanitize_profile,
            ..
        } => {
            run_import_command(
                &config,
                limit,
                continue_on_error,
                test_mode,
                resume.as_deref(),
                retry_failed,
                sanitize_profile.as_ref(),
                logger,
            )
            .await
        }
        EffectiveCommand::Pull => {
            println!("Pull is handled by the generated ./to-digi launcher.");
            println!(
                "For direct Docker use, run: docker pull {}",
                deployment::default_image_reference()
            );
            logger.line("Pull command invoked inside importer; no Docker resources modified.")?;
            Ok(0)
        }
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
        EffectiveCommand::Pull => {
            logger.kv("PLU write permitted", "no")?;
            logger.kv("Docker pull command", "launcher-handled")?;
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
            logger.line("Internal DIGIweb group UUID: resolved by DIGIweb")?;
            logger.kv("Group validation", "accepted as positive integer")?;
        }
    }
    let required_references = collect_required_references(&plus);
    for reference in &required_references {
        logger.line(format!(
            "Required DIGIweb reference: department {} + group {} from PLUs {:?} => {}",
            reference.department_number,
            reference.group_number,
            reference.source_plu_numbers,
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
    let source = read_source_context(config, logger, profile)?;
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
    let source = read_source_context(config, logger, Some(profile))?;
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
    let source = read_source_context(config, logger, profile)?;
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
    println!("Starting import readiness verification...");
    println!("Outputs will be written to logs.txt");
    logger.line("Verify scope: import-readiness verification only; no source-versus-DIGIweb post-import comparison is attempted.")?;
    let profile = load_optional_sanitization_profile(sanitize_profile_path)?;
    let source = read_source_context(config, logger, profile)?;
    let analysis_report = build_analysis_report(&source);
    for plu in &source.valid_plus {
        DigiwebPluPayload::from_plu(plu, &config.digiweb)?;
    }
    logger.kv("Payload validation", "PASSED")?;
    validate_connection_urls(config)?;
    let client_secret = load_client_secret(config)?;
    logger.kv(
        "Client secret",
        client_secret_log_message(config, environment_secret_present()),
    )?;
    let client = DigiwebClient::new(config.clone())?;
    authenticate(client.http(), config, &client_secret).await?;
    logger.kv(
        "Local source analysis status",
        analysis_report.analysis_status.as_text(),
    )?;
    logger.kv(
        "Local source validation",
        if analysis_report.analysis_status == analysis::model::AnalysisStatus::Fail {
            "FAILED"
        } else {
            "PASSED"
        },
    )?;
    logger.kv("DIGIweb authentication", "PASSED")?;
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
    logger.kv("DIGIweb department/group existence", "NOT CHECKED")?;
    logger.kv("Write operation attempted", "NO")?;
    logger.kv(
        "IMPORT READINESS",
        if source.valid_plus.is_empty() {
            "NOT READY"
        } else {
            "READY"
        },
    )?;
    logger.flush()?;
    let exit_code = if source.valid_plus.is_empty() { 2 } else { 0 };
    println!(
        "Final status: {}",
        if exit_code == 0 { "READY" } else { "NOT_READY" }
    );
    Ok(exit_code)
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
