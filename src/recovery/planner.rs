use crate::recovery::model::{
    ImportManifest, RecordStatus, ResumePlan, ResumePlanItem, ResumePlanItemKind,
};

pub fn build_resume_plan(manifest: &ImportManifest, retry_failed: bool) -> ResumePlan {
    let mut items = Vec::new();
    let mut existing_requests_to_poll = 0;
    let mut not_attempted_to_submit = 0;
    let mut confirmed_failures = 0;
    let mut ambiguous_submissions = 0;
    let mut already_successful = 0;

    for status in [
        RecordStatus::RequestAccepted,
        RecordStatus::Processing,
        RecordStatus::UnknownStatus,
        RecordStatus::NotAttempted,
        RecordStatus::Failed,
        RecordStatus::AmbiguousSubmission,
        RecordStatus::Success,
    ] {
        for record in manifest
            .records
            .iter()
            .filter(|record| record.status == status)
        {
            let kind = match record.status {
                RecordStatus::RequestAccepted | RecordStatus::Processing => {
                    existing_requests_to_poll += 1;
                    ResumePlanItemKind::PollExistingRequest
                }
                RecordStatus::UnknownStatus if record.request_id.is_some() => {
                    existing_requests_to_poll += 1;
                    ResumePlanItemKind::PollExistingRequest
                }
                RecordStatus::UnknownStatus => {
                    ambiguous_submissions += 1;
                    ResumePlanItemKind::SkipAmbiguous
                }
                RecordStatus::NotAttempted => {
                    not_attempted_to_submit += 1;
                    ResumePlanItemKind::SubmitNotAttempted
                }
                RecordStatus::Failed if retry_failed => {
                    confirmed_failures += 1;
                    ResumePlanItemKind::RetryConfirmedFailure
                }
                RecordStatus::Failed => {
                    confirmed_failures += 1;
                    ResumePlanItemKind::SkipFailed
                }
                RecordStatus::AmbiguousSubmission | RecordStatus::SubmissionStarted => {
                    ambiguous_submissions += 1;
                    ResumePlanItemKind::SkipAmbiguous
                }
                RecordStatus::Success => {
                    already_successful += 1;
                    ResumePlanItemKind::SkipAlreadySuccessful
                }
            };
            items.push(ResumePlanItem {
                plu_number: record.plu_number,
                kind,
            });
        }
    }

    ResumePlan {
        items,
        existing_requests_to_poll,
        not_attempted_to_submit,
        confirmed_failures,
        ambiguous_submissions,
        already_successful,
    }
}

#[cfg(test)]
mod tests {
    use crate::recovery::model::{
        ImportManifest, ManifestOptions, PluManifestRecord, RecordStatus, SourceIdentity,
        TargetIdentity,
    };

    use super::*;

    fn manifest_with(statuses: &[RecordStatus]) -> ImportManifest {
        let records = statuses
            .iter()
            .enumerate()
            .map(|(index, status)| {
                let mut record = PluManifestRecord::new(
                    (index + 1) as u64,
                    Some(1),
                    Some(997),
                    index + 1,
                    "a".repeat(64),
                );
                record.status = *status;
                if matches!(
                    status,
                    RecordStatus::RequestAccepted
                        | RecordStatus::Processing
                        | RecordStatus::UnknownStatus
                ) {
                    record.request_id = Some(format!("req-{}", index + 1));
                }
                record
            })
            .collect::<Vec<_>>();
        ImportManifest::new(
            SourceIdentity {
                filename: "plu.mdb".to_string(),
                size_bytes: 1,
                sha256: "b".repeat(64),
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
            statuses.len(),
            records,
        )
    }

    #[test]
    fn resume_plan_polls_existing_before_submitting_new_records() {
        let manifest = manifest_with(&[
            RecordStatus::NotAttempted,
            RecordStatus::Processing,
            RecordStatus::Success,
        ]);

        let plan = build_resume_plan(&manifest, false);

        assert_eq!(plan.items[0].plu_number, 2);
        assert_eq!(plan.items[0].kind, ResumePlanItemKind::PollExistingRequest);
        assert_eq!(plan.items[1].plu_number, 1);
        assert_eq!(plan.items[1].kind, ResumePlanItemKind::SubmitNotAttempted);
        assert_eq!(
            plan.items[2].kind,
            ResumePlanItemKind::SkipAlreadySuccessful
        );
    }

    #[test]
    fn retry_failed_only_changes_confirmed_failed_records() {
        let mut manifest = manifest_with(&[
            RecordStatus::Failed,
            RecordStatus::UnknownStatus,
            RecordStatus::AmbiguousSubmission,
        ]);
        manifest.records[1].request_id = Some("req-2".to_string());

        let plan = build_resume_plan(&manifest, true);

        assert!(
            plan.items
                .iter()
                .any(|item| item.kind == ResumePlanItemKind::RetryConfirmedFailure)
        );
        assert!(
            plan.items
                .iter()
                .any(|item| item.kind == ResumePlanItemKind::PollExistingRequest)
        );
        assert!(!plan.items.iter().any(
            |item| item.plu_number == 2 && item.kind == ResumePlanItemKind::SubmitNotAttempted
        ));
    }
}
