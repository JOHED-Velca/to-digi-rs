use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;
use std::time::Duration;

use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};

use crate::config::DigiwebConfig;
use crate::digiweb::payload::DigiwebPluPayload;
use crate::error::AppError;
use crate::models::plu::Plu;
use crate::recovery::sha256_json;
use crate::source::SourceDataset;
use crate::validation::issue::{Severity, ValidationIssue};
use crate::validation::validator::ValidationReport;

const CONSOLE_PROBLEM_LIMIT: usize = 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DiagnosticCategory {
    MissingRequiredField,
    DuplicateBarcode,
    InvalidDepartment,
    InvalidGroup,
    InvalidLabelFormat,
    BarcodeFormat,
    PrintFormat,
    SellingDate,
    Quantity,
    Mapping,
    CustomerActionRequired,
}

impl DiagnosticCategory {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::MissingRequiredField => "missing-required-field",
            Self::DuplicateBarcode => "duplicate-barcode",
            Self::InvalidDepartment => "invalid-department",
            Self::InvalidGroup => "invalid-group",
            Self::InvalidLabelFormat => "invalid-label-format",
            Self::BarcodeFormat => "barcode-format",
            Self::PrintFormat => "print-format",
            Self::SellingDate => "selling-date",
            Self::Quantity => "quantity",
            Self::Mapping => "mapping",
            Self::CustomerActionRequired => "customer-action-required",
        }
    }

    pub fn parse(value: &str) -> Result<Self, String> {
        match value.trim().to_ascii_lowercase().as_str() {
            "missing-required-field" => Ok(Self::MissingRequiredField),
            "duplicate-barcode" => Ok(Self::DuplicateBarcode),
            "invalid-department" => Ok(Self::InvalidDepartment),
            "invalid-group" => Ok(Self::InvalidGroup),
            "invalid-label-format" => Ok(Self::InvalidLabelFormat),
            "barcode-format" => Ok(Self::BarcodeFormat),
            "print-format" => Ok(Self::PrintFormat),
            "selling-date" => Ok(Self::SellingDate),
            "quantity" => Ok(Self::Quantity),
            "mapping" => Ok(Self::Mapping),
            "customer-action-required" => Ok(Self::CustomerActionRequired),
            other => Err(format!("unknown diagnostic category '{other}'")),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum DiagnosticDisposition {
    WouldSubmit,
    SkippedInvalid,
    SkippedDuplicateBarcode,
    CustomerActionRequired,
    PayloadBuildFailed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiagnosticProblem {
    pub plu_number: Option<u64>,
    pub department: Option<String>,
    pub category: DiagnosticCategory,
    pub classification: String,
    pub field: String,
    pub raw_value: Option<String>,
    pub normalized_value: Option<String>,
    pub reason: String,
    pub import_action: DiagnosticDisposition,
    pub automatic_correction: bool,
    pub conflict_with_plu: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DuplicateBarcodeGroup {
    pub effective_barcode: String,
    pub canonical_plu: u64,
    pub skipped_plus: Vec<u64>,
    pub all_plus: Vec<u64>,
    pub raw_barcodes: BTreeMap<u64, String>,
    pub barcode_types: BTreeMap<u64, String>,
    pub barcode_reference_numbers: BTreeMap<u64, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LabelFormatRequirement {
    pub label_format: u32,
    pub plu_count: usize,
    pub plu_numbers: Vec<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiagnosticsSummary {
    pub source_plus: usize,
    pub problem_plus: usize,
    pub customer_action_required: usize,
    pub missing_required_name: usize,
    pub duplicate_barcode: usize,
    pub skipped_duplicate_barcode: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiagnosticsSafety {
    pub authentication_attempted: bool,
    pub digiweb_api_requests_attempted: bool,
    pub source_database_modified: bool,
    pub plus_submitted: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiagnosticsReport {
    pub schema_version: u32,
    pub application_version: String,
    pub command: String,
    pub generated_at: String,
    pub started_at: String,
    pub finished_at: String,
    pub source_path: String,
    pub source_sha256: String,
    pub summary: DiagnosticsSummary,
    pub safety: DiagnosticsSafety,
    pub problems: Vec<DiagnosticProblem>,
    pub duplicate_barcode_groups: Vec<DuplicateBarcodeGroup>,
    pub required_label_formats: Vec<LabelFormatRequirement>,
    pub timings: Vec<DiagnosticTiming>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiagnosticTiming {
    pub phase: String,
    pub milliseconds: u128,
}

impl DiagnosticTiming {
    pub fn from_duration(phase: impl Into<String>, duration: Duration) -> Self {
        Self {
            phase: phase.into(),
            milliseconds: duration.as_millis(),
        }
    }
}

pub struct DiagnosticsInput<'a> {
    pub source_path: &'a str,
    pub source_sha256: &'a str,
    pub started_at: DateTime<Local>,
    pub finished_at: DateTime<Local>,
    pub dataset: &'a SourceDataset,
    pub all_normalized_plus: &'a [Plu],
    pub valid_plus: &'a [Plu],
    pub row_issues: &'a [ValidationIssue],
    pub validation_report: &'a ValidationReport,
    pub timings: Vec<DiagnosticTiming>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DryRunManifest {
    pub schema_version: u32,
    pub application_version: String,
    pub generated_at: String,
    pub source_path: String,
    pub source_sha256: String,
    pub summary: DryRunSummary,
    pub safety: DryRunSafety,
    pub records: Vec<DryRunRecord>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DryRunSummary {
    pub total_source_plus: usize,
    pub selected: usize,
    pub valid: usize,
    pub would_submit: usize,
    pub skipped_invalid: usize,
    pub skipped_duplicate_barcode: usize,
    pub customer_action_required: usize,
    pub payload_build_failures: usize,
    pub api_write_requests: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DryRunSafety {
    pub authentication_attempted: bool,
    pub digiweb_api_requests_attempted: bool,
    pub api_write_requests: usize,
    pub digiweb_modified: bool,
    pub source_database_modified: bool,
    pub plus_submitted: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DryRunRecord {
    pub plu_number: u64,
    pub department: Option<u32>,
    pub group: Option<u32>,
    pub disposition: DiagnosticDisposition,
    pub reason: Option<String>,
    pub payload_sha256: Option<String>,
}

pub fn build_diagnostics_report(input: DiagnosticsInput<'_>) -> DiagnosticsReport {
    let duplicate_groups = duplicate_barcode_groups(input.all_normalized_plus);
    let problems = diagnostic_problems(
        input.dataset,
        input.all_normalized_plus,
        input.row_issues,
        input.validation_report,
        &duplicate_groups,
    );
    let problem_plus = problems
        .iter()
        .filter_map(|problem| problem.plu_number)
        .collect::<BTreeSet<_>>()
        .len();
    let customer_action_required = problems
        .iter()
        .filter(|problem| problem.classification == "CUSTOMER_ACTION_REQUIRED")
        .count();
    DiagnosticsReport {
        schema_version: 1,
        application_version: env!("CARGO_PKG_VERSION").to_string(),
        command: "diagnose".to_string(),
        generated_at: Local::now().to_rfc3339(),
        started_at: input.started_at.to_rfc3339(),
        finished_at: input.finished_at.to_rfc3339(),
        source_path: input.source_path.to_string(),
        source_sha256: input.source_sha256.to_string(),
        summary: DiagnosticsSummary {
            source_plus: input.dataset.plu_rows.len(),
            problem_plus,
            customer_action_required,
            missing_required_name: problems
                .iter()
                .filter(|problem| {
                    problem.field == "product name"
                        && problem.category == DiagnosticCategory::MissingRequiredField
                })
                .count(),
            duplicate_barcode: duplicate_groups
                .iter()
                .map(|group| group.skipped_plus.len())
                .sum(),
            skipped_duplicate_barcode: duplicate_groups
                .iter()
                .map(|group| group.skipped_plus.len())
                .sum(),
        },
        safety: DiagnosticsSafety {
            authentication_attempted: false,
            digiweb_api_requests_attempted: false,
            source_database_modified: false,
            plus_submitted: 0,
        },
        problems,
        duplicate_barcode_groups: duplicate_groups,
        required_label_formats: label_format_requirements(input.valid_plus),
        timings: input.timings,
    }
}

pub fn filter_diagnostics(
    report: &DiagnosticsReport,
    invalid_only: bool,
    plu: Option<u64>,
    category: Option<DiagnosticCategory>,
) -> DiagnosticsReport {
    let mut filtered = report.clone();
    filtered.problems.retain(|problem| {
        let invalid_match = !invalid_only
            || matches!(
                problem.import_action,
                DiagnosticDisposition::SkippedInvalid
                    | DiagnosticDisposition::SkippedDuplicateBarcode
                    | DiagnosticDisposition::CustomerActionRequired
                    | DiagnosticDisposition::PayloadBuildFailed
            );
        let plu_match = plu.is_none_or(|target| problem.plu_number == Some(target));
        let category_match = category.is_none_or(|target| {
            problem.category == target
                || (target == DiagnosticCategory::CustomerActionRequired
                    && problem.classification == "CUSTOMER_ACTION_REQUIRED")
        });
        invalid_match && plu_match && category_match
    });
    filtered.duplicate_barcode_groups.retain(|group| {
        let category_match =
            category.is_none_or(|target| target == DiagnosticCategory::DuplicateBarcode);
        let plu_match = plu.is_none_or(|target| group.all_plus.contains(&target));
        category_match && plu_match
    });
    filtered
}

pub fn write_diagnostics_reports(
    text_path: &Path,
    json_path: &Path,
    report: &DiagnosticsReport,
) -> Result<(), AppError> {
    fs::write(text_path, render_diagnostics_text(report))
        .map_err(|err| AppError::Logging(format!("failed to write diagnostics report: {err}")))?;
    let json = serde_json::to_string_pretty(report).map_err(|err| {
        AppError::Internal(format!("diagnostics JSON serialization failed: {err}"))
    })?;
    fs::write(json_path, json)
        .map_err(|err| AppError::Logging(format!("failed to write diagnostics JSON: {err}")))?;
    Ok(())
}

pub fn render_diagnostics_console(report: &DiagnosticsReport) -> String {
    let mut out = String::new();
    line(&mut out, "PLU DIAGNOSTICS COMPLETE");
    blank(&mut out);
    line(
        &mut out,
        format!("Source PLUs: {}", report.summary.source_plus),
    );
    line(
        &mut out,
        format!("Problem PLUs: {}", report.summary.problem_plus),
    );
    blank(&mut out);
    line(
        &mut out,
        format!(
            "Customer action required: {}",
            report.summary.customer_action_required
        ),
    );
    line(
        &mut out,
        format!(
            "  Missing required name: {}",
            report.summary.missing_required_name
        ),
    );
    line(
        &mut out,
        format!("  Duplicate barcode: {}", report.summary.duplicate_barcode),
    );
    blank(&mut out);
    if report.summary.skipped_duplicate_barcode > 0 {
        line(&mut out, "Skipped duplicate barcode:");
        for group in &report.duplicate_barcode_groups {
            for plu in &group.skipped_plus {
                line(&mut out, format!("  PLU {plu}"));
            }
        }
        blank(&mut out);
    }
    for problem in report.problems.iter().take(CONSOLE_PROBLEM_LIMIT) {
        line(
            &mut out,
            format!(
                "PLU {} | {} | {}",
                problem
                    .plu_number
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "unknown".to_string()),
                problem.field,
                problem.reason
            ),
        );
    }
    if report.problems.len() > CONSOLE_PROBLEM_LIMIT {
        line(
            &mut out,
            format!(
                "... {} more problem(s) in diagnostics-report.txt",
                report.problems.len() - CONSOLE_PROBLEM_LIMIT
            ),
        );
    }
    blank(&mut out);
    line(&mut out, "Detailed report:");
    line(&mut out, "diagnostics-report.txt");
    line(&mut out, "JSON report:");
    line(&mut out, "diagnostics-report.json");
    out
}

pub fn render_diagnostics_text(report: &DiagnosticsReport) -> String {
    let mut out = render_diagnostics_console(report);
    blank(&mut out);
    line(&mut out, "SAFETY");
    line(&mut out, "Authentication attempted: NO");
    line(&mut out, "DIGIweb API requests attempted: NO");
    line(&mut out, "Source database modified: NO");
    line(&mut out, "PLUs submitted: 0");
    blank(&mut out);
    line(&mut out, "PROBLEMS");
    if report.problems.is_empty() {
        line(&mut out, "None");
    }
    for problem in &report.problems {
        line(
            &mut out,
            format!(
                "PLU {}",
                problem
                    .plu_number
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "unknown".to_string())
            ),
        );
        line(&mut out, format!("Category: {}", problem.classification));
        line(&mut out, format!("Field: {}", problem.field));
        line(
            &mut out,
            format!(
                "Raw value: {}",
                problem.raw_value.as_deref().unwrap_or("unknown")
            ),
        );
        line(
            &mut out,
            format!(
                "Normalized value: {}",
                problem.normalized_value.as_deref().unwrap_or("unknown")
            ),
        );
        if let Some(conflict) = problem.conflict_with_plu {
            line(&mut out, format!("Conflict with PLU: {conflict}"));
        }
        line(&mut out, format!("Reason: {}", problem.reason));
        line(
            &mut out,
            format!("Import action: {:?}", problem.import_action),
        );
        line(
            &mut out,
            format!(
                "Automatic correction: {}",
                if problem.automatic_correction {
                    "YES"
                } else {
                    "NO"
                }
            ),
        );
        blank(&mut out);
    }
    line(&mut out, "SKIPPED - DUPLICATE BARCODE");
    if report.duplicate_barcode_groups.is_empty() {
        line(&mut out, "None");
    }
    for group in &report.duplicate_barcode_groups {
        line(&mut out, format!("Barcode: {}", group.effective_barcode));
        line(&mut out, format!("Kept PLU: {}", group.canonical_plu));
        line(&mut out, "Skipped PLUs:");
        for plu in &group.skipped_plus {
            line(&mut out, format!("- {plu}"));
        }
        line(
            &mut out,
            "Customer action: Assign unique valid barcodes to the skipped PLUs and add/reimport them manually.",
        );
        blank(&mut out);
    }
    line(&mut out, "REQUIRED LABEL FORMATS");
    if report.required_label_formats.is_empty() {
        line(&mut out, "None");
    }
    for format in &report.required_label_formats {
        line(&mut out, format!("Label Format {}", format.label_format));
        line(&mut out, format!("Used by: {} PLUs", format.plu_count));
        line(
            &mut out,
            format!("PLUs: {}", join_numbers(&format.plu_numbers)),
        );
    }
    blank(&mut out);
    line(&mut out, "PERFORMANCE");
    for timing in &report.timings {
        line(
            &mut out,
            format!("{}: {} ms", timing.phase, timing.milliseconds),
        );
    }
    out
}

pub fn duplicate_barcode_groups(plus: &[Plu]) -> Vec<DuplicateBarcodeGroup> {
    let mut by_barcode: BTreeMap<String, Vec<&Plu>> = BTreeMap::new();
    for plu in plus {
        if let Some(barcode) = plu
            .barcode
            .as_ref()
            .filter(|value| !value.trim().is_empty())
        {
            by_barcode.entry(barcode.clone()).or_default().push(plu);
        }
    }
    by_barcode
        .into_iter()
        .filter_map(|(barcode, mut plus)| {
            plus.sort_by_key(|plu| plu.plu_number);
            if plus.len() <= 1 {
                return None;
            }
            let canonical = plus[0];
            let skipped = plus
                .iter()
                .skip(1)
                .map(|plu| plu.plu_number)
                .collect::<Vec<_>>();
            Some(DuplicateBarcodeGroup {
                effective_barcode: barcode,
                canonical_plu: canonical.plu_number,
                skipped_plus: skipped,
                all_plus: plus.iter().map(|plu| plu.plu_number).collect(),
                raw_barcodes: plus
                    .iter()
                    .filter_map(|plu| Some((plu.plu_number, plu.source_barcode.clone()?)))
                    .collect(),
                barcode_types: plus
                    .iter()
                    .filter_map(|plu| Some((plu.plu_number, plu.barcode_type.clone()?)))
                    .collect(),
                barcode_reference_numbers: plus
                    .iter()
                    .filter_map(|plu| Some((plu.plu_number, plu.barcode_ref_no.clone()?)))
                    .collect(),
            })
        })
        .collect()
}

pub fn label_format_requirements(plus: &[Plu]) -> Vec<LabelFormatRequirement> {
    let mut by_format: BTreeMap<u32, BTreeSet<u64>> = BTreeMap::new();
    for plu in plus {
        if let Some(label_format) = plu.label_format {
            by_format
                .entry(label_format)
                .or_default()
                .insert(plu.plu_number);
        }
    }
    by_format
        .into_iter()
        .map(|(label_format, plus)| {
            let plu_numbers = plus.into_iter().collect::<Vec<_>>();
            LabelFormatRequirement {
                label_format,
                plu_count: plu_numbers.len(),
                plu_numbers,
            }
        })
        .collect()
}

pub fn build_dry_run_manifest(
    source_path: &str,
    source_sha256: &str,
    dataset: &SourceDataset,
    all_plus: &[Plu],
    valid_plus: &[Plu],
    diagnostics: &DiagnosticsReport,
    config: &DigiwebConfig,
    limit: Option<usize>,
) -> Result<DryRunManifest, AppError> {
    let selected = select_dry_run_plus(valid_plus, limit);
    let selected_count = selected.len();
    let mut records = Vec::new();
    for plu in selected {
        match DigiwebPluPayload::from_plu(plu, config) {
            Ok(payload) => records.push(DryRunRecord {
                plu_number: plu.plu_number,
                department: plu.department_number,
                group: plu.group_number,
                disposition: DiagnosticDisposition::WouldSubmit,
                reason: None,
                payload_sha256: Some(sha256_json(&payload)?),
            }),
            Err(err) => records.push(DryRunRecord {
                plu_number: plu.plu_number,
                department: plu.department_number,
                group: plu.group_number,
                disposition: DiagnosticDisposition::PayloadBuildFailed,
                reason: Some(err.to_string()),
                payload_sha256: None,
            }),
        }
    }
    let valid_numbers = valid_plus
        .iter()
        .map(|plu| plu.plu_number)
        .collect::<BTreeSet<_>>();
    for problem in &diagnostics.problems {
        let Some(plu_number) = problem.plu_number else {
            continue;
        };
        if valid_numbers.contains(&plu_number) {
            continue;
        }
        let (department, group) = all_plus
            .iter()
            .find(|plu| plu.plu_number == plu_number)
            .map(|plu| (plu.department_number, plu.group_number))
            .unwrap_or((None, None));
        records.push(DryRunRecord {
            plu_number,
            department,
            group,
            disposition: problem.import_action.clone(),
            reason: Some(problem.reason.clone()),
            payload_sha256: None,
        });
    }
    records.sort_by_key(|record| (record.plu_number, disposition_sort(&record.disposition)));
    records.dedup_by_key(|record| record.plu_number);
    let summary = DryRunSummary {
        total_source_plus: dataset.plu_rows.len(),
        selected: selected_count,
        valid: valid_plus.len(),
        would_submit: records
            .iter()
            .filter(|record| record.disposition == DiagnosticDisposition::WouldSubmit)
            .count(),
        skipped_invalid: records
            .iter()
            .filter(|record| record.disposition == DiagnosticDisposition::SkippedInvalid)
            .count(),
        skipped_duplicate_barcode: records
            .iter()
            .filter(|record| record.disposition == DiagnosticDisposition::SkippedDuplicateBarcode)
            .count(),
        customer_action_required: records
            .iter()
            .filter(|record| record.disposition == DiagnosticDisposition::CustomerActionRequired)
            .count(),
        payload_build_failures: records
            .iter()
            .filter(|record| record.disposition == DiagnosticDisposition::PayloadBuildFailed)
            .count(),
        api_write_requests: 0,
    };
    Ok(DryRunManifest {
        schema_version: 1,
        application_version: env!("CARGO_PKG_VERSION").to_string(),
        generated_at: Local::now().to_rfc3339(),
        source_path: source_path.to_string(),
        source_sha256: source_sha256.to_string(),
        summary,
        safety: DryRunSafety {
            authentication_attempted: false,
            digiweb_api_requests_attempted: false,
            api_write_requests: 0,
            digiweb_modified: false,
            source_database_modified: false,
            plus_submitted: 0,
        },
        records,
    })
}

pub fn write_dry_run_outputs(
    text_path: &Path,
    json_path: &Path,
    manifest: &DryRunManifest,
) -> Result<(), AppError> {
    fs::write(text_path, render_dry_run_report(manifest))
        .map_err(|err| AppError::Logging(format!("failed to write dry-run report: {err}")))?;
    let json = serde_json::to_string_pretty(manifest)
        .map_err(|err| AppError::Internal(format!("dry-run JSON serialization failed: {err}")))?;
    fs::write(json_path, json)
        .map_err(|err| AppError::Logging(format!("failed to write dry-run manifest: {err}")))?;
    Ok(())
}

pub fn render_dry_run_console(manifest: &DryRunManifest) -> String {
    let mut out = String::new();
    line(&mut out, "DRY RUN COMPLETE");
    line(&mut out, "NO API WRITES");
    blank(&mut out);
    line(
        &mut out,
        format!("Source PLUs: {}", manifest.summary.total_source_plus),
    );
    line(&mut out, format!("Selected: {}", manifest.summary.selected));
    line(
        &mut out,
        format!("Would submit: {}", manifest.summary.would_submit),
    );
    line(
        &mut out,
        format!("Skipped invalid: {}", manifest.summary.skipped_invalid),
    );
    line(
        &mut out,
        format!(
            "Skipped duplicate barcode: {}",
            manifest.summary.skipped_duplicate_barcode
        ),
    );
    line(
        &mut out,
        format!(
            "Payload build failures: {}",
            manifest.summary.payload_build_failures
        ),
    );
    line(
        &mut out,
        format!(
            "API write requests: {}",
            manifest.summary.api_write_requests
        ),
    );
    blank(&mut out);
    line(&mut out, "Dry-run report:");
    line(&mut out, "dry-run-report.txt");
    line(&mut out, "Dry-run manifest:");
    line(&mut out, "dry-run-manifest.json");
    out
}

pub fn render_dry_run_report(manifest: &DryRunManifest) -> String {
    let mut out = render_dry_run_console(manifest);
    blank(&mut out);
    line(&mut out, "SAFETY");
    line(&mut out, "Authentication attempted: NO");
    line(&mut out, "DIGIweb API requests attempted: NO");
    line(&mut out, "API write requests: 0");
    line(&mut out, "DIGIweb modified: NO");
    line(&mut out, "Source database modified: NO");
    line(&mut out, "PLUs submitted: 0");
    blank(&mut out);
    line(&mut out, "RECORDS");
    for record in &manifest.records {
        line(
            &mut out,
            format!(
                "PLU {} | {:?} | {}",
                record.plu_number,
                record.disposition,
                record.reason.as_deref().unwrap_or("ready")
            ),
        );
    }
    out
}

fn diagnostic_problems(
    dataset: &SourceDataset,
    all_plus: &[Plu],
    row_issues: &[ValidationIssue],
    validation_report: &ValidationReport,
    duplicate_groups: &[DuplicateBarcodeGroup],
) -> Vec<DiagnosticProblem> {
    let mut problems = Vec::new();
    for issue in row_issues {
        problems.push(problem_from_issue(dataset, all_plus, issue, false));
    }
    let duplicate_skipped = duplicate_groups
        .iter()
        .flat_map(|group| {
            group
                .skipped_plus
                .iter()
                .map(|plu| (*plu, group.canonical_plu))
        })
        .collect::<BTreeMap<_, _>>();
    for issue in &validation_report.issues {
        if issue.severity != Severity::Error {
            continue;
        }
        let mut problem = problem_from_issue(dataset, all_plus, issue, true);
        if issue.field == "barcode" && issue.message.contains("duplicate barcode") {
            problem.category = DiagnosticCategory::DuplicateBarcode;
            problem.import_action = DiagnosticDisposition::SkippedDuplicateBarcode;
            problem.conflict_with_plu = issue
                .plu_number
                .and_then(|plu| duplicate_skipped.get(&plu).copied())
                .or_else(|| parse_conflict_plu(&issue.message));
            problem.raw_value = issue
                .plu_number
                .and_then(|plu| {
                    all_plus
                        .iter()
                        .find(|candidate| candidate.plu_number == plu)
                })
                .and_then(|plu| plu.source_barcode.clone());
            problem.normalized_value = issue
                .plu_number
                .and_then(|plu| {
                    all_plus
                        .iter()
                        .find(|candidate| candidate.plu_number == plu)
                })
                .and_then(|plu| plu.barcode.clone());
        }
        problems.push(problem);
    }
    problems.sort_by(|left, right| {
        left.plu_number
            .cmp(&right.plu_number)
            .then(left.field.cmp(&right.field))
            .then(left.reason.cmp(&right.reason))
    });
    problems
}

fn problem_from_issue(
    dataset: &SourceDataset,
    all_plus: &[Plu],
    issue: &ValidationIssue,
    normalized_issue: bool,
) -> DiagnosticProblem {
    let category = category_for_issue(issue);
    let field = display_field(&issue.field).to_string();
    let raw_value = issue
        .plu_number
        .and_then(|plu| raw_value_for_issue(dataset, plu, &issue.field));
    let normalized_value = issue.plu_number.and_then(|plu| {
        all_plus
            .iter()
            .find(|candidate| candidate.plu_number == plu)
            .and_then(|plu| normalized_value_for_issue(plu, &issue.field))
    });
    DiagnosticProblem {
        plu_number: issue.plu_number,
        department: department_for_issue(dataset, all_plus, issue.plu_number),
        category,
        classification: "CUSTOMER_ACTION_REQUIRED".to_string(),
        field,
        raw_value,
        normalized_value: normalized_value.or_else(|| normalized_issue.then(String::new)),
        reason: issue.message.clone(),
        import_action: if category == DiagnosticCategory::DuplicateBarcode {
            DiagnosticDisposition::SkippedDuplicateBarcode
        } else if category == DiagnosticCategory::MissingRequiredField {
            DiagnosticDisposition::CustomerActionRequired
        } else {
            DiagnosticDisposition::SkippedInvalid
        },
        automatic_correction: false,
        conflict_with_plu: parse_conflict_plu(&issue.message),
    }
}

fn category_for_issue(issue: &ValidationIssue) -> DiagnosticCategory {
    match issue.field.as_str() {
        "name" => DiagnosticCategory::MissingRequiredField,
        "barcode" if issue.message.contains("duplicate barcode") => {
            DiagnosticCategory::DuplicateBarcode
        }
        "barcode" => DiagnosticCategory::BarcodeFormat,
        "department_number" => DiagnosticCategory::InvalidDepartment,
        "group_number" => DiagnosticCategory::InvalidGroup,
        "label_format" => DiagnosticCategory::InvalidLabelFormat,
        "selling_date_term" => DiagnosticCategory::SellingDate,
        "quantity" | "quantity_symbol" => DiagnosticCategory::Quantity,
        _ => DiagnosticCategory::Mapping,
    }
}

fn display_field(field: &str) -> &str {
    match field {
        "name" => "product name",
        "department_number" => "department",
        "group_number" => "group",
        "label_format" => "label format",
        other => other,
    }
}

fn raw_value_for_issue(dataset: &SourceDataset, plu_number: u64, field: &str) -> Option<String> {
    let row = dataset.plu_rows.iter().find(|row| {
        row.get("Plucode")
            .and_then(|value| value.trim().parse::<u64>().ok())
            == Some(plu_number)
    })?;
    let candidates: &[&str] = match field {
        "name" => &["Name 1", "Name", "ProductName"],
        "barcode" => &["Barcode"],
        "department_number" => &["Department"],
        "group_number" => &["Main Group Code"],
        "label_format" => &["PRINT FORMAT CODE", "Print Format Code"],
        "selling_date_term" => &["BEST BEFORE", "Best Before"],
        _ => &[],
    };
    candidates
        .iter()
        .find_map(|column| row.get(column).map(ToOwned::to_owned))
}

fn normalized_value_for_issue(plu: &Plu, field: &str) -> Option<String> {
    match field {
        "name" => Some(plu.name.clone()),
        "barcode" => plu.barcode.clone(),
        "department_number" => plu.department_number.map(|value| value.to_string()),
        "group_number" => plu.group_number.map(|value| value.to_string()),
        "label_format" => plu.label_format.map(|value| value.to_string()),
        "selling_date_term" => plu.selling_date_term.map(|value| value.to_string()),
        _ => None,
    }
}

fn department_for_issue(
    dataset: &SourceDataset,
    all_plus: &[Plu],
    plu_number: Option<u64>,
) -> Option<String> {
    let plu_number = plu_number?;
    all_plus
        .iter()
        .find(|plu| plu.plu_number == plu_number)
        .and_then(|plu| plu.department_number.map(|value| value.to_string()))
        .or_else(|| {
            dataset.plu_rows.iter().find_map(|row| {
                let row_plu = row.get("Plucode")?.trim().parse::<u64>().ok()?;
                (row_plu == plu_number).then(|| row.get("Department").unwrap_or("").to_string())
            })
        })
}

fn parse_conflict_plu(message: &str) -> Option<u64> {
    message
        .split_whitespace()
        .rev()
        .find_map(|part| part.parse::<u64>().ok())
}

fn select_dry_run_plus(plus: &[Plu], limit: Option<usize>) -> &[Plu] {
    match limit {
        Some(limit) => &plus[..plus.len().min(limit)],
        None => plus,
    }
}

fn disposition_sort(disposition: &DiagnosticDisposition) -> u8 {
    match disposition {
        DiagnosticDisposition::WouldSubmit => 0,
        DiagnosticDisposition::SkippedInvalid => 1,
        DiagnosticDisposition::SkippedDuplicateBarcode => 2,
        DiagnosticDisposition::CustomerActionRequired => 3,
        DiagnosticDisposition::PayloadBuildFailed => 4,
    }
}

fn join_numbers(numbers: &[u64]) -> String {
    if numbers.is_empty() {
        "none".to_string()
    } else {
        numbers
            .iter()
            .map(|value| value.to_string())
            .collect::<Vec<_>>()
            .join(", ")
    }
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
    use std::collections::BTreeMap;

    use super::*;
    use crate::models::plu::PriceMode;
    use crate::source::{SourceDataset, SourceRow};
    use crate::validation::validator::{ValidationReport, validate_plus};
    use rust_decimal::Decimal;

    fn plu(plu_number: u64, barcode: &str) -> Plu {
        Plu {
            plu_number,
            store_number: 1,
            department_number: Some(1),
            group_number: Some(997),
            source_department: Some("0001".to_string()),
            source_group: Some("997".to_string()),
            group_default_applied: false,
            name: format!("PLU {plu_number}"),
            barcode: Some(barcode.to_string()),
            barcode_type: Some("5".to_string()),
            barcode_ref_no: Some("5".to_string()),
            source_barcode: Some(barcode.to_string()),
            source_barcode_format: Some("05".to_string()),
            source_flag_data: Some("02".to_string()),
            price: Decimal::new(100, 2),
            price_mode: PriceMode::ByEach,
            price_calc_method: Some(0),
            quantity: Some(0),
            quantity_symbol: Some(0),
            tare: Some(Decimal::ZERO),
            discount_type: Some(0),
            packing_date_print: Some(0),
            packing_time_print: Some(0),
            selling_date_print: Some(0),
            selling_date_term: Some(0),
            label_format: Some(6),
            traceability: Some(0),
            short_description: None,
            key_label: None,
            expiration_days: None,
            ingredients: None,
            nutrition_facts: Vec::new(),
            source_pluing_row_count: 0,
        }
    }

    fn source_dataset(rows: Vec<(&str, &str, &str, &str)>) -> SourceDataset {
        SourceDataset {
            plu_rows: rows
                .into_iter()
                .map(|(plu, department, name, barcode)| {
                    let mut values = BTreeMap::new();
                    values.insert("Plucode".to_string(), plu.to_string());
                    values.insert("Department".to_string(), department.to_string());
                    values.insert("Name 1".to_string(), name.to_string());
                    values.insert("Barcode".to_string(), barcode.to_string());
                    SourceRow {
                        table: "Pludata".to_string(),
                        values,
                    }
                })
                .collect(),
            ingredient_rows: Vec::new(),
            nutrition_rows: Vec::new(),
        }
    }

    fn diagnostics_report(
        dataset: &SourceDataset,
        all_plus: &[Plu],
        valid_plus: &[Plu],
        row_issues: &[ValidationIssue],
        validation_report: &ValidationReport,
    ) -> DiagnosticsReport {
        let now = Local::now();
        build_diagnostics_report(DiagnosticsInput {
            source_path: "plu.mdb",
            source_sha256: "abc123",
            started_at: now,
            finished_at: now,
            dataset,
            all_normalized_plus: all_plus,
            valid_plus,
            row_issues,
            validation_report,
            timings: Vec::new(),
        })
    }

    #[test]
    fn duplicate_barcode_groups_preserve_first_plu_as_canonical() {
        let groups = duplicate_barcode_groups(&[
            plu(20, "0200001"),
            plu(21, "0200001"),
            plu(22, "0200001"),
            plu(1344, "0200002"),
            plu(9317, "0200002"),
        ]);

        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].canonical_plu, 20);
        assert_eq!(groups[0].skipped_plus, vec![21, 22]);
        assert_eq!(groups[1].canonical_plu, 1344);
        assert_eq!(groups[1].skipped_plus, vec![9317]);
    }

    #[test]
    fn label_format_requirements_are_aggregated() {
        let mut a = plu(18, "0200018");
        a.label_format = Some(6);
        let mut b = plu(20, "0200020");
        b.label_format = Some(6);
        let mut c = plu(30, "0200030");
        c.label_format = Some(8);

        let requirements = label_format_requirements(&[a, b, c]);

        assert_eq!(requirements[0].label_format, 6);
        assert_eq!(requirements[0].plu_numbers, vec![18, 20]);
        assert_eq!(requirements[1].label_format, 8);
    }

    #[test]
    fn missing_product_name_is_row_level_customer_action_without_default() {
        let dataset = source_dataset(vec![("0", "0001", "", "")]);
        let row_issues = vec![ValidationIssue::error(
            Some(0),
            "name",
            "product name is missing",
        )];
        let validation_report = ValidationReport::default();

        let report = diagnostics_report(&dataset, &[], &[], &row_issues, &validation_report);

        assert_eq!(report.summary.missing_required_name, 1);
        let problem = &report.problems[0];
        assert_eq!(problem.plu_number, Some(0));
        assert_eq!(problem.department.as_deref(), Some("0001"));
        assert_eq!(problem.raw_value.as_deref(), Some(""));
        assert_eq!(problem.normalized_value, None);
        assert_eq!(
            problem.import_action,
            DiagnosticDisposition::CustomerActionRequired
        );
        assert!(!problem.automatic_correction);
    }

    #[test]
    fn dry_run_manifest_records_duplicate_barcode_skips_without_api_writes() {
        let plus = vec![plu(20, "0200001"), plu(21, "0200001"), plu(22, "0200001")];
        let validation_report = validate_plus(&plus);
        let valid_plus = vec![plus[0].clone()];
        let dataset = source_dataset(vec![
            ("20", "0001", "PLU 20", "0200001"),
            ("21", "0001", "PLU 21", "0200001"),
            ("22", "0001", "PLU 22", "0200001"),
        ]);
        let diagnostics = diagnostics_report(&dataset, &plus, &valid_plus, &[], &validation_report);

        let manifest = build_dry_run_manifest(
            "plu.mdb",
            "abc123",
            &dataset,
            &plus,
            &valid_plus,
            &diagnostics,
            &DigiwebConfig::default(),
            Some(1),
        )
        .expect("manifest");

        assert_eq!(manifest.summary.selected, 1);
        assert_eq!(manifest.summary.would_submit, 1);
        assert_eq!(manifest.summary.skipped_duplicate_barcode, 2);
        assert_eq!(manifest.summary.api_write_requests, 0);
        assert!(!manifest.safety.authentication_attempted);
        assert!(manifest.records.iter().any(|record| record.plu_number == 20
            && record.disposition == DiagnosticDisposition::WouldSubmit));
        assert!(manifest.records.iter().any(|record| record.plu_number == 21
            && record.disposition == DiagnosticDisposition::SkippedDuplicateBarcode));
        assert!(manifest.records.iter().any(|record| record.plu_number == 22
            && record.disposition == DiagnosticDisposition::SkippedDuplicateBarcode));
    }
}
