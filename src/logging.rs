use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

use chrono::{DateTime, Local};

use crate::error::AppError;

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
}
