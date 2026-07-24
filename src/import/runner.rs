use std::fs;
use std::path::{Path, PathBuf};

use crate::config::{AppConfig, client_secret_log_message, load_client_secret};
use crate::digiweb::auth::authenticate;
use crate::digiweb::client::DigiwebClient;
use crate::digiweb::payload::DigiwebPluPayload;
use crate::digiweb::status::ProcessingStatus;
use crate::error::AppError;
use crate::import::result::{ImportSummary, RecordImportResult};
use crate::logging::AuditLogger;
use crate::models::plu::Plu;
use crate::recovery::model::{
    ImportManifest, ManifestOptions, PluManifestRecord, RecordStatus, ResumePlanItemKind,
    SourceIdentity, TargetIdentity,
};
use crate::recovery::{
    ManifestLock, atomic_write_manifest, build_resume_plan, load_manifest, sha256_json,
    validate_resume_compatibility,
};
use chrono::Local;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImportRunOptions {
    pub limit: Option<usize>,
    pub continue_after_record_failure: bool,
    pub test_mode: bool,
    pub retry_failed: bool,
}

pub async fn run_import(
    config: AppConfig,
    plus: &[Plu],
    source_identity: SourceIdentity,
    target_identity: TargetIdentity,
    manifest_path: &Path,
    resume_manifest: Option<&Path>,
    options: ImportRunOptions,
    logger: &mut AuditLogger,
) -> Result<ImportSummary, AppError> {
    config.token_url()?;
    config.plu_upsert_path()?;
    let client = DigiwebClient::new(config.clone())?;

    prepare_payload_preview_dir(config.import.write_payload_preview)?;

    let mut manifest = if let Some(path) = resume_manifest {
        logger.line("RESUMING IMPORT")?;
        logger.kv("Manifest", &path.display().to_string())?;
        load_manifest(path)?
    } else {
        let selected = select_records_to_send(plus, options.limit);
        let payloads = build_payloads(&selected, &config)?;
        let records = selected
            .iter()
            .zip(payloads.iter())
            .enumerate()
            .map(|(index, (plu, payload))| {
                Ok(PluManifestRecord::new(
                    plu.plu_number,
                    plu.department_number,
                    plu.group_number,
                    index + 1,
                    sha256_json(payload)?,
                ))
            })
            .collect::<Result<Vec<_>, AppError>>()?;
        let manifest = ImportManifest::new(
            source_identity.clone(),
            target_identity.clone(),
            ManifestOptions {
                limit: options.limit,
                continue_on_error: options.continue_after_record_failure,
                test_alias_used: options.test_mode,
            },
            plus.len(),
            records,
        );
        atomic_write_manifest(manifest_path, &manifest)?;
        logger.line("IMPORT RUN CREATED")?;
        logger.kv("Manifest", &manifest_path.display().to_string())?;
        logger.kv(
            "Selected PLUs",
            &manifest.selection.selected_count.to_string(),
        )?;
        logger.kv("Not attempted", &manifest.summary.not_attempted.to_string())?;
        logger.kv("Already successful", "0")?;
        print!(
            "IMPORT RUN CREATED\n\nManifest:\n{}\n\nSelected PLUs: {}\nNot attempted: {}\nAlready successful: 0\n\n",
            manifest_path.display(),
            manifest.selection.selected_count,
            manifest.summary.not_attempted
        );
        manifest
    };
    let active_manifest_path = resume_manifest.unwrap_or(manifest_path);
    let _lock = ManifestLock::acquire(active_manifest_path)?;
    let continue_after_record_failure =
        if resume_manifest.is_some() && !options.continue_after_record_failure {
            manifest.options.continue_on_error
        } else {
            options.continue_after_record_failure
        };

    let selected_plus = selected_plus_from_manifest(plus, &manifest)?;
    let payloads = build_payloads(&selected_plus, &config)?;
    validate_resume_compatibility(
        &manifest,
        &source_identity,
        &target_identity,
        &selected_plus
            .iter()
            .map(|plu| (*plu).clone())
            .collect::<Vec<_>>(),
        &payloads,
        &config,
    )?;

    if resume_manifest.is_some() {
        let restarted_transients_changed = manifest.mark_restarted_transients_ambiguous();
        let plan = build_resume_plan(&manifest, options.retry_failed);
        let active_work = plan.items.iter().any(|item| {
            matches!(
                item.kind,
                ResumePlanItemKind::PollExistingRequest
                    | ResumePlanItemKind::SubmitNotAttempted
                    | ResumePlanItemKind::RetryConfirmedFailure
            )
        });
        if active_work {
            manifest.recalculate_summary_for_active_run();
            atomic_write_manifest(active_manifest_path, &manifest)?;
        } else if restarted_transients_changed {
            manifest.recalculate_summary();
            atomic_write_manifest(active_manifest_path, &manifest)?;
        }
        logger.kv("Resume manifest controls PLU selection", "yes")?;
        logger.kv(
            "Legacy import-selection flags were ignored",
            if options.limit.is_some() || options.test_mode {
                "yes"
            } else {
                "no"
            },
        )?;
        logger.kv("Already successful", &plan.already_successful.to_string())?;
        logger.kv(
            "Existing requests to poll",
            &plan.existing_requests_to_poll.to_string(),
        )?;
        logger.kv("Confirmed failures", &plan.confirmed_failures.to_string())?;
        logger.kv(
            "Ambiguous submissions",
            &plan.ambiguous_submissions.to_string(),
        )?;
        logger.kv("Not attempted", &plan.not_attempted_to_submit.to_string())?;
        print!(
            "RESUMING IMPORT\n\nManifest:\n{}\n\nSelected PLUs: {}\nAlready successful: {}\nExisting requests to poll: {}\nConfirmed failures: {}\nAmbiguous submissions: {}\nNot attempted: {}\n\n",
            active_manifest_path.display(),
            manifest.selection.selected_count,
            plan.already_successful,
            plan.existing_requests_to_poll,
            plan.confirmed_failures,
            plan.ambiguous_submissions,
            plan.not_attempted_to_submit
        );
    } else {
        if options.test_mode {
            logger.line("Test mode enabled: equivalent to --limit 1.")?;
        }
        if let Some(limit) = options.limit {
            logger.kv("Import limit", &limit.to_string())?;
            if plus.len() > selected_plus.len() {
                logger.warning(format!(
                    "{} PLU(s) will be intentionally excluded by the import limit.",
                    plus.len() - selected_plus.len()
                ))?;
            }
        }
    }
    if let Some(first) = selected_plus.first() {
        logger.kv("Selected first valid PLU", &first.plu_number.to_string())?;
        logger.kv(
            "Matching PluIng rows for selected PLU",
            &first.source_pluing_row_count.to_string(),
        )?;
        logger.kv(
            "Selected PLU group default applied",
            if first.group_default_applied {
                "yes"
            } else {
                "no"
            },
        )?;
    }

    logger.line("Authenticating with DIGIweb.")?;
    let client_secret = load_client_secret(&config)?;
    logger.kv(
        "Client secret",
        client_secret_log_message(&config, std::env::var("DIGIWEB_CLIENT_SECRET").is_ok()),
    )?;
    let token = authenticate(client.http(), &config, &client_secret).await?;
    logger.kv("Authentication result", "SUCCESS")?;

    let plan = if resume_manifest.is_some() {
        build_resume_plan(&manifest, options.retry_failed).items
    } else {
        manifest
            .records
            .iter()
            .map(|record| crate::recovery::model::ResumePlanItem {
                plu_number: record.plu_number,
                kind: ResumePlanItemKind::SubmitNotAttempted,
            })
            .collect::<Vec<_>>()
    };

    let mut interrupt_signal = Box::pin(tokio::signal::ctrl_c());
    let mut interrupted = false;
    for item in plan {
        match item.kind {
            ResumePlanItemKind::SkipAlreadySuccessful | ResumePlanItemKind::SkipFailed => {
                continue;
            }
            ResumePlanItemKind::SkipAmbiguous => {
                logger.warning(format!(
                    "PLU {} was not resent because its previous submission is ambiguous.",
                    item.plu_number
                ))?;
                if !continue_after_record_failure {
                    break;
                }
                continue;
            }
            ResumePlanItemKind::PollExistingRequest => {
                let record_index = manifest_record_index(&manifest, item.plu_number)?;
                let progress = format!("[resume:{}]", item.plu_number);
                let poll_result = tokio::select! {
                    result = poll_manifest_record(
                        &mut manifest,
                        record_index,
                        active_manifest_path,
                        &client,
                        &token,
                        logger,
                        &progress,
                    ) => Some(result),
                    signal = &mut interrupt_signal => {
                        if let Err(err) = signal {
                            Some(Err(AppError::Internal(format!("failed to listen for interrupt signal: {err}"))))
                        } else {
                            None
                        }
                    }
                };
                if let Some(result) = poll_result {
                    result?;
                } else {
                    persist_interrupted_manifest(&mut manifest, active_manifest_path, logger)?;
                    interrupted = true;
                    break;
                }
                if should_stop_after_manifest_record(&manifest.records[record_index])
                    && !continue_after_record_failure
                {
                    break;
                }
            }
            ResumePlanItemKind::SubmitNotAttempted | ResumePlanItemKind::RetryConfirmedFailure => {
                let record_index = manifest_record_index(&manifest, item.plu_number)?;
                let selection_index = manifest.records[record_index].selection_index;
                let selected_count = manifest.selection.selected_count;
                let plu = selected_plus
                    .iter()
                    .find(|plu| plu.plu_number == item.plu_number)
                    .ok_or_else(|| {
                        AppError::Internal(format!("selected PLU {} missing", item.plu_number))
                    })?;
                let payload = &payloads[selection_index - 1];
                let progress = format!("[{selection_index}/{selected_count}]");
                let submit_result = tokio::select! {
                    result = submit_manifest_record(
                        &mut manifest,
                        record_index,
                        active_manifest_path,
                        &client,
                        &token,
                        plu,
                        payload,
                        &config,
                        logger,
                        &progress,
                    ) => Some(result),
                    signal = &mut interrupt_signal => {
                        if let Err(err) = signal {
                            Some(Err(AppError::Internal(format!("failed to listen for interrupt signal: {err}"))))
                        } else {
                            None
                        }
                    }
                };
                if let Some(result) = submit_result {
                    result?;
                } else {
                    persist_interrupted_manifest(&mut manifest, active_manifest_path, logger)?;
                    interrupted = true;
                    break;
                }
                if should_stop_after_manifest_record(&manifest.records[record_index])
                    && !continue_after_record_failure
                {
                    break;
                }
            }
        }
    }
    if !interrupted {
        manifest.recalculate_summary();
        atomic_write_manifest(active_manifest_path, &manifest)?;
    }
    let status = manifest.run_status;
    logger.line(if resume_manifest.is_some() {
        "RESUME COMPLETE"
    } else {
        "IMPORT RUN COMPLETE"
    })?;
    logger.kv("Manifest status", status.as_text())?;
    logger.kv("Manifest", &active_manifest_path.display().to_string())?;
    print!(
        "{}\n\nManifest status: {}\n\nSelected PLUs: {}\nSuccessful: {}\nFailed: {}\nUnknown status: {}\nAmbiguous submissions: {}\nNot attempted: {}\n",
        if resume_manifest.is_some() {
            "RESUME COMPLETE"
        } else {
            "IMPORT RUN COMPLETE"
        },
        status.as_text(),
        manifest.selection.selected_count,
        manifest.summary.success,
        manifest.summary.failed,
        manifest.summary.unknown_status,
        manifest.summary.ambiguous_submission,
        manifest.summary.not_attempted
    );
    Ok(summary_from_manifest(&manifest, plus.len()))
}

fn persist_interrupted_manifest(
    manifest: &mut ImportManifest,
    manifest_path: &Path,
    logger: &mut AuditLogger,
) -> Result<(), AppError> {
    manifest.mark_interrupted();
    atomic_write_manifest(manifest_path, manifest)?;
    logger.warning("Import interrupted; recovery manifest was marked interrupted.")?;
    Ok(())
}

async fn submit_manifest_record(
    manifest: &mut ImportManifest,
    record_index: usize,
    manifest_path: &Path,
    client: &DigiwebClient,
    token: &crate::digiweb::auth::AccessToken,
    plu: &Plu,
    payload: &DigiwebPluPayload,
    config: &AppConfig,
    logger: &mut AuditLogger,
    progress: &str,
) -> Result<(), AppError> {
    let timer = std::time::Instant::now();
    logger.line(format!("{progress} Importing PLU {}", plu.plu_number))?;
    if config.import.write_payload_preview {
        let path = write_payload_preview(plu.plu_number, payload)?;
        logger.line(format!(
            "{progress} Payload preview written: {}",
            path.display()
        ))?;
    }

    {
        let record = &mut manifest.records[record_index];
        record.begin_attempt()?;
    }
    manifest.recalculate_summary_for_active_run();
    atomic_write_manifest(manifest_path, manifest)?;

    match client
        .submit_plu_once(token, payload, logger, progress)
        .await
    {
        Ok(outcome) => {
            if let Some(request_id) = outcome.request_id.clone() {
                {
                    let record = &mut manifest.records[record_index];
                    record.mark_request_accepted(
                        request_id,
                        Some(outcome.initial_status.as_str().to_string()),
                    )?;
                }
                manifest.recalculate_summary_for_active_run();
                atomic_write_manifest(manifest_path, manifest)?;
            }
            match outcome.initial_status {
                ProcessingStatus::Success => {
                    manifest.records[record_index].mark_success("SUCCESS")?;
                    logger.line(format!("{progress} Final status: SUCCESS"))?;
                }
                ProcessingStatus::Fail => {
                    let failure = outcome
                        .message
                        .unwrap_or_else(|| "DIGIweb final status FAIL".to_string());
                    manifest.records[record_index].mark_failed("DIGIweb processing", &failure)?;
                    logger.error(format!(
                        "{progress} PLU {} failed: {}",
                        plu.plu_number, failure
                    ))?;
                }
                ProcessingStatus::Processing
                    if manifest.records[record_index].request_id.is_some() =>
                {
                    poll_manifest_record(
                        manifest,
                        record_index,
                        manifest_path,
                        client,
                        token,
                        logger,
                        progress,
                    )
                    .await?;
                }
                _ if manifest.records[record_index].request_id.is_some() => {
                    let message = outcome.message.unwrap_or_else(|| {
                        "DIGIweb accepted the submission but the final status is unknown"
                            .to_string()
                    });
                    manifest.records[record_index].mark_unknown(message)?;
                    logger.warning(format!(
                        "{progress} PLU {} submitted with unknown final status",
                        plu.plu_number
                    ))?;
                }
                _ => {
                    let message = outcome.message.unwrap_or_else(|| {
                        "Submission result is unknown and no request id was recorded".to_string()
                    });
                    manifest.records[record_index].mark_ambiguous(message)?;
                    logger.warning(format!(
                        "{progress} PLU {} submission is ambiguous and will not be retried automatically",
                        plu.plu_number
                    ))?;
                }
            }
        }
        Err(err) if matches!(err, AppError::Network(_)) => {
            manifest.records[record_index].mark_ambiguous(err.to_string())?;
            logger.error(format!(
                "{progress} PLU {} submission is ambiguous after network error: {}",
                plu.plu_number, err
            ))?;
        }
        Err(err) => {
            manifest.records[record_index].mark_failed(err.stage(), err.to_string())?;
            logger.error(format!("{progress} PLU {} failed: {}", plu.plu_number, err))?;
        }
    }
    manifest.recalculate_summary_for_active_run();
    atomic_write_manifest(manifest_path, manifest)?;
    logger.line(format!(
        "{progress} Duration ms: {}",
        timer.elapsed().as_millis()
    ))?;
    Ok(())
}

fn build_payloads(plus: &[&Plu], config: &AppConfig) -> Result<Vec<DigiwebPluPayload>, AppError> {
    plus.iter()
        .map(|plu| DigiwebPluPayload::from_plu(plu, &config.digiweb))
        .collect()
}

fn selected_plus_from_manifest<'a>(
    plus: &'a [Plu],
    manifest: &ImportManifest,
) -> Result<Vec<&'a Plu>, AppError> {
    manifest
        .records
        .iter()
        .map(|record| {
            plus.iter()
                .find(|plu| plu.plu_number == record.plu_number)
                .ok_or_else(|| {
                    AppError::Config(format!(
                        "The normalized selected PLUs differ from the manifest. Missing PLU {}.",
                        record.plu_number
                    ))
                })
        })
        .collect()
}

fn manifest_record_index(manifest: &ImportManifest, plu_number: u64) -> Result<usize, AppError> {
    manifest
        .records
        .iter()
        .position(|record| record.plu_number == plu_number)
        .ok_or_else(|| {
            AppError::Internal(format!("manifest record not found for PLU {plu_number}"))
        })
}

fn should_stop_after_manifest_record(record: &PluManifestRecord) -> bool {
    matches!(
        record.status,
        RecordStatus::Failed | RecordStatus::UnknownStatus | RecordStatus::AmbiguousSubmission
    )
}

fn summary_from_manifest(manifest: &ImportManifest, valid_count: usize) -> ImportSummary {
    let submitted = manifest.summary.success
        + manifest.summary.failed
        + manifest.summary.unknown_status
        + manifest.summary.ambiguous_submission
        + manifest.summary.processing
        + manifest.summary.request_accepted
        + manifest.summary.submission_started;
    let unknown = manifest.summary.unknown_status
        + manifest.summary.ambiguous_submission
        + manifest.summary.processing
        + manifest.summary.request_accepted
        + manifest.summary.submission_started;
    ImportSummary {
        discovered: valid_count,
        selected: manifest.selection.selected_count,
        submitted,
        succeeded: manifest.summary.success,
        failed: manifest.summary.failed,
        unknown,
        intentionally_skipped_by_limit: manifest.selection.excluded_by_limit,
        not_attempted_after_stop: manifest.summary.not_attempted,
        records: manifest
            .records
            .iter()
            .filter(|record| record.status != RecordStatus::NotAttempted)
            .map(|record| RecordImportResult {
                plu_number: record.plu_number,
                started_at: record
                    .submission_started_at
                    .unwrap_or_else(|| record.completed_at.unwrap_or_else(Local::now)),
                api_request_id: record.request_id.clone(),
                http_result: if record.request_id.is_some() {
                    "2xx".to_string()
                } else {
                    "n/a".to_string()
                },
                final_status: match record.status {
                    RecordStatus::Success => ProcessingStatus::Success,
                    RecordStatus::Failed => ProcessingStatus::Fail,
                    _ => ProcessingStatus::SubmittedStatusUnknown,
                },
                failure_message: record.last_error.clone(),
                duration_ms: 0,
            })
            .collect(),
    }
}

async fn poll_manifest_record(
    manifest: &mut ImportManifest,
    record_index: usize,
    manifest_path: &Path,
    client: &DigiwebClient,
    token: &crate::digiweb::auth::AccessToken,
    logger: &mut AuditLogger,
    progress: &str,
) -> Result<(), AppError> {
    let request_id = manifest.records[record_index]
        .request_id
        .clone()
        .ok_or_else(|| {
            AppError::Internal(format!(
                "PLU {} cannot be polled without request id",
                manifest.records[record_index].plu_number
            ))
        })?;
    manifest.records[record_index].mark_processing("PROCESSING")?;
    manifest.recalculate_summary_for_active_run();
    atomic_write_manifest(manifest_path, manifest)?;

    match client.poll_request_status(token, &request_id, logger).await {
        Ok(response) if response.status == ProcessingStatus::Success => {
            manifest.records[record_index].mark_success(response.status.as_str())?;
            logger.line(format!("{progress} Final status: SUCCESS"))?;
        }
        Ok(response) if response.status == ProcessingStatus::Fail => {
            manifest.records[record_index].mark_failed(
                "DIGIweb processing",
                response
                    .message
                    .unwrap_or_else(|| "DIGIweb final status FAIL".to_string()),
            )?;
            logger.line(format!("{progress} Final status: FAIL"))?;
        }
        Ok(response) => {
            manifest.records[record_index].mark_unknown(
                response.message.unwrap_or_else(|| {
                    format!("DIGIweb final status {}", response.status.as_str())
                }),
            )?;
            logger.warning(format!(
                "{progress} Request {} remains unresolved",
                request_id
            ))?;
        }
        Err(err) => {
            manifest.records[record_index].mark_unknown(format!(
                "status polling failed for existing request {request_id}: {err}"
            ))?;
            logger.warning(format!(
                "{progress} Existing request {} status remains unknown: {}",
                request_id, err
            ))?;
        }
    }
    manifest.recalculate_summary_for_active_run();
    atomic_write_manifest(manifest_path, manifest)?;
    Ok(())
}

fn prepare_payload_preview_dir(enabled: bool) -> Result<(), AppError> {
    prepare_payload_preview_dir_in_dir(Path::new("."), enabled)
}

fn prepare_payload_preview_dir_in_dir(base_dir: &Path, enabled: bool) -> Result<(), AppError> {
    if !enabled {
        return Ok(());
    }
    let dir = base_dir.join("payload-previews");
    if !dir.exists() {
        return Ok(());
    }
    let entries = fs::read_dir(&dir).map_err(|err| {
        AppError::Logging(format!(
            "failed to inspect payload preview directory '{}': {err}",
            dir.display()
        ))
    })?;
    for entry in entries {
        let entry = entry.map_err(|err| {
            AppError::Logging(format!(
                "failed to inspect payload preview directory '{}': {err}",
                dir.display()
            ))
        })?;
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) == Some("json") {
            fs::remove_file(&path).map_err(|err| {
                AppError::Logging(format!(
                    "failed to remove old payload preview '{}': {err}",
                    path.display()
                ))
            })?;
        }
    }
    Ok(())
}

fn write_payload_preview(
    plu_number: u64,
    payload: &DigiwebPluPayload,
) -> Result<PathBuf, AppError> {
    write_payload_preview_in_dir(Path::new("."), plu_number, payload)
}

fn write_payload_preview_in_dir(
    base_dir: &Path,
    plu_number: u64,
    payload: &DigiwebPluPayload,
) -> Result<PathBuf, AppError> {
    let dir = base_dir.join("payload-previews");
    fs::create_dir_all(&dir).map_err(|err| {
        AppError::Logging(format!(
            "failed to create payload preview directory '{}': {err}",
            dir.display()
        ))
    })?;
    let path = dir.join(format!("plu-{plu_number}.json"));
    let preview = serde_json::to_string_pretty(payload)
        .map_err(|err| AppError::Internal(format!("payload preview failed: {err}")))?;
    fs::write(&path, preview).map_err(|err| {
        AppError::Logging(format!(
            "failed to write payload preview '{}': {err}",
            path.display()
        ))
    })?;
    Ok(fs::canonicalize(&path).unwrap_or(path))
}

pub fn select_records_to_send(plus: &[Plu], limit: Option<usize>) -> Vec<&Plu> {
    plus.iter().take(limit.unwrap_or(usize::MAX)).collect()
}

#[cfg(test)]
pub fn skipped_after_stop(
    selected_count: usize,
    succeeded: usize,
    failed: usize,
    unknown: usize,
) -> usize {
    selected_count.saturating_sub(succeeded + failed + unknown)
}

#[cfg(test)]
mod tests {
    use rust_decimal::Decimal;

    use super::*;
    use crate::models::plu::PriceMode;

    fn plu(plu_number: u64) -> Plu {
        Plu {
            plu_number,
            store_number: 1,
            department_number: Some(1),
            group_number: Some(1),
            source_department: Some("0001".to_string()),
            source_group: Some("1".to_string()),
            group_default_applied: false,
            name: format!("PLU {plu_number}"),
            barcode: Some(format!("020{plu_number:05}")),
            barcode_type: Some("5".to_string()),
            barcode_ref_no: Some("5".to_string()),
            source_barcode: Some(plu_number.to_string()),
            source_barcode_format: Some("05".to_string()),
            source_flag_data: Some("02".to_string()),
            price: Decimal::new(100, 2),
            price_mode: PriceMode::ByEach,
            price_calc_method: None,
            quantity: None,
            quantity_symbol: None,
            tare: None,
            discount_type: None,
            packing_date_print: None,
            packing_time_print: None,
            selling_date_print: None,
            selling_date_term: None,
            label_format: None,
            traceability: None,
            short_description: None,
            key_label: None,
            expiration_days: None,
            ingredients: None,
            nutrition_facts: Vec::new(),
            source_pluing_row_count: 0,
        }
    }

    #[test]
    fn limit_one_limits_selection_to_one_record() {
        let records = vec![plu(1), plu(2), plu(3)];

        let selected = select_records_to_send(&records, Some(1));

        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].plu_number, 1);
    }

    #[test]
    fn send_only_first_plu_selects_first_valid_normalized_plu() {
        let valid_after_row_skips = vec![plu(1), plu(2), plu(3)];

        let selected = select_records_to_send(&valid_after_row_skips, Some(1));

        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].plu_number, 1);
    }

    #[test]
    fn stop_after_first_selected_failure_does_not_double_count_unselected_plus() {
        let all_valid = vec![plu(1), plu(2), plu(3), plu(4)];
        let selected = select_records_to_send(&all_valid, Some(1));
        let skipped_by_first_plu_mode = all_valid.len() - selected.len();
        let skipped_after_failure = skipped_after_stop(selected.len(), 0, 1, 0);

        assert_eq!(skipped_by_first_plu_mode, 3);
        assert_eq!(skipped_after_failure, 0);
    }

    #[test]
    fn no_limit_selects_all_valid_plus() {
        let records = vec![plu(1), plu(4), plu(2), plu(3)];

        let selected = select_records_to_send(&records, None);

        assert_eq!(
            selected
                .iter()
                .map(|plu| plu.plu_number)
                .collect::<Vec<_>>(),
            vec![1, 4, 2, 3]
        );
    }

    #[test]
    fn limit_two_selects_first_two_valid_plus() {
        let records = vec![plu(1), plu(4), plu(2), plu(3)];

        let selected = select_records_to_send(&records, Some(2));

        assert_eq!(
            selected
                .iter()
                .map(|plu| plu.plu_number)
                .collect::<Vec<_>>(),
            vec![1, 4]
        );
    }

    #[test]
    fn large_limit_selects_all_valid_plus() {
        let records = vec![plu(1), plu(4), plu(2), plu(3)];

        let selected = select_records_to_send(&records, Some(10));

        assert_eq!(selected.len(), 4);
    }

    #[test]
    fn preview_file_is_written_for_payload() {
        let temp = tempfile::tempdir().expect("tempdir");
        let payload =
            DigiwebPluPayload::from_plu(&plu(1), &crate::config::DigiwebConfig::default())
                .expect("payload");

        let path = write_payload_preview_in_dir(temp.path(), 1, &payload).expect("preview");
        let contents = fs::read_to_string(&path).expect("read");

        assert!(contents.contains("\"pluno\": 1"));
        assert!(contents.contains("\"plubarcodetype\": \"5\""));
        assert!(!contents.to_ascii_lowercase().contains("secret"));
        assert!(!contents.to_ascii_lowercase().contains("token"));
    }

    #[test]
    fn preview_file_matches_submitted_json() {
        let temp = tempfile::tempdir().expect("tempdir");
        let payload =
            DigiwebPluPayload::from_plu(&plu(4), &crate::config::DigiwebConfig::default())
                .expect("payload");

        let path = write_payload_preview_in_dir(temp.path(), 4, &payload).expect("preview");
        let contents = fs::read_to_string(&path).expect("read");
        let expected = serde_json::to_string_pretty(&payload).expect("json");

        assert_eq!(contents, expected);
    }

    #[test]
    fn old_preview_json_files_are_cleaned_when_enabled() {
        let temp = tempfile::tempdir().expect("tempdir");
        let dir = temp.path().join("payload-previews");
        fs::create_dir_all(&dir).expect("dir");
        fs::write(dir.join("plu-1.json"), "{}").expect("old json");
        fs::write(dir.join("keep.txt"), "keep").expect("old txt");

        prepare_payload_preview_dir_in_dir(temp.path(), true).expect("clean");

        assert!(!dir.join("plu-1.json").exists());
        assert!(dir.join("keep.txt").exists());
    }

    #[test]
    fn preview_directory_is_not_created_when_disabled() {
        let temp = tempfile::tempdir().expect("tempdir");

        prepare_payload_preview_dir_in_dir(temp.path(), false).expect("disabled");

        assert!(!temp.path().join("payload-previews").exists());
    }
}
