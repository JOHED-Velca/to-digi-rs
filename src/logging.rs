use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

use chrono::{DateTime, Local};

use crate::error::AppError;

pub struct FinalImportLog<'a> {
    pub status: &'a str,
    pub source_discovered: usize,
    pub placeholders_ignored: usize,
    pub validation_skipped: usize,
    pub valid_available: usize,
    pub selected: usize,
    pub submitted: usize,
    pub succeeded: usize,
    pub failed: usize,
    pub unknown: usize,
    pub not_attempted: usize,
    pub intentionally_skipped_by_limit: usize,
    pub successful_plu_numbers: &'a [u64],
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
            "Source PLUs discovered",
            &summary.source_discovered.to_string(),
        )?;
        self.kv(
            "Empty placeholder PLUs ignored",
            &summary.placeholders_ignored.to_string(),
        )?;
        self.kv(
            "PLUs skipped due to validation error",
            &summary.validation_skipped.to_string(),
        )?;
        self.kv("Valid PLUs available", &summary.valid_available.to_string())?;
        self.kv("Valid PLUs selected", &summary.selected.to_string())?;
        self.line("")?;
        self.kv("PLUs submitted", &summary.submitted.to_string())?;
        self.kv("PLUs successfully imported", &summary.succeeded.to_string())?;
        self.kv("PLUs failed", &summary.failed.to_string())?;
        self.kv("PLUs with unknown status", &summary.unknown.to_string())?;
        self.kv("PLUs not attempted", &summary.not_attempted.to_string())?;
        self.kv(
            "PLUs intentionally skipped by limit",
            &summary.intentionally_skipped_by_limit.to_string(),
        )?;
        self.line("")?;
        self.kv(
            "Successful PLUs",
            &format_plu_list(summary.successful_plu_numbers),
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
        "<none>".to_string()
    } else {
        values
            .iter()
            .map(u64::to_string)
            .collect::<Vec<_>>()
            .join(", ")
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
                validation_skipped: 0,
                valid_available: 4,
                selected: 1,
                submitted: 1,
                succeeded: 1,
                failed: 0,
                unknown: 0,
                not_attempted: 0,
                intentionally_skipped_by_limit: 3,
                successful_plu_numbers: &[1],
            })
            .expect("summary");

        let contents = fs::read_to_string(path).expect("read");
        assert!(contents.contains("FINAL STATUS: SUCCESS"));
        assert!(contents.contains("PLUs intentionally skipped by limit: 3"));
        assert!(contents.contains("Successful PLUs: 1"));
    }
}
