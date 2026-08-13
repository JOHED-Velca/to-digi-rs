use std::collections::VecDeque;
use std::fs;
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::config::{AppConfig, client_secret_log_message, load_client_secret};
use crate::digiweb::auth::AuthSession;
use crate::digiweb::client::DigiwebClient;
use crate::digiweb::payload::DigiwebPluPayload;
use crate::digiweb::preflight::{ReferenceConfirmationStatus, evaluate_reference_readiness};
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
use crate::sanitization::SanitizationIntegration;
use crate::selection::{SelectionCriteria, SelectionMode, select_eligible_plus};
use chrono::Local;
use tokio::time::sleep;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImportRunOptions {
    pub limit: Option<usize>,
    pub requested_plu: Option<u64>,
    pub continue_after_record_failure: bool,
    pub test_mode: bool,
    pub retry_failed: bool,
}

#[derive(Debug, Clone)]
struct InFlightRequest {
    record_index: usize,
    request_id: String,
    accepted_at: Instant,
}

#[derive(Debug, Default)]
struct RuntimeImportMetrics {
    total_submissions: usize,
    total_polls: usize,
    max_in_flight_observed: usize,
    submission_latencies_ms: Vec<u128>,
    processing_latencies_ms: Vec<u128>,
}

#[derive(Debug, Clone, PartialEq)]
struct ProgressSnapshot {
    selected: usize,
    completed: usize,
    success: usize,
    failed: usize,
    unknown: usize,
    active: usize,
    remaining: usize,
    elapsed: Duration,
}

impl ProgressSnapshot {
    fn percent(&self) -> f64 {
        if self.selected == 0 {
            100.0
        } else {
            (self.completed as f64 / self.selected as f64) * 100.0
        }
    }

    fn rate_per_second(&self) -> f64 {
        let elapsed = self.elapsed.as_secs_f64();
        if elapsed <= 0.0 {
            0.0
        } else {
            self.completed as f64 / elapsed
        }
    }

    fn eta(&self) -> Option<Duration> {
        let rate = self.rate_per_second();
        if rate <= 0.0 || self.remaining == 0 {
            None
        } else {
            Some(Duration::from_secs_f64(self.remaining as f64 / rate))
        }
    }
}

struct ProgressReporter {
    interactive: bool,
    last_printed: Option<Instant>,
}

pub async fn run_import(
    config: AppConfig,
    plus: &[Plu],
    source_identity: SourceIdentity,
    target_identity: TargetIdentity,
    manifest_path: &Path,
    resume_manifest: Option<&Path>,
    sanitization: Option<SanitizationIntegration>,
    options: ImportRunOptions,
    logger: &mut AuditLogger,
) -> Result<ImportSummary, AppError> {
    config.token_url()?;
    config.plu_upsert_path()?;
    let criteria = SelectionCriteria {
        limit: options.limit,
        requested_plu: options.requested_plu,
        test_mode: options.test_mode,
    };
    if resume_manifest.is_none() {
        let selected_for_readiness = select_eligible_plus(
            plus,
            plus,
            &[],
            &crate::validation::validator::ValidationReport::default(),
            criteria,
        )
        .map_err(|failure| AppError::ValidationPayload(failure.message()))?;
        enforce_reference_readiness(&selected_for_readiness, &config, logger)?;
    }
    let client = DigiwebClient::new(config.clone())?;

    prepare_payload_preview_dir(config.import.write_payload_preview)?;

    let mut manifest = if let Some(path) = resume_manifest {
        logger.line("RESUMING IMPORT")?;
        logger.kv("Manifest", &path.display().to_string())?;
        load_manifest(path)?
    } else {
        let selected = select_records_to_send(plus, criteria);
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
        let mut manifest = ImportManifest::new(
            source_identity.clone(),
            target_identity.clone(),
            ManifestOptions {
                limit: options.limit,
                selection_mode: criteria.mode(),
                requested_plu: criteria.requested_plu,
                continue_on_error: options.continue_after_record_failure,
                test_alias_used: options.test_mode,
            },
            plus.len(),
            records,
        );
        if let Some(sanitization) = &sanitization {
            let snapshot_name = "sanitization-profile.snapshot.toml";
            let snapshot_path = manifest_path
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .join(snapshot_name);
            write_profile_snapshot(&snapshot_path, &sanitization.normalized_profile_toml)?;
            manifest.sanitization = sanitization.manifest_metadata(snapshot_name);
        }
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
        enforce_reference_readiness(&selected_plus, &config, logger)?;
    }

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
            if options.limit.is_some() || options.test_mode || options.requested_plu.is_some() {
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
        if let Some(plu) = options.requested_plu {
            logger.kv("Selection mode", SelectionMode::Plu.as_str())?;
            logger.kv("Requested PLU", &plu.to_string())?;
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
        client_secret_log_message(&config, environment_secret_present()),
    )?;
    let mut auth_session = AuthSession::start(client.http(), &config, client_secret).await?;
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

    let run_timer = Instant::now();
    let mut runtime_metrics = RuntimeImportMetrics::default();
    let interrupted = run_bounded_import_loop(
        &mut manifest,
        active_manifest_path,
        &client,
        &mut auth_session,
        &selected_plus,
        &payloads,
        &config,
        continue_after_record_failure,
        plan,
        logger,
        &mut runtime_metrics,
        run_timer,
    )
    .await?;
    if !interrupted {
        manifest.recalculate_summary();
        finalize_manifest_metrics(
            &mut manifest,
            &runtime_metrics,
            run_timer.elapsed(),
            auth_session.refresh_count(),
        );
        atomic_write_manifest(active_manifest_path, &manifest)?;
    } else {
        finalize_manifest_metrics(
            &mut manifest,
            &runtime_metrics,
            run_timer.elapsed(),
            auth_session.refresh_count(),
        );
        atomic_write_manifest(&active_manifest_path, &manifest)?;
    }
    let status = manifest.run_status;
    logger.line(if resume_manifest.is_some() {
        "RESUME COMPLETE"
    } else {
        "IMPORT RUN COMPLETE"
    })?;
    logger.kv("Manifest status", status.as_text())?;
    logger.kv("Manifest", &active_manifest_path.display().to_string())?;
    logger.kv(
        "Max in-flight observed",
        &manifest.metrics.max_in_flight_observed.to_string(),
    )?;
    logger.kv(
        "Total submissions",
        &manifest.metrics.total_submissions.to_string(),
    )?;
    logger.kv("Total polls", &manifest.metrics.total_polls.to_string())?;
    logger.kv(
        "Average PLUs/sec",
        &manifest.metrics.average_plus_per_second,
    )?;
    print!(
        "{}\n\nManifest status: {}\n\nSelected PLUs: {}\nSuccessful: {}\nFailed: {}\nUnknown status: {}\nAmbiguous submissions: {}\nNot attempted: {}\nMax in-flight observed: {}\nAverage PLUs/sec: {}\n",
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
        manifest.summary.not_attempted,
        manifest.metrics.max_in_flight_observed,
        manifest.metrics.average_plus_per_second
    );
    Ok(summary_from_manifest(&manifest, plus.len()))
}

async fn run_bounded_import_loop(
    manifest: &mut ImportManifest,
    manifest_path: &Path,
    client: &DigiwebClient,
    auth_session: &mut AuthSession,
    selected_plus: &[&Plu],
    payloads: &[DigiwebPluPayload],
    config: &AppConfig,
    continue_after_record_failure: bool,
    plan: Vec<crate::recovery::model::ResumePlanItem>,
    logger: &mut AuditLogger,
    metrics: &mut RuntimeImportMetrics,
    run_started: Instant,
) -> Result<bool, AppError> {
    let max_in_flight = config.import.max_in_flight.clamp(1, 64);
    let poll_interval = Duration::from_millis(config.timeouts.poll_interval_millis.max(1));
    logger.kv("Max in-flight requests", &max_in_flight.to_string())?;
    logger.kv(
        "Shared poll interval ms",
        &config.timeouts.poll_interval_millis.to_string(),
    )?;
    let mut pending = VecDeque::from(plan);
    let mut in_flight = Vec::<InFlightRequest>::new();
    let mut stop_new_submissions = false;
    let mut interrupt_signal = Box::pin(tokio::signal::ctrl_c());
    let mut progress = ProgressReporter::new();
    progress.print(true, manifest, in_flight.len(), run_started)?;

    loop {
        while !stop_new_submissions && in_flight.len() < max_in_flight {
            let Some(item) = pending.pop_front() else {
                break;
            };
            match item.kind {
                ResumePlanItemKind::SkipAlreadySuccessful | ResumePlanItemKind::SkipFailed => {}
                ResumePlanItemKind::SkipAmbiguous => {
                    logger.warning(format!(
                        "PLU {} was not resent because its previous submission is ambiguous.",
                        item.plu_number
                    ))?;
                    if !continue_after_record_failure {
                        stop_new_submissions = true;
                    }
                }
                ResumePlanItemKind::PollExistingRequest => {
                    let record_index = manifest_record_index(manifest, item.plu_number)?;
                    let request_id = manifest.records[record_index]
                        .request_id
                        .clone()
                        .ok_or_else(|| {
                            AppError::Internal(format!(
                                "PLU {} cannot be resumed without request id",
                                item.plu_number
                            ))
                        })?;
                    in_flight.push(InFlightRequest {
                        record_index,
                        request_id,
                        accepted_at: Instant::now(),
                    });
                    metrics.max_in_flight_observed =
                        metrics.max_in_flight_observed.max(in_flight.len());
                }
                ResumePlanItemKind::SubmitNotAttempted
                | ResumePlanItemKind::RetryConfirmedFailure => {
                    let record_index = manifest_record_index(manifest, item.plu_number)?;
                    let selection_index = manifest.records[record_index].selection_index;
                    let plu = selected_plus
                        .iter()
                        .find(|plu| plu.plu_number == item.plu_number)
                        .ok_or_else(|| {
                            AppError::Internal(format!("selected PLU {} missing", item.plu_number))
                        })?;
                    let payload = &payloads[selection_index - 1];
                    let progress_label =
                        format!("[{selection_index}/{}]", manifest.selection.selected_count);
                    let submit_result = tokio::select! {
                        result = submit_manifest_record_without_polling(
                            manifest,
                            record_index,
                            manifest_path,
                            client,
                            auth_session,
                            plu,
                            payload,
                            config,
                            logger,
                            &progress_label,
                            metrics,
                        ) => Some(result),
                        signal = &mut interrupt_signal => {
                            if let Err(err) = signal {
                                Some(Err(AppError::Internal(format!("failed to listen for interrupt signal: {err}"))))
                            } else {
                                None
                            }
                        }
                    };
                    let Some(submit_result) = submit_result else {
                        persist_interrupted_manifest(manifest, manifest_path, logger)?;
                        return Ok(true);
                    };
                    if let Some(active) = submit_result? {
                        in_flight.push(active);
                        metrics.max_in_flight_observed =
                            metrics.max_in_flight_observed.max(in_flight.len());
                    }
                    if should_stop_after_manifest_record(&manifest.records[record_index])
                        && !continue_after_record_failure
                    {
                        stop_new_submissions = true;
                    }
                }
            }
            progress.print(false, manifest, in_flight.len(), run_started)?;
        }

        if in_flight.is_empty() {
            if pending.is_empty() || stop_new_submissions {
                break;
            }
            continue;
        }

        let mut index = 0;
        while index < in_flight.len() {
            let progress_label = format!(
                "[{}/{}]",
                manifest.records[in_flight[index].record_index].selection_index,
                manifest.selection.selected_count
            );
            if in_flight_timed_out(&in_flight[index], config) {
                mark_in_flight_timeout(
                    manifest,
                    &in_flight[index],
                    manifest_path,
                    logger,
                    &progress_label,
                )?;
                let active = in_flight.remove(index);
                metrics
                    .processing_latencies_ms
                    .push(active.accepted_at.elapsed().as_millis());
                if should_stop_after_manifest_record(&manifest.records[active.record_index])
                    && !continue_after_record_failure
                {
                    stop_new_submissions = true;
                }
                progress.print(false, manifest, in_flight.len(), run_started)?;
                continue;
            }
            let poll_result = tokio::select! {
                result = poll_in_flight_record_once(
                    manifest,
                    &in_flight[index],
                    manifest_path,
                    client,
                    auth_session,
                    logger,
                    &progress_label,
                    metrics,
                ) => Some(result),
                signal = &mut interrupt_signal => {
                    if let Err(err) = signal {
                        Some(Err(AppError::Internal(format!("failed to listen for interrupt signal: {err}"))))
                    } else {
                        None
                    }
                }
            };
            let Some(poll_result) = poll_result else {
                persist_interrupted_manifest(manifest, manifest_path, logger)?;
                return Ok(true);
            };
            let terminal = poll_result?;
            if terminal {
                let active = in_flight.remove(index);
                metrics
                    .processing_latencies_ms
                    .push(active.accepted_at.elapsed().as_millis());
                if should_stop_after_manifest_record(&manifest.records[active.record_index])
                    && !continue_after_record_failure
                {
                    stop_new_submissions = true;
                }
            } else {
                index += 1;
            }
            progress.print(false, manifest, in_flight.len(), run_started)?;
        }

        if in_flight.is_empty() && (pending.is_empty() || stop_new_submissions) {
            break;
        }
        let sleep_result = tokio::select! {
            _ = sleep(poll_interval) => Some(()),
            signal = &mut interrupt_signal => {
                if let Err(err) = signal {
                    return Err(AppError::Internal(format!("failed to listen for interrupt signal: {err}")));
                }
                None
            }
        };
        if sleep_result.is_none() {
            persist_interrupted_manifest(manifest, manifest_path, logger)?;
            return Ok(true);
        }
    }

    progress.print(true, manifest, in_flight.len(), run_started)?;
    Ok(false)
}

async fn submit_manifest_record_without_polling(
    manifest: &mut ImportManifest,
    record_index: usize,
    manifest_path: &Path,
    client: &DigiwebClient,
    auth_session: &mut AuthSession,
    plu: &Plu,
    payload: &DigiwebPluPayload,
    config: &AppConfig,
    logger: &mut AuditLogger,
    progress: &str,
    metrics: &mut RuntimeImportMetrics,
) -> Result<Option<InFlightRequest>, AppError> {
    let timer = Instant::now();
    logger.line(format!("{progress} Importing PLU {}", plu.plu_number))?;
    if config.import.write_payload_preview {
        let path = write_payload_preview(plu.plu_number, payload)?;
        logger.line(format!(
            "{progress} Payload preview written: {}",
            path.display()
        ))?;
    }

    manifest.records[record_index].begin_attempt()?;
    manifest.recalculate_summary_for_active_run();
    atomic_write_manifest(manifest_path, manifest)?;

    metrics.total_submissions = metrics.total_submissions.saturating_add(1);
    let refresh_count_before = auth_session.refresh_count();
    match client
        .submit_plu_with_auth_session(auth_session, payload, logger, progress)
        .await
    {
        Ok(outcome) => {
            metrics
                .submission_latencies_ms
                .push(timer.elapsed().as_millis());
            manifest.records[record_index].set_authentication_retries(
                auth_session
                    .refresh_count()
                    .saturating_sub(refresh_count_before),
            );
            if let Some(request_id) = outcome.request_id.clone() {
                manifest.records[record_index].mark_request_accepted(
                    request_id.clone(),
                    Some(outcome.initial_status.as_str().to_string()),
                )?;
                manifest.recalculate_summary_for_active_run();
                atomic_write_manifest(manifest_path, manifest)?;
                if outcome.initial_status == ProcessingStatus::Processing {
                    return Ok(Some(InFlightRequest {
                        record_index,
                        request_id,
                        accepted_at: Instant::now(),
                    }));
                }
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
                ProcessingStatus::Processing => {
                    let message = outcome.message.unwrap_or_else(|| {
                        "DIGIweb accepted the submission but no request id was recorded".to_string()
                    });
                    manifest.records[record_index].mark_unknown(message)?;
                    logger.warning(format!(
                        "{progress} PLU {} submitted with unknown final status",
                        plu.plu_number
                    ))?;
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
            manifest.records[record_index].set_authentication_retries(
                auth_session
                    .refresh_count()
                    .saturating_sub(refresh_count_before),
            );
            manifest.records[record_index].mark_ambiguous(err.to_string())?;
            logger.error(format!(
                "{progress} PLU {} submission is ambiguous after network error: {}",
                plu.plu_number, err
            ))?;
        }
        Err(err) if matches!(err, AppError::Auth(_)) => {
            manifest.records[record_index].set_authentication_retries(
                auth_session
                    .refresh_count()
                    .saturating_sub(refresh_count_before),
            );
            manifest.records[record_index].mark_failed(err.stage(), err.to_string())?;
            manifest.recalculate_summary_for_active_run();
            atomic_write_manifest(manifest_path, manifest)?;
            logger.error("IMPORT STOPPED - AUTHENTICATION COULD NOT BE RESTORED")?;
            logger.error(format!("{progress} PLU {} failed: {}", plu.plu_number, err))?;
            return Err(err);
        }
        Err(err) => {
            manifest.records[record_index].mark_failed(err.stage(), err.to_string())?;
            logger.error(format!("{progress} PLU {} failed: {}", plu.plu_number, err))?;
        }
    }
    manifest.recalculate_summary_for_active_run();
    atomic_write_manifest(manifest_path, manifest)?;
    logger.line(format!(
        "{progress} Submission duration ms: {}",
        timer.elapsed().as_millis()
    ))?;
    Ok(None)
}

async fn poll_in_flight_record_once(
    manifest: &mut ImportManifest,
    active: &InFlightRequest,
    manifest_path: &Path,
    client: &DigiwebClient,
    auth_session: &mut AuthSession,
    logger: &mut AuditLogger,
    progress: &str,
    metrics: &mut RuntimeImportMetrics,
) -> Result<bool, AppError> {
    metrics.total_polls = metrics.total_polls.saturating_add(1);
    let refresh_count_before = auth_session.refresh_count();
    match client
        .poll_request_status_once_with_auth_session(
            auth_session,
            &active.request_id,
            logger,
            Some(progress),
        )
        .await
    {
        Ok(response) if response.status == ProcessingStatus::Success => {
            manifest.records[active.record_index].set_authentication_retries(
                auth_session
                    .refresh_count()
                    .saturating_sub(refresh_count_before),
            );
            manifest.records[active.record_index].mark_success(response.status.as_str())?;
            logger.line(format!("{progress} Final status: SUCCESS"))?;
            manifest.recalculate_summary_for_active_run();
            atomic_write_manifest(manifest_path, manifest)?;
            Ok(true)
        }
        Ok(response) if response.status == ProcessingStatus::Fail => {
            manifest.records[active.record_index].set_authentication_retries(
                auth_session
                    .refresh_count()
                    .saturating_sub(refresh_count_before),
            );
            manifest.records[active.record_index].mark_failed(
                "DIGIweb processing",
                response
                    .message
                    .unwrap_or_else(|| "DIGIweb final status FAIL".to_string()),
            )?;
            logger.line(format!("{progress} Final status: FAIL"))?;
            manifest.recalculate_summary_for_active_run();
            atomic_write_manifest(manifest_path, manifest)?;
            Ok(true)
        }
        Ok(response) if response.status == ProcessingStatus::Processing => {
            let was_already_processing =
                manifest.records[active.record_index].status == RecordStatus::Processing;
            manifest.records[active.record_index].set_authentication_retries(
                auth_session
                    .refresh_count()
                    .saturating_sub(refresh_count_before),
            );
            manifest.records[active.record_index].mark_processing(response.status.as_str())?;
            if !was_already_processing {
                manifest.recalculate_summary_for_active_run();
                atomic_write_manifest(manifest_path, manifest)?;
            }
            Ok(false)
        }
        Ok(response) => {
            manifest.records[active.record_index].set_authentication_retries(
                auth_session
                    .refresh_count()
                    .saturating_sub(refresh_count_before),
            );
            manifest.records[active.record_index].mark_unknown(
                response.message.unwrap_or_else(|| {
                    format!("DIGIweb final status {}", response.status.as_str())
                }),
            )?;
            logger.warning(format!(
                "{progress} Request {} remains unresolved",
                active.request_id
            ))?;
            manifest.recalculate_summary_for_active_run();
            atomic_write_manifest(manifest_path, manifest)?;
            Ok(true)
        }
        Err(err) if matches!(err, AppError::Auth(_)) => {
            manifest.records[active.record_index].set_authentication_retries(
                auth_session
                    .refresh_count()
                    .saturating_sub(refresh_count_before),
            );
            manifest.records[active.record_index].mark_unknown(format!(
                "status polling failed for existing request {}: {err}",
                active.request_id
            ))?;
            manifest.recalculate_summary_for_active_run();
            atomic_write_manifest(manifest_path, manifest)?;
            logger.warning(format!(
                "{progress} Existing request {} status remains unknown: {}",
                active.request_id, err
            ))?;
            Err(err)
        }
        Err(err) => {
            manifest.records[active.record_index].set_authentication_retries(
                auth_session
                    .refresh_count()
                    .saturating_sub(refresh_count_before),
            );
            manifest.records[active.record_index].mark_unknown(format!(
                "status polling failed for existing request {}: {err}",
                active.request_id
            ))?;
            logger.warning(format!(
                "{progress} Existing request {} status remains unknown: {}",
                active.request_id, err
            ))?;
            manifest.recalculate_summary_for_active_run();
            atomic_write_manifest(manifest_path, manifest)?;
            Ok(true)
        }
    }
}

fn in_flight_timed_out(active: &InFlightRequest, config: &AppConfig) -> bool {
    active.accepted_at.elapsed() >= Duration::from_secs(config.timeouts.poll_timeout_seconds)
}

fn mark_in_flight_timeout(
    manifest: &mut ImportManifest,
    active: &InFlightRequest,
    manifest_path: &Path,
    logger: &mut AuditLogger,
    progress: &str,
) -> Result<(), AppError> {
    manifest.records[active.record_index].mark_unknown(format!(
        "{} while polling existing request {}",
        ProcessingStatus::UnknownOrTimeout.as_str(),
        active.request_id
    ))?;
    logger.warning(format!(
        "{progress} Request {} timed out before final DIGIweb status was confirmed",
        active.request_id
    ))?;
    manifest.recalculate_summary_for_active_run();
    atomic_write_manifest(manifest_path, manifest)
}

fn finalize_manifest_metrics(
    manifest: &mut ImportManifest,
    runtime: &RuntimeImportMetrics,
    elapsed: Duration,
    authentication_refresh_count: u32,
) {
    let completed = manifest.summary.success
        + manifest.summary.failed
        + manifest.summary.unknown_status
        + manifest.summary.ambiguous_submission;
    let rate = if elapsed.as_secs_f64() > 0.0 {
        completed as f64 / elapsed.as_secs_f64()
    } else {
        0.0
    };
    manifest.metrics.elapsed_ms = elapsed.as_millis();
    manifest.metrics.average_plus_per_second = format!("{rate:.2}");
    manifest.metrics.max_in_flight_observed = runtime.max_in_flight_observed;
    manifest.metrics.total_submissions = runtime.total_submissions;
    manifest.metrics.total_polls = runtime.total_polls;
    manifest.metrics.authentication_refresh_count = authentication_refresh_count;
    manifest.metrics.average_submission_latency_ms = average_u128(&runtime.submission_latencies_ms);
    manifest.metrics.average_processing_latency_ms = average_u128(&runtime.processing_latencies_ms);
}

fn average_u128(values: &[u128]) -> Option<u128> {
    if values.is_empty() {
        None
    } else {
        Some(values.iter().sum::<u128>() / values.len() as u128)
    }
}

impl ProgressReporter {
    fn new() -> Self {
        Self {
            interactive: io::stdout().is_terminal(),
            last_printed: None,
        }
    }

    fn print(
        &mut self,
        force: bool,
        manifest: &ImportManifest,
        active: usize,
        started: Instant,
    ) -> Result<(), AppError> {
        let now = Instant::now();
        if !force
            && self
                .last_printed
                .is_some_and(|last| now.duration_since(last) < Duration::from_secs(5))
        {
            return Ok(());
        }
        self.last_printed = Some(now);
        let snapshot = progress_snapshot(manifest, active, started.elapsed());
        if self.interactive {
            print!("\r{}", render_interactive_progress(&snapshot));
            io::stdout()
                .flush()
                .map_err(|err| AppError::Logging(format!("failed to flush progress: {err}")))?;
            if force && snapshot.completed == snapshot.selected {
                println!();
            }
        } else {
            println!("{}", render_noninteractive_progress(&snapshot));
        }
        Ok(())
    }
}

fn progress_snapshot(
    manifest: &ImportManifest,
    active: usize,
    elapsed: Duration,
) -> ProgressSnapshot {
    let unknown = manifest.summary.unknown_status + manifest.summary.ambiguous_submission;
    let completed = manifest.summary.success + manifest.summary.failed + unknown;
    ProgressSnapshot {
        selected: manifest.selection.selected_count,
        completed,
        success: manifest.summary.success,
        failed: manifest.summary.failed,
        unknown,
        active,
        remaining: manifest.summary.not_attempted,
        elapsed,
    }
}

fn render_interactive_progress(snapshot: &ProgressSnapshot) -> String {
    render_interactive_progress_with_options(snapshot, terminal_width(), unicode_progress_enabled())
}

fn render_interactive_progress_with_options(
    snapshot: &ProgressSnapshot,
    terminal_width: usize,
    unicode: bool,
) -> String {
    let bar_width = terminal_width.saturating_sub(96).clamp(12, 32);
    let filled = ((snapshot.percent() / 100.0) * bar_width as f64).round() as usize;
    let filled = filled.min(bar_width);
    let (filled_char, empty_char) = if unicode { ("█", "░") } else { ("#", "-") };
    let bar = format!(
        "{}{}",
        filled_char.repeat(filled),
        empty_char.repeat(bar_width - filled)
    );
    let unknown_segment = if snapshot.unknown > 0 {
        format!(" | ? {}", snapshot.unknown)
    } else {
        String::new()
    };
    format!(
        "Importing [{bar}] {:.1}% {}/{} | ok {} | fail {}{} | active {} | {:.1}/s | {} | ETA {}",
        snapshot.percent(),
        snapshot.completed,
        snapshot.selected,
        snapshot.success,
        snapshot.failed,
        unknown_segment,
        snapshot.active,
        snapshot.rate_per_second(),
        format_duration(snapshot.elapsed),
        snapshot
            .eta()
            .map(format_duration)
            .unwrap_or_else(|| "--:--".to_string())
    )
}

fn terminal_width() -> usize {
    std::env::var("COLUMNS")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value >= 40)
        .unwrap_or(100)
}

fn unicode_progress_enabled() -> bool {
    !std::env::var("TO_DIGI_RS_ASCII_PROGRESS")
        .map(|value| matches!(value.trim(), "1" | "true" | "TRUE" | "yes" | "YES"))
        .unwrap_or(false)
}

fn render_noninteractive_progress(snapshot: &ProgressSnapshot) -> String {
    format!(
        "PROGRESS selected={} completed={} percent={:.1} success={} failed={} unknown={} active={} remaining={} rate={:.1}/s elapsed={} eta={}",
        snapshot.selected,
        snapshot.completed,
        snapshot.percent(),
        snapshot.success,
        snapshot.failed,
        snapshot.unknown,
        snapshot.active,
        snapshot.remaining,
        snapshot.rate_per_second(),
        format_duration(snapshot.elapsed),
        snapshot
            .eta()
            .map(format_duration)
            .unwrap_or_else(|| "unknown".to_string())
    )
}

fn format_duration(duration: Duration) -> String {
    let total = duration.as_secs();
    let hours = total / 3600;
    let minutes = (total % 3600) / 60;
    let seconds = total % 60;
    if hours > 0 {
        format!("{hours:02}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes:02}:{seconds:02}")
    }
}

fn enforce_reference_readiness(
    selected_plus: &[&Plu],
    config: &AppConfig,
    logger: &mut AuditLogger,
) -> Result<(), AppError> {
    let selected_for_readiness_owned = selected_plus
        .iter()
        .map(|plu| (*plu).clone())
        .collect::<Vec<_>>();
    let reference_readiness =
        evaluate_reference_readiness(&selected_for_readiness_owned, &config.verification, 0)?;
    if reference_readiness.is_ready() {
        return Ok(());
    }

    logger.line("IMPORT BLOCKED")?;
    logger.line("Unverified required references:")?;
    for department in reference_readiness
        .departments
        .iter()
        .filter(|reference| reference.status == ReferenceConfirmationStatus::Unverified)
    {
        logger.line(format!(
            "- Department {} | Used by: {} PLUs | Examples: {}",
            department.number,
            department.source_plu_numbers.len(),
            format_limited_u64s(&department.source_plu_numbers, 8)
        ))?;
    }
    for group in reference_readiness
        .groups
        .iter()
        .filter(|reference| reference.status == ReferenceConfirmationStatus::Unverified)
    {
        logger.line(format!(
            "- Department {} / Group {} | Used by: {} PLUs | Examples: {}",
            group.department,
            group.number,
            group.source_plu_numbers.len(),
            format_limited_u64s(&group.source_plu_numbers, 8)
        ))?;
    }
    for label_format in reference_readiness
        .label_formats
        .iter()
        .filter(|reference| reference.status == ReferenceConfirmationStatus::Unverified)
    {
        logger.line(format!(
            "- Label Format {} | Used by: {} PLUs | Examples: {}",
            label_format.number,
            label_format.source_plu_numbers.len(),
            format_limited_u64s(&label_format.source_plu_numbers, 8)
        ))?;
    }
    logger.line("No PLUs were submitted.")?;
    logger.kv("PLU write requests", "0")?;
    logger.line("Run: ./to-digi verify")?;
    print!(
        "IMPORT BLOCKED\n\nUnverified required references: {}\n\nNo PLUs were submitted.\n\nRun:\n./to-digi verify\n\n",
        reference_readiness.unverified_reference_count
    );
    Err(AppError::ValidationPayload(
        "import blocked by unverified required DIGIweb references".to_string(),
    ))
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

#[allow(dead_code)]
async fn submit_manifest_record(
    manifest: &mut ImportManifest,
    record_index: usize,
    manifest_path: &Path,
    client: &DigiwebClient,
    auth_session: &mut crate::digiweb::auth::AuthSession,
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

    let refresh_count_before = auth_session.refresh_count();
    match client
        .submit_plu_with_auth_session(auth_session, payload, logger, progress)
        .await
    {
        Ok(outcome) => {
            manifest.records[record_index].set_authentication_retries(
                auth_session
                    .refresh_count()
                    .saturating_sub(refresh_count_before),
            );
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
                        auth_session,
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
            manifest.records[record_index].set_authentication_retries(
                auth_session
                    .refresh_count()
                    .saturating_sub(refresh_count_before),
            );
            manifest.records[record_index].mark_ambiguous(err.to_string())?;
            logger.error(format!(
                "{progress} PLU {} submission is ambiguous after network error: {}",
                plu.plu_number, err
            ))?;
        }
        Err(err) if matches!(err, AppError::Auth(_)) => {
            manifest.records[record_index].set_authentication_retries(
                auth_session
                    .refresh_count()
                    .saturating_sub(refresh_count_before),
            );
            manifest.records[record_index].mark_failed(err.stage(), err.to_string())?;
            manifest.recalculate_summary_for_active_run();
            atomic_write_manifest(manifest_path, manifest)?;
            logger.error(format!("{progress} PLU {} failed: {}", plu.plu_number, err))?;
            logger.error("IMPORT STOPPED - AUTHENTICATION COULD NOT BE RESTORED")?;
            logger.line("No additional PLUs were submitted.")?;
            logger.line("The recovery manifest was preserved.")?;
            println!("IMPORT STOPPED - AUTHENTICATION COULD NOT BE RESTORED");
            println!("No additional PLUs were submitted.");
            println!("The recovery manifest was preserved.");
            return Err(err);
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

#[allow(dead_code)]
async fn poll_manifest_record(
    manifest: &mut ImportManifest,
    record_index: usize,
    manifest_path: &Path,
    client: &DigiwebClient,
    auth_session: &mut crate::digiweb::auth::AuthSession,
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

    let refresh_count_before = auth_session.refresh_count();
    match client
        .poll_request_status_with_auth_session(auth_session, &request_id, logger)
        .await
    {
        Ok(response) if response.status == ProcessingStatus::Success => {
            manifest.records[record_index].set_authentication_retries(
                auth_session
                    .refresh_count()
                    .saturating_sub(refresh_count_before),
            );
            manifest.records[record_index].mark_success(response.status.as_str())?;
            logger.line(format!("{progress} Final status: SUCCESS"))?;
        }
        Ok(response) if response.status == ProcessingStatus::Fail => {
            manifest.records[record_index].set_authentication_retries(
                auth_session
                    .refresh_count()
                    .saturating_sub(refresh_count_before),
            );
            manifest.records[record_index].mark_failed(
                "DIGIweb processing",
                response
                    .message
                    .unwrap_or_else(|| "DIGIweb final status FAIL".to_string()),
            )?;
            logger.line(format!("{progress} Final status: FAIL"))?;
        }
        Ok(response) => {
            manifest.records[record_index].set_authentication_retries(
                auth_session
                    .refresh_count()
                    .saturating_sub(refresh_count_before),
            );
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
        Err(err) if matches!(err, AppError::Auth(_)) => {
            manifest.records[record_index].set_authentication_retries(
                auth_session
                    .refresh_count()
                    .saturating_sub(refresh_count_before),
            );
            manifest.records[record_index].mark_unknown(format!(
                "status polling failed for existing request {request_id}: {err}"
            ))?;
            manifest.recalculate_summary_for_active_run();
            atomic_write_manifest(manifest_path, manifest)?;
            logger.warning(format!(
                "{progress} Existing request {} status remains unknown: {}",
                request_id, err
            ))?;
            logger.error("IMPORT STOPPED - AUTHENTICATION COULD NOT BE RESTORED")?;
            logger.line("No additional PLUs were submitted.")?;
            logger.line("The recovery manifest was preserved.")?;
            println!("IMPORT STOPPED - AUTHENTICATION COULD NOT BE RESTORED");
            println!("No additional PLUs were submitted.");
            println!("The recovery manifest was preserved.");
            return Err(err);
        }
        Err(err) => {
            manifest.records[record_index].set_authentication_retries(
                auth_session
                    .refresh_count()
                    .saturating_sub(refresh_count_before),
            );
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

fn write_profile_snapshot(path: &Path, contents: &str) -> Result<(), AppError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| {
            AppError::Logging(format!(
                "failed to create profile snapshot directory '{}': {err}",
                parent.display()
            ))
        })?;
    }
    fs::write(path, contents).map_err(|err| {
        AppError::Logging(format!(
            "failed to write sanitization profile snapshot '{}': {err}",
            path.display()
        ))
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).map_err(|err| {
            AppError::Logging(format!(
                "failed to set sanitization profile snapshot permissions '{}': {err}",
                path.display()
            ))
        })?;
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

pub fn select_records_to_send(plus: &[Plu], criteria: SelectionCriteria) -> Vec<&Plu> {
    if let Some(plu_number) = criteria.requested_plu {
        plus.iter()
            .filter(|plu| plu.plu_number == plu_number)
            .collect()
    } else {
        plus.iter()
            .take(criteria.limit.unwrap_or(usize::MAX))
            .collect()
    }
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
    use crate::recovery::load_manifest;

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
            source_tare: None,
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
            nutrition_profile: None,
            nutrition_remaps: Vec::new(),
            source_pluing_row_count: 0,
        }
    }

    fn criteria(
        limit: Option<usize>,
        requested_plu: Option<u64>,
        test_mode: bool,
    ) -> SelectionCriteria {
        SelectionCriteria {
            limit,
            requested_plu,
            test_mode,
        }
    }

    fn progress_manifest(statuses: &[RecordStatus]) -> ImportManifest {
        let records = statuses
            .iter()
            .enumerate()
            .map(|(index, status)| {
                let mut record = PluManifestRecord::new(
                    (index + 1) as u64,
                    Some(1),
                    Some(1),
                    index + 1,
                    format!("hash-{index}"),
                );
                record.status = *status;
                record
            })
            .collect::<Vec<_>>();
        let mut manifest = ImportManifest::new(
            SourceIdentity {
                filename: "plu.mdb".to_string(),
                size_bytes: 1,
                sha256: "0".repeat(64),
            },
            TargetIdentity {
                base_url: "https://example.invalid".to_string(),
                store_number: 1,
                client_id: "digi".to_string(),
            },
            ManifestOptions {
                limit: None,
                selection_mode: SelectionMode::All,
                requested_plu: None,
                continue_on_error: true,
                test_alias_used: false,
            },
            records.len(),
            records,
        );
        manifest.recalculate_summary();
        manifest
    }

    fn source_identity() -> SourceIdentity {
        SourceIdentity {
            filename: "plu.mdb".to_string(),
            size_bytes: 123,
            sha256: "0".repeat(64),
        }
    }

    fn target_identity(base_url: &str) -> TargetIdentity {
        TargetIdentity {
            base_url: base_url.to_string(),
            store_number: 1,
            client_id: "digi".to_string(),
        }
    }

    fn write_resume_manifest(
        path: &Path,
        plus: &[Plu],
        config: &AppConfig,
        statuses: &[(RecordStatus, Option<&str>)],
    ) -> ImportManifest {
        let selected = plus.iter().collect::<Vec<_>>();
        let payloads = build_payloads(&selected, config).expect("payloads");
        let records = plus
            .iter()
            .zip(payloads.iter())
            .enumerate()
            .map(|(index, (plu, payload))| {
                PluManifestRecord::new(
                    plu.plu_number,
                    plu.department_number,
                    plu.group_number,
                    index + 1,
                    sha256_json(payload).expect("payload hash"),
                )
            })
            .collect::<Vec<_>>();
        let mut manifest = ImportManifest::new(
            source_identity(),
            target_identity(&config.digiweb.base_url),
            ManifestOptions {
                limit: None,
                selection_mode: SelectionMode::All,
                requested_plu: None,
                continue_on_error: config.import.continue_after_record_failure,
                test_alias_used: false,
            },
            plus.len(),
            records,
        );
        for (index, (status, request_id)) in statuses.iter().enumerate() {
            let record = &mut manifest.records[index];
            match status {
                RecordStatus::NotAttempted => {}
                RecordStatus::RequestAccepted => {
                    record.begin_attempt().expect("begin");
                    record
                        .mark_request_accepted(
                            request_id.expect("request id").to_string(),
                            Some("TODO".to_string()),
                        )
                        .expect("accepted");
                }
                RecordStatus::Processing => {
                    record.begin_attempt().expect("begin");
                    record
                        .mark_request_accepted(
                            request_id.expect("request id").to_string(),
                            Some("TODO".to_string()),
                        )
                        .expect("accepted");
                    record.mark_processing("PROCESSING").expect("processing");
                }
                RecordStatus::Success => {
                    record.begin_attempt().expect("begin");
                    if let Some(request_id) = request_id {
                        record
                            .mark_request_accepted(
                                (*request_id).to_string(),
                                Some("TODO".to_string()),
                            )
                            .expect("accepted");
                    }
                    record.mark_success("SUCCESS").expect("success");
                }
                RecordStatus::Failed => {
                    record.begin_attempt().expect("begin");
                    record
                        .mark_failed("test", "confirmed failure")
                        .expect("fail");
                }
                RecordStatus::UnknownStatus => {
                    record.begin_attempt().expect("begin");
                    if let Some(request_id) = request_id {
                        record
                            .mark_request_accepted(
                                (*request_id).to_string(),
                                Some("TODO".to_string()),
                            )
                            .expect("accepted");
                    }
                    record.mark_unknown("unknown").expect("unknown");
                }
                RecordStatus::AmbiguousSubmission => {
                    record.begin_attempt().expect("begin");
                    record.mark_ambiguous("ambiguous").expect("ambiguous");
                }
                RecordStatus::SubmissionStarted => {
                    record.begin_attempt().expect("begin");
                }
            }
        }
        manifest.recalculate_summary();
        atomic_write_manifest(path, &manifest).expect("manifest");
        manifest
    }

    #[test]
    fn limit_one_limits_selection_to_one_record() {
        let records = vec![plu(1), plu(2), plu(3)];

        let selected = select_records_to_send(&records, criteria(Some(1), None, false));

        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].plu_number, 1);
    }

    #[test]
    fn send_only_first_plu_selects_first_valid_normalized_plu() {
        let valid_after_row_skips = vec![plu(1), plu(2), plu(3)];

        let selected =
            select_records_to_send(&valid_after_row_skips, criteria(Some(1), None, false));

        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].plu_number, 1);
    }

    #[test]
    fn stop_after_first_selected_failure_does_not_double_count_unselected_plus() {
        let all_valid = vec![plu(1), plu(2), plu(3), plu(4)];
        let selected = select_records_to_send(&all_valid, criteria(Some(1), None, false));
        let skipped_by_first_plu_mode = all_valid.len() - selected.len();
        let skipped_after_failure = skipped_after_stop(selected.len(), 0, 1, 0);

        assert_eq!(skipped_by_first_plu_mode, 3);
        assert_eq!(skipped_after_failure, 0);
    }

    #[test]
    fn no_limit_selects_all_valid_plus() {
        let records = vec![plu(1), plu(4), plu(2), plu(3)];

        let selected = select_records_to_send(&records, criteria(None, None, false));

        assert_eq!(
            selected
                .iter()
                .map(|plu| plu.plu_number)
                .collect::<Vec<_>>(),
            vec![1, 4, 2, 3]
        );
    }

    #[test]
    fn progress_snapshot_reports_counts_rate_and_eta() {
        let manifest = progress_manifest(&[
            RecordStatus::Success,
            RecordStatus::Failed,
            RecordStatus::Processing,
            RecordStatus::NotAttempted,
        ]);

        let snapshot = progress_snapshot(&manifest, 1, Duration::from_secs(2));

        assert_eq!(snapshot.selected, 4);
        assert_eq!(snapshot.completed, 2);
        assert_eq!(snapshot.success, 1);
        assert_eq!(snapshot.failed, 1);
        assert_eq!(snapshot.active, 1);
        assert_eq!(snapshot.remaining, 1);
        assert_eq!(snapshot.percent(), 50.0);
        assert_eq!(snapshot.rate_per_second(), 1.0);
        assert_eq!(snapshot.eta(), Some(Duration::from_secs(1)));
    }

    #[test]
    fn progress_rendering_contains_required_live_metrics() {
        let manifest = progress_manifest(&[
            RecordStatus::Success,
            RecordStatus::UnknownStatus,
            RecordStatus::NotAttempted,
        ]);
        let snapshot = progress_snapshot(&manifest, 1, Duration::from_secs(10));

        let line = render_noninteractive_progress(&snapshot);

        assert!(line.starts_with("PROGRESS "));
        assert!(line.contains("selected=3"));
        assert!(line.contains("completed=2"));
        assert!(line.contains("success=1"));
        assert!(line.contains("unknown=1"));
        assert!(line.contains("active=1"));
        assert!(line.contains("remaining=1"));
        assert!(line.contains("eta="));
    }

    #[test]
    fn terminal_progress_renders_final_one_hundred_percent_state() {
        let manifest = progress_manifest(&[RecordStatus::Success, RecordStatus::Success]);
        let snapshot = progress_snapshot(&manifest, 0, Duration::from_secs(1));

        let line = render_interactive_progress_with_options(&snapshot, 100, false);

        assert!(line.contains("[############]"));
        assert!(line.contains("100.0% 2/2"));
        assert!(line.contains("active 0"));
        assert!(line.contains("ETA --:--"));
    }

    #[test]
    fn interactive_progress_uses_bar_style_and_omits_zero_unknown_noise() {
        let manifest = progress_manifest(&[
            RecordStatus::Success,
            RecordStatus::Success,
            RecordStatus::NotAttempted,
            RecordStatus::NotAttempted,
        ]);
        let snapshot = progress_snapshot(&manifest, 2, Duration::from_secs(4));

        let line = render_interactive_progress_with_options(&snapshot, 80, true);

        assert!(line.starts_with("Importing ["));
        assert!(line.contains("50.0% 2/4"));
        assert!(line.contains("ok 2"));
        assert!(line.contains("fail 0"));
        assert!(!line.contains("| ? 0"));
        assert!(line.contains("active 2"));
        assert!(line.contains("0.5/s"));
        assert!(!line.contains('\n'));
    }

    #[test]
    fn interactive_progress_shows_unknown_when_present_and_ascii_fallback() {
        let manifest = progress_manifest(&[
            RecordStatus::Success,
            RecordStatus::UnknownStatus,
            RecordStatus::AmbiguousSubmission,
            RecordStatus::NotAttempted,
        ]);
        let snapshot = progress_snapshot(&manifest, 1, Duration::from_secs(2));

        let line = render_interactive_progress_with_options(&snapshot, 72, false);

        assert!(line.contains("[#########---]"));
        assert!(line.contains("| ? 2"));
        assert!(line.contains("75.0% 3/4"));
    }

    #[test]
    fn manifest_metrics_are_persisted_from_runtime_counters() {
        let mut manifest = progress_manifest(&[RecordStatus::Success, RecordStatus::Failed]);
        let runtime = RuntimeImportMetrics {
            total_submissions: 2,
            total_polls: 5,
            max_in_flight_observed: 2,
            submission_latencies_ms: vec![10, 30],
            processing_latencies_ms: vec![100, 300],
        };

        finalize_manifest_metrics(&mut manifest, &runtime, Duration::from_secs(2), 1);

        assert_eq!(manifest.metrics.elapsed_ms, 2000);
        assert_eq!(manifest.metrics.average_plus_per_second, "1.00");
        assert_eq!(manifest.metrics.max_in_flight_observed, 2);
        assert_eq!(manifest.metrics.total_submissions, 2);
        assert_eq!(manifest.metrics.total_polls, 5);
        assert_eq!(manifest.metrics.authentication_refresh_count, 1);
        assert_eq!(manifest.metrics.average_submission_latency_ms, Some(20));
        assert_eq!(manifest.metrics.average_processing_latency_ms, Some(200));
    }

    #[test]
    fn limit_two_selects_first_two_valid_plus() {
        let records = vec![plu(1), plu(4), plu(2), plu(3)];

        let selected = select_records_to_send(&records, criteria(Some(2), None, false));

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

        let selected = select_records_to_send(&records, criteria(Some(10), None, false));

        assert_eq!(selected.len(), 4);
    }

    #[test]
    fn exact_plu_selection_selects_only_requested_plu() {
        let records = vec![plu(18), plu(721), plu(1)];

        let selected = select_records_to_send(&records, criteria(None, Some(721), false));

        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].plu_number, 721);
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

    #[tokio::test]
    async fn max_in_flight_one_preserves_submit_then_poll_order() {
        let server = TestServer::start(vec![
            token_response("token-a"),
            accepted_response("req-1"),
            success_response("req-1"),
            accepted_response("req-2"),
            success_response("req-2"),
        ])
        .await;
        let mut config = test_import_config(&server.base_url);
        config.import.max_in_flight = 1;
        config.import.write_payload_preview = false;
        config.timeouts.poll_interval_millis = 1;
        let temp = tempfile::tempdir().expect("tempdir");
        let manifest_path = temp.path().join("import-results.json");
        let log_path = temp.path().join("logs.txt");
        let mut logger = AuditLogger::create(&log_path).expect("logger");

        run_import(
            config,
            &[plu(1), plu(2)],
            SourceIdentity {
                filename: "plu.mdb".to_string(),
                size_bytes: 123,
                sha256: "0".repeat(64),
            },
            TargetIdentity {
                base_url: server.base_url.clone(),
                store_number: 1,
                client_id: "digi".to_string(),
            },
            &manifest_path,
            None,
            None,
            ImportRunOptions {
                limit: None,
                requested_plu: None,
                continue_after_record_failure: false,
                test_mode: false,
                retry_failed: false,
            },
            &mut logger,
        )
        .await
        .expect("import");

        let manifest = load_manifest(&manifest_path).expect("manifest");
        assert_eq!(manifest.metrics.max_in_flight_observed, 1);
        assert_eq!(manifest.summary.success, 2);
        assert_eq!(
            server.request_lines(),
            vec![
                "POST /token HTTP/1.1",
                "POST /api/v1/third-party/plus/write HTTP/1.1",
                "GET /status/req-1 HTTP/1.1",
                "POST /api/v1/third-party/plus/write HTTP/1.1",
                "GET /status/req-2 HTTP/1.1",
            ]
        );
    }

    #[tokio::test]
    async fn bounded_import_submits_multiple_before_waiting_for_terminal_status() {
        let server = TestServer::start(vec![
            token_response("token-a"),
            accepted_response("req-1"),
            accepted_response("req-2"),
            success_response("req-1"),
            success_response("req-2"),
            accepted_response("req-3"),
            success_response("req-3"),
        ])
        .await;
        let mut config = test_import_config(&server.base_url);
        config.import.max_in_flight = 2;
        config.import.write_payload_preview = false;
        config.timeouts.poll_interval_millis = 1;
        let temp = tempfile::tempdir().expect("tempdir");
        let manifest_path = temp.path().join("import-results.json");
        let log_path = temp.path().join("logs.txt");
        let mut logger = AuditLogger::create(&log_path).expect("logger");

        run_import(
            config,
            &[plu(1), plu(2), plu(3)],
            SourceIdentity {
                filename: "plu.mdb".to_string(),
                size_bytes: 123,
                sha256: "0".repeat(64),
            },
            TargetIdentity {
                base_url: server.base_url.clone(),
                store_number: 1,
                client_id: "digi".to_string(),
            },
            &manifest_path,
            None,
            None,
            ImportRunOptions {
                limit: None,
                requested_plu: None,
                continue_after_record_failure: false,
                test_mode: false,
                retry_failed: false,
            },
            &mut logger,
        )
        .await
        .expect("import");

        let manifest = load_manifest(&manifest_path).expect("manifest");
        let requests = server.request_lines();
        assert_eq!(manifest.summary.success, 3);
        assert_eq!(manifest.metrics.max_in_flight_observed, 2);
        assert_eq!(manifest.metrics.total_submissions, 3);
        assert_eq!(manifest.metrics.total_polls, 3);
        assert_eq!(requests[1], "POST /api/v1/third-party/plus/write HTTP/1.1");
        assert_eq!(requests[2], "POST /api/v1/third-party/plus/write HTTP/1.1");
        assert_eq!(requests[3], "GET /status/req-1 HTTP/1.1");
    }

    #[tokio::test]
    async fn max_in_flight_sixteen_never_submits_seventeenth_before_polling() {
        let mut responses = vec![token_response("token-a")];
        for index in 1..=16 {
            responses.push(accepted_response(&format!("req-{index}")));
        }
        for index in 1..=16 {
            responses.push(success_response(&format!("req-{index}")));
        }
        responses.push(accepted_response("req-17"));
        responses.push(success_response("req-17"));
        let server = TestServer::start(responses).await;
        let mut config = test_import_config(&server.base_url);
        config.import.max_in_flight = 16;
        config.import.write_payload_preview = false;
        config.timeouts.poll_interval_millis = 1;
        let temp = tempfile::tempdir().expect("tempdir");
        let manifest_path = temp.path().join("import-results.json");
        let log_path = temp.path().join("logs.txt");
        let mut logger = AuditLogger::create(&log_path).expect("logger");
        let records = (1..=17).map(plu).collect::<Vec<_>>();

        run_import(
            config,
            &records,
            source_identity(),
            target_identity(&server.base_url),
            &manifest_path,
            None,
            None,
            ImportRunOptions {
                limit: None,
                requested_plu: None,
                continue_after_record_failure: false,
                test_mode: false,
                retry_failed: false,
            },
            &mut logger,
        )
        .await
        .expect("import");

        let manifest = load_manifest(&manifest_path).expect("manifest");
        let requests = server.request_lines();
        assert_eq!(manifest.metrics.max_in_flight_observed, 16);
        assert_eq!(manifest.summary.success, 17);
        assert!(
            requests[1..=16]
                .iter()
                .all(|line| line.starts_with("POST /api/v1/third-party/plus/write"))
        );
        assert_eq!(requests[17], "GET /status/req-1 HTTP/1.1");
        assert!(
            requests[18..]
                .iter()
                .any(|line| line.starts_with("POST /api/v1/third-party/plus/write"))
        );
    }

    #[tokio::test]
    async fn request_accepted_and_processing_resume_by_polling_without_repost() {
        let server = TestServer::start(vec![
            token_response("token-a"),
            success_response("req-1"),
            success_response("req-2"),
            accepted_response("req-5"),
            success_response("req-5"),
        ])
        .await;
        let mut config = test_import_config(&server.base_url);
        config.import.max_in_flight = 2;
        config.import.write_payload_preview = false;
        config.timeouts.poll_interval_millis = 1;
        let temp = tempfile::tempdir().expect("tempdir");
        let manifest_path = temp.path().join("import-results.json");
        let log_path = temp.path().join("logs.txt");
        let mut logger = AuditLogger::create(&log_path).expect("logger");
        let records = (1..=5).map(plu).collect::<Vec<_>>();
        let stored = write_resume_manifest(
            &manifest_path,
            &records,
            &config,
            &[
                (RecordStatus::RequestAccepted, Some("req-1")),
                (RecordStatus::Processing, Some("req-2")),
                (RecordStatus::Success, Some("req-3")),
                (RecordStatus::Failed, None),
                (RecordStatus::NotAttempted, None),
            ],
        );
        assert_eq!(
            load_manifest(&manifest_path).expect("manifest").records[0]
                .request_id
                .as_deref(),
            Some("req-1")
        );
        assert_eq!(stored.summary.request_accepted, 1);

        run_import(
            config,
            &records,
            source_identity(),
            target_identity(&server.base_url),
            &manifest_path,
            Some(&manifest_path),
            None,
            ImportRunOptions {
                limit: Some(1),
                requested_plu: None,
                continue_after_record_failure: false,
                test_mode: false,
                retry_failed: false,
            },
            &mut logger,
        )
        .await
        .expect("resume");

        let manifest = load_manifest(&manifest_path).expect("manifest");
        let requests = server.request_lines();
        assert_eq!(manifest.summary.success, 4);
        assert_eq!(manifest.summary.failed, 1);
        assert_eq!(manifest.records[0].request_id.as_deref(), Some("req-1"));
        assert_eq!(manifest.records[1].request_id.as_deref(), Some("req-2"));
        assert_eq!(
            requests
                .iter()
                .filter(|line| line.starts_with("POST /api/v1/third-party/plus/write"))
                .count(),
            1
        );
        assert!(requests.contains(&"GET /status/req-1 HTTP/1.1".to_string()));
        assert!(requests.contains(&"GET /status/req-2 HTTP/1.1".to_string()));
    }

    #[tokio::test]
    async fn manifest_persists_started_before_post_and_request_id_before_poll() {
        let temp = tempfile::tempdir().expect("tempdir");
        let manifest_path = temp.path().join("import-results.json");
        let observer_errors = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let observer_manifest_path = manifest_path.clone();
        let observer_errors_for_server = observer_errors.clone();
        let server =
            TestServer::start_with_observer(
                vec![
                    token_response("token-a"),
                    accepted_response("req-1"),
                    success_response("req-1"),
                ],
                std::sync::Arc::new(move |_index, line| {
                    if line.starts_with("POST /api/v1/third-party/plus/write") {
                        match load_manifest(&observer_manifest_path) {
                            Ok(manifest) => {
                                let record = &manifest.records[0];
                                if record.status != RecordStatus::SubmissionStarted
                                    || record.request_id.is_some()
                                {
                                    observer_errors_for_server.lock().expect("errors").push(
                                        format!(
                                            "POST saw unsafe manifest state {:?} request_id={:?}",
                                            record.status, record.request_id
                                        ),
                                    );
                                }
                            }
                            Err(err) => observer_errors_for_server
                                .lock()
                                .expect("errors")
                                .push(err.to_string()),
                        }
                    }
                    if line.starts_with("GET /status/req-1") {
                        match load_manifest(&observer_manifest_path) {
                            Ok(manifest) => {
                                let record = &manifest.records[0];
                                if record.request_id.as_deref() != Some("req-1")
                                    || !matches!(
                                        record.status,
                                        RecordStatus::RequestAccepted | RecordStatus::Processing
                                    )
                                {
                                    observer_errors_for_server.lock().expect("errors").push(
                                        format!(
                                            "GET saw unsafe manifest state {:?} request_id={:?}",
                                            record.status, record.request_id
                                        ),
                                    );
                                }
                            }
                            Err(err) => observer_errors_for_server
                                .lock()
                                .expect("errors")
                                .push(err.to_string()),
                        }
                    }
                }),
            )
            .await;
        let mut config = test_import_config(&server.base_url);
        config.import.max_in_flight = 1;
        config.import.write_payload_preview = false;
        config.timeouts.poll_interval_millis = 1;
        let log_path = temp.path().join("logs.txt");
        let mut logger = AuditLogger::create(&log_path).expect("logger");

        run_import(
            config,
            &[plu(1)],
            source_identity(),
            target_identity(&server.base_url),
            &manifest_path,
            None,
            None,
            ImportRunOptions {
                limit: None,
                requested_plu: None,
                continue_after_record_failure: false,
                test_mode: false,
                retry_failed: false,
            },
            &mut logger,
        )
        .await
        .expect("import");

        assert_eq!(
            observer_errors.lock().expect("errors").as_slice(),
            &[] as &[String]
        );
    }

    #[test]
    fn in_flight_timeout_marks_record_unknown_with_request_id() {
        let temp = tempfile::tempdir().expect("tempdir");
        let manifest_path = temp.path().join("import-results.json");
        let log_path = temp.path().join("logs.txt");
        let mut logger = AuditLogger::create(&log_path).expect("logger");
        let mut manifest = progress_manifest(&[RecordStatus::RequestAccepted]);
        manifest.records[0].request_id = Some("req-timeout".to_string());
        manifest.recalculate_summary_for_active_run();
        atomic_write_manifest(&manifest_path, &manifest).expect("manifest");
        let active = InFlightRequest {
            record_index: 0,
            request_id: "req-timeout".to_string(),
            accepted_at: Instant::now() - Duration::from_secs(10),
        };

        mark_in_flight_timeout(&mut manifest, &active, &manifest_path, &mut logger, "[1/1]")
            .expect("timeout");

        assert_eq!(manifest.summary.unknown_status, 1);
        assert_eq!(
            manifest.records[0].request_id.as_deref(),
            Some("req-timeout")
        );
        assert!(
            manifest.records[0]
                .last_error
                .as_deref()
                .unwrap_or_default()
                .contains("UNKNOWN_OR_TIMEOUT")
        );
    }

    #[tokio::test]
    async fn unknown_remote_status_is_counted_unknown_not_failed() {
        let server = TestServer::start(vec![
            token_response("token-a"),
            accepted_response("req-1"),
            raw_response(
                200,
                "OK",
                &[("Content-Type", "application/json")],
                r#"{"id":"req-1","status":"SURPRISE","type":"Plu","method":"WRITE"}"#,
            ),
        ])
        .await;
        let mut config = test_import_config(&server.base_url);
        config.import.write_payload_preview = false;
        config.timeouts.poll_interval_millis = 1;
        let temp = tempfile::tempdir().expect("tempdir");
        let manifest_path = temp.path().join("import-results.json");
        let log_path = temp.path().join("logs.txt");
        let mut logger = AuditLogger::create(&log_path).expect("logger");

        let summary = run_import(
            config,
            &[plu(1)],
            source_identity(),
            target_identity(&server.base_url),
            &manifest_path,
            None,
            None,
            ImportRunOptions {
                limit: None,
                requested_plu: None,
                continue_after_record_failure: false,
                test_mode: false,
                retry_failed: false,
            },
            &mut logger,
        )
        .await
        .expect("import");

        let manifest = load_manifest(&manifest_path).expect("manifest");
        assert_eq!(summary.failed, 0);
        assert_eq!(summary.unknown, 1);
        assert_eq!(manifest.summary.failed, 0);
        assert_eq!(manifest.summary.unknown_status, 1);
    }

    #[tokio::test]
    async fn post_network_error_marks_ambiguous_and_does_not_retry_automatically() {
        let server = TestServer::start(vec![token_response("token-a")]).await;
        let mut config = test_import_config(&server.base_url);
        config.import.write_payload_preview = false;
        config.timeouts.poll_interval_millis = 1;
        let temp = tempfile::tempdir().expect("tempdir");
        let manifest_path = temp.path().join("import-results.json");
        let log_path = temp.path().join("logs.txt");
        let mut logger = AuditLogger::create(&log_path).expect("logger");

        let summary = run_import(
            config,
            &[plu(1), plu(2)],
            source_identity(),
            target_identity(&server.base_url),
            &manifest_path,
            None,
            None,
            ImportRunOptions {
                limit: None,
                requested_plu: None,
                continue_after_record_failure: false,
                test_mode: false,
                retry_failed: false,
            },
            &mut logger,
        )
        .await
        .expect("ambiguous import result");

        let manifest = load_manifest(&manifest_path).expect("manifest");
        assert_eq!(summary.unknown, 1);
        assert_eq!(summary.not_attempted_after_stop, 1);
        assert_eq!(manifest.summary.ambiguous_submission, 1);
        assert_eq!(manifest.summary.not_attempted, 1);
        assert_eq!(manifest.records[0].request_id, None);
        assert_eq!(manifest.records[0].attempt_count, 1);
        assert_eq!(manifest.records[1].status, RecordStatus::NotAttempted);
        assert_eq!(
            server
                .request_lines()
                .iter()
                .filter(|line| line.starts_with("POST /api/v1/third-party/plus/write"))
                .count(),
            0
        );
    }

    #[tokio::test]
    async fn definitive_failure_stops_new_submissions_and_reconciles_in_flight() {
        let server = TestServer::start(vec![
            token_response("token-a"),
            accepted_response("req-1"),
            accepted_response("req-2"),
            fail_response("req-1"),
            success_response("req-2"),
        ])
        .await;
        let mut config = test_import_config(&server.base_url);
        config.import.max_in_flight = 2;
        config.import.write_payload_preview = false;
        config.import.continue_after_record_failure = false;
        config.timeouts.poll_interval_millis = 1;
        let temp = tempfile::tempdir().expect("tempdir");
        let manifest_path = temp.path().join("import-results.json");
        let log_path = temp.path().join("logs.txt");
        let mut logger = AuditLogger::create(&log_path).expect("logger");

        run_import(
            config,
            &[plu(1), plu(2), plu(3)],
            SourceIdentity {
                filename: "plu.mdb".to_string(),
                size_bytes: 123,
                sha256: "0".repeat(64),
            },
            TargetIdentity {
                base_url: server.base_url.clone(),
                store_number: 1,
                client_id: "digi".to_string(),
            },
            &manifest_path,
            None,
            None,
            ImportRunOptions {
                limit: None,
                requested_plu: None,
                continue_after_record_failure: false,
                test_mode: false,
                retry_failed: false,
            },
            &mut logger,
        )
        .await
        .expect("completed with failed PLU");

        let manifest = load_manifest(&manifest_path).expect("manifest");
        let requests = server.request_lines();
        assert_eq!(manifest.summary.success, 1);
        assert_eq!(manifest.summary.failed, 1);
        assert_eq!(manifest.summary.not_attempted, 1);
        assert_eq!(manifest.records[0].request_id.as_deref(), Some("req-1"));
        assert_eq!(manifest.records[1].request_id.as_deref(), Some("req-2"));
        assert!(
            !requests
                .iter()
                .skip(5)
                .any(|line| line.starts_with("POST /api/v1/third-party/plus/write"))
        );
    }

    #[tokio::test]
    async fn continue_after_record_failure_keeps_submitting_after_confirmed_failure() {
        let server = TestServer::start(vec![
            token_response("token-a"),
            accepted_response("req-1"),
            accepted_response("req-2"),
            fail_response("req-1"),
            success_response("req-2"),
            accepted_response("req-3"),
            success_response("req-3"),
        ])
        .await;
        let mut config = test_import_config(&server.base_url);
        config.import.max_in_flight = 2;
        config.import.write_payload_preview = false;
        config.import.continue_after_record_failure = true;
        config.timeouts.poll_interval_millis = 1;
        let temp = tempfile::tempdir().expect("tempdir");
        let manifest_path = temp.path().join("import-results.json");
        let log_path = temp.path().join("logs.txt");
        let mut logger = AuditLogger::create(&log_path).expect("logger");

        run_import(
            config,
            &[plu(1), plu(2), plu(3)],
            SourceIdentity {
                filename: "plu.mdb".to_string(),
                size_bytes: 123,
                sha256: "0".repeat(64),
            },
            TargetIdentity {
                base_url: server.base_url.clone(),
                store_number: 1,
                client_id: "digi".to_string(),
            },
            &manifest_path,
            None,
            None,
            ImportRunOptions {
                limit: None,
                requested_plu: None,
                continue_after_record_failure: true,
                test_mode: false,
                retry_failed: false,
            },
            &mut logger,
        )
        .await
        .expect("completed with failed PLU");

        let manifest = load_manifest(&manifest_path).expect("manifest");
        let requests = server.request_lines();
        assert_eq!(manifest.summary.success, 2);
        assert_eq!(manifest.summary.failed, 1);
        assert_eq!(manifest.summary.not_attempted, 0);
        assert_eq!(
            requests
                .iter()
                .filter(|line| line.starts_with("POST /api/v1/third-party/plus/write"))
                .count(),
            3
        );
    }

    #[tokio::test]
    async fn unrecoverable_submission_auth_failure_stops_even_with_continue_on_error() {
        let server = TestServer::start(vec![
            token_response("token-a"),
            raw_response(401, "Unauthorized", &[], ""),
            token_response("token-b"),
            raw_response(401, "Unauthorized", &[], ""),
            token_response("token-c"),
            raw_response(401, "Unauthorized", &[], ""),
        ])
        .await;
        let mut config = test_import_config(&server.base_url);
        config.import.write_payload_preview = false;
        config.import.continue_after_record_failure = true;
        let temp = tempfile::tempdir().expect("tempdir");
        let manifest_path = temp.path().join("import-results.json");
        let log_path = temp.path().join("logs.txt");
        let mut logger = AuditLogger::create(&log_path).expect("logger");
        let records = vec![plu(1), plu(2), plu(3)];

        let err = run_import(
            config,
            &records,
            SourceIdentity {
                filename: "plu.mdb".to_string(),
                size_bytes: 123,
                sha256: "0".repeat(64),
            },
            TargetIdentity {
                base_url: server.base_url.clone(),
                store_number: 1,
                client_id: "digi".to_string(),
            },
            &manifest_path,
            None,
            None,
            ImportRunOptions {
                limit: None,
                requested_plu: None,
                continue_after_record_failure: true,
                test_mode: false,
                retry_failed: false,
            },
            &mut logger,
        )
        .await
        .expect_err("auth failure");
        logger.flush().expect("flush");

        let manifest = load_manifest(&manifest_path).expect("manifest");
        let log = fs::read_to_string(log_path).expect("log");
        assert!(matches!(err, AppError::Auth(_)));
        assert_eq!(manifest.summary.failed, 1);
        assert_eq!(manifest.summary.not_attempted, 2);
        assert_eq!(manifest.records[0].attempt_count, 1);
        assert_eq!(manifest.records[0].attempts[0].authentication_retries, 2);
        assert_eq!(manifest.records[0].request_id, None);
        assert_eq!(manifest.records[1].status, RecordStatus::NotAttempted);
        assert_eq!(manifest.records[2].status, RecordStatus::NotAttempted);
        assert!(log.contains("IMPORT STOPPED - AUTHENTICATION COULD NOT BE RESTORED"));
        assert!(!log.contains("token-a"));
        assert!(!log.contains("token-b"));
        assert!(!log.contains("client-secret"));
    }

    #[tokio::test]
    async fn import_refuses_before_write_when_required_reference_is_unverified() {
        let mut config = AppConfig::default();
        config.digiweb.client_secret = "client-secret".to_string();
        config.import.write_payload_preview = false;
        let temp = tempfile::tempdir().expect("tempdir");
        let manifest_path = temp.path().join("import-results.json");
        let log_path = temp.path().join("logs.txt");
        let mut logger = AuditLogger::create(&log_path).expect("logger");
        let records = vec![plu(1)];

        let err = run_import(
            config,
            &records,
            SourceIdentity {
                filename: "plu.mdb".to_string(),
                size_bytes: 123,
                sha256: "0".repeat(64),
            },
            TargetIdentity {
                base_url: "https://example.invalid".to_string(),
                store_number: 1,
                client_id: "digi".to_string(),
            },
            &manifest_path,
            None,
            None,
            ImportRunOptions {
                limit: None,
                requested_plu: None,
                continue_after_record_failure: false,
                test_mode: false,
                retry_failed: false,
            },
            &mut logger,
        )
        .await
        .expect_err("blocked");
        logger.flush().expect("flush");

        let log = fs::read_to_string(log_path).expect("log");
        assert!(matches!(err, AppError::ValidationPayload(_)));
        assert!(log.contains("IMPORT BLOCKED"));
        assert!(log.contains("No PLUs were submitted."));
        assert!(log.contains("PLU write requests: 0"));
        assert!(!manifest_path.exists());
    }

    #[tokio::test]
    async fn targeted_import_manifest_records_requested_plu_selection() {
        let server = TestServer::start(vec![
            token_response("token-a"),
            raw_response(
                201,
                "Created",
                &[
                    ("Content-Type", "application/json"),
                    ("Location", "http://localhost/status/req-721"),
                ],
                r#"{"id":"req-721","status":"TODO","type":"Plu","method":"WRITE"}"#,
            ),
            raw_response(
                200,
                "OK",
                &[("Content-Type", "application/json")],
                r#"{"id":"req-721","status":"SUCCESS","type":"Plu","method":"WRITE"}"#,
            ),
        ])
        .await;
        let mut config = test_import_config(&server.base_url);
        config.import.write_payload_preview = false;
        config.verification.confirmed_label_formats = vec![1];
        let temp = tempfile::tempdir().expect("tempdir");
        let manifest_path = temp.path().join("import-results.json");
        let log_path = temp.path().join("logs.txt");
        let mut logger = AuditLogger::create(&log_path).expect("logger");
        let mut target = plu(721);
        target.label_format = Some(0);
        let records = vec![plu(18), target];

        let summary = run_import(
            config,
            &records,
            SourceIdentity {
                filename: "plu.mdb".to_string(),
                size_bytes: 123,
                sha256: "0".repeat(64),
            },
            TargetIdentity {
                base_url: server.base_url.clone(),
                store_number: 1,
                client_id: "digi".to_string(),
            },
            &manifest_path,
            None,
            None,
            ImportRunOptions {
                limit: None,
                requested_plu: Some(721),
                continue_after_record_failure: false,
                test_mode: false,
                retry_failed: false,
            },
            &mut logger,
        )
        .await
        .expect("import");

        let manifest = load_manifest(&manifest_path).expect("manifest");
        assert_eq!(summary.selected, 1);
        assert_eq!(summary.records[0].plu_number, 721);
        assert_eq!(manifest.options.selection_mode, SelectionMode::Plu);
        assert_eq!(manifest.options.requested_plu, Some(721));
        assert_eq!(manifest.selection.selected_order, vec![721]);
        assert_eq!(manifest.selection.selected_count, 1);
    }

    #[tokio::test]
    async fn targeted_import_unverified_effective_label_format_blocks_before_write() {
        let mut config = AppConfig::default();
        config.digiweb.client_secret = "client-secret".to_string();
        config.import.write_payload_preview = false;
        config.verification.confirmed_departments = vec![1];
        config.verification.confirmed_groups = vec!["1:1".to_string()];
        let temp = tempfile::tempdir().expect("tempdir");
        let manifest_path = temp.path().join("import-results.json");
        let log_path = temp.path().join("logs.txt");
        let mut logger = AuditLogger::create(&log_path).expect("logger");
        let mut target = plu(721);
        target.label_format = Some(0);

        let err = run_import(
            config,
            &[target],
            SourceIdentity {
                filename: "plu.mdb".to_string(),
                size_bytes: 123,
                sha256: "0".repeat(64),
            },
            TargetIdentity {
                base_url: "https://example.invalid".to_string(),
                store_number: 1,
                client_id: "digi".to_string(),
            },
            &manifest_path,
            None,
            None,
            ImportRunOptions {
                limit: None,
                requested_plu: Some(721),
                continue_after_record_failure: false,
                test_mode: false,
                retry_failed: false,
            },
            &mut logger,
        )
        .await
        .expect_err("blocked");
        logger.flush().expect("flush");

        let log = fs::read_to_string(log_path).expect("log");
        assert!(matches!(err, AppError::ValidationPayload(_)));
        assert!(log.contains("Label Format 1"));
        assert!(log.contains("PLU write requests: 0"));
        assert!(!manifest_path.exists());
    }

    fn test_import_config(base_url: &str) -> AppConfig {
        AppConfig {
            digiweb: crate::config::DigiwebConfig {
                base_url: base_url.to_string(),
                client_id: "digi".to_string(),
                client_secret: "client-secret".to_string(),
                log_credentials_for_testing: false,
                token_url: format!("{base_url}/token"),
                store_number: 1,
                allow_invalid_certificates: false,
                plu_upsert_path: "/api/v1/third-party/plus/write".to_string(),
                request_status_path_template: "/status/{request_id}".to_string(),
                plu_barcode_type: String::new(),
                plu_barcode_ref_no: String::new(),
            },
            timeouts: crate::config::TimeoutConfig {
                request_seconds: 5,
                poll_interval_seconds: 1,
                poll_interval_millis: 1,
                poll_timeout_seconds: 5,
            },
            import: crate::config::ImportConfig::default(),
            mapping: crate::config::MappingConfig::default(),
            profiles: crate::config::ProfileConfig::default(),
            verification: crate::config::VerificationConfig {
                confirmed_departments: vec![1],
                confirmed_groups: vec!["1:1".to_string()],
                confirmed_label_formats: Vec::new(),
            },
        }
    }

    fn token_response(token: &str) -> String {
        raw_response(
            200,
            "OK",
            &[("Content-Type", "application/json")],
            &format!(r#"{{"access_token":"{token}","expires_in":300}}"#),
        )
    }

    fn accepted_response(request_id: &str) -> String {
        raw_response(
            201,
            "Created",
            &[
                ("Content-Type", "application/json"),
                ("Location", &format!("http://localhost/status/{request_id}")),
            ],
            &format!(r#"{{"id":"{request_id}","status":"TODO","type":"Plu","method":"WRITE"}}"#),
        )
    }

    fn success_response(request_id: &str) -> String {
        raw_response(
            200,
            "OK",
            &[("Content-Type", "application/json")],
            &format!(r#"{{"id":"{request_id}","status":"SUCCESS","type":"Plu","method":"WRITE"}}"#),
        )
    }

    #[allow(dead_code)]
    fn processing_response(request_id: &str) -> String {
        raw_response(
            200,
            "OK",
            &[("Content-Type", "application/json")],
            &format!(
                r#"{{"id":"{request_id}","status":"PROCESSING","type":"Plu","method":"WRITE"}}"#
            ),
        )
    }

    fn fail_response(request_id: &str) -> String {
        raw_response(
            200,
            "OK",
            &[("Content-Type", "application/json")],
            &format!(
                r#"{{"id":"{request_id}","status":"FAIL","type":"Plu","method":"WRITE","latestFeedback":"rejected"}}"#
            ),
        )
    }

    fn raw_response(
        status_code: u16,
        reason: &str,
        headers: &[(&str, &str)],
        body: &str,
    ) -> String {
        let mut response = format!("HTTP/1.1 {status_code} {reason}\r\n");
        let has_content_length = headers
            .iter()
            .any(|(name, _)| name.eq_ignore_ascii_case("content-length"));
        for (name, value) in headers {
            response.push_str(&format!("{name}: {value}\r\n"));
        }
        if !has_content_length {
            response.push_str(&format!("Content-Length: {}\r\n", body.len()));
        }
        response.push_str("Connection: close\r\n\r\n");
        response.push_str(body);
        response
    }

    struct TestServer {
        base_url: String,
        requests: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    }

    impl TestServer {
        async fn start(responses: Vec<String>) -> Self {
            Self::start_with_observer(responses, std::sync::Arc::new(|_, _| {})).await
        }

        async fn start_with_observer(
            responses: Vec<String>,
            observer: std::sync::Arc<dyn Fn(usize, &str) + Send + Sync>,
        ) -> Self {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            use tokio::net::TcpListener;

            let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
            let addr = listener.local_addr().expect("addr");
            let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
            let request_log = requests.clone();
            tokio::spawn(async move {
                for (index, response) in responses.into_iter().enumerate() {
                    let Ok((mut stream, _peer)) = listener.accept().await else {
                        return;
                    };
                    let mut buffer = [0_u8; 4096];
                    let size = stream.read(&mut buffer).await.unwrap_or_default();
                    let request = String::from_utf8_lossy(&buffer[..size]);
                    if let Some(line) = request.lines().next() {
                        request_log
                            .lock()
                            .expect("request log")
                            .push(line.to_string());
                        observer(index + 1, line);
                    }
                    let _ = stream.write_all(response.as_bytes()).await;
                }
            });
            Self {
                base_url: format!("http://{addr}"),
                requests,
            }
        }

        fn request_lines(&self) -> Vec<String> {
            self.requests.lock().expect("request log").clone()
        }
    }
}
