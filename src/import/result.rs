use chrono::{DateTime, Local};

use crate::digiweb::status::ProcessingStatus;

#[derive(Debug, Clone)]
pub struct RecordImportResult {
    pub plu_number: u64,
    pub started_at: DateTime<Local>,
    pub api_request_id: Option<String>,
    pub http_result: String,
    pub final_status: ProcessingStatus,
    pub failure_message: Option<String>,
    pub duration_ms: u128,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FinalStatus {
    Success,
    CompletedWithErrors,
    Failed,
}

impl FinalStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Success => "SUCCESS",
            Self::CompletedWithErrors => "COMPLETED_WITH_ERRORS",
            Self::Failed => "FAILED",
        }
    }

    pub fn exit_code(self) -> i32 {
        match self {
            Self::Success => 0,
            Self::CompletedWithErrors => 1,
            Self::Failed => 2,
        }
    }
}

#[derive(Debug, Default)]
pub struct ImportSummary {
    pub discovered: usize,
    pub selected: usize,
    pub submitted: usize,
    pub succeeded: usize,
    pub failed: usize,
    pub unknown: usize,
    pub intentionally_skipped_by_limit: usize,
    pub not_attempted_after_stop: usize,
    pub records: Vec<RecordImportResult>,
}

impl ImportSummary {
    pub fn final_status(&self) -> FinalStatus {
        if self.failed == 0 && self.unknown == 0 && self.not_attempted_after_stop == 0 {
            FinalStatus::Success
        } else {
            FinalStatus::CompletedWithErrors
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn final_status_success_exit_zero() {
        let summary = ImportSummary {
            discovered: 1,
            selected: 1,
            submitted: 1,
            succeeded: 1,
            failed: 0,
            unknown: 0,
            intentionally_skipped_by_limit: 0,
            not_attempted_after_stop: 0,
            records: Vec::new(),
        };

        assert_eq!(summary.final_status(), FinalStatus::Success);
        assert_eq!(summary.final_status().exit_code(), 0);
    }

    #[test]
    fn final_status_partial_failure_exit_one() {
        let summary = ImportSummary {
            discovered: 2,
            selected: 2,
            submitted: 2,
            succeeded: 1,
            failed: 1,
            unknown: 0,
            intentionally_skipped_by_limit: 0,
            not_attempted_after_stop: 0,
            records: Vec::new(),
        };

        assert_eq!(summary.final_status(), FinalStatus::CompletedWithErrors);
        assert_eq!(summary.final_status().exit_code(), 1);
    }

    #[test]
    fn final_status_unknown_exit_one() {
        let summary = ImportSummary {
            discovered: 1,
            selected: 1,
            submitted: 1,
            succeeded: 0,
            failed: 0,
            unknown: 1,
            intentionally_skipped_by_limit: 0,
            not_attempted_after_stop: 0,
            records: Vec::new(),
        };

        assert_eq!(summary.final_status(), FinalStatus::CompletedWithErrors);
        assert_eq!(summary.final_status().exit_code(), 1);
    }

    #[test]
    fn intentional_limit_skip_does_not_make_final_status_error() {
        let summary = ImportSummary {
            discovered: 4,
            selected: 1,
            submitted: 1,
            succeeded: 1,
            failed: 0,
            unknown: 0,
            intentionally_skipped_by_limit: 3,
            not_attempted_after_stop: 0,
            records: Vec::new(),
        };

        assert_eq!(summary.final_status(), FinalStatus::Success);
        assert_eq!(summary.final_status().exit_code(), 0);
    }
}
