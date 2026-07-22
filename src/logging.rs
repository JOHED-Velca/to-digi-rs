use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

use chrono::{DateTime, Local};

use crate::error::AppError;

pub struct FinalImportLog<'a> {
    pub status: &'a str,
    pub source_discovered: usize,
    pub placeholders_ignored: usize,
    pub placeholder_plu_numbers: &'a [u64],
    pub invalid_source_rows: usize,
    pub validation_skipped: usize,
    pub normalized: usize,
    pub valid: usize,
    pub selected: usize,
    pub submitted: usize,
    pub succeeded: usize,
    pub failed: usize,
    pub unknown: usize,
    pub not_attempted: usize,
    pub intentionally_skipped_by_limit: usize,
    pub successful_plu_numbers: &'a [u64],
    pub failed_plu_numbers: &'a [u64],
    pub unknown_plu_numbers: &'a [u64],
    pub dry_run: bool,
}

pub struct AuditLogger {
    writer: BufWriter<File>,
    started_at: DateTime<Local>,
}

impl AuditLogger {
    pub fn create(path: &Path) -> Result<Self, AppError> {
        let file = File::create(path).map_err(|err| AppError::Logging(err.to_string()))?;
        let started_at = Local::now();
        let mut logger = Self {
            writer: BufWriter::new(file),
            started_at,
        };
        logger.line("to-digi-rs")?;
        logger.kv("Application version", env!("CARGO_PKG_VERSION"))?;
        logger.kv("Started", &started_at.to_rfc3339())?;
        logger.line("")?;
        Ok(logger)
    }

    pub fn line(&mut self, message: impl AsRef<str>) -> Result<(), AppError> {
        writeln!(self.writer, "{}", message.as_ref())
            .map_err(|err| AppError::Logging(err.to_string()))
    }

    pub fn kv(&mut self, key: &str, value: &str) -> Result<(), AppError> {
        self.line(format!("{key}: {value}"))
    }

    pub fn warning(&mut self, message: impl AsRef<str>) -> Result<(), AppError> {
        self.line(format!("WARNING: {}", message.as_ref()))
    }

    pub fn error(&mut self, message: impl AsRef<str>) -> Result<(), AppError> {
        self.line(format!("ERROR: {}", message.as_ref()))
    }

    #[allow(dead_code)]
    pub fn final_success(
        &mut self,
        discovered: usize,
        submitted: usize,
        succeeded: usize,
        failed: usize,
        skipped: usize,
        unknown: usize,
        status: &str,
    ) -> Result<(), AppError> {
        let finished = Local::now();
        self.line("==================================================")?;
        self.kv("FINAL STATUS", status)?;
        self.kv("PLUs discovered", &discovered.to_string())?;
        self.kv("PLUs submitted", &submitted.to_string())?;
        self.kv("PLUs successfully imported", &succeeded.to_string())?;
        self.kv("PLUs failed", &failed.to_string())?;
        self.kv("PLUs skipped", &skipped.to_string())?;
        self.kv("PLUs submitted with unknown status", &unknown.to_string())?;
        self.kv("Started", &self.started_at.to_rfc3339())?;
        self.kv("Finished", &finished.to_rfc3339())?;
        self.line("==================================================")?;
        self.flush()
    }

    pub fn final_import_summary(&mut self, summary: FinalImportLog<'_>) -> Result<(), AppError> {
        let finished = Local::now();
        self.line("==================================================")?;
        self.kv("FINAL STATUS", summary.status)?;
        self.line("")?;
        self.kv(
            "Source rows discovered",
            &summary.source_discovered.to_string(),
        )?;
        self.kv(
            "Empty source placeholders ignored",
            &summary.placeholders_ignored.to_string(),
        )?;
        self.kv(
            "Invalid source rows",
            &summary.invalid_source_rows.to_string(),
        )?;
        self.kv("Normalized PLUs", &summary.normalized.to_string())?;
        self.kv(
            "PLUs skipped due to validation error",
            &summary.validation_skipped.to_string(),
        )?;
        if summary.dry_run {
            self.kv("Valid PLUs identified", &summary.valid.to_string())?;
        } else {
            self.kv("Valid PLUs available", &summary.valid.to_string())?;
        }
        self.kv("PLUs selected for import", &summary.selected.to_string())?;
        self.line("")?;
        self.kv("PLUs submitted", &summary.submitted.to_string())?;
        self.kv("PLUs successfully imported", &summary.succeeded.to_string())?;
        self.kv("PLUs failed", &summary.failed.to_string())?;
        self.kv("PLUs with unknown status", &summary.unknown.to_string())?;
        self.kv(
            "PLUs intentionally skipped by first-PLU limit",
            &summary.intentionally_skipped_by_limit.to_string(),
        )?;
        self.kv(
            "PLUs not attempted after failure",
            &summary.not_attempted.to_string(),
        )?;
        if summary.dry_run {
            self.line("Import intentionally disabled by inspection-only mode.")?;
        }
        self.line("")?;
        self.kv(
            "Successful PLUs",
            &format_plu_list(summary.successful_plu_numbers),
        )?;
        self.kv("Failed PLUs", &format_plu_list(summary.failed_plu_numbers))?;
        self.kv(
            "Unknown-status PLUs",
            &format_plu_list(summary.unknown_plu_numbers),
        )?;
        self.kv(
            "Ignored source placeholders",
            &format_plu_list(summary.placeholder_plu_numbers),
        )?;
        self.kv("Started", &self.started_at.to_rfc3339())?;
        self.kv("Finished", &finished.to_rfc3339())?;
        self.line("==================================================")?;
        self.flush()
    }

    pub fn final_failure(
        &mut self,
        stage: &str,
        error: &str,
        no_records_sent: bool,
    ) -> Result<(), AppError> {
        let finished = Local::now();
        self.line("==================================================")?;
        self.kv("FINAL STATUS", "FAILED")?;
        self.kv("Stage", stage)?;
        self.kv("Error", error)?;
        if no_records_sent {
            self.line("No PLU records were sent.")?;
        }
        self.kv("Started", &self.started_at.to_rfc3339())?;
        self.kv("Finished", &finished.to_rfc3339())?;
        self.line("==================================================")?;
        self.flush()
    }

    pub fn flush(&mut self) -> Result<(), AppError> {
        self.writer
            .flush()
            .map_err(|err| AppError::Logging(err.to_string()))
    }
}

fn format_plu_list(values: &[u64]) -> String {
    if values.is_empty() {
        "None".to_string()
    } else {
        const MAX_INLINE: usize = 50;
        let shown = values
            .iter()
            .take(MAX_INLINE)
            .map(u64::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        if values.len() > MAX_INLINE {
            format!("{shown}, ... ({} more)", values.len() - MAX_INLINE)
        } else {
            shown
        }
    }
}

#[allow(dead_code)]
pub fn redact_for_log(value: &str) -> String {
    if value.len() <= 8 {
        "<redacted>".to_string()
    } else {
        format!("{}...<redacted>", &value[..4])
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::*;

    #[test]
    fn redacts_short_and_long_values() {
        assert_eq!(redact_for_log("secret"), "<redacted>");
        assert_eq!(redact_for_log("abcdefghijkl"), "abcd...<redacted>");
    }

    #[test]
    fn final_status_is_written_to_log() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("logs.txt");
        let mut logger = AuditLogger::create(&path).expect("logger");

        logger
            .final_success(1, 1, 1, 0, 0, 0, "SUCCESS")
            .expect("final");

        let contents = fs::read_to_string(path).expect("read");
        assert!(contents.contains("FINAL STATUS: SUCCESS"));
        assert!(contents.contains("PLUs submitted: 1"));
        assert!(contents.contains("PLUs successfully imported: 1"));
    }

    #[test]
    fn final_import_summary_separates_limit_skip_from_errors() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("logs.txt");
        let mut logger = AuditLogger::create(&path).expect("logger");

        logger
            .final_import_summary(FinalImportLog {
                status: "SUCCESS",
                source_discovered: 5,
                placeholders_ignored: 1,
                placeholder_plu_numbers: &[0],
                invalid_source_rows: 0,
                validation_skipped: 0,
                normalized: 4,
                valid: 4,
                selected: 1,
                submitted: 1,
                succeeded: 1,
                failed: 0,
                unknown: 0,
                not_attempted: 0,
                intentionally_skipped_by_limit: 3,
                successful_plu_numbers: &[1],
                failed_plu_numbers: &[],
                unknown_plu_numbers: &[],
                dry_run: false,
            })
            .expect("summary");

        let contents = fs::read_to_string(path).expect("read");
        assert!(contents.contains("FINAL STATUS: SUCCESS"));
        assert!(contents.contains("PLUs intentionally skipped by first-PLU limit: 3"));
        assert!(contents.contains("Successful PLUs: 1"));
        assert!(contents.contains("Failed PLUs: None"));
        assert!(contents.contains("Ignored source placeholders: 0"));
    }

    #[test]
    fn final_import_summary_uses_dry_run_wording() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("logs.txt");
        let mut logger = AuditLogger::create(&path).expect("logger");

        logger
            .final_import_summary(FinalImportLog {
                status: "SUCCESS",
                source_discovered: 5,
                placeholders_ignored: 1,
                placeholder_plu_numbers: &[0],
                invalid_source_rows: 0,
                validation_skipped: 0,
                normalized: 4,
                valid: 4,
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
            })
            .expect("summary");

        let contents = fs::read_to_string(path).expect("read");
        assert!(contents.contains("Valid PLUs identified: 4"));
        assert!(contents.contains("PLUs submitted: 0"));
        assert!(contents.contains("Import intentionally disabled by inspection-only mode."));
        assert!(!contents.contains("PLUs failed: 1"));
    }

    #[test]
    fn long_plu_lists_are_bounded() {
        let values = (1..=60).collect::<Vec<_>>();

        let formatted = format_plu_list(&values);

        assert!(formatted.contains("... (10 more)"));
        assert!(!formatted.contains("60"));
    }
}
