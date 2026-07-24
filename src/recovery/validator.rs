use std::collections::HashSet;

use chrono::{DateTime, Local};

use crate::config::AppConfig;
use crate::error::AppError;
use crate::recovery::model::{
    ImportManifest, ManifestSummary, PluManifestRecord, RecordStatus, RunStatus, SourceIdentity,
    TargetIdentity,
};
use crate::recovery::{MANIFEST_SCHEMA_VERSION, sha256_json};
use crate::{digiweb::payload::DigiwebPluPayload, models::plu::Plu};

pub fn validate_manifest(manifest: &ImportManifest) -> Result<(), AppError> {
    if manifest.schema_version != MANIFEST_SCHEMA_VERSION {
        return invalid_manifest(format!(
            "unsupported schema version {}",
            manifest.schema_version
        ));
    }
    if manifest.application_version.trim().is_empty() {
        return invalid_manifest("application version is missing");
    }
    validate_timestamp(manifest.created_at, "created_at")?;
    validate_timestamp(manifest.updated_at, "updated_at")?;
    validate_hash("source.sha256", &manifest.source.sha256)?;
    if manifest.source.filename != "plu.mdb" {
        return invalid_manifest("source filename must be plu.mdb");
    }
    if manifest.target.base_url.trim().is_empty()
        || manifest.target.client_id.trim().is_empty()
        || manifest.target.store_number == 0
    {
        return invalid_manifest("target identity is incomplete");
    }
    if manifest.selection.selected_count != manifest.records.len() {
        return invalid_manifest("selection selected_count does not match records length");
    }
    if manifest.selection.selected_order.len() != manifest.records.len() {
        return invalid_manifest("selection selected_order does not match records length");
    }
    let mut plu_numbers = HashSet::new();
    let mut indices = HashSet::new();
    for record in &manifest.records {
        validate_record(record)?;
        if !plu_numbers.insert(record.plu_number) {
            return invalid_manifest(format!("duplicate PLU record {}", record.plu_number));
        }
        if !indices.insert(record.selection_index) {
            return invalid_manifest(format!(
                "duplicate selection index {}",
                record.selection_index
            ));
        }
    }
    let order = manifest
        .records
        .iter()
        .map(|record| record.plu_number)
        .collect::<Vec<_>>();
    if order != manifest.selection.selected_order {
        return invalid_manifest("selected_order does not match record order");
    }
    let recalculated = ManifestSummary::from_records(&manifest.records);
    if recalculated != manifest.summary {
        return invalid_manifest("summary counts do not match records");
    }
    validate_run_status(manifest)?;
    Ok(())
}

pub fn validate_resume_compatibility(
    manifest: &ImportManifest,
    source: &SourceIdentity,
    target: &TargetIdentity,
    selected_plus: &[Plu],
    payloads: &[DigiwebPluPayload],
    config: &AppConfig,
) -> Result<(), AppError> {
    validate_manifest(manifest)?;
    if &manifest.source != source {
        return Err(AppError::Config(
            "The current plu.mdb does not match the source recorded in this manifest. Resume was cancelled before authentication or API submission. Start a new import instead of resuming this manifest."
                .to_string(),
        ));
    }
    if &manifest.target != target {
        return Err(AppError::Config(
            "The current DIGIweb target does not match the manifest target. Resume was cancelled."
                .to_string(),
        ));
    }
    if manifest.records.len() != selected_plus.len() || selected_plus.len() != payloads.len() {
        return Err(AppError::Config(
            "The normalized selected PLUs differ from the manifest. Resume was cancelled before authentication or API submission."
                .to_string(),
        ));
    }
    for ((record, plu), payload) in manifest.records.iter().zip(selected_plus).zip(payloads) {
        if record.plu_number != plu.plu_number
            || record.department != plu.department_number
            || record.group != plu.group_number
        {
            return Err(AppError::Config(
                "The normalized selected PLUs differ from the manifest. Resume was cancelled before authentication or API submission."
                    .to_string(),
            ));
        }
        let payload_sha256 = sha256_json(payload)?;
        if payload_sha256 != record.payload_sha256 {
            return Err(AppError::Config(
                "The current payload hash differs from the manifest. Resume was cancelled before authentication or API submission."
                    .to_string(),
            ));
        }
    }
    if manifest.options.continue_on_error != config.import.continue_after_record_failure {
        // This is intentionally allowed; runtime options decide continuation. Keep validation
        // focused on source, target, and payload identity.
    }
    Ok(())
}

pub fn target_identity(config: &AppConfig) -> TargetIdentity {
    TargetIdentity {
        base_url: config.digiweb.base_url.trim_end_matches('/').to_string(),
        store_number: config.digiweb.store_number,
        client_id: config.digiweb.client_id.clone(),
    }
}

fn validate_record(record: &PluManifestRecord) -> Result<(), AppError> {
    validate_hash("payload_sha256", &record.payload_sha256)?;
    if record.selection_index == 0 {
        return invalid_manifest(format!(
            "PLU {} has invalid selection index 0",
            record.plu_number
        ));
    }
    if record.attempt_count as usize != record.attempts.len() {
        return invalid_manifest(format!(
            "PLU {} attempt_count does not match attempts length",
            record.plu_number
        ));
    }
    match record.status {
        RecordStatus::RequestAccepted | RecordStatus::Processing => {
            if record.request_id.as_deref().unwrap_or("").trim().is_empty() {
                return invalid_manifest(format!(
                    "PLU {} requires a request_id for status {}",
                    record.plu_number,
                    record.status.as_text()
                ));
            }
        }
        RecordStatus::NotAttempted => {
            if record.request_id.is_some() {
                return invalid_manifest(format!(
                    "PLU {} cannot have a request_id while NOT_ATTEMPTED",
                    record.plu_number
                ));
            }
        }
        _ => {}
    }
    for attempt in &record.attempts {
        validate_timestamp(attempt.started_at, "attempt.started_at")?;
        if let Some(finished_at) = attempt.finished_at {
            validate_timestamp(finished_at, "attempt.finished_at")?;
        }
        if let Some(message) = &attempt.error_message {
            let lower = message.to_ascii_lowercase();
            if lower.contains("access_token")
                || lower.contains("authorization")
                || lower.contains("client_secret")
            {
                return invalid_manifest(format!(
                    "PLU {} attempt contains secret-bearing text",
                    record.plu_number
                ));
            }
        }
    }
    Ok(())
}

fn validate_run_status(manifest: &ImportManifest) -> Result<(), AppError> {
    let derived = manifest.derived_run_status();
    if manifest.run_status == derived {
        return Ok(());
    }
    if manifest.run_status == RunStatus::InProgress
        || matches!(
            (manifest.run_status, derived),
            (RunStatus::Interrupted, RunStatus::Incomplete)
        )
    {
        return Ok(());
    }
    invalid_manifest(format!(
        "run_status {} does not agree with record states ({})",
        manifest.run_status.as_text(),
        derived.as_text()
    ))
}

fn validate_hash(name: &str, value: &str) -> Result<(), AppError> {
    if value.len() == 64 && value.chars().all(|ch| ch.is_ascii_hexdigit()) {
        Ok(())
    } else {
        invalid_manifest(format!("{name} must be a SHA-256 hex string"))
    }
}

fn validate_timestamp(_value: DateTime<Local>, _name: &str) -> Result<(), AppError> {
    Ok(())
}

fn invalid_manifest<T>(message: impl AsRef<str>) -> Result<T, AppError> {
    Err(AppError::Config(format!(
        "import-results.json is invalid or internally inconsistent: {}. The manifest was not modified and no API requests were attempted.",
        message.as_ref()
    )))
}

#[cfg(test)]
mod tests {
    use crate::recovery::model::{
        ImportManifest, ManifestOptions, PluManifestRecord, RecordStatus, RunStatus, SourceIdentity,
    };

    use super::*;

    fn manifest() -> ImportManifest {
        ImportManifest::new(
            SourceIdentity {
                filename: "plu.mdb".to_string(),
                size_bytes: 10,
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
            vec![PluManifestRecord::new(
                1,
                Some(1),
                Some(997),
                1,
                "b".repeat(64),
            )],
        )
    }

    #[test]
    fn duplicate_plu_records_are_invalid() {
        let mut manifest = manifest();
        manifest.records.push(manifest.records[0].clone());
        manifest.selection.selected_count = 2;
        manifest.selection.selected_order.push(1);
        manifest.recalculate_summary();

        assert!(validate_manifest(&manifest).is_err());
    }

    #[test]
    fn summary_mismatch_is_invalid() {
        let mut manifest = manifest();
        manifest.summary.success = 99;

        assert!(validate_manifest(&manifest).is_err());
    }

    #[test]
    fn processing_without_request_id_is_invalid() {
        let mut manifest = manifest();
        manifest.records[0].status = RecordStatus::Processing;
        manifest.recalculate_summary();

        assert!(validate_manifest(&manifest).is_err());
    }

    #[test]
    fn stored_in_progress_with_pending_records_is_valid() {
        let manifest = manifest();

        assert_eq!(manifest.run_status, RunStatus::InProgress);
        assert!(validate_manifest(&manifest).is_ok());
    }

    #[test]
    fn stored_in_progress_with_completed_records_is_valid_for_crash_recovery() {
        let mut manifest = manifest();
        manifest.records[0].status = RecordStatus::Success;
        manifest.recalculate_summary_for_active_run();

        assert_eq!(manifest.run_status, RunStatus::InProgress);
        assert!(validate_manifest(&manifest).is_ok());
    }

    #[test]
    fn stored_terminal_status_must_match_records() {
        let mut manifest = manifest();
        manifest.run_status = RunStatus::Success;

        let error = validate_manifest(&manifest).expect_err("invalid");

        assert!(error.to_string().contains("run_status SUCCESS"));
    }
}
