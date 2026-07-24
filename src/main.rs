mod analysis;
mod cli;
mod config;
mod digiweb;
mod error;
mod import;
mod logging;
mod models;
mod recovery;
mod source;
mod validation;

use std::path::{Path, PathBuf};
use std::time::Instant;

use analysis::model::ReferenceTableSnapshot;
use analysis::{
    AnalysisInput, collect_analysis, render_console_summary, write_json_report, write_text_report,
};
use clap::Parser;
use cli::{Cli, EffectiveCommand, effective_command};
use config::{AppConfig, client_secret_log_message, load_client_secret};
use digiweb::auth::authenticate;
use digiweb::client::DigiwebClient;
use digiweb::preflight::collect_required_references;
use error::AppError;
use import::runner::{ImportRunOptions, run_import};
use logging::{AuditLogger, FinalImportLog};
use models::plu::Plu;
use recovery::validator::target_identity;
use recovery::{DEFAULT_MANIFEST_PATH, SourceIdentity, sha256_file};
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
            err.exit_code()
        }
    }
}

async fn run_inner(cli: &Cli, logger: &mut AuditLogger) -> Result<i32, AppError> {
    let config_path = Path::new("config.toml");
    let config_exists = config_path.exists();
    let config = AppConfig::load(config_path)?;
    let command = effective_command(cli, &config);
    if matches!(command, EffectiveCommand::Analyze { .. }) && !config_exists {
        logger.line("config.toml not found; using built-in analysis mapping defaults.")?;
    }
    if !matches!(command, EffectiveCommand::Analyze { .. }) {
        if !config_exists {
            return Err(AppError::Config(format!(
                "config.toml is required for the '{}' command",
                command.name()
            )));
        }
        config.validate_startup()?;
    }
    log_command(&command, logger)?;
    if !matches!(command, EffectiveCommand::Analyze { .. }) {
        logger.kv("DIGIweb target URL", &config.digiweb.base_url)?;
        if config.digiweb.allow_invalid_certificates {
            logger.warning("TLS certificate validation is disabled.")?;
        }

        if config.digiweb.log_credentials_for_testing {
            logger.warning("Testing credential logging is enabled. Only the Client ID is written; client secrets are never logged.")?;
            logger.kv("DIGIweb Client ID", &config.digiweb.client_id)?;
        }
    }

    if command.uses_legacy_config() {
        logger.warning("Legacy [import] behavior flags are deprecated and will be removed in a future release. Use CLI commands and flags instead.")?;
    }

    match command {
        EffectiveCommand::Analyze { .. } => run_analyze(&config, logger),
        EffectiveCommand::Import {
            limit,
            continue_on_error,
            test_mode,
            resume,
            retry_failed,
            ..
        } => {
            run_import_command(
                &config,
                limit,
                continue_on_error,
                test_mode,
                resume.as_deref(),
                retry_failed,
                logger,
            )
            .await
        }
        EffectiveCommand::TestConnection => run_test_connection(&config, logger).await,
        EffectiveCommand::Verify => run_verify(&config, logger).await,
    }
}

fn log_command(command: &EffectiveCommand, logger: &mut AuditLogger) -> Result<(), AppError> {
    logger.kv("Command", command.name())?;
    match command {
        EffectiveCommand::Analyze { .. } => {
            logger.kv("Network access permitted", "no")?;
            logger.kv("Authentication attempted", "NO")?;
            logger.kv("DIGIweb API requests attempted", "NO")?;
            logger.kv("Source database modified", "NO")?;
        }
        EffectiveCommand::Import {
            limit,
            continue_on_error,
            test_mode,
            resume,
            retry_failed,
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
            logger.kv(
                "Continue on error",
                if *continue_on_error { "true" } else { "false" },
            )?;
            logger.kv(
                "Legacy configuration used",
                if *legacy_used { "yes" } else { "no" },
            )?;
        }
        EffectiveCommand::TestConnection => {
            logger.kv("PLU write permitted", "no")?;
        }
        EffectiveCommand::Verify => {
            logger.kv("PLU write permitted", "no")?;
        }
    }
    Ok(())
}

fn read_source_context(
    config: &AppConfig,
    logger: &mut AuditLogger,
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

    let normalization_report =
        normalize_dataset(&dataset, &config.mapping, config.digiweb.store_number)?;
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
        dataset,
        plus,
        valid_plus,
        validation_report,
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

fn run_analyze(config: &AppConfig, logger: &mut AuditLogger) -> Result<i32, AppError> {
    logger.line("ANALYSIS ONLY")?;
    logger.line("Network access permitted: NO")?;
    logger.line("Authentication attempted: NO")?;
    logger.line("DIGIweb API requests attempted: NO")?;
    logger.line("Source database modified: NO")?;
    let source = read_source_context(config, logger)?;
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
    Ok(report.analysis_status.exit_code())
}

async fn run_import_command(
    config: &AppConfig,
    limit: Option<usize>,
    continue_on_error: bool,
    test_mode: bool,
    resume_manifest: Option<&Path>,
    retry_failed: bool,
    logger: &mut AuditLogger,
) -> Result<i32, AppError> {
    let source = read_source_context(config, logger)?;
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

    let source_identity = SourceIdentity {
        filename: FIXED_SOURCE_FILE.to_string(),
        size_bytes: source.source_file_size_bytes,
        sha256: sha256_file(Path::new(FIXED_SOURCE_FILE))?,
    };
    let target_identity = target_identity(config);
    let manifest_path = manifest_path_from_environment(resume_manifest)?;
    if resume_manifest.is_none() {
        logger.kv("Import manifest", &manifest_path.display().to_string())?;
    }

    let summary = run_import(
        config.clone(),
        &source.valid_plus,
        source_identity,
        target_identity,
        &manifest_path,
        resume_manifest,
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
    Ok(final_status.exit_code())
}

async fn run_test_connection(
    config: &AppConfig,
    logger: &mut AuditLogger,
) -> Result<i32, AppError> {
    validate_connection_urls(config)?;
    let client_secret = load_client_secret(config)?;
    logger.kv(
        "Client secret",
        client_secret_log_message(config, std::env::var("DIGIWEB_CLIENT_SECRET").is_ok()),
    )?;
    let started = Instant::now();
    let client = DigiwebClient::new(config.clone())?;
    authenticate(client.http(), config, &client_secret).await?;
    logger.line("DIGIweb connection test: SUCCESS")?;
    logger.kv("Base URL reachable", "yes")?;
    logger.kv("Authentication successful", "yes")?;
    logger.line("No PLU data was submitted.")?;
    logger.kv("Elapsed ms", &started.elapsed().as_millis().to_string())?;
    logger.flush()?;
    Ok(0)
}

async fn run_verify(config: &AppConfig, logger: &mut AuditLogger) -> Result<i32, AppError> {
    logger.line("Verify scope: import-readiness verification only; no source-versus-DIGIweb post-import comparison is attempted.")?;
    let source = read_source_context(config, logger)?;
    let analysis_report = build_analysis_report(&source);
    validate_connection_urls(config)?;
    let client_secret = load_client_secret(config)?;
    logger.kv(
        "Client secret",
        client_secret_log_message(config, std::env::var("DIGIWEB_CLIENT_SECRET").is_ok()),
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
    Ok(if source.valid_plus.is_empty() { 2 } else { 0 })
}

fn validate_connection_urls(config: &AppConfig) -> Result<(), AppError> {
    reqwest::Url::parse(&config.digiweb.base_url)
        .map_err(|err| AppError::Config(format!("invalid digiweb.base_url: {err}")))?;
    reqwest::Url::parse(config.token_url()?)
        .map_err(|err| AppError::Config(format!("invalid digiweb.token_url: {err}")))?;
    Ok(())
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
