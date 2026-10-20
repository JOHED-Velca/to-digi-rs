use std::path::Path;
use std::time::Instant;

use ratatui::Terminal;
use ratatui::backend::Backend;

use crate::cli::ConfirmTarget;
use crate::config::AppConfig;
use crate::config_setup::{apply_customer_config_update, validate_digiweb_host};
use crate::confirm::{
    ConfirmationPlan, apply_confirmation_to_config_text, render_confirmation_preview,
    write_confirmed_config,
};
use crate::diagnostics::{
    DiagnosticTiming, DiagnosticsInput, build_diagnostics_report, build_dry_run_manifest,
    write_diagnostics_reports, write_dry_run_outputs,
};
use crate::digiweb::auth::authenticate;
use crate::digiweb::client::DigiwebClient;
use crate::digiweb::payload::DigiwebPluPayload;
use crate::digiweb::preflight::{AuthenticationReadinessStatus, evaluate_reference_readiness};
use crate::error::AppError;
use crate::import::runner::ImportProgressSnapshot;
use crate::logging::{AuditLogger, FinalImportLog};
use crate::recovery::validator::target_identity;
use crate::selection::{SelectionCriteria, select_eligible_plus, selection_error};
use crate::source::FIXED_SOURCE_FILE;
use crate::{
    VerifyReportInput, build_analysis_report, build_verify_report, dry_run_selected_success,
    excluded_plu_numbers, final_readiness_result, load_client_secret,
    load_optional_sanitization_profile, log_duplicate_barcode_section,
    manifest_path_from_environment, profile_for_import_or_resume, read_source_context,
    skipped_duplicate_barcode_count, write_dry_run_payload_previews, write_json_report,
    write_text_report, write_verify_reports,
};

use super::app::TerminalApp;
use super::model::{
    AnalysisSummary, DeploymentSummary, DiagnosticsSummaryView, DryRunSummaryView,
    ImportFailureView, ImportSummaryView, ReadinessSummaryView, Screen, UiOperation,
};
use super::render::render_app;
pub(super) async fn run_ui_operation<B: Backend>(
    terminal: &mut Terminal<B>,
    app: &mut TerminalApp,
    operation: UiOperation,
    logger: &mut AuditLogger,
) {
    app.busy_message = Some(match operation {
        UiOperation::DeploymentCheck => "Checking deployment...".to_string(),
        UiOperation::SaveHost(_) => "Saving DIGIweb host...".to_string(),
        UiOperation::SaveSecret(_) => "Saving client secret...".to_string(),
        UiOperation::Analyze => "Analyzing MDB...".to_string(),
        UiOperation::Diagnose => "Building diagnostics...".to_string(),
        UiOperation::ConfirmReferences => "Recording confirmations...".to_string(),
        UiOperation::Verify => "Verifying readiness...".to_string(),
        UiOperation::DryRun(_) => "Running dry run...".to_string(),
        UiOperation::Import { .. } => "Importing PLUs...".to_string(),
    });
    app.status_message = None;
    app.input_error = None;

    let result = match operation {
        UiOperation::DeploymentCheck => {
            app.deployment = Some(check_deployment());
            Ok("Deployment checked.".to_string())
        }
        UiOperation::SaveHost(value) => match save_digiweb_host(&value) {
            Ok(()) => {
                app.input_mode = None;
                app.input_buffer.clear();
                Ok("OK DIGIweb Host updated".to_string())
            }
            Err(err) => {
                app.input_error = Some(short_error(&err));
                Err(err)
            }
        },
        UiOperation::SaveSecret(secret) => {
            let result = save_client_secret(secret);
            app.input_buffer.clear();
            match result {
                Ok(()) => {
                    app.input_mode = None;
                    Ok("OK Client secret updated".to_string())
                }
                Err(err) => {
                    app.input_error = Some(short_error(&err));
                    Err(err)
                }
            }
        }
        UiOperation::Analyze => match run_analysis_service(app.selected_profile.clone(), logger) {
            Ok(summary) => {
                app.analysis = Some(summary);
                Ok("Analysis complete.".to_string())
            }
            Err(err) => Err(err),
        },
        UiOperation::Diagnose => {
            match run_diagnostics_service(app.selected_profile.clone(), logger) {
                Ok(summary) => {
                    app.diagnostics = Some(summary);
                    Ok("Diagnostics complete.".to_string())
                }
                Err(err) => Err(err),
            }
        }
        UiOperation::ConfirmReferences => {
            match run_confirm_service(app.selected_profile.clone(), logger) {
                Ok(()) => {
                    app.history.clear();
                    app.screen = Screen::Main;
                    app.selected_index = 0;
                    Ok("OK DIGIweb references recorded.".to_string())
                }
                Err(err) => Err(err),
            }
        }
        UiOperation::Verify => match run_verify_service(app.selected_profile.clone(), logger).await
        {
            Ok(summary) => {
                app.readiness = Some(summary);
                Ok("Readiness checked.".to_string())
            }
            Err(err) => Err(err),
        },
        UiOperation::DryRun(selection) => {
            match run_dry_run_service(app.selected_profile.clone(), selection, logger) {
                Ok(summary) => {
                    app.dry_run = Some(summary);
                    Ok("Dry run complete.".to_string())
                }
                Err(err) => Err(err),
            }
        }
        UiOperation::Import {
            selection,
            continue_on_error,
        } => {
            let selected_profile = app.selected_profile.clone();
            app.screen = Screen::FullImportProgress;
            app.selected_index = 0;
            app.import_progress = None;
            app.busy_message = None;
            let mut draw_error = None;
            let mut progress_observer = |snapshot: ImportProgressSnapshot| {
                app.import_progress = Some(snapshot);
                if let Err(err) = terminal.draw(|frame| render_app(frame, app)) {
                    draw_error = Some(format!("terminal draw failed: {err}"));
                }
            };
            match run_import_service(
                selected_profile,
                selection,
                continue_on_error,
                logger,
                Some(&mut progress_observer),
            )
            .await
            {
                Ok(summary) => {
                    app.import_result = Some(summary);
                    app.screen = Screen::Completion;
                    app.selected_index = 0;
                    if let Some(err) = draw_error {
                        Err(AppError::Internal(err))
                    } else {
                        Ok("Import complete.".to_string())
                    }
                }
                Err(err) => Err(err),
            }
        }
    };

    app.busy_message = None;
    if let Err(err) = logger.flush() {
        app.status_message = Some(format!("Logging error: {err}"));
    } else {
        app.status_message = Some(result.unwrap_or_else(|err| short_error(&err)));
    }
}

fn check_deployment() -> DeploymentSummary {
    DeploymentSummary {
        mdb_found: Path::new(FIXED_SOURCE_FILE).is_file(),
        config_ready: Path::new("config.toml").is_file(),
        profiles_ready: Path::new("profiles").is_dir(),
    }
}

fn save_digiweb_host(value: &str) -> Result<(), AppError> {
    let host = validate_digiweb_host(value)?;
    apply_customer_config_update(Path::new("config.toml"), Some(&host), None)?;
    Ok(())
}

fn save_client_secret(secret: String) -> Result<(), AppError> {
    if secret.trim().is_empty() {
        return Err(AppError::Config(
            "Client secret must not be empty".to_string(),
        ));
    }
    let secret = secrecy::SecretString::new(secret);
    apply_customer_config_update(Path::new("config.toml"), None, Some(&secret))?;
    Ok(())
}

pub(super) fn current_digiweb_host_text() -> String {
    AppConfig::load(Path::new("config.toml"))
        .ok()
        .and_then(|config| {
            reqwest::Url::parse(&config.digiweb.base_url)
                .ok()
                .and_then(|url| url.host_str().map(ToString::to_string))
        })
        .unwrap_or_else(|| "192.168.0.150".to_string())
}

fn profile_selection(profile: Option<String>) -> Option<crate::cli::ProfileSelection> {
    profile
        .filter(|value| !value.trim().is_empty())
        .map(|value| crate::cli::ProfileSelection::BuiltIn(value.to_ascii_lowercase()))
}

fn config_for_offline_operation() -> Result<AppConfig, AppError> {
    if Path::new("config.toml").exists() {
        AppConfig::load(Path::new("config.toml"))
    } else {
        Ok(AppConfig::default())
    }
}

fn run_analysis_service(
    profile: Option<String>,
    logger: &mut AuditLogger,
) -> Result<AnalysisSummary, AppError> {
    logger.line("TUI ANALYSIS")?;
    logger.line("Network access permitted: NO")?;
    logger.line("DIGIweb API requests attempted: NO")?;
    let config = config_for_offline_operation()?;
    let profile_selection = profile_selection(profile);
    let profile = load_optional_sanitization_profile(profile_selection.as_ref())?;
    let source = read_source_context(&config, logger, profile, false)?;
    let report = build_analysis_report(&source);
    write_text_report(Path::new("analysis-report.txt"), &report)?;
    write_json_report(Path::new("analysis-report.json"), &report)?;
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
    Ok(AnalysisSummary {
        source_plus: report.summary.source_plus_discovered,
        eligible: report.summary.valid_plus,
        excluded: report.summary.invalid_plus,
        departments: report
            .departments
            .iter()
            .map(|item| item.department_number)
            .collect(),
        groups: report
            .groups
            .iter()
            .map(|item| (item.department_number, item.group_number))
            .collect(),
        label_formats: report
            .label_formats
            .iter()
            .map(|item| item.label_format)
            .collect(),
        warning_count: report.warnings.len(),
        blocking_count: report.blocking_errors.len(),
        first_valid_plu: report.summary.valid_plu_numbers.first().copied(),
    })
}

fn run_diagnostics_service(
    profile: Option<String>,
    logger: &mut AuditLogger,
) -> Result<DiagnosticsSummaryView, AppError> {
    logger.line("TUI DIAGNOSTICS")?;
    logger.line("Network access permitted: NO")?;
    logger.line("DIGIweb API requests attempted: NO")?;
    let config = config_for_offline_operation()?;
    let profile_selection = profile_selection(profile);
    let profile = load_optional_sanitization_profile(profile_selection.as_ref())?;
    let started_at = chrono::Local::now();
    let started = Instant::now();
    let source = read_source_context(&config, logger, profile, false)?;
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
            "TUI diagnostics source read and normalization",
            started.elapsed(),
        )],
    });
    write_diagnostics_reports(
        Path::new("diagnostics-report.txt"),
        Path::new("diagnostics-report.json"),
        &report,
    )?;
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
    Ok(DiagnosticsSummaryView {
        eligible: source.valid_plus.len(),
        excluded: source.plus.len().saturating_sub(source.valid_plus.len())
            + source.row_issues.len(),
        missing_name_count: report.summary.missing_required_name,
        duplicate_barcode_count: report.summary.duplicate_barcode,
        problem_count: report.summary.problem_plus,
    })
}

fn run_confirm_service(profile: Option<String>, logger: &mut AuditLogger) -> Result<(), AppError> {
    logger.line("TUI REFERENCE CONFIRMATION")?;
    let config = AppConfig::load(Path::new("config.toml"))?;
    let profile_selection = profile_selection(profile);
    let profile = load_optional_sanitization_profile(profile_selection.as_ref())?;
    let profile_label = profile_selection
        .as_ref()
        .map(crate::cli::ProfileSelection::display)
        .unwrap_or_else(|| "none".to_string());
    let source = read_source_context(&config, logger, profile, false)?;
    let excluded_count = excluded_plu_numbers(&source).len();
    let readiness =
        evaluate_reference_readiness(&source.valid_plus, &config.verification, excluded_count)?;
    let plan = ConfirmationPlan::from_readiness(
        profile_label,
        source.valid_plus.len(),
        ConfirmTarget::All,
        &readiness,
    );
    let _preview = render_confirmation_preview(&plan, false);
    if plan.additions.is_empty() {
        logger.line("No new confirmations were required; config.toml was not changed.")?;
        return Ok(());
    }
    let config_path = Path::new("config.toml");
    let current_text = std::fs::read_to_string(config_path).map_err(|err| {
        AppError::Config(format!(
            "failed to read config '{}': {err}",
            config_path.display()
        ))
    })?;
    let updated_text = apply_confirmation_to_config_text(&current_text, &plan.additions)?;
    if updated_text != current_text {
        let backup = write_confirmed_config(config_path, &updated_text)?;
        logger.kv("Updated config", "config.toml")?;
        logger.kv("Config backup", &backup.display().to_string())?;
    }
    Ok(())
}

async fn run_verify_service(
    profile: Option<String>,
    logger: &mut AuditLogger,
) -> Result<ReadinessSummaryView, AppError> {
    logger.line("TUI VERIFY")?;
    let started_at = chrono::Local::now();
    let config = AppConfig::load(Path::new("config.toml"))?;
    let profile_selection = profile_selection(profile);
    let profile = load_optional_sanitization_profile(profile_selection.as_ref())?;
    let source = read_source_context(&config, logger, profile, false)?;
    for plu in &source.valid_plus {
        DigiwebPluPayload::from_plu(plu, &config.digiweb)?;
    }
    let excluded_plu_numbers = excluded_plu_numbers(&source);
    let excluded_count = excluded_plu_numbers.len();
    let reference_readiness =
        evaluate_reference_readiness(&source.valid_plus, &config.verification, excluded_count)?;
    crate::validate_connection_urls(&config)?;
    let client_secret = load_client_secret(&config)?;
    let client = DigiwebClient::new(config.clone())?;
    let auth_result = authenticate(client.http(), &config, &client_secret).await;
    let authentication_error = auth_result.as_ref().err().map(|err| short_error(err));
    let authentication = if auth_result.is_ok() {
        AuthenticationReadinessStatus::Passed
    } else {
        AuthenticationReadinessStatus::Failed
    };
    let readiness = final_readiness_result(authentication, &reference_readiness);
    let finished_at = chrono::Local::now();
    let verify_report = build_verify_report(VerifyReportInput {
        source: &source,
        config: &config,
        authentication,
        reference_readiness: &reference_readiness,
        readiness_result: readiness,
        excluded_plu_numbers: &excluded_plu_numbers,
        started_at,
        finished_at,
    });
    write_verify_reports(&verify_report)?;
    logger.kv("DIGIweb authentication", authentication.as_str())?;
    logger.kv("IMPORT READINESS", readiness.as_str())?;
    if let Err(err) = auth_result {
        logger.warning(format!("Readiness authentication failed: {err}"))?;
    }
    Ok(ReadinessSummaryView {
        readiness,
        authentication,
        authentication_error,
        eligible: source.valid_plus.len(),
        excluded: excluded_count,
        unverified_references: reference_readiness.unverified_reference_count,
    })
}

fn run_dry_run_service(
    profile: Option<String>,
    selection: SelectionCriteria,
    logger: &mut AuditLogger,
) -> Result<DryRunSummaryView, AppError> {
    logger.line("TUI DRY RUN")?;
    let config = config_for_offline_operation()?;
    let profile_selection = profile_selection(profile);
    let profile = load_optional_sanitization_profile(profile_selection.as_ref())?;
    let started_at = chrono::Local::now();
    let started = Instant::now();
    let source = read_source_context(&config, logger, profile, false)?;
    let selected_plus = select_eligible_plus(
        &source.plus,
        &source.valid_plus,
        &source.row_issues,
        &source.validation_report,
        selection,
    )
    .map_err(|failure| selection_error(&failure))?;
    let diagnostics = build_diagnostics_report(DiagnosticsInput {
        source_path: FIXED_SOURCE_FILE,
        source_sha256: &source.source_identity.sha256,
        started_at,
        finished_at: chrono::Local::now(),
        dataset: &source.dataset,
        all_normalized_plus: &source.plus,
        valid_plus: &source.valid_plus,
        row_issues: &source.row_issues,
        validation_report: &source.validation_report,
        timings: vec![DiagnosticTiming::from_duration(
            "TUI dry-run source read and normalization",
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
    write_dry_run_payload_previews(&config, &selected_plus)?;
    write_dry_run_outputs(
        Path::new("dry-run-report.txt"),
        Path::new("dry-run-manifest.json"),
        &manifest,
    )?;
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
    Ok(DryRunSummaryView {
        selected: manifest.summary.selection.selected,
        would_submit: manifest.summary.selection.would_submit,
        api_write_requests: manifest.summary.api_write_requests,
        selected_plu: manifest.summary.selection.selected_order.first().copied(),
        eligible: manifest.summary.valid,
        excluded: manifest.summary.customer_action_required,
        success: dry_run_selected_success(&manifest),
    })
}

async fn run_import_service(
    profile: Option<String>,
    selection: SelectionCriteria,
    continue_on_error: bool,
    logger: &mut AuditLogger,
    progress_observer: Option<&mut dyn FnMut(ImportProgressSnapshot)>,
) -> Result<ImportSummaryView, AppError> {
    let config = AppConfig::load(Path::new("config.toml"))?;
    let profile_selection = profile_selection(profile);
    let profile = profile_for_import_or_resume(None, profile_selection.as_ref())?;
    let source = read_source_context(&config, logger, profile, false)?;
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
    let manifest_path = manifest_path_from_environment(None)?;
    let options = crate::import::runner::ImportRunOptions {
        limit: selection.limit,
        requested_plu: selection.requested_plu,
        continue_after_record_failure: continue_on_error,
        test_mode: selection.test_mode,
        retry_failed: false,
    };
    let summary = if let Some(observer) = progress_observer {
        crate::import::runner::run_import_quiet_with_progress(
            config.clone(),
            &source.valid_plus,
            source.source_identity.clone(),
            target_identity(&config),
            &manifest_path,
            None,
            source.sanitization.clone(),
            options,
            logger,
            observer,
        )
        .await?
    } else {
        crate::import::runner::run_import_quiet(
            config.clone(),
            &source.valid_plus,
            source.source_identity.clone(),
            target_identity(&config),
            &manifest_path,
            None,
            source.sanitization.clone(),
            options,
            logger,
        )
        .await?
    };
    let status = summary.final_status().as_str().to_string();
    logger.final_import_summary(FinalImportLog {
        status: &status,
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
        successful_plu_numbers: &[],
        failed_plu_numbers: &[],
        unknown_plu_numbers: &[],
        dry_run: false,
    })?;
    let failures = summary
        .records
        .iter()
        .filter(|record| record.failure_message.is_some())
        .take(50)
        .map(|record| {
            let barcode_type = source
                .valid_plus
                .iter()
                .find(|plu| plu.plu_number == record.plu_number)
                .and_then(|plu| plu.barcode_type.as_deref());
            ImportFailureView {
                plu_number: record.plu_number,
                status: record.final_status.as_str().to_string(),
                stage: record
                    .failure_stage
                    .clone()
                    .unwrap_or_else(|| "DIGIweb processing".to_string()),
                reason: concise_import_failure(
                    record.failure_message.as_deref().unwrap_or("Import failed"),
                    barcode_type,
                ),
            }
        })
        .collect();
    Ok(ImportSummaryView {
        selected: summary.selected,
        submitted: summary.submitted,
        succeeded: summary.succeeded,
        failed: summary.failed,
        unknown: summary.unknown,
        not_attempted: summary.not_attempted_after_stop,
        selected_plu: selection.requested_plu.or_else(|| {
            selection
                .test_mode
                .then(|| summary.records.first().map(|record| record.plu_number))
                .flatten()
        }),
        continue_on_error,
        failures,
        status,
    })
}

pub(super) fn concise_import_failure(message: &str, barcode_type: Option<&str>) -> String {
    let sanitized = crate::recovery::sanitize_manifest_error(message);
    let barcode = value_after_label(&sanitized, "the barcode:")
        .or_else(|| value_after_label(&sanitized, "barcode:"));
    let actual_length = value_after_label(&sanitized, "length barcode:");
    let required_length = value_after_label(&sanitized, "length needed:");
    if let (Some(barcode), Some(actual_length), Some(required_length), Some(barcode_type)) =
        (barcode, actual_length, required_length, barcode_type)
    {
        return format!(
            "Barcode {barcode} has length {actual_length}; barcode type {barcode_type} requires length {required_length}."
        );
    }

    let concise = sanitized.split_whitespace().collect::<Vec<_>>().join(" ");
    if concise.chars().count() <= 240 {
        concise
    } else {
        concise.chars().take(237).collect::<String>() + "..."
    }
}

fn value_after_label<'a>(message: &'a str, label: &str) -> Option<&'a str> {
    let lower = message.to_ascii_lowercase();
    let start = lower.find(label)? + label.len();
    message[start..]
        .trim_start()
        .split(|ch: char| ch.is_whitespace() || matches!(ch, ',' | ';' | '.'))
        .find(|value| !value.is_empty())
}

fn short_error(err: &AppError) -> String {
    err.to_string()
        .lines()
        .next()
        .unwrap_or("operation failed")
        .to_string()
}
