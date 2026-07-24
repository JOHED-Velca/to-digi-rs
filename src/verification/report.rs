use std::fs;
use std::path::Path;

use chrono::Local;
use serde::{Deserialize, Serialize};

use crate::error::AppError;

#[derive(Debug, Clone)]
pub struct VerificationDiscoveryInput {
    pub store_number: u32,
    pub source_file: String,
    pub limit: Option<usize>,
    pub valid_plu_count: usize,
    pub selected_plu_numbers: Vec<u64>,
    pub source_rows_discovered: usize,
    pub placeholders_ignored: usize,
    pub invalid_source_rows: usize,
    pub validation_skipped: usize,
    pub poll_interval_seconds: u64,
    pub timeout_seconds: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VerificationReport {
    pub schema_version: u32,
    pub application_version: String,
    pub generated_at: String,
    pub verification_status: VerificationStatus,
    pub scope: VerificationScope,
    pub source_summary: VerificationSourceSummary,
    pub coverage: VerificationCoverage,
    pub summary: VerificationSummary,
    pub field_summary: Vec<FieldSummary>,
    pub results: Vec<PluVerificationResult>,
    pub api_evidence: Vec<ApiEvidence>,
    pub missing_api_information: Vec<String>,
    pub timing: VerificationTiming,
    pub safety: VerificationSafety,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum VerificationStatus {
    BlockedApiDiscovery,
}

impl VerificationStatus {
    pub fn as_text(self) -> &'static str {
        match self {
            Self::BlockedApiDiscovery => "BLOCKED_API_DISCOVERY",
        }
    }

    pub fn exit_code(self) -> i32 {
        match self {
            Self::BlockedApiDiscovery => 2,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VerificationScope {
    pub store_number: u32,
    pub source_file: String,
    pub limit: Option<usize>,
    pub effective_limit: usize,
    pub valid_plu_count: usize,
    pub selected_plu_numbers: Vec<u64>,
    pub excluded_by_limit: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VerificationSourceSummary {
    pub source_rows_discovered: usize,
    pub placeholders_ignored: usize,
    pub invalid_source_rows: usize,
    pub validation_skipped: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VerificationCoverage {
    pub core_plu_data: CoverageStatus,
    pub barcode_data: CoverageStatus,
    pub ingredients: CoverageStatus,
    pub nutrition_facts: CoverageStatus,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum CoverageStatus {
    ApiNotConfirmed,
}

impl CoverageStatus {
    pub fn as_text(self) -> &'static str {
        match self {
            Self::ApiNotConfirmed => "API NOT CONFIRMED",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VerificationSummary {
    pub expected: usize,
    pub confirmed: usize,
    pub missing: usize,
    pub mismatched: usize,
    pub unverified: usize,
    pub duplicate: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FieldSummary {
    pub field_name: String,
    pub matched: usize,
    pub expected: usize,
    pub status: FieldSummaryStatus,
    pub explanation: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum FieldSummaryStatus {
    Unavailable,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PluVerificationResult {
    pub plu_number: u64,
    pub classification: PluClassification,
    pub explanation: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum PluClassification {
    Unverified,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ApiEvidence {
    pub source: String,
    pub finding: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VerificationTiming {
    pub poll_interval_seconds: u64,
    pub timeout_seconds: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VerificationSafety {
    pub write_requests_attempted: bool,
    pub database_access_attempted: bool,
    pub authentication_attempted: bool,
    pub digiweb_api_requests_attempted: bool,
}

pub fn build_discovery_blocked_report(input: VerificationDiscoveryInput) -> VerificationReport {
    let expected = input.selected_plu_numbers.len();
    VerificationReport {
        schema_version: 1,
        application_version: env!("CARGO_PKG_VERSION").to_string(),
        generated_at: Local::now().to_rfc3339(),
        verification_status: VerificationStatus::BlockedApiDiscovery,
        scope: VerificationScope {
            store_number: input.store_number,
            source_file: input.source_file,
            limit: input.limit,
            effective_limit: expected,
            valid_plu_count: input.valid_plu_count,
            selected_plu_numbers: input.selected_plu_numbers.clone(),
            excluded_by_limit: input.valid_plu_count.saturating_sub(expected),
        },
        source_summary: VerificationSourceSummary {
            source_rows_discovered: input.source_rows_discovered,
            placeholders_ignored: input.placeholders_ignored,
            invalid_source_rows: input.invalid_source_rows,
            validation_skipped: input.validation_skipped,
        },
        coverage: VerificationCoverage {
            core_plu_data: CoverageStatus::ApiNotConfirmed,
            barcode_data: CoverageStatus::ApiNotConfirmed,
            ingredients: CoverageStatus::ApiNotConfirmed,
            nutrition_facts: CoverageStatus::ApiNotConfirmed,
        },
        summary: VerificationSummary {
            expected,
            confirmed: 0,
            missing: 0,
            mismatched: 0,
            unverified: expected,
            duplicate: 0,
        },
        field_summary: unavailable_field_summaries(expected),
        results: input
            .selected_plu_numbers
            .into_iter()
            .map(|plu_number| PluVerificationResult {
                plu_number,
                classification: PluClassification::Unverified,
                explanation: "No confirmed supported PLU read API is available.".to_string(),
            })
            .collect(),
        api_evidence: evidence(),
        missing_api_information: missing_api_information(),
        timing: VerificationTiming {
            poll_interval_seconds: input.poll_interval_seconds,
            timeout_seconds: input.timeout_seconds,
        },
        safety: VerificationSafety {
            write_requests_attempted: false,
            database_access_attempted: false,
            authentication_attempted: false,
            digiweb_api_requests_attempted: false,
        },
    }
}

pub fn write_text_report(path: &Path, report: &VerificationReport) -> Result<(), AppError> {
    fs::write(path, render_text_report(report)).map_err(|err| {
        AppError::Logging(format!(
            "failed to write {}: {err}",
            path.file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("verification-report.txt")
        ))
    })
}

pub fn write_json_report(path: &Path, report: &VerificationReport) -> Result<(), AppError> {
    let json = serde_json::to_string_pretty(report).map_err(|err| {
        AppError::Internal(format!("verification JSON serialization failed: {err}"))
    })?;
    fs::write(path, json).map_err(|err| {
        AppError::Logging(format!(
            "failed to write {}: {err}",
            path.file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("verification-report.json")
        ))
    })
}

pub fn render_console_summary(report: &VerificationReport) -> String {
    let mut out = String::new();
    line(&mut out, "POST-IMPORT VERIFICATION BLOCKED");
    blank(&mut out);
    line(
        &mut out,
        format!(
            "Verification status: {}",
            report.verification_status.as_text()
        ),
    );
    blank(&mut out);
    line(
        &mut out,
        format!("Expected PLUs: {}", report.summary.expected),
    );
    line(
        &mut out,
        format!("Confirmed PLUs: {}", report.summary.confirmed),
    );
    line(
        &mut out,
        format!("Missing PLUs: {}", report.summary.missing),
    );
    line(
        &mut out,
        format!("Mismatched PLUs: {}", report.summary.mismatched),
    );
    line(
        &mut out,
        format!("Unverified PLUs: {}", report.summary.unverified),
    );
    line(
        &mut out,
        format!("Duplicate DIGIweb matches: {}", report.summary.duplicate),
    );
    line(
        &mut out,
        format!(
            "Effective limit: {}",
            report
                .scope
                .limit
                .map(|limit| limit.to_string())
                .unwrap_or_else(|| "none".to_string())
        ),
    );
    line(
        &mut out,
        format!("PLUs excluded by limit: {}", report.scope.excluded_by_limit),
    );
    blank(&mut out);
    line(&mut out, "Verification coverage:");
    line(
        &mut out,
        format!(
            "- Core PLU data: {}",
            report.coverage.core_plu_data.as_text()
        ),
    );
    line(
        &mut out,
        format!("- Barcode data: {}", report.coverage.barcode_data.as_text()),
    );
    line(
        &mut out,
        format!("- Ingredients: {}", report.coverage.ingredients.as_text()),
    );
    line(
        &mut out,
        format!(
            "- Nutrition facts: {}",
            report.coverage.nutrition_facts.as_text()
        ),
    );
    blank(&mut out);
    line(&mut out, "API information still required:");
    for item in &report.missing_api_information {
        line(&mut out, format!("- {item}"));
    }
    blank(&mut out);
    line(&mut out, "PLU write requests attempted: NO");
    line(&mut out, "Database access attempted: NO");
    line(
        &mut out,
        "No authentication or DIGIweb API requests were attempted.",
    );
    blank(&mut out);
    line(&mut out, "Text verification report:");
    line(&mut out, "./verification-report.txt");
    blank(&mut out);
    line(&mut out, "JSON verification report:");
    line(&mut out, "./verification-report.json");
    out
}

fn render_text_report(report: &VerificationReport) -> String {
    let mut out = String::new();
    line(&mut out, "DIGIweb Post-Import Verification Report");
    line(
        &mut out,
        format!("Application version: {}", report.application_version),
    );
    line(
        &mut out,
        format!(
            "Verification status: {}",
            report.verification_status.as_text()
        ),
    );
    blank(&mut out);

    section(&mut out, "1. Verification scope");
    line(
        &mut out,
        format!("Source file: {}", report.scope.source_file),
    );
    line(
        &mut out,
        format!("Store number: {}", report.scope.store_number),
    );
    line(
        &mut out,
        format!(
            "Limit: {}",
            report
                .scope
                .limit
                .map(|limit| limit.to_string())
                .unwrap_or_else(|| "none".to_string())
        ),
    );
    line(
        &mut out,
        format!("Effective selected PLUs: {}", report.scope.effective_limit),
    );
    line(
        &mut out,
        format!("PLUs excluded by limit: {}", report.scope.excluded_by_limit),
    );
    blank(&mut out);

    section(&mut out, "2. Source summary");
    line(
        &mut out,
        format!(
            "Source rows discovered: {}",
            report.source_summary.source_rows_discovered
        ),
    );
    line(
        &mut out,
        format!(
            "Empty placeholders ignored: {}",
            report.source_summary.placeholders_ignored
        ),
    );
    line(
        &mut out,
        format!(
            "Invalid source rows: {}",
            report.source_summary.invalid_source_rows
        ),
    );
    line(
        &mut out,
        format!(
            "Validation skipped PLUs: {}",
            report.source_summary.validation_skipped
        ),
    );
    blank(&mut out);

    section(&mut out, "3. DIGIweb API coverage");
    line(
        &mut out,
        format!("Core PLU data: {}", report.coverage.core_plu_data.as_text()),
    );
    line(
        &mut out,
        format!("Barcode data: {}", report.coverage.barcode_data.as_text()),
    );
    line(
        &mut out,
        format!("Ingredients: {}", report.coverage.ingredients.as_text()),
    );
    line(
        &mut out,
        format!(
            "Nutrition facts: {}",
            report.coverage.nutrition_facts.as_text()
        ),
    );
    blank(&mut out);

    section(&mut out, "4. Summary counts");
    line(
        &mut out,
        format!("Expected PLUs: {}", report.summary.expected),
    );
    line(
        &mut out,
        format!("Confirmed PLUs: {}", report.summary.confirmed),
    );
    line(
        &mut out,
        format!("Missing PLUs: {}", report.summary.missing),
    );
    line(
        &mut out,
        format!("Mismatched PLUs: {}", report.summary.mismatched),
    );
    line(
        &mut out,
        format!("Unverified PLUs: {}", report.summary.unverified),
    );
    line(
        &mut out,
        format!("Duplicate matches: {}", report.summary.duplicate),
    );
    blank(&mut out);

    section(&mut out, "5. Field-level summary");
    for field in &report.field_summary {
        line(
            &mut out,
            format!(
                "- {}: {}/{} matched ({:?})",
                field.field_name, field.matched, field.expected, field.status
            ),
        );
        line(&mut out, format!("  {}", field.explanation));
    }
    blank(&mut out);

    section(&mut out, "6. Confirmed PLUs");
    line(&mut out, "None. Operational verification is blocked.");
    blank(&mut out);

    section(&mut out, "7. Missing PLUs");
    line(&mut out, "None classified. No read API was called.");
    blank(&mut out);

    section(&mut out, "8. Mismatched PLUs");
    line(&mut out, "None classified. No read API was called.");
    blank(&mut out);

    section(&mut out, "9. Unverified PLUs");
    for result in &report.results {
        line(
            &mut out,
            format!("PLU {}: {}", result.plu_number, result.explanation),
        );
    }
    blank(&mut out);

    section(&mut out, "10. Duplicate matches");
    line(&mut out, "None classified. No read API was called.");
    blank(&mut out);

    section(&mut out, "11. API and timing information");
    line(
        &mut out,
        format!(
            "Verification wait timeout: {} seconds",
            report.timing.timeout_seconds
        ),
    );
    line(
        &mut out,
        format!(
            "Verification polling interval: {} seconds",
            report.timing.poll_interval_seconds
        ),
    );
    for evidence in &report.api_evidence {
        line(
            &mut out,
            format!("- {}: {}", evidence.source, evidence.finding),
        );
    }
    line(&mut out, "Required before continuing:");
    for item in &report.missing_api_information {
        line(&mut out, format!("- {item}"));
    }
    blank(&mut out);

    section(&mut out, "12. Read-only safety confirmation");
    line(&mut out, "PLU write requests attempted: NO");
    line(&mut out, "Database access attempted: NO");
    line(&mut out, "Authentication attempted: NO");
    line(&mut out, "DIGIweb API requests attempted: NO");
    out
}

fn unavailable_field_summaries(expected: usize) -> Vec<FieldSummary> {
    [
        "PLU number",
        "Department",
        "Group",
        "Product name",
        "Price",
        "Barcode",
        "Ingredients",
        "Nutrition facts",
    ]
    .into_iter()
    .map(|field_name| FieldSummary {
        field_name: field_name.to_string(),
        matched: 0,
        expected,
        status: FieldSummaryStatus::Unavailable,
        explanation: "No confirmed supported PLU read API is available.".to_string(),
    })
    .collect()
}

fn evidence() -> Vec<ApiEvidence> {
    vec![
        ApiEvidence {
            source: "DIGIweb_ThirdParty_API_20260607.pdf, section 4.7 PLU".to_string(),
            finding:
                "PLU endpoint is documented with Method: POST, PATCH, DELETE; no GET method is listed."
                    .to_string(),
        },
        ApiEvidence {
            source: "DIGIweb_ThirdParty_API_20260607.pdf, section 4.4.1 PLU - Write".to_string(),
            finding: "Supported upsert endpoint is /api/v1/third-party/plus/write.".to_string(),
        },
        ApiEvidence {
            source: "ToDIGIweb ManageApiDIGIweb.vb and mod_dca_sms.vb".to_string(),
            finding: "Existing VB.NET code uses PLU write and request-status GET; it does not contain a PLU readback endpoint.".to_string(),
        },
    ]
}

fn missing_api_information() -> Vec<String> {
    vec![
        "Confirmed read-only PLU lookup endpoint path and HTTP method.".to_string(),
        "Required lookup parameters, including whether store, department, group, and PLU number are all part of the identity.".to_string(),
        "Response schema for core PLU fields, barcode data, ingredients, and nutrition facts.".to_string(),
        "Pagination or batching semantics, if lookup can return multiple PLUs.".to_string(),
        "Whether inactive/deleted PLUs are returned or filtered.".to_string(),
        "Field representation for department/group references when DIGIweb returns UUIDs.".to_string(),
    ]
}

fn section(out: &mut String, title: &str) {
    line(out, title);
}

fn line(out: &mut String, value: impl AsRef<str>) {
    out.push_str(value.as_ref());
    out.push('\n');
}

fn blank(out: &mut String) {
    out.push('\n');
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report() -> VerificationReport {
        build_discovery_blocked_report(VerificationDiscoveryInput {
            store_number: 1,
            source_file: "plu.mdb".to_string(),
            limit: Some(1),
            valid_plu_count: 4,
            selected_plu_numbers: vec![1],
            source_rows_discovered: 5,
            placeholders_ignored: 1,
            invalid_source_rows: 0,
            validation_skipped: 0,
            poll_interval_seconds: 2,
            timeout_seconds: 60,
        })
    }

    #[test]
    fn discovery_blocked_report_is_read_only_and_honest() {
        let report = report();

        assert_eq!(
            report.verification_status,
            VerificationStatus::BlockedApiDiscovery
        );
        assert_eq!(report.summary.expected, 1);
        assert_eq!(report.summary.unverified, 1);
        assert_eq!(report.scope.excluded_by_limit, 3);
        assert_eq!(report.scope.selected_plu_numbers, vec![1]);
        assert_eq!(report.timing.poll_interval_seconds, 2);
        assert_eq!(report.timing.timeout_seconds, 60);
        assert!(!report.safety.write_requests_attempted);
        assert!(!report.safety.database_access_attempted);
        assert!(!report.safety.authentication_attempted);
        assert!(
            report
                .api_evidence
                .iter()
                .any(|evidence| evidence.finding.contains("no GET method"))
        );
    }

    #[test]
    fn console_summary_does_not_claim_success() {
        let output = render_console_summary(&report());

        assert!(output.contains("POST-IMPORT VERIFICATION BLOCKED"));
        assert!(output.contains("Verification status: BLOCKED_API_DISCOVERY"));
        assert!(output.contains("Unverified PLUs: 1"));
        assert!(output.contains("PLU write requests attempted: NO"));
        assert!(output.contains("No authentication or DIGIweb API requests were attempted."));
        assert!(output.contains("Effective limit: 1"));
        assert!(output.contains("PLUs excluded by limit: 3"));
        assert!(!output.contains("Verification status: PASS"));
    }

    #[test]
    fn json_report_is_parseable_and_has_no_secrets() {
        let json = serde_json::to_string_pretty(&report()).expect("json");
        let parsed: serde_json::Value = serde_json::from_str(&json).expect("parse");

        assert_eq!(parsed["schema_version"], 1);
        assert_eq!(parsed["application_version"], "0.6.0");
        assert!(!json.contains("Authorization"));
        assert!(!json.contains("access_token"));
        assert!(!json.contains("client_secret"));
    }
}
