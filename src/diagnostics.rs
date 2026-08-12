use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;
use std::time::Duration;

use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};

use crate::config::DigiwebConfig;
use crate::digiweb::payload::DigiwebPluPayload;
use crate::error::AppError;
use crate::models::nutrition::{NutritionFact, NutritionRemapDetail};
use crate::models::plu::{Plu, effective_label_format, label_format_normalization_description};
use crate::recovery::sha256_json;
use crate::selection::{SelectionCriteria, SelectionFailure, SelectionMode, selection_error};
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
    pub server_reference_required: bool,
    pub semantic_status: String,
    pub raw_zero_defaulted_count: usize,
    pub raw_value_counts: BTreeMap<u32, usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluDiagnosticDetail {
    pub plu_number: u64,
    pub local_validation_status: String,
    pub disposition: DiagnosticDisposition,
    pub department: Option<u32>,
    pub group: Option<u32>,
    pub product_name_present: bool,
    pub raw_barcode: Option<String>,
    pub effective_barcode: Option<String>,
    pub source_barcode_format: Option<String>,
    pub barcode_type: Option<String>,
    pub barcode_reference_number: Option<String>,
    pub label_format: Option<u32>,
    pub effective_label_format: Option<u32>,
    pub label_format_normalization: String,
    pub label_format_server_reference_required: bool,
    pub label_format_semantic_status: String,
    pub best_before: Option<u32>,
    pub use_by: Option<u32>,
    pub quantity: Option<u32>,
    pub quantity_symbol: Option<u32>,
    pub raw_tare: Option<String>,
    pub effective_tare: Option<String>,
    pub ingredients_present: bool,
    pub ingredient_source_row_count: usize,
    pub nutrition_fact_count: usize,
    pub nutrition_profile: Option<String>,
    pub nutrition_remaps: Vec<NutritionRemapDetail>,
    pub nutrition_facts: Vec<NutritionFact>,
    pub required_references: Vec<RequiredReference>,
    pub payload_destinations: Vec<String>,
    pub duplicate_context: Option<DuplicateBarcodeContext>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequiredReference {
    pub reference_type: String,
    pub reference_number: u32,
    pub parent_reference: Option<u32>,
    pub source_field: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DuplicateBarcodeContext {
    pub effective_barcode: String,
    pub canonical_plu: u64,
    pub all_conflicting_plus: Vec<u64>,
    pub skipped_plus: Vec<u64>,
    pub requested_plu_disposition: DiagnosticDisposition,
    pub raw_barcodes: BTreeMap<u64, String>,
    pub barcode_types: BTreeMap<u64, String>,
    pub barcode_reference_numbers: BTreeMap<u64, String>,
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
    pub plu_details: Vec<PluDiagnosticDetail>,
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
    pub selection: DryRunSelectionSummary,
    pub source_validation_findings: DryRunSourceValidationSummary,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DryRunSelectionSummary {
    pub selection_mode: SelectionMode,
    pub requested_plu: Option<u64>,
    pub selected_order: Vec<u64>,
    pub selected: usize,
    pub would_submit: usize,
    pub selected_invalid_skips: usize,
    pub selected_duplicate_skips: usize,
    pub selected_payload_failures: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DryRunSourceValidationSummary {
    pub source_invalid_plus: usize,
    pub duplicate_barcode_plus: usize,
    pub customer_action_required_plus: usize,
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
        plu_details: plu_details(
            input.all_normalized_plus,
            input.valid_plus,
            input.validation_report,
            &duplicate_groups,
        ),
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
    filtered.plu_details.retain(|detail| {
        let plu_match = plu.is_none_or(|target| detail.plu_number == target);
        let invalid_match =
            !invalid_only || detail.disposition != DiagnosticDisposition::WouldSubmit;
        let category_match = category.is_none_or(|target| match target {
            DiagnosticCategory::DuplicateBarcode => detail.duplicate_context.is_some(),
            DiagnosticCategory::InvalidDepartment => detail.department.is_none(),
            DiagnosticCategory::InvalidGroup => detail.group.is_none(),
            DiagnosticCategory::InvalidLabelFormat => detail.label_format.is_some(),
            DiagnosticCategory::CustomerActionRequired => {
                detail.disposition != DiagnosticDisposition::WouldSubmit
            }
            _ => true,
        });
        plu_match && invalid_match && category_match
    });
    filtered.required_label_formats.retain(|requirement| {
        let category_match =
            category.is_none_or(|target| target == DiagnosticCategory::InvalidLabelFormat);
        let plu_match = plu.is_none_or(|target| requirement.plu_numbers.contains(&target));
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
    if report.plu_details.len() <= 5 {
        for detail in &report.plu_details {
            blank(&mut out);
            line(&mut out, format!("PLU {} detail", detail.plu_number));
            line(
                &mut out,
                format!(
                    "Local validation status: {}",
                    detail.local_validation_status
                ),
            );
            line(&mut out, format!("Disposition: {:?}", detail.disposition));
            line(
                &mut out,
                format!(
                    "Department: {}",
                    detail
                        .department
                        .map(|value| value.to_string())
                        .unwrap_or_else(|| "unknown".to_string())
                ),
            );
            line(
                &mut out,
                format!(
                    "Group: {}",
                    detail
                        .group
                        .map(|value| value.to_string())
                        .unwrap_or_else(|| "unknown".to_string())
                ),
            );
            line(
                &mut out,
                format!(
                    "Product name present: {}",
                    yes_no(detail.product_name_present)
                ),
            );
            line(
                &mut out,
                format!(
                    "Raw barcode: {}",
                    detail.raw_barcode.as_deref().unwrap_or("unknown")
                ),
            );
            line(
                &mut out,
                format!(
                    "Effective DIGIweb barcode: {}",
                    detail.effective_barcode.as_deref().unwrap_or("unknown")
                ),
            );
            line(
                &mut out,
                format!(
                    "Barcode format/type/reference: {}/{}/{}",
                    detail.source_barcode_format.as_deref().unwrap_or("unknown"),
                    detail.barcode_type.as_deref().unwrap_or("unknown"),
                    detail
                        .barcode_reference_number
                        .as_deref()
                        .unwrap_or("unknown")
                ),
            );
            if let Some(label_format) = detail.label_format {
                line(&mut out, format!("Raw Label Format: {label_format}"));
                line(
                    &mut out,
                    format!(
                        "Effective Label Format: {}",
                        detail
                            .effective_label_format
                            .map(|value| value.to_string())
                            .unwrap_or_else(|| "unknown".to_string())
                    ),
                );
                line(
                    &mut out,
                    format!("Normalization: {}", detail.label_format_normalization),
                );
                line(
                    &mut out,
                    format!(
                        "Label Format server reference required: {}",
                        yes_no(detail.label_format_server_reference_required)
                    ),
                );
                line(
                    &mut out,
                    format!(
                        "Label Format semantic status: {}",
                        detail.label_format_semantic_status
                    ),
                );
            }
            line(
                &mut out,
                format!(
                    "Best Before: {}",
                    detail
                        .best_before
                        .map(|value| value.to_string())
                        .unwrap_or_else(|| "none".to_string())
                ),
            );
            line(
                &mut out,
                format!(
                    "Use By: {}",
                    detail
                        .use_by
                        .map(|value| value.to_string())
                        .unwrap_or_else(|| "none".to_string())
                ),
            );
            line(
                &mut out,
                format!(
                    "Quantity/symbol: {}/{}",
                    detail
                        .quantity
                        .map(|value| value.to_string())
                        .unwrap_or_else(|| "none".to_string()),
                    detail
                        .quantity_symbol
                        .map(|value| value.to_string())
                        .unwrap_or_else(|| "none".to_string()),
                ),
            );
            line(
                &mut out,
                format!("Raw Tare: {}", detail.raw_tare.as_deref().unwrap_or("none")),
            );
            line(
                &mut out,
                format!(
                    "Effective DIGIweb Tare: {}",
                    detail.effective_tare.as_deref().unwrap_or("none")
                ),
            );
            line(
                &mut out,
                format!(
                    "Ingredients present/count: {}/{}",
                    yes_no(detail.ingredients_present),
                    detail.ingredient_source_row_count
                ),
            );
            line(
                &mut out,
                format!("Nutrition facts count: {}", detail.nutrition_fact_count),
            );
            if let Some(profile) = &detail.nutrition_profile {
                line(&mut out, format!("Nutrition profile: {profile}"));
            }
            if !detail.nutrition_remaps.is_empty() {
                line(&mut out, "Nutrition profile remaps:");
                for remap in &detail.nutrition_remaps {
                    line(
                        &mut out,
                        format!(
                            "- {} -> {} {} = {} | suppress ingredient: {}",
                            remap.source_field,
                            remap.nutrient,
                            remap.value_role,
                            remap.effective_value,
                            yes_no(remap.suppressed_from_ingredients)
                        ),
                    );
                }
            }
            if !detail.nutrition_facts.is_empty() {
                line(&mut out, "Effective nutrition facts:");
                for fact in &detail.nutrition_facts {
                    line(
                        &mut out,
                        format!(
                            "- {} amount={} percent={}",
                            fact.name,
                            fact.amount.as_deref().unwrap_or("none"),
                            fact.unit.as_deref().unwrap_or("none")
                        ),
                    );
                }
            }
            if !detail.required_references.is_empty() {
                line(&mut out, "Required DIGIweb references:");
                for reference in &detail.required_references {
                    line(
                        &mut out,
                        format!(
                            "- {} {} parent={} source={}",
                            reference.reference_type,
                            reference.reference_number,
                            reference
                                .parent_reference
                                .map(|value| value.to_string())
                                .unwrap_or_else(|| "none".to_string()),
                            reference.source_field
                        ),
                    );
                }
            }
            line(
                &mut out,
                format!(
                    "Payload destination summary: {}",
                    detail.payload_destinations.join(", ")
                ),
            );
            if let Some(context) = &detail.duplicate_context {
                line(&mut out, "Duplicate barcode conflict group:");
                line(
                    &mut out,
                    format!("Effective barcode: {}", context.effective_barcode),
                );
                line(
                    &mut out,
                    format!("Canonical/kept PLU: {}", context.canonical_plu),
                );
                line(
                    &mut out,
                    format!(
                        "All conflicting PLUs: {}",
                        join_numbers(&context.all_conflicting_plus)
                    ),
                );
                line(
                    &mut out,
                    format!("Skipped PLUs: {}", join_numbers(&context.skipped_plus)),
                );
                line(
                    &mut out,
                    format!(
                        "Requested PLU disposition: {:?}",
                        context.requested_plu_disposition
                    ),
                );
            }
        }
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
            format!(
                "Server reference required: {}",
                yes_no(format.server_reference_required)
            ),
        );
        line(
            &mut out,
            format!("Semantic status: {}", format.semantic_status),
        );
        if format.raw_zero_defaulted_count > 0 {
            line(
                &mut out,
                format!(
                    "Source normalization: {} PLUs defaulted from raw Label Format 0",
                    format.raw_zero_defaulted_count
                ),
            );
        }
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
    let mut by_format: BTreeMap<u32, (BTreeSet<u64>, BTreeMap<u32, usize>)> = BTreeMap::new();
    for plu in plus {
        if let (Some(raw_label_format), Some(effective)) =
            (plu.label_format, effective_label_format(plu.label_format))
        {
            let entry = by_format.entry(effective).or_default();
            entry.0.insert(plu.plu_number);
            *entry.1.entry(raw_label_format).or_default() += 1;
        }
    }
    by_format
        .into_iter()
        .map(|(label_format, (plus, raw_value_counts))| {
            let plu_numbers = plus.into_iter().collect::<Vec<_>>();
            LabelFormatRequirement {
                label_format,
                plu_count: plu_numbers.len(),
                plu_numbers,
                server_reference_required: label_format_server_reference_required(label_format),
                semantic_status: label_format_semantic_status(label_format).to_string(),
                raw_zero_defaulted_count: raw_value_counts.get(&0).copied().unwrap_or_default(),
                raw_value_counts,
            }
        })
        .collect()
}

fn label_format_server_reference_required(label_format: u32) -> bool {
    label_format > 0
}

fn label_format_semantic_status(label_format: u32) -> &'static str {
    if label_format == 1 {
        "effective_label_format_reference_may_include_raw_zero_defaults"
    } else {
        "positive_label_format_reference"
    }
}

fn plu_details(
    all_plus: &[Plu],
    valid_plus: &[Plu],
    validation_report: &ValidationReport,
    duplicate_groups: &[DuplicateBarcodeGroup],
) -> Vec<PluDiagnosticDetail> {
    let valid_numbers = valid_plus
        .iter()
        .map(|plu| plu.plu_number)
        .collect::<BTreeSet<_>>();
    let error_numbers = validation_report
        .issues
        .iter()
        .filter(|issue| issue.severity == Severity::Error)
        .filter_map(|issue| issue.plu_number)
        .collect::<BTreeSet<_>>();
    all_plus
        .iter()
        .map(|plu| {
            let duplicate_context = duplicate_groups
                .iter()
                .find(|group| group.all_plus.contains(&plu.plu_number))
                .map(|group| DuplicateBarcodeContext {
                    effective_barcode: group.effective_barcode.clone(),
                    canonical_plu: group.canonical_plu,
                    all_conflicting_plus: group.all_plus.clone(),
                    skipped_plus: group.skipped_plus.clone(),
                    requested_plu_disposition: if group.skipped_plus.contains(&plu.plu_number) {
                        DiagnosticDisposition::SkippedDuplicateBarcode
                    } else {
                        DiagnosticDisposition::WouldSubmit
                    },
                    raw_barcodes: group.raw_barcodes.clone(),
                    barcode_types: group.barcode_types.clone(),
                    barcode_reference_numbers: group.barcode_reference_numbers.clone(),
                });
            let disposition = if duplicate_context
                .as_ref()
                .is_some_and(|context| context.skipped_plus.contains(&plu.plu_number))
            {
                DiagnosticDisposition::SkippedDuplicateBarcode
            } else if valid_numbers.contains(&plu.plu_number) {
                DiagnosticDisposition::WouldSubmit
            } else if error_numbers.contains(&plu.plu_number) {
                DiagnosticDisposition::SkippedInvalid
            } else {
                DiagnosticDisposition::WouldSubmit
            };
            let mut required_references = Vec::new();
            if let Some(department) = plu.department_number {
                required_references.push(RequiredReference {
                    reference_type: "department".to_string(),
                    reference_number: department,
                    parent_reference: None,
                    source_field: "Pludata.Department -> pludepartmentno".to_string(),
                });
            }
            if let (Some(department), Some(group)) = (plu.department_number, plu.group_number) {
                required_references.push(RequiredReference {
                    reference_type: "group".to_string(),
                    reference_number: group,
                    parent_reference: Some(department),
                    source_field: "Pludata.Main Group Code -> plugroupno".to_string(),
                });
            }
            if let Some(label_format) = effective_label_format(plu.label_format) {
                required_references.push(RequiredReference {
                    reference_type: "label_format".to_string(),
                    reference_number: label_format,
                    parent_reference: None,
                    source_field: "Pludata.Print Format Code -> plulabelformat".to_string(),
                });
            }
            PluDiagnosticDetail {
                plu_number: plu.plu_number,
                local_validation_status: if valid_numbers.contains(&plu.plu_number) {
                    "valid".to_string()
                } else {
                    "invalid".to_string()
                },
                disposition,
                department: plu.department_number,
                group: plu.group_number,
                product_name_present: !plu.name.trim().is_empty(),
                raw_barcode: plu.source_barcode.clone(),
                effective_barcode: plu.barcode.clone(),
                source_barcode_format: plu.source_barcode_format.clone(),
                barcode_type: plu.barcode_type.clone(),
                barcode_reference_number: plu.barcode_ref_no.clone(),
                label_format: plu.label_format,
                effective_label_format: effective_label_format(plu.label_format),
                label_format_normalization: label_format_normalization_description(
                    plu.label_format,
                )
                .to_string(),
                label_format_server_reference_required: effective_label_format(plu.label_format)
                    .is_some_and(label_format_server_reference_required),
                label_format_semantic_status: effective_label_format(plu.label_format)
                    .map(label_format_semantic_status)
                    .unwrap_or("absent")
                    .to_string(),
                best_before: plu.selling_date_term,
                use_by: plu.expiration_days,
                quantity: plu.quantity,
                quantity_symbol: plu.quantity_symbol,
                raw_tare: plu.source_tare.clone(),
                effective_tare: plu.tare.map(|value| value.to_string()),
                ingredients_present: plu
                    .ingredients
                    .as_ref()
                    .is_some_and(|value| !value.is_empty()),
                ingredient_source_row_count: plu.source_pluing_row_count,
                nutrition_fact_count: plu.nutrition_facts.len(),
                nutrition_profile: plu.nutrition_profile.clone(),
                nutrition_remaps: plu.nutrition_remaps.clone(),
                nutrition_facts: plu.nutrition_facts.clone(),
                required_references,
                payload_destinations: payload_destinations_for_plu(plu),
                duplicate_context,
            }
        })
        .collect()
}

fn payload_destinations_for_plu(plu: &Plu) -> Vec<String> {
    let mut destinations = vec![
        "pluno",
        "pludepartmentno",
        "plugroupno",
        "plubarcodedata",
        "plucommname",
        "pluunitprice",
    ]
    .into_iter()
    .map(ToOwned::to_owned)
    .collect::<Vec<_>>();
    if plu.label_format.is_some() {
        destinations.push("plulabelformat".to_string());
    }
    if plu
        .ingredients
        .as_ref()
        .is_some_and(|value| !value.is_empty())
    {
        destinations.push("pluingredients".to_string());
    }
    if !plu.nutrition_facts.is_empty() {
        destinations.push("plunft.data".to_string());
    }
    destinations
}

pub fn build_dry_run_manifest(
    source_path: &str,
    source_sha256: &str,
    dataset: &SourceDataset,
    all_plus: &[Plu],
    valid_plus: &[Plu],
    diagnostics: &DiagnosticsReport,
    config: &DigiwebConfig,
    criteria: SelectionCriteria,
) -> Result<DryRunManifest, AppError> {
    let selected = select_dry_run_plus(all_plus, valid_plus, diagnostics, criteria)
        .map_err(|failure| selection_error(&failure))?;
    let selected_count = selected.len();
    let mut records = Vec::new();
    for plu in &selected {
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
    let selected_numbers = selected
        .iter()
        .map(|plu| plu.plu_number)
        .collect::<BTreeSet<_>>();
    let selected_order = selected
        .iter()
        .map(|plu| plu.plu_number)
        .collect::<Vec<_>>();
    let selection_would_submit = records
        .iter()
        .filter(|record| selected_numbers.contains(&record.plu_number))
        .filter(|record| record.disposition == DiagnosticDisposition::WouldSubmit)
        .count();
    let selection_skipped_invalid = records
        .iter()
        .filter(|record| selected_numbers.contains(&record.plu_number))
        .filter(|record| record.disposition == DiagnosticDisposition::SkippedInvalid)
        .count();
    let selection_skipped_duplicate = records
        .iter()
        .filter(|record| selected_numbers.contains(&record.plu_number))
        .filter(|record| record.disposition == DiagnosticDisposition::SkippedDuplicateBarcode)
        .count();
    let selection_payload_failures = records
        .iter()
        .filter(|record| selected_numbers.contains(&record.plu_number))
        .filter(|record| record.disposition == DiagnosticDisposition::PayloadBuildFailed)
        .count();
    let source_invalid_plus = diagnostics
        .problems
        .iter()
        .filter_map(|problem| problem.plu_number)
        .collect::<BTreeSet<_>>()
        .len();
    let duplicate_barcode_plus = diagnostics.summary.skipped_duplicate_barcode;
    let customer_action_required_plus = diagnostics
        .problems
        .iter()
        .filter(|problem| problem.classification == "CUSTOMER_ACTION_REQUIRED")
        .filter_map(|problem| problem.plu_number)
        .collect::<BTreeSet<_>>()
        .len();
    let summary = DryRunSummary {
        total_source_plus: dataset.plu_rows.len(),
        selected: selected_count,
        valid: valid_plus.len(),
        would_submit: selection_would_submit,
        skipped_invalid: selection_skipped_invalid,
        skipped_duplicate_barcode: selection_skipped_duplicate,
        customer_action_required: records
            .iter()
            .filter(|record| selected_numbers.contains(&record.plu_number))
            .filter(|record| record.disposition == DiagnosticDisposition::CustomerActionRequired)
            .count(),
        payload_build_failures: selection_payload_failures,
        api_write_requests: 0,
        selection: DryRunSelectionSummary {
            selection_mode: criteria.mode(),
            requested_plu: criteria.requested_plu,
            selected_order,
            selected: selected_count,
            would_submit: selection_would_submit,
            selected_invalid_skips: selection_skipped_invalid,
            selected_duplicate_skips: selection_skipped_duplicate,
            selected_payload_failures: selection_payload_failures,
        },
        source_validation_findings: DryRunSourceValidationSummary {
            source_invalid_plus,
            duplicate_barcode_plus,
            customer_action_required_plus,
        },
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

fn select_dry_run_plus<'a>(
    all_plus: &'a [Plu],
    valid_plus: &'a [Plu],
    diagnostics: &DiagnosticsReport,
    criteria: SelectionCriteria,
) -> Result<Vec<&'a Plu>, SelectionFailure> {
    if let Some(plu_number) = criteria.requested_plu {
        if let Some(plu) = valid_plus.iter().find(|plu| plu.plu_number == plu_number) {
            return Ok(vec![plu]);
        }
        let exists = all_plus.iter().any(|plu| plu.plu_number == plu_number)
            || diagnostics
                .problems
                .iter()
                .any(|problem| problem.plu_number == Some(plu_number));
        if exists {
            return Err(SelectionFailure::NotEligible {
                plu_number,
                reasons: diagnostics
                    .problems
                    .iter()
                    .filter(|problem| problem.plu_number == Some(plu_number))
                    .map(|problem| problem.reason.clone())
                    .collect(),
            });
        }
        return Err(SelectionFailure::NotFound { plu_number });
    }
    Ok(valid_plus
        .iter()
        .take(criteria.limit.unwrap_or(usize::MAX))
        .collect())
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
    line(&mut out, "DRY-RUN SELECTION");
    line(
        &mut out,
        format!(
            "Selection mode: {}",
            manifest.summary.selection.selection_mode.as_str()
        ),
    );
    if let Some(plu) = manifest.summary.selection.requested_plu {
        line(&mut out, format!("Requested PLU: {plu}"));
    }
    if !manifest.summary.selection.selected_order.is_empty() {
        line(
            &mut out,
            format!(
                "Selected PLU order: {}",
                manifest
                    .summary
                    .selection
                    .selected_order
                    .iter()
                    .map(|plu| plu.to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        );
    }
    line(
        &mut out,
        format!("Selected: {}", manifest.summary.selection.selected),
    );
    line(
        &mut out,
        format!("Would submit: {}", manifest.summary.selection.would_submit),
    );
    line(
        &mut out,
        format!(
            "Selected invalid skips: {}",
            manifest.summary.selection.selected_invalid_skips
        ),
    );
    line(
        &mut out,
        format!(
            "Selected duplicate skips: {}",
            manifest.summary.selection.selected_duplicate_skips
        ),
    );
    line(
        &mut out,
        format!(
            "Selected payload failures: {}",
            manifest.summary.selection.selected_payload_failures
        ),
    );
    blank(&mut out);
    line(&mut out, "SOURCE VALIDATION FINDINGS");
    line(
        &mut out,
        format!(
            "Source invalid PLUs: {}",
            manifest
                .summary
                .source_validation_findings
                .source_invalid_plus
        ),
    );
    line(
        &mut out,
        format!(
            "Duplicate-barcode PLUs: {}",
            manifest
                .summary
                .source_validation_findings
                .duplicate_barcode_plus
        ),
    );
    line(
        &mut out,
        format!(
            "Customer-action-required PLUs: {}",
            manifest
                .summary
                .source_validation_findings
                .customer_action_required_plus
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

fn yes_no(value: bool) -> &'static str {
    if value { "YES" } else { "NO" }
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
            source_tare: None,
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
            nutrition_profile: None,
            nutrition_remaps: Vec::new(),
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
    fn label_format_zero_defaults_to_effective_one_required_reference() {
        let mut a = plu(18, "0200018");
        a.label_format = Some(0);

        let requirements = label_format_requirements(&[a]);

        assert_eq!(requirements.len(), 1);
        assert_eq!(requirements[0].label_format, 1);
        assert!(requirements[0].server_reference_required);
        assert_eq!(requirements[0].raw_zero_defaulted_count, 1);
        assert_eq!(requirements[0].raw_value_counts.get(&0), Some(&1));
        assert!(requirements[0].semantic_status.contains("effective"));
    }

    #[test]
    fn multiple_raw_label_format_zero_plus_aggregate_under_effective_one() {
        let mut a = plu(18, "0200018");
        a.label_format = Some(0);
        let mut b = plu(19, "0200019");
        b.label_format = Some(0);
        let mut c = plu(20, "0200020");
        c.label_format = Some(1);

        let requirements = label_format_requirements(&[a, b, c]);

        assert_eq!(requirements.len(), 1);
        assert_eq!(requirements[0].label_format, 1);
        assert_eq!(requirements[0].plu_count, 3);
        assert_eq!(requirements[0].plu_numbers, vec![18, 19, 20]);
        assert_eq!(requirements[0].raw_zero_defaulted_count, 2);
        assert_eq!(requirements[0].raw_value_counts.get(&0), Some(&2));
        assert_eq!(requirements[0].raw_value_counts.get(&1), Some(&1));
    }

    #[test]
    fn valid_plu_specific_diagnostics_include_label_format_and_references() {
        let mut detail_plu = plu(18, "0200018");
        detail_plu.label_format = Some(6);
        detail_plu.ingredients = Some("Ingredient text".to_string());
        detail_plu.source_pluing_row_count = 2;
        detail_plu
            .nutrition_facts
            .push(crate::models::nutrition::NutritionFact {
                name: "Calories".to_string(),
                amount: Some("10".to_string()),
                unit: None,
            });
        let dataset = source_dataset(vec![("18", "0001", "PLU 18", "0200018")]);
        let validation_report = validate_plus(&[detail_plu.clone()]);
        let report = diagnostics_report(
            &dataset,
            &[detail_plu.clone()],
            &[detail_plu],
            &[],
            &validation_report,
        );
        let filtered = filter_diagnostics(&report, false, Some(18), None);
        let text = render_diagnostics_text(&filtered);

        assert_eq!(filtered.plu_details.len(), 1);
        assert!(text.contains("PLU 18 detail"));
        assert!(text.contains("Local validation status: valid"));
        assert!(text.contains("Disposition: WouldSubmit"));
        assert!(text.contains("Raw Label Format: 6"));
        assert!(text.contains("Effective Label Format: 6"));
        assert!(text.contains("Normalization: none"));
        assert!(text.contains("Label Format server reference required: YES"));
        assert!(text.contains("label_format 6"));
        assert!(text.contains("plulabelformat"));
        assert!(text.contains("Nutrition facts count: 1"));
    }

    #[test]
    fn plu_specific_diagnostics_show_raw_and_effective_label_format_zero() {
        let mut detail_plu = plu(721, "0200721");
        detail_plu.label_format = Some(0);
        let dataset = source_dataset(vec![("721", "0001", "PLU 721", "0200721")]);
        let validation_report = validate_plus(&[detail_plu.clone()]);
        let report = diagnostics_report(
            &dataset,
            &[detail_plu.clone()],
            &[detail_plu],
            &[],
            &validation_report,
        );
        let filtered = filter_diagnostics(&report, false, Some(721), None);
        let text = render_diagnostics_text(&filtered);

        assert!(text.contains("Raw Label Format: 0"));
        assert!(text.contains("Effective Label Format: 1"));
        assert!(text.contains("Normalization: Label Format 0 defaults to 1"));
        assert!(text.contains("Label Format server reference required: YES"));
        assert!(text.contains("label_format 1"));
        assert!(!text.contains("label_format 0"));
    }

    #[test]
    fn plu_specific_diagnostics_show_raw_and_effective_tare() {
        let mut detail_plu = plu(9807, "029807");
        detail_plu.source_tare = Some("14".to_string());
        detail_plu.tare = Some(Decimal::new(14, 3));
        let dataset = source_dataset(vec![("9807", "0001", "PLU 9807", "9807")]);
        let validation_report = validate_plus(&[detail_plu.clone()]);
        let report = diagnostics_report(
            &dataset,
            &[detail_plu.clone()],
            &[detail_plu],
            &[],
            &validation_report,
        );
        let filtered = filter_diagnostics(&report, false, Some(9807), None);
        let text = render_diagnostics_text(&filtered);

        assert_eq!(filtered.plu_details.len(), 1);
        assert_eq!(filtered.plu_details[0].raw_tare.as_deref(), Some("14"));
        assert_eq!(
            filtered.plu_details[0].effective_tare.as_deref(),
            Some("0.014")
        );
        assert!(text.contains("Raw Tare: 14"));
        assert!(text.contains("Effective DIGIweb Tare: 0.014"));
    }

    #[test]
    fn plu_specific_diagnostics_show_profile_nutrition_remaps_and_effective_facts() {
        let mut detail_plu = plu(18, "0200018");
        detail_plu.nutrition_profile = Some("bigway".to_string());
        detail_plu
            .nutrition_remaps
            .push(crate::models::nutrition::NutritionRemapDetail {
                source_field: "Ing Name 96".to_string(),
                nutrient: "Iron".to_string(),
                value_role: "amount".to_string(),
                effective_value: "12".to_string(),
                suppressed_from_ingredients: true,
            });
        detail_plu
            .nutrition_facts
            .push(crate::models::nutrition::NutritionFact {
                name: "Iron".to_string(),
                amount: Some("12".to_string()),
                unit: None,
            });
        let dataset = source_dataset(vec![("18", "0001", "PLU 18", "0200018")]);
        let validation_report = validate_plus(&[detail_plu.clone()]);
        let report = diagnostics_report(
            &dataset,
            &[detail_plu.clone()],
            &[detail_plu],
            &[],
            &validation_report,
        );

        let filtered = filter_diagnostics(&report, false, Some(18), None);
        let text = render_diagnostics_text(&filtered);

        assert!(text.contains("Nutrition profile: bigway"));
        assert!(text.contains("Ing Name 96 -> Iron amount = 12"));
        assert!(text.contains("suppress ingredient: YES"));
        assert!(text.contains("Effective nutrition facts:"));
        assert!(text.contains("Iron amount=12 percent=none"));
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
            SelectionCriteria {
                limit: Some(1),
                requested_plu: None,
                test_mode: true,
            },
        )
        .expect("manifest");

        assert_eq!(manifest.summary.selected, 1);
        assert_eq!(manifest.summary.would_submit, 1);
        assert_eq!(manifest.summary.skipped_duplicate_barcode, 0);
        assert_eq!(
            manifest
                .summary
                .source_validation_findings
                .duplicate_barcode_plus,
            2
        );
        assert_eq!(manifest.summary.api_write_requests, 0);
        assert!(!manifest.safety.authentication_attempted);
        assert!(manifest.records.iter().any(|record| record.plu_number == 20
            && record.disposition == DiagnosticDisposition::WouldSubmit));
        assert!(manifest.records.iter().any(|record| record.plu_number == 21
            && record.disposition == DiagnosticDisposition::SkippedDuplicateBarcode));
        assert!(manifest.records.iter().any(|record| record.plu_number == 22
            && record.disposition == DiagnosticDisposition::SkippedDuplicateBarcode));
    }

    #[test]
    fn dry_run_manifest_payload_hash_uses_effective_label_format_one() {
        let mut selected = plu(721, "0200721");
        selected.label_format = Some(0);
        let validation_report = validate_plus(&[selected.clone()]);
        let dataset = source_dataset(vec![("721", "0001", "PLU 721", "0200721")]);
        let diagnostics = diagnostics_report(
            &dataset,
            &[selected.clone()],
            &[selected.clone()],
            &[],
            &validation_report,
        );

        let manifest = build_dry_run_manifest(
            "plu.mdb",
            "abc123",
            &dataset,
            &[selected.clone()],
            &[selected.clone()],
            &diagnostics,
            &DigiwebConfig::default(),
            SelectionCriteria {
                limit: Some(1),
                requested_plu: None,
                test_mode: true,
            },
        )
        .expect("manifest");
        let expected_payload =
            DigiwebPluPayload::from_plu(&selected, &DigiwebConfig::default()).expect("payload");
        let expected_hash = sha256_json(&expected_payload).expect("hash");

        assert_eq!(expected_payload.plulabelformat, Some(1));
        assert_eq!(
            manifest.records[0].payload_sha256.as_deref(),
            Some(expected_hash.as_str())
        );
    }

    #[test]
    fn dry_run_manifest_records_exact_plu_selection() {
        let mut plu_721 = plu(721, "0200721");
        plu_721.label_format = Some(0);
        let plus = vec![plu(18, "0200018"), plu_721.clone(), plu(1, "0200001")];
        let validation_report = validate_plus(&plus);
        let dataset = source_dataset(vec![
            ("18", "0001", "PLU 18", "0200018"),
            ("721", "0001", "PLU 721", "0200721"),
            ("1", "0001", "PLU 1", "0200001"),
        ]);
        let diagnostics = diagnostics_report(&dataset, &plus, &plus, &[], &validation_report);

        let manifest = build_dry_run_manifest(
            "plu.mdb",
            "abc123",
            &dataset,
            &plus,
            &plus,
            &diagnostics,
            &DigiwebConfig::default(),
            SelectionCriteria {
                limit: None,
                requested_plu: Some(721),
                test_mode: false,
            },
        )
        .expect("manifest");
        let payload =
            DigiwebPluPayload::from_plu(&plu_721, &DigiwebConfig::default()).expect("payload");
        let json = serde_json::to_string(&payload).expect("json");
        let payload_hash = sha256_json(&payload).expect("hash");

        assert_eq!(
            manifest.summary.selection.selection_mode,
            SelectionMode::Plu
        );
        assert_eq!(manifest.summary.selection.requested_plu, Some(721));
        assert_eq!(manifest.summary.selection.selected_order, vec![721]);
        assert_eq!(manifest.summary.selection.selected, 1);
        assert_eq!(manifest.summary.selection.would_submit, 1);
        let selected_record = manifest
            .records
            .iter()
            .find(|record| record.plu_number == 721)
            .expect("selected record");
        assert!(json.contains("\"plulabelformat\":1"));
        assert!(!json.contains("\"plulabelformat\":0"));
        assert_eq!(
            selected_record.payload_sha256.as_deref(),
            Some(payload_hash.as_str())
        );
    }

    #[test]
    fn dry_run_manifest_uses_normalized_legacy_tare_payload() {
        let mut plu_9807 = plu(9807, "029807");
        plu_9807.source_tare = Some("14".to_string());
        plu_9807.tare = Some(Decimal::new(14, 3));
        let plus = vec![plu_9807.clone()];
        let validation_report = validate_plus(&plus);
        let dataset = source_dataset(vec![("9807", "0001", "PLU 9807", "9807")]);
        let diagnostics = diagnostics_report(&dataset, &plus, &plus, &[], &validation_report);

        let manifest = build_dry_run_manifest(
            "plu.mdb",
            "abc123",
            &dataset,
            &plus,
            &plus,
            &diagnostics,
            &DigiwebConfig::default(),
            SelectionCriteria {
                limit: None,
                requested_plu: Some(9807),
                test_mode: false,
            },
        )
        .expect("manifest");
        let normalized_payload =
            DigiwebPluPayload::from_plu(&plu_9807, &DigiwebConfig::default()).expect("payload");
        let normalized_hash = sha256_json(&normalized_payload).expect("hash");
        let mut old_wrong_plu = plu_9807;
        old_wrong_plu.tare = Some(Decimal::new(14, 0));
        let old_wrong_payload =
            DigiwebPluPayload::from_plu(&old_wrong_plu, &DigiwebConfig::default())
                .expect("payload");
        let old_wrong_hash = sha256_json(&old_wrong_payload).expect("hash");

        let selected_record = manifest
            .records
            .iter()
            .find(|record| record.plu_number == 9807)
            .expect("selected record");
        assert_eq!(
            selected_record.payload_sha256.as_deref(),
            Some(normalized_hash.as_str())
        );
        assert_ne!(
            selected_record.payload_sha256.as_deref(),
            Some(old_wrong_hash.as_str())
        );
    }

    #[test]
    fn plu_specific_duplicate_diagnostics_include_canonical_and_group_context() {
        let plus = vec![plu(20, "0200001"), plu(21, "0200001"), plu(22, "0200001")];
        let validation_report = validate_plus(&plus);
        let valid_plus = vec![plus[0].clone()];
        let dataset = source_dataset(vec![
            ("20", "0001", "PLU 20", "0200001"),
            ("21", "0001", "PLU 21", "0200001"),
            ("22", "0001", "PLU 22", "0200001"),
        ]);
        let report = diagnostics_report(&dataset, &plus, &valid_plus, &[], &validation_report);
        let filtered = filter_diagnostics(&report, false, Some(21), None);
        let text = render_diagnostics_text(&filtered);

        assert!(text.contains("PLU 21 detail"));
        assert!(text.contains("Raw barcode: 0200001"));
        assert!(text.contains("Effective DIGIweb barcode: 0200001"));
        assert!(text.contains("Canonical/kept PLU: 20"));
        assert!(text.contains("All conflicting PLUs: 20, 21, 22"));
        assert!(text.contains("Requested PLU disposition: SkippedDuplicateBarcode"));
    }

    #[test]
    fn dry_run_limit_one_separates_selected_counts_from_source_findings() {
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
            SelectionCriteria {
                limit: Some(1),
                requested_plu: None,
                test_mode: true,
            },
        )
        .expect("manifest");

        assert_eq!(manifest.summary.selection.selected, 1);
        assert_eq!(manifest.summary.selection.would_submit, 1);
        assert_eq!(manifest.summary.selection.selected_duplicate_skips, 0);
        assert_eq!(
            manifest
                .summary
                .source_validation_findings
                .duplicate_barcode_plus,
            2
        );
    }
}
