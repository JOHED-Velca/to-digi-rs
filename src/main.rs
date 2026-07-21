mod config;
mod digiweb;
mod error;
mod import;
mod logging;
mod models;
mod source;
mod validation;

use std::path::Path;

use config::{AppConfig, client_secret_log_message, load_client_secret};
use digiweb::preflight::collect_required_references;
use error::AppError;
use import::runner::run_import;
use logging::AuditLogger;
use source::mapping::{normalize_dataset, validate_source_schema};
use source::mdb_tools::MdbTools;
use source::{FIXED_SOURCE_FILE, VerifiedSourceFile};
use validation::issue::Severity;
use validation::validator::{valid_plu_candidates, validate_plus};

#[tokio::main]
async fn main() {
    let exit_code = match AuditLogger::create(Path::new("logs.txt")) {
        Ok(mut logger) => run(&mut logger).await,
        Err(err) => {
            eprintln!("failed to create logs.txt: {err}");
            4
        }
    };
    std::process::exit(exit_code);
}

async fn run(logger: &mut AuditLogger) -> i32 {
    match run_inner(logger).await {
        Ok(code) => code,
        Err(err) => {
            let _ = logger.error(err.to_string());
            let _ = logger.final_failure(err.stage(), &err.to_string(), true);
            err.exit_code()
        }
    }
}

async fn run_inner(logger: &mut AuditLogger) -> Result<i32, AppError> {
    let config = AppConfig::load(Path::new("config.toml"))?;
    config.validate_startup()?;
    logger.kv("DIGIweb target URL", &config.digiweb.base_url)?;
    if config.digiweb.allow_invalid_certificates {
        logger.warning("TLS certificate validation is disabled.")?;
    }

    if config.digiweb.log_credentials_for_testing {
        logger.warning("Testing credential logging is enabled. Only the Client ID is written; client secrets are never logged.")?;
        logger.kv("DIGIweb Client ID", &config.digiweb.client_id)?;
    }

    let client_secret = if config.import.dry_run_inspect_only {
        None
    } else {
        Some(load_client_secret(&config)?)
    };
    if client_secret.is_some() {
        logger.kv(
            "Client secret",
            client_secret_log_message(&config, std::env::var("DIGIWEB_CLIENT_SECRET").is_ok()),
        )?;
    } else {
        logger.kv(
            "Client secret",
            "not loaded because dry_run_inspect_only is enabled",
        )?;
    }

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

    if config.import.dry_run_inspect_only {
        logger.line("Dry-run inspection mode is enabled; MDB inspection completed and no normalization, validation, authentication, or API requests will be attempted.")?;
        logger.final_success(
            dataset.plu_rows.len(),
            0,
            0,
            dataset.plu_rows.len(),
            "SUCCESS",
        )?;
        return Ok(0);
    }

    let normalization_report =
        normalize_dataset(&dataset, &config.mapping, config.digiweb.store_number)?;
    for issue in &normalization_report.row_issues {
        let plu = issue
            .plu_number
            .map(|value| value.to_string())
            .unwrap_or_else(|| "unknown".to_string());
        logger.line(format!("PLU {plu} skipped: {}", issue.message))?;
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
        logger.final_failure(
            "validation",
            &format!(
                "{} blocking validation error(s)",
                validation_report.error_count()
            ),
            true,
        )?;
        return Ok(2);
    }
    let valid_plus = valid_plu_candidates(&plus, &validation_report);
    for plu in &plus {
        if !valid_plus
            .iter()
            .any(|candidate| candidate.plu_number == plu.plu_number)
        {
            logger.line(format!("PLU {} skipped: validation errors", plu.plu_number))?;
        }
    }
    logger.kv("Valid PLUs available", &valid_plus.len().to_string())?;
    if valid_plus.is_empty() {
        logger.final_failure("validation", "no valid PLUs are available to send", true)?;
        return Ok(2);
    }
    if validation_report
        .issues
        .iter()
        .any(|issue| issue.severity == Severity::Warning)
    {
        logger.line("Validation warnings are present; continuing because no blocking validation errors were found.")?;
    }

    let client_secret = client_secret.ok_or(AppError::MissingEnv("DIGIWEB_CLIENT_SECRET"))?;
    let summary = run_import(config, client_secret, &valid_plus, logger).await?;
    for record in &summary.records {
        if record.final_status != digiweb::status::ProcessingStatus::Success {
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
    logger.final_success(
        summary.discovered,
        summary.succeeded,
        summary.failed,
        summary.skipped,
        final_status.as_str(),
    )?;
    Ok(final_status.exit_code())
}
