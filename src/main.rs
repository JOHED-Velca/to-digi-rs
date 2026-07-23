mod cli;
mod config;
mod digiweb;
mod error;
mod import;
mod logging;
mod models;
mod source;
mod validation;

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;
use std::time::Instant;

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
    let config = AppConfig::load(Path::new("config.toml"))?;
    config.validate_startup()?;
    let command = effective_command(cli, &config);
    log_command(&command, logger)?;
    logger.kv("DIGIweb target URL", &config.digiweb.base_url)?;
    if config.digiweb.allow_invalid_certificates {
        logger.warning("TLS certificate validation is disabled.")?;
    }

    if config.digiweb.log_credentials_for_testing {
        logger.warning("Testing credential logging is enabled. Only the Client ID is written; client secrets are never logged.")?;
        logger.kv("DIGIweb Client ID", &config.digiweb.client_id)?;
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
            ..
        } => run_import_command(&config, limit, continue_on_error, test_mode, logger).await,
        EffectiveCommand::TestConnection => run_test_connection(&config, logger).await,
        EffectiveCommand::Verify => run_verify(&config, logger).await,
    }
}

fn log_command(command: &EffectiveCommand, logger: &mut AuditLogger) -> Result<(), AppError> {
    logger.kv("Command", command.name())?;
    match command {
        EffectiveCommand::Analyze { .. } => {
            logger.kv("Network access permitted", "no")?;
        }
        EffectiveCommand::Import {
            limit,
            continue_on_error,
            test_mode,
            legacy_used,
            defaulted_from_no_command,
        } => {
            if *defaulted_from_no_command {
                logger.line("No command supplied; defaulting to import.")?;
            }
            if *test_mode {
                logger.line("Test mode enabled: equivalent to --limit 1.")?;
            }
            logger.kv(
                "Import limit",
                &limit
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "none".to_string()),
            )?;
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

    let (schema, dataset) = MdbTools::read_dataset(source_file.path(), &config.mapping, logger)?;
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
    })
}

fn run_analyze(config: &AppConfig, logger: &mut AuditLogger) -> Result<i32, AppError> {
    logger.line("ANALYSIS ONLY")?;
    logger.line("No authentication or DIGIweb API requests were attempted.")?;
    let source = read_source_context(config, logger)?;
    write_analysis_report(&source)?;
    logger.kv("Analysis report", "analysis-report.txt")?;
    logger.final_import_summary(FinalImportLog {
        status: "SUCCESS",
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
    Ok(0)
}

async fn run_import_command(
    config: &AppConfig,
    limit: Option<usize>,
    continue_on_error: bool,
    test_mode: bool,
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

    let client_secret = load_client_secret(config)?;
    logger.kv(
        "Client secret",
        client_secret_log_message(config, std::env::var("DIGIWEB_CLIENT_SECRET").is_ok()),
    )?;
    let summary = run_import(
        config.clone(),
        client_secret,
        &source.valid_plus,
        ImportRunOptions {
            limit,
            continue_after_record_failure: continue_on_error,
            test_mode,
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
    validate_connection_urls(config)?;
    let client_secret = load_client_secret(config)?;
    logger.kv(
        "Client secret",
        client_secret_log_message(config, std::env::var("DIGIWEB_CLIENT_SECRET").is_ok()),
    )?;
    let client = DigiwebClient::new(config.clone())?;
    authenticate(client.http(), config, &client_secret).await?;
    logger.kv("Local source validation", "PASSED")?;
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
}

fn write_analysis_report(source: &SourceContext) -> Result<(), AppError> {
    let mut departments = BTreeSet::new();
    let mut groups = BTreeSet::new();
    let mut barcode_formats = BTreeSet::new();
    let mut barcode_types = BTreeSet::new();
    let mut price_modes = BTreeSet::new();
    let mut pluing_counts = Vec::new();
    for plu in &source.plus {
        if let Some(department) = plu.department_number {
            departments.insert(department);
        }
        if let Some(group) = plu.group_number {
            groups.insert(format!(
                "{}:{}",
                plu.department_number
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "missing".to_string()),
                group
            ));
        }
        if let Some(format) = &plu.source_barcode_format {
            barcode_formats.insert(format.clone());
        }
        if let Some(barcode_type) = &plu.barcode_type {
            barcode_types.insert(barcode_type.clone());
        }
        price_modes.insert(format!("{:?}", plu.price_mode));
        pluing_counts.push(format!(
            "PLU {} department {:?}: {} matching PluIng rows",
            plu.plu_number, plu.department_number, plu.source_pluing_row_count
        ));
    }
    let required_references = collect_required_references(&source.plus)
        .iter()
        .map(|reference| {
            format!(
                "department {} + group {} from PLUs {:?}: {}",
                reference.department_number,
                reference.group_number,
                reference.source_plu_numbers,
                reference.status.as_str()
            )
        })
        .collect::<Vec<_>>();
    let report = format!(
        "\
ANALYSIS ONLY
No authentication or DIGIweb API requests were attempted.

Source rows discovered: {}
MDB tables discovered: {}
Empty placeholder rows: {}
Normalized PLUs: {}
Valid PLUs: {}
Invalid PLUs: {}
PLU numbers: {}
Departments discovered: {}
Required department/group combinations: {}
Group references discovered: {}
Explicit group references: {}
PLUs defaulted to group 997: {}
PLUs with invalid group values: {}
Barcode formats found: {}
Derived barcode types: {}
Price modes found: {}
Matching PluIng rows:
{}
Unmatched PluIng rows: {}
Ingredient availability: {} PLUs with ingredients
Nutrition availability: {} PLUs with nutrition facts
Validation errors: {}
Validation warnings: {}
",
        source.dataset.plu_rows.len(),
        source.schema.tables.join(", "),
        source.placeholder_ignored,
        source.plus.len(),
        source.valid_plus.len(),
        source.invalid_source_rows,
        source
            .plus
            .iter()
            .map(|plu| plu.plu_number.to_string())
            .collect::<Vec<_>>()
            .join(", "),
        departments
            .iter()
            .map(u32::to_string)
            .collect::<Vec<_>>()
            .join(", "),
        required_references.join("; "),
        groups.into_iter().collect::<Vec<_>>().join(", "),
        source.explicit_group_references,
        source.defaulted_group_references,
        source.invalid_group_values,
        barcode_formats.into_iter().collect::<Vec<_>>().join(", "),
        barcode_types.into_iter().collect::<Vec<_>>().join(", "),
        price_modes.into_iter().collect::<Vec<_>>().join(", "),
        pluing_counts.join("\n"),
        source.orphan_pluing_rows,
        source
            .plus
            .iter()
            .filter(|plu| plu.ingredients.is_some())
            .count(),
        source
            .plus
            .iter()
            .filter(|plu| !plu.nutrition_facts.is_empty())
            .count(),
        source.validation_report.error_count(),
        source.validation_report.warning_count(),
    );
    fs::write("analysis-report.txt", report)
        .map_err(|err| AppError::Logging(format!("failed to write analysis-report.txt: {err}")))
}

fn is_empty_placeholder_issue(issue: &validation::issue::ValidationIssue) -> bool {
    issue.plu_number == Some(0) && issue.message.contains("missing product name")
}
