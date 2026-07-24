use std::collections::BTreeMap;

use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};

use crate::error::AppError;
use crate::recovery::{MANIFEST_SCHEMA_VERSION, sanitize_manifest_error};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ImportManifest {
    pub schema_version: u32,
    pub application_version: String,
    pub manifest_id: String,
    pub run_status: RunStatus,
    pub created_at: DateTime<Local>,
    pub updated_at: DateTime<Local>,
    pub source: SourceIdentity,
    pub target: TargetIdentity,
    pub options: ManifestOptions,
    pub selection: ManifestSelection,
    pub summary: ManifestSummary,
    pub records: Vec<PluManifestRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SourceIdentity {
    pub filename: String,
    pub size_bytes: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TargetIdentity {
    pub base_url: String,
    pub store_number: u32,
    pub client_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ManifestOptions {
    pub limit: Option<usize>,
    pub continue_on_error: bool,
    pub test_alias_used: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ManifestSelection {
    pub valid_plu_count: usize,
    pub selected_count: usize,
    pub excluded_by_limit: usize,
    pub selected_order: Vec<u64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ManifestSummary {
    pub not_attempted: usize,
    pub submission_started: usize,
    pub request_accepted: usize,
    pub processing: usize,
    pub success: usize,
    pub failed: usize,
    pub unknown_status: usize,
    pub ambiguous_submission: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PluManifestRecord {
    pub plu_number: u64,
    pub department: Option<u32>,
    pub group: Option<u32>,
    pub selection_index: usize,
    pub payload_sha256: String,
    pub status: RecordStatus,
    pub attempt_count: u32,
    pub request_id: Option<String>,
    pub last_known_remote_status: Option<String>,
    pub submission_started_at: Option<DateTime<Local>>,
    pub request_accepted_at: Option<DateTime<Local>>,
    pub last_polled_at: Option<DateTime<Local>>,
    pub completed_at: Option<DateTime<Local>>,
    pub not_attempted_reason: Option<String>,
    pub last_error: Option<String>,
    pub attempts: Vec<AttemptRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AttemptRecord {
    pub attempt_number: u32,
    pub started_at: DateTime<Local>,
    pub request_id: Option<String>,
    pub submission_result: AttemptSubmissionResult,
    pub last_remote_status: Option<String>,
    pub finished_at: Option<DateTime<Local>>,
    pub error_stage: Option<String>,
    pub error_message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AttemptSubmissionResult {
    Started,
    Accepted,
    Rejected,
    Unknown,
    Skipped,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum RecordStatus {
    NotAttempted,
    SubmissionStarted,
    RequestAccepted,
    Processing,
    Success,
    Failed,
    UnknownStatus,
    AmbiguousSubmission,
}

impl RecordStatus {
    pub fn as_text(self) -> &'static str {
        match self {
            Self::NotAttempted => "NOT_ATTEMPTED",
            Self::SubmissionStarted => "SUBMISSION_STARTED",
            Self::RequestAccepted => "REQUEST_ACCEPTED",
            Self::Processing => "PROCESSING",
            Self::Success => "SUCCESS",
            Self::Failed => "FAILED",
            Self::UnknownStatus => "UNKNOWN_STATUS",
            Self::AmbiguousSubmission => "AMBIGUOUS_SUBMISSION",
        }
    }

    pub fn may_transition_to(self, next: Self) -> bool {
        use RecordStatus::*;
        matches!(
            (self, next),
            (NotAttempted, SubmissionStarted)
                | (SubmissionStarted, RequestAccepted)
                | (SubmissionStarted, AmbiguousSubmission)
                | (SubmissionStarted, Failed)
                | (RequestAccepted, Processing)
                | (RequestAccepted, Success)
                | (RequestAccepted, Failed)
                | (RequestAccepted, UnknownStatus)
                | (Processing, Processing)
                | (Processing, Success)
                | (Processing, Failed)
                | (Processing, UnknownStatus)
                | (UnknownStatus, Processing)
                | (UnknownStatus, Success)
                | (UnknownStatus, Failed)
                | (UnknownStatus, UnknownStatus)
                | (Failed, SubmissionStarted)
        )
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    InProgress,
    Success,
    CompletedWithFailures,
    Incomplete,
    Interrupted,
}

impl RunStatus {
    pub fn as_text(self) -> &'static str {
        match self {
            Self::InProgress => "IN_PROGRESS",
            Self::Success => "SUCCESS",
            Self::CompletedWithFailures => "COMPLETED_WITH_FAILURES",
            Self::Incomplete => "INCOMPLETE",
            Self::Interrupted => "INTERRUPTED",
        }
    }

    #[allow(dead_code)]
    pub fn exit_code(self) -> i32 {
        match self {
            Self::Success => 0,
            Self::CompletedWithFailures | Self::Incomplete | Self::Interrupted => 1,
            Self::InProgress => 1,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResumePlan {
    pub items: Vec<ResumePlanItem>,
    pub existing_requests_to_poll: usize,
    pub not_attempted_to_submit: usize,
    pub confirmed_failures: usize,
    pub ambiguous_submissions: usize,
    pub already_successful: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResumePlanItem {
    pub plu_number: u64,
    pub kind: ResumePlanItemKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResumePlanItemKind {
    PollExistingRequest,
    SubmitNotAttempted,
    RetryConfirmedFailure,
    SkipAlreadySuccessful,
    SkipFailed,
    SkipAmbiguous,
}

impl ImportManifest {
    pub fn new(
        source: SourceIdentity,
        target: TargetIdentity,
        options: ManifestOptions,
        valid_plu_count: usize,
        records: Vec<PluManifestRecord>,
    ) -> Self {
        let now = Local::now();
        let selected_order = records
            .iter()
            .map(|record| record.plu_number)
            .collect::<Vec<_>>();
        let mut manifest = Self {
            schema_version: MANIFEST_SCHEMA_VERSION,
            application_version: env!("CARGO_PKG_VERSION").to_string(),
            manifest_id: format!("{}-{}", now.format("%Y%m%d%H%M%S%3f"), std::process::id()),
            run_status: RunStatus::InProgress,
            created_at: now,
            updated_at: now,
            source,
            target,
            options,
            selection: ManifestSelection {
                valid_plu_count,
                selected_count: records.len(),
                excluded_by_limit: valid_plu_count.saturating_sub(records.len()),
                selected_order,
            },
            summary: ManifestSummary::default(),
            records,
        };
        manifest.recalculate_summary_for_active_run();
        manifest
    }

    pub fn recalculate_summary(&mut self) {
        self.summary = ManifestSummary::from_records(&self.records);
        self.run_status = self.derived_run_status();
        self.updated_at = Local::now();
    }

    pub fn recalculate_summary_for_active_run(&mut self) {
        self.summary = ManifestSummary::from_records(&self.records);
        self.run_status = RunStatus::InProgress;
        self.updated_at = Local::now();
    }

    pub fn mark_interrupted(&mut self) {
        self.summary = ManifestSummary::from_records(&self.records);
        self.run_status = RunStatus::Interrupted;
        self.updated_at = Local::now();
    }

    pub fn derived_run_status(&self) -> RunStatus {
        if self.summary.not_attempted > 0
            || self.summary.submission_started > 0
            || self.summary.request_accepted > 0
            || self.summary.processing > 0
            || self.summary.unknown_status > 0
            || self.summary.ambiguous_submission > 0
        {
            RunStatus::Incomplete
        } else if self.summary.failed > 0 {
            RunStatus::CompletedWithFailures
        } else {
            RunStatus::Success
        }
    }

    pub fn mark_restarted_transients_ambiguous(&mut self) -> bool {
        let mut changed = false;
        for record in &mut self.records {
            if record.status == RecordStatus::SubmissionStarted {
                record.status = RecordStatus::AmbiguousSubmission;
                record.last_error = Some(
                    "Previous process stopped after submission started and before a request id was recorded."
                        .to_string(),
                );
                if let Some(attempt) = record.attempts.last_mut() {
                    attempt.submission_result = AttemptSubmissionResult::Unknown;
                    attempt.error_stage = Some("resume safety".to_string());
                    attempt.error_message = record.last_error.clone();
                    attempt.finished_at = Some(Local::now());
                }
                changed = true;
            }
            if record.status == RecordStatus::UnknownStatus && record.request_id.is_none() {
                record.status = RecordStatus::AmbiguousSubmission;
                record.last_error = Some(
                    "Unknown status without a request id cannot be polled or resent safely."
                        .to_string(),
                );
                changed = true;
            }
        }
        if changed {
            self.recalculate_summary();
        }
        changed
    }

    #[allow(dead_code)]
    pub fn record_mut(&mut self, plu_number: u64) -> Result<&mut PluManifestRecord, AppError> {
        self.records
            .iter_mut()
            .find(|record| record.plu_number == plu_number)
            .ok_or_else(|| {
                AppError::Internal(format!("manifest record not found for PLU {plu_number}"))
            })
    }
}

impl ManifestSummary {
    pub fn from_records(records: &[PluManifestRecord]) -> Self {
        let mut counts = BTreeMap::<RecordStatus, usize>::new();
        for record in records {
            *counts.entry(record.status).or_default() += 1;
        }
        Self {
            not_attempted: *counts.get(&RecordStatus::NotAttempted).unwrap_or(&0),
            submission_started: *counts.get(&RecordStatus::SubmissionStarted).unwrap_or(&0),
            request_accepted: *counts.get(&RecordStatus::RequestAccepted).unwrap_or(&0),
            processing: *counts.get(&RecordStatus::Processing).unwrap_or(&0),
            success: *counts.get(&RecordStatus::Success).unwrap_or(&0),
            failed: *counts.get(&RecordStatus::Failed).unwrap_or(&0),
            unknown_status: *counts.get(&RecordStatus::UnknownStatus).unwrap_or(&0),
            ambiguous_submission: *counts.get(&RecordStatus::AmbiguousSubmission).unwrap_or(&0),
        }
    }
}

impl PluManifestRecord {
    pub fn new(
        plu_number: u64,
        department: Option<u32>,
        group: Option<u32>,
        selection_index: usize,
        payload_sha256: String,
    ) -> Self {
        Self {
            plu_number,
            department,
            group,
            selection_index,
            payload_sha256,
            status: RecordStatus::NotAttempted,
            attempt_count: 0,
            request_id: None,
            last_known_remote_status: None,
            submission_started_at: None,
            request_accepted_at: None,
            last_polled_at: None,
            completed_at: None,
            not_attempted_reason: None,
            last_error: None,
            attempts: Vec::new(),
        }
    }

    pub fn transition_to(&mut self, next: RecordStatus) -> Result<(), AppError> {
        if self.status == next || self.status.may_transition_to(next) {
            self.status = next;
            Ok(())
        } else {
            Err(AppError::Internal(format!(
                "invalid manifest state transition for PLU {}: {} -> {}",
                self.plu_number,
                self.status.as_text(),
                next.as_text()
            )))
        }
    }

    pub fn begin_attempt(&mut self) -> Result<(), AppError> {
        self.attempt_count = self.attempt_count.saturating_add(1);
        let now = Local::now();
        self.submission_started_at = Some(now);
        self.request_id = None;
        self.last_error = None;
        self.transition_to(RecordStatus::SubmissionStarted)?;
        self.attempts.push(AttemptRecord {
            attempt_number: self.attempt_count,
            started_at: now,
            request_id: None,
            submission_result: AttemptSubmissionResult::Started,
            last_remote_status: None,
            finished_at: None,
            error_stage: None,
            error_message: None,
        });
        Ok(())
    }

    pub fn mark_request_accepted(
        &mut self,
        request_id: String,
        remote_status: Option<String>,
    ) -> Result<(), AppError> {
        let now = Local::now();
        self.request_id = Some(request_id.clone());
        self.request_accepted_at = Some(now);
        self.last_known_remote_status = remote_status;
        self.transition_to(RecordStatus::RequestAccepted)?;
        if let Some(attempt) = self.attempts.last_mut() {
            attempt.request_id = Some(request_id);
            attempt.submission_result = AttemptSubmissionResult::Accepted;
            attempt.last_remote_status = self.last_known_remote_status.clone();
        }
        Ok(())
    }

    pub fn mark_processing(&mut self, remote_status: impl Into<String>) -> Result<(), AppError> {
        let remote_status = remote_status.into();
        self.last_known_remote_status = Some(remote_status.clone());
        self.last_polled_at = Some(Local::now());
        self.transition_to(RecordStatus::Processing)?;
        if let Some(attempt) = self.attempts.last_mut() {
            attempt.last_remote_status = Some(remote_status);
        }
        Ok(())
    }

    pub fn mark_success(&mut self, remote_status: impl Into<String>) -> Result<(), AppError> {
        let remote_status = remote_status.into();
        let now = Local::now();
        self.last_known_remote_status = Some(remote_status.clone());
        self.last_polled_at = Some(now);
        self.completed_at = Some(now);
        self.transition_to(RecordStatus::Success)?;
        if let Some(attempt) = self.attempts.last_mut() {
            attempt.last_remote_status = Some(remote_status);
            attempt.finished_at = Some(now);
        }
        Ok(())
    }

    pub fn mark_failed(&mut self, stage: &str, message: impl AsRef<str>) -> Result<(), AppError> {
        let now = Local::now();
        self.completed_at = Some(now);
        self.last_error = Some(sanitize_manifest_error(message));
        self.transition_to(RecordStatus::Failed)?;
        if let Some(attempt) = self.attempts.last_mut() {
            attempt.submission_result = if attempt.request_id.is_some() {
                AttemptSubmissionResult::Accepted
            } else {
                AttemptSubmissionResult::Rejected
            };
            attempt.finished_at = Some(now);
            attempt.error_stage = Some(stage.to_string());
            attempt.error_message = self.last_error.clone();
        }
        Ok(())
    }

    pub fn mark_unknown(&mut self, message: impl AsRef<str>) -> Result<(), AppError> {
        let now = Local::now();
        self.last_polled_at = Some(now);
        self.last_error = Some(sanitize_manifest_error(message));
        self.transition_to(RecordStatus::UnknownStatus)?;
        if let Some(attempt) = self.attempts.last_mut() {
            attempt.finished_at = Some(now);
            attempt.error_stage = Some("status polling".to_string());
            attempt.error_message = self.last_error.clone();
        }
        Ok(())
    }

    pub fn mark_ambiguous(&mut self, message: impl AsRef<str>) -> Result<(), AppError> {
        let now = Local::now();
        self.completed_at = Some(now);
        self.last_error = Some(sanitize_manifest_error(message));
        self.status = RecordStatus::AmbiguousSubmission;
        if let Some(attempt) = self.attempts.last_mut() {
            attempt.submission_result = AttemptSubmissionResult::Unknown;
            attempt.finished_at = Some(now);
            attempt.error_stage = Some("ambiguous submission".to_string());
            attempt.error_message = self.last_error.clone();
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(status: RecordStatus) -> PluManifestRecord {
        let mut record = PluManifestRecord::new(1, Some(1), Some(997), 1, "a".repeat(64));
        record.status = status;
        record
    }

    #[test]
    fn state_transitions_allow_safe_sequence_and_reject_invalid_jump() {
        let mut manifest_record = record(RecordStatus::NotAttempted);

        manifest_record.begin_attempt().expect("begin");
        assert_eq!(manifest_record.status, RecordStatus::SubmissionStarted);
        manifest_record
            .mark_request_accepted("abc".to_string(), Some("TODO".to_string()))
            .expect("accepted");
        assert_eq!(manifest_record.status, RecordStatus::RequestAccepted);
        manifest_record
            .mark_processing("PROCESSING")
            .expect("processing");
        manifest_record.mark_success("SUCCESS").expect("success");

        let mut invalid = record(RecordStatus::Success);
        assert!(
            invalid
                .transition_to(RecordStatus::SubmissionStarted)
                .is_err()
        );
    }

    #[test]
    fn summary_counts_all_states() {
        let summary = ManifestSummary::from_records(&[
            record(RecordStatus::NotAttempted),
            record(RecordStatus::Success),
            record(RecordStatus::Failed),
            record(RecordStatus::UnknownStatus),
            record(RecordStatus::AmbiguousSubmission),
        ]);

        assert_eq!(summary.not_attempted, 1);
        assert_eq!(summary.success, 1);
        assert_eq!(summary.failed, 1);
        assert_eq!(summary.unknown_status, 1);
        assert_eq!(summary.ambiguous_submission, 1);
    }

    fn manifest_with_records(records: Vec<PluManifestRecord>) -> ImportManifest {
        ImportManifest::new(
            SourceIdentity {
                filename: "plu.mdb".to_string(),
                size_bytes: 1,
                sha256: "a".repeat(64),
            },
            TargetIdentity {
                base_url: "https://example".to_string(),
                store_number: 1,
                client_id: "digi".to_string(),
            },
            ManifestOptions {
                limit: None,
                continue_on_error: false,
                test_alias_used: false,
            },
            1,
            records,
        )
    }

    #[test]
    fn new_manifest_starts_in_progress_and_serializes_run_status() {
        let manifest = manifest_with_records(vec![record(RecordStatus::NotAttempted)]);
        let json = serde_json::to_string(&manifest).expect("json");

        assert_eq!(manifest.run_status, RunStatus::InProgress);
        assert!(json.contains("\"run_status\":\"in_progress\""));
    }

    #[test]
    fn completed_manifest_with_all_success_persists_success() {
        let mut manifest = manifest_with_records(vec![record(RecordStatus::Success)]);

        manifest.recalculate_summary();

        assert_eq!(manifest.run_status, RunStatus::Success);
    }

    #[test]
    fn completed_manifest_with_failures_persists_completed_with_failures() {
        let mut manifest = manifest_with_records(vec![
            record(RecordStatus::Success),
            record(RecordStatus::Failed),
        ]);

        manifest.recalculate_summary();

        assert_eq!(manifest.run_status, RunStatus::CompletedWithFailures);
    }

    #[test]
    fn completed_manifest_with_pending_records_persists_incomplete() {
        let mut manifest = manifest_with_records(vec![
            record(RecordStatus::Success),
            record(RecordStatus::UnknownStatus),
        ]);

        manifest.recalculate_summary();

        assert_eq!(manifest.run_status, RunStatus::Incomplete);
    }

    #[test]
    fn interrupted_manifest_persists_interrupted() {
        let mut manifest = manifest_with_records(vec![record(RecordStatus::NotAttempted)]);

        manifest.mark_interrupted();

        assert_eq!(manifest.run_status, RunStatus::Interrupted);
    }

    #[test]
    fn restarted_transient_submission_becomes_ambiguous() {
        let mut manifest = manifest_with_records(vec![record(RecordStatus::SubmissionStarted)]);

        assert!(manifest.mark_restarted_transients_ambiguous());
        assert_eq!(
            manifest.records[0].status,
            RecordStatus::AmbiguousSubmission
        );
        assert_eq!(manifest.summary.ambiguous_submission, 1);
    }
}
