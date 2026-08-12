use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;
use std::time::Duration;

use chrono::{DateTime, Local};
use serde::Serialize;

use crate::analysis::model::ReferenceTableSnapshot;
use crate::error::AppError;
use crate::models::plu::{Plu, effective_label_format};
use crate::source::mapping::{
    BARCODE_COLUMNS, BARCODE_FORMAT_COLUMNS, BEST_BEFORE_COLUMNS, BEST_BEFORE_FLAG_COLUMNS,
    DEPARTMENT_COLUMNS, EXPIRATION_COLUMNS, INGREDIENT_TEXT_COLUMNS, KEY_LABEL_COLUMNS,
    PACK_DATE_FLAG_COLUMNS, PLU_NUMBER_COLUMNS, PLUING_NUTRITION_COLUMNS, PRICE_COLUMNS,
    PRINT_FORMAT_COLUMNS, QUANTITY_COLUMNS, QUANTITY_SYMBOL_COLUMNS, SHORT_DESCRIPTION_COLUMNS,
    TARE_COLUMNS,
};
use crate::source::{SourceDataset, SourceRow};
use crate::validation::issue::ValidationIssue;
use crate::validation::validator::ValidationReport;

const EXAMPLE_LIMIT: usize = 20;

pub struct DiscoveryInput<'a> {
    pub command: &'a str,
    pub source_path: &'a str,
    pub source_sha256: &'a str,
    pub started_at: DateTime<Local>,
    pub finished_at: DateTime<Local>,
    pub dataset: &'a SourceDataset,
    pub valid_plus: &'a [Plu],
    pub all_normalized_plus: &'a [Plu],
    pub row_issues: &'a [ValidationIssue],
    pub validation_report: &'a ValidationReport,
    pub placeholder_ignored: usize,
    pub reference_tables: &'a [ReferenceTableSnapshot],
    pub timings: Vec<PhaseTiming>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DiscoveryReport {
    pub schema_version: u32,
    pub application_version: String,
    pub command: String,
    pub generated_at: String,
    pub started_at: String,
    pub finished_at: String,
    pub source: DiscoverySource,
    pub safety: DiscoverySafety,
    pub summary: DiscoverySummary,
    pub references: ReferenceDiscovery,
    pub fields: Vec<FieldDiscovery>,
    pub sanitization_candidates: SanitizationCandidateSummary,
    pub setup: Vec<RequiredDepartment>,
    pub required_label_formats: Vec<RequiredLabelFormat>,
    pub timings: Vec<PhaseTiming>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DiscoverySource {
    pub path: String,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DiscoverySafety {
    pub authentication_attempted: bool,
    pub digiweb_api_requests_attempted: bool,
    pub source_database_modified: bool,
    pub plus_submitted: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DiscoverySummary {
    pub plu_rows: usize,
    pub empty_placeholders: usize,
    pub import_candidates: usize,
    pub normalized_plus: usize,
    pub already_valid: usize,
    pub validation_errors: usize,
    pub validation_warnings: usize,
    pub active: usize,
    pub inactive: usize,
    pub unknown_activity: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReferenceDiscovery {
    pub department_table_rows: usize,
    pub departments_used: usize,
    pub unused_department_rows: usize,
    pub group_table_rows: usize,
    pub groups_used: usize,
    pub unused_group_rows: usize,
    pub missing_department_names: usize,
    pub missing_group_names: usize,
    pub empty_group_references: usize,
    pub invalid_group_references: usize,
    pub label_formats_used: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FieldDiscovery {
    pub field: String,
    pub source_columns: Vec<String>,
    pub sanitization_classification: String,
    pub present_valid: usize,
    pub empty: usize,
    pub zero: usize,
    pub malformed: usize,
    pub negative: usize,
    pub out_of_supported_range: usize,
    pub unsupported: usize,
    pub deterministic_normalization: usize,
    pub safe_profile_candidate: usize,
    pub human_decision_required: usize,
    pub examples: Vec<FieldExample>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FieldExample {
    pub category: String,
    pub plu_number: Option<u64>,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SanitizationCandidateSummary {
    pub no_correction_needed: usize,
    pub deterministic_generic_normalization: usize,
    pub safe_profile_candidates: usize,
    pub human_decision_required: usize,
    pub blocking_unresolved: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RequiredDepartment {
    pub department_number: u32,
    pub source_name: Option<String>,
    pub plu_count: usize,
    pub groups: Vec<RequiredGroup>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RequiredGroup {
    pub group_number: u32,
    pub source_name: Option<String>,
    pub plu_count: usize,
    pub default_applied_count: usize,
    pub action: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RequiredLabelFormat {
    pub label_format: u32,
    pub plu_count: usize,
    pub plu_numbers: Vec<u64>,
    pub server_reference_required: bool,
    pub semantic_status: String,
    pub raw_zero_defaulted_count: usize,
    pub raw_value_counts: BTreeMap<u32, usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PhaseTiming {
    pub phase: String,
    pub milliseconds: u128,
}

impl PhaseTiming {
    pub fn from_duration(phase: impl Into<String>, duration: Duration) -> Self {
        Self {
            phase: phase.into(),
            milliseconds: duration.as_millis(),
        }
    }
}

pub fn build_discovery_report(input: DiscoveryInput<'_>) -> DiscoveryReport {
    let activity = activity_counts(input.dataset);
    let fields = field_discoveries(input.dataset);
    let references = reference_discovery(&input);
    let setup = required_setup(&input);
    let required_label_formats = required_label_formats(input.valid_plus);
    let sanitization_candidates = sanitization_summary(&fields, &input);
    DiscoveryReport {
        schema_version: 1,
        application_version: env!("CARGO_PKG_VERSION").to_string(),
        command: input.command.to_string(),
        generated_at: Local::now().to_rfc3339(),
        started_at: input.started_at.to_rfc3339(),
        finished_at: input.finished_at.to_rfc3339(),
        source: DiscoverySource {
            path: input.source_path.to_string(),
            sha256: input.source_sha256.to_string(),
        },
        safety: DiscoverySafety {
            authentication_attempted: false,
            digiweb_api_requests_attempted: false,
            source_database_modified: false,
            plus_submitted: 0,
        },
        summary: DiscoverySummary {
            plu_rows: input.dataset.plu_rows.len(),
            empty_placeholders: input.placeholder_ignored,
            import_candidates: input
                .dataset
                .plu_rows
                .len()
                .saturating_sub(input.placeholder_ignored),
            normalized_plus: input.all_normalized_plus.len(),
            already_valid: input.valid_plus.len(),
            validation_errors: input.validation_report.error_count(),
            validation_warnings: input.validation_report.warning_count(),
            active: activity.active,
            inactive: activity.inactive,
            unknown_activity: activity.unknown,
        },
        references,
        fields,
        sanitization_candidates,
        setup,
        required_label_formats,
        timings: input.timings,
    }
}

pub fn write_discovery_reports(
    text_path: &Path,
    json_path: &Path,
    report: &DiscoveryReport,
) -> Result<(), AppError> {
    fs::write(text_path, render_discovery_text(report))
        .map_err(|err| AppError::Internal(format!("failed to write discovery report: {err}")))?;
    let json = serde_json::to_string_pretty(report)
        .map_err(|err| AppError::Internal(format!("discovery JSON serialization failed: {err}")))?;
    fs::write(json_path, json)
        .map_err(|err| AppError::Internal(format!("failed to write discovery JSON: {err}")))?;
    Ok(())
}

pub fn render_discovery_console(report: &DiscoveryReport) -> String {
    let mut out = String::new();
    line(&mut out, "CUSTOMER MDB DISCOVERY");
    blank(&mut out);
    line(&mut out, "Source");
    kv(&mut out, "PLU rows", report.summary.plu_rows);
    kv(
        &mut out,
        "Empty placeholders",
        report.summary.empty_placeholders,
    );
    kv(
        &mut out,
        "Import candidates",
        report.summary.import_candidates,
    );
    kv(&mut out, "Already valid", report.summary.already_valid);
    blank(&mut out);
    line(&mut out, "References");
    kv(
        &mut out,
        "Department table rows",
        report.references.department_table_rows,
    );
    kv(
        &mut out,
        "Departments actually used",
        report.references.departments_used,
    );
    kv(
        &mut out,
        "Groups actually used",
        report.references.groups_used,
    );
    kv(
        &mut out,
        "Label formats observed",
        report.references.label_formats_used,
    );
    kv(
        &mut out,
        "Unknown group names",
        report.references.missing_group_names,
    );
    if !report.required_label_formats.is_empty() {
        line(&mut out, "Label Formats");
        for label_format in &report.required_label_formats {
            line(
                &mut out,
                format!(
                    "  Label Format {} | PLUs: {} | Used by: {} | Server reference required: {} | {}",
                    label_format.label_format,
                    label_format.plu_count,
                    join_examples(&label_format.plu_numbers, 8),
                    yes_no(label_format.server_reference_required),
                    label_format.semantic_status
                ),
            );
            if label_format.raw_zero_defaulted_count > 0 {
                line(
                    &mut out,
                    format!(
                        "    Source normalization: {} PLUs defaulted from raw Label Format 0",
                        label_format.raw_zero_defaulted_count
                    ),
                );
            }
        }
    }
    blank(&mut out);
    line(&mut out, "Potential sanitization");
    for field in &report.fields {
        let problems = field.deterministic_normalization
            + field.safe_profile_candidate
            + field.human_decision_required;
        if problems > 0 {
            kv(&mut out, &field.field, problems);
        }
    }
    blank(&mut out);
    line(&mut out, "Result");
    kv(
        &mut out,
        "Need deterministic repair",
        report.sanitization_candidates.safe_profile_candidates,
    );
    kv(
        &mut out,
        "Need human review",
        report.sanitization_candidates.human_decision_required,
    );
    kv(
        &mut out,
        "Blocking/unresolved",
        report.sanitization_candidates.blocking_unresolved,
    );
    blank(&mut out);
    line(&mut out, "Detailed report:");
    line(&mut out, "./discovery-report.txt");
    line(&mut out, "Machine-readable report:");
    line(&mut out, "./discovery-report.json");
    out
}

fn render_discovery_text(report: &DiscoveryReport) -> String {
    let mut out = render_discovery_console(report);
    blank(&mut out);
    line(&mut out, "SAFETY");
    line(&mut out, "Authentication attempted: NO");
    line(&mut out, "DIGIweb API requests attempted: NO");
    line(&mut out, "Source database modified: NO");
    line(&mut out, "PLUs submitted: 0");
    blank(&mut out);
    line(&mut out, "REQUIRED DIGIWEB SETUP");
    for department in &report.setup {
        line(
            &mut out,
            format!(
                "Department {} | Name: {} | PLUs: {}",
                department.department_number,
                department.source_name.as_deref().unwrap_or("UNKNOWN"),
                department.plu_count
            ),
        );
        for group in &department.groups {
            line(
                &mut out,
                format!(
                    "  Group {} | Name: {} | PLUs: {} | Action: {}",
                    group.group_number,
                    group.source_name.as_deref().unwrap_or("UNKNOWN"),
                    group.plu_count,
                    group.action
                ),
            );
        }
    }
    if !report.required_label_formats.is_empty() {
        line(&mut out, "Label Formats");
        for label_format in &report.required_label_formats {
            line(
                &mut out,
                format!(
                    "  Label Format {} | PLUs: {} | Server reference required: {} | Semantic status: {} | Used by: {}",
                    label_format.label_format,
                    label_format.plu_count,
                    yes_no(label_format.server_reference_required),
                    label_format.semantic_status,
                    join_numbers(&label_format.plu_numbers)
                ),
            );
            if label_format.raw_zero_defaulted_count > 0 {
                line(
                    &mut out,
                    format!(
                        "    Raw Label Format 0 -> effective {}: {} PLUs",
                        label_format.label_format, label_format.raw_zero_defaulted_count
                    ),
                );
            }
            line(
                &mut out,
                format!("    Raw value counts: {:?}", label_format.raw_value_counts),
            );
        }
    }
    blank(&mut out);
    line(&mut out, "FIELD QUALITY");
    for field in &report.fields {
        line(&mut out, &field.field);
        line(
            &mut out,
            format!("  Source columns: {}", field.source_columns.join(", ")),
        );
        line(
            &mut out,
            format!(
                "  Sanitization classification: {}",
                field.sanitization_classification
            ),
        );
        line(
            &mut out,
            format!("  Present/valid: {}", field.present_valid),
        );
        line(&mut out, format!("  Empty: {}", field.empty));
        line(&mut out, format!("  Zero: {}", field.zero));
        line(&mut out, format!("  Malformed: {}", field.malformed));
        line(
            &mut out,
            format!("  Out of supported range: {}", field.out_of_supported_range),
        );
        line(
            &mut out,
            format!(
                "  Human decision required: {}",
                field.human_decision_required
            ),
        );
        if !field.examples.is_empty() {
            line(&mut out, "  Examples:");
            for example in &field.examples {
                line(
                    &mut out,
                    format!(
                        "    {} PLU={} value={:?}",
                        example.category,
                        example
                            .plu_number
                            .map(|value| value.to_string())
                            .unwrap_or_else(|| "unknown".to_string()),
                        example.value
                    ),
                );
            }
        }
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

pub fn field_discoveries(dataset: &SourceDataset) -> Vec<FieldDiscovery> {
    let mut fields = vec![
        numeric_field(
            "PLU code",
            PLU_NUMBER_COLUMNS,
            &dataset.plu_rows,
            NumericPolicy::Positive,
        ),
        numeric_field(
            "Department",
            DEPARTMENT_COLUMNS,
            &dataset.plu_rows,
            NumericPolicy::Positive,
        ),
        group_field(&dataset.plu_rows),
        digits_field("Barcode", BARCODE_COLUMNS, &dataset.plu_rows, false),
        numeric_field(
            "Barcode Format",
            BARCODE_FORMAT_COLUMNS,
            &dataset.plu_rows,
            NumericPolicy::NonNegativeDefaultable,
        ),
        numeric_field(
            "Print Format Code",
            PRINT_FORMAT_COLUMNS,
            &dataset.plu_rows,
            NumericPolicy::NonNegativeOptional,
        ),
        best_before_field(&dataset.plu_rows),
        flag_field(
            "Best Before Flag",
            BEST_BEFORE_FLAG_COLUMNS,
            &dataset.plu_rows,
        ),
        numeric_field(
            "Use By Date",
            EXPIRATION_COLUMNS,
            &dataset.plu_rows,
            NumericPolicy::NonNegativeOptional,
        ),
        flag_field(
            "Use By Date Flag",
            PACK_DATE_FLAG_COLUMNS,
            &dataset.plu_rows,
        ),
        digits_field(
            "Ingredients Code",
            INGREDIENT_TEXT_COLUMNS,
            &dataset.ingredient_rows,
            true,
        ),
        numeric_field(
            "Price",
            PRICE_COLUMNS,
            &dataset.plu_rows,
            NumericPolicy::DecimalNonNegative,
        ),
        numeric_field(
            "Tare",
            TARE_COLUMNS,
            &dataset.plu_rows,
            NumericPolicy::DecimalNonNegativeOptional,
        ),
        numeric_field(
            "Quantity",
            QUANTITY_COLUMNS,
            &dataset.plu_rows,
            NumericPolicy::NonNegativeOptional,
        ),
        numeric_field(
            "Quantity Symbol",
            QUANTITY_SYMBOL_COLUMNS,
            &dataset.plu_rows,
            NumericPolicy::NonNegativeOptional,
        ),
        presence_field(
            "Active",
            &["Active", "ACTIVE", "Status", "STATUS"],
            &dataset.plu_rows,
        ),
        presence_field(
            "Status",
            &["Status", "STATUS", "Active", "ACTIVE"],
            &dataset.plu_rows,
        ),
        presence_field(
            "Short Description",
            SHORT_DESCRIPTION_COLUMNS,
            &dataset.plu_rows,
        ),
        presence_field("Key Label", KEY_LABEL_COLUMNS, &dataset.plu_rows),
        nutrition_field(dataset),
    ];
    for field in &mut fields {
        field.sanitization_classification = classify_field(field).to_string();
    }
    fields
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NumericPolicy {
    Positive,
    NonNegativeOptional,
    NonNegativeDefaultable,
    DecimalNonNegative,
    DecimalNonNegativeOptional,
}

fn numeric_field(
    name: &str,
    columns: &[&str],
    rows: &[SourceRow],
    policy: NumericPolicy,
) -> FieldDiscovery {
    let mut field = base_field(name, columns);
    for row in rows {
        let value = first_raw(row, columns);
        let plu_number = first_raw(row, PLU_NUMBER_COLUMNS).and_then(parse_u64);
        match value.map(str::trim) {
            None | Some("") => {
                field.empty += 1;
                if policy == NumericPolicy::NonNegativeDefaultable {
                    field.deterministic_normalization += 1;
                }
            }
            Some(value) => {
                let decimal = value.parse::<rust_decimal::Decimal>();
                match decimal {
                    Ok(number) if number < rust_decimal::Decimal::ZERO => {
                        field.negative += 1;
                        push_example(&mut field, "negative", plu_number, value);
                    }
                    Ok(number) if number.is_zero() => {
                        field.zero += 1;
                        field.present_valid += 1;
                    }
                    Ok(_) => field.present_valid += 1,
                    Err(_) => {
                        field.malformed += 1;
                        push_example(&mut field, "malformed", plu_number, value);
                    }
                }
            }
        }
    }
    field
}

fn group_field(rows: &[SourceRow]) -> FieldDiscovery {
    let mut field = base_field("Main Group Code", crate::source::mapping::GROUP_COLUMNS);
    for row in rows {
        let value = first_raw(row, crate::source::mapping::GROUP_COLUMNS);
        let plu_number = first_raw(row, PLU_NUMBER_COLUMNS).and_then(parse_u64);
        match value.map(str::trim) {
            None | Some("") => {
                field.empty += 1;
                field.human_decision_required += 1;
                push_example(&mut field, "human_decision_required", plu_number, "");
            }
            Some(value) => match value.parse::<i64>() {
                Ok(number) if number <= 0 => {
                    field.unsupported += 1;
                    field.human_decision_required += 1;
                    push_example(&mut field, "unsupported", plu_number, value);
                }
                Ok(_) => field.present_valid += 1,
                Err(_) => {
                    field.malformed += 1;
                    field.human_decision_required += 1;
                    push_example(&mut field, "malformed", plu_number, value);
                }
            },
        }
    }
    field
}

fn best_before_field(rows: &[SourceRow]) -> FieldDiscovery {
    let mut field = base_field("Best Before", BEST_BEFORE_COLUMNS);
    for row in rows {
        let value = first_raw(row, BEST_BEFORE_COLUMNS);
        let plu_number = first_raw(row, PLU_NUMBER_COLUMNS).and_then(parse_u64);
        match value.map(str::trim) {
            None | Some("") => {
                field.empty += 1;
                field.safe_profile_candidate += 1;
            }
            Some(value) => match value.parse::<i64>() {
                Ok(0) => {
                    field.zero += 1;
                    field.present_valid += 1;
                }
                Ok(1..=999) => field.present_valid += 1,
                Ok(number) if number < 0 => {
                    field.negative += 1;
                    field.safe_profile_candidate += 1;
                    push_example(&mut field, "negative", plu_number, value);
                }
                Ok(_) => {
                    field.out_of_supported_range += 1;
                    field.safe_profile_candidate += 1;
                    push_example(&mut field, "out_of_supported_range", plu_number, value);
                }
                Err(_) => {
                    field.malformed += 1;
                    field.safe_profile_candidate += 1;
                    push_example(&mut field, "malformed", plu_number, value);
                }
            },
        }
    }
    field
}

fn digits_field(
    name: &str,
    columns: &[&str],
    rows: &[SourceRow],
    optional: bool,
) -> FieldDiscovery {
    let mut field = base_field(name, columns);
    for row in rows {
        let value = first_raw(row, columns);
        let plu_number = first_raw(row, PLU_NUMBER_COLUMNS).and_then(parse_u64);
        match value.map(str::trim) {
            None | Some("") => {
                field.empty += 1;
                if !optional {
                    field.human_decision_required += 1;
                    push_example(&mut field, "empty", plu_number, "");
                }
            }
            Some(value) if value.chars().all(|ch| ch.is_ascii_digit()) => field.present_valid += 1,
            Some(value) => {
                field.malformed += 1;
                field.human_decision_required += 1;
                push_example(&mut field, "malformed", plu_number, value);
            }
        }
    }
    field
}

fn flag_field(name: &str, columns: &[&str], rows: &[SourceRow]) -> FieldDiscovery {
    let mut field = base_field(name, columns);
    for row in rows {
        match first_raw(row, columns).map(str::trim) {
            None | Some("") => field.empty += 1,
            Some(value)
                if matches!(
                    value.to_ascii_uppercase().as_str(),
                    "Y" | "YES" | "TRUE" | "1" | "N" | "NO" | "FALSE" | "0"
                ) =>
            {
                field.present_valid += 1
            }
            Some(value) => {
                field.unsupported += 1;
                push_example(
                    &mut field,
                    "unsupported",
                    first_raw(row, PLU_NUMBER_COLUMNS).and_then(parse_u64),
                    value,
                );
            }
        }
    }
    field
}

fn presence_field(name: &str, columns: &[&str], rows: &[SourceRow]) -> FieldDiscovery {
    let mut field = base_field(name, columns);
    for row in rows {
        match first_raw(row, columns).map(str::trim) {
            None | Some("") => field.empty += 1,
            Some(_) => field.present_valid += 1,
        }
    }
    field
}

fn nutrition_field(dataset: &SourceDataset) -> FieldDiscovery {
    let columns = PLUING_NUTRITION_COLUMNS
        .iter()
        .flat_map(|(_, amount, pct)| [Some(*amount), *pct])
        .flatten()
        .collect::<Vec<_>>();
    let mut field = base_field("Nutrition facts", &columns);
    for row in &dataset.nutrition_rows {
        let populated = columns
            .iter()
            .filter_map(|column| row.get(column))
            .filter(|value| !value.trim().is_empty())
            .count();
        if populated == 0 {
            field.empty += 1;
        } else {
            field.present_valid += 1;
        }
    }
    field
}

fn base_field(name: &str, columns: &[&str]) -> FieldDiscovery {
    FieldDiscovery {
        field: name.to_string(),
        source_columns: columns.iter().map(|value| (*value).to_string()).collect(),
        sanitization_classification: String::new(),
        present_valid: 0,
        empty: 0,
        zero: 0,
        malformed: 0,
        negative: 0,
        out_of_supported_range: 0,
        unsupported: 0,
        deterministic_normalization: 0,
        safe_profile_candidate: 0,
        human_decision_required: 0,
        examples: Vec::new(),
    }
}

fn classify_field(field: &FieldDiscovery) -> &'static str {
    if field.human_decision_required > 0 {
        "D_human_decision_required"
    } else if field.safe_profile_candidate > 0 {
        "C_safe_profile_candidate"
    } else if field.deterministic_normalization > 0 {
        "B_deterministic_generic_normalization"
    } else {
        "A_no_correction_needed"
    }
}

fn sanitization_summary(
    fields: &[FieldDiscovery],
    input: &DiscoveryInput<'_>,
) -> SanitizationCandidateSummary {
    SanitizationCandidateSummary {
        no_correction_needed: input.valid_plus.len(),
        deterministic_generic_normalization: fields
            .iter()
            .map(|field| field.deterministic_normalization)
            .sum(),
        safe_profile_candidates: fields
            .iter()
            .map(|field| field.safe_profile_candidate)
            .sum(),
        human_decision_required: fields
            .iter()
            .map(|field| field.human_decision_required)
            .sum(),
        blocking_unresolved: input.validation_report.error_count(),
    }
}

fn reference_discovery(input: &DiscoveryInput<'_>) -> ReferenceDiscovery {
    let departments = required_departments(input.valid_plus);
    let groups = required_groups(input.valid_plus);
    let department_rows = reference_table(input.reference_tables, "Department")
        .map(|table| table.row_count)
        .unwrap_or_default();
    let group_rows = reference_table(input.reference_tables, "Maingroup")
        .map(|table| table.row_count)
        .unwrap_or_default();
    let department_names = department_names(input.reference_tables);
    let group_names = group_names(input.reference_tables);
    ReferenceDiscovery {
        department_table_rows: department_rows,
        departments_used: departments.len(),
        unused_department_rows: department_rows.saturating_sub(departments.len()),
        group_table_rows: group_rows,
        groups_used: groups.len(),
        unused_group_rows: group_rows.saturating_sub(groups.len()),
        missing_department_names: departments
            .iter()
            .filter(|department| !department_names.contains_key(department))
            .count(),
        missing_group_names: groups
            .iter()
            .filter(|group| !group_names.contains_key(group))
            .count(),
        empty_group_references: input
            .all_normalized_plus
            .iter()
            .filter(|plu| plu.group_default_applied)
            .count(),
        invalid_group_references: input
            .row_issues
            .iter()
            .filter(|issue| issue.field == "group_number")
            .count(),
        label_formats_used: required_label_formats(input.valid_plus).len(),
    }
}

fn required_setup(input: &DiscoveryInput<'_>) -> Vec<RequiredDepartment> {
    let department_names = department_names(input.reference_tables);
    let group_names = group_names(input.reference_tables);
    let mut departments: BTreeMap<u32, BTreeMap<u32, (usize, usize)>> = BTreeMap::new();
    let mut department_counts: BTreeMap<u32, usize> = BTreeMap::new();
    for plu in input.valid_plus {
        let (Some(department), Some(group)) = (plu.department_number, plu.group_number) else {
            continue;
        };
        *department_counts.entry(department).or_default() += 1;
        let entry = departments
            .entry(department)
            .or_default()
            .entry(group)
            .or_default();
        entry.0 += 1;
        if plu.group_default_applied {
            entry.1 += 1;
        }
    }
    departments
        .into_iter()
        .map(|(department_number, groups)| RequiredDepartment {
            department_number,
            source_name: department_names.get(&department_number).cloned(),
            plu_count: department_counts
                .get(&department_number)
                .copied()
                .unwrap_or_default(),
            groups: groups
                .into_iter()
                .map(|(group_number, (plu_count, default_applied_count))| {
                    let source_name = group_names.get(&(department_number, group_number)).cloned();
                    let action = if source_name.is_some() {
                        "confirm/create in DIGIweb".to_string()
                    } else if default_applied_count > 0 {
                        "human decision required; group came from empty source value".to_string()
                    } else {
                        "determine/create DIGIweb group name".to_string()
                    };
                    RequiredGroup {
                        group_number,
                        source_name,
                        plu_count,
                        default_applied_count,
                        action,
                    }
                })
                .collect(),
        })
        .collect()
}

fn required_departments(plus: &[Plu]) -> BTreeSet<u32> {
    plus.iter()
        .filter_map(|plu| plu.department_number)
        .collect()
}

fn required_groups(plus: &[Plu]) -> BTreeSet<(u32, u32)> {
    plus.iter()
        .filter_map(|plu| Some((plu.department_number?, plu.group_number?)))
        .collect()
}

fn required_label_formats(plus: &[Plu]) -> Vec<RequiredLabelFormat> {
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
            RequiredLabelFormat {
                label_format,
                plu_count: plu_numbers.len(),
                plu_numbers,
                server_reference_required: true,
                semantic_status: if raw_value_counts.get(&0).copied().unwrap_or_default() > 0 {
                    "effective_label_format_reference_includes_raw_zero_defaults".to_string()
                } else {
                    "positive_label_format_reference".to_string()
                },
                raw_zero_defaulted_count: raw_value_counts.get(&0).copied().unwrap_or_default(),
                raw_value_counts,
            }
        })
        .collect()
}

fn department_names(tables: &[ReferenceTableSnapshot]) -> BTreeMap<u32, String> {
    let Some(table) = reference_table(tables, "Department").filter(|table| table.present) else {
        return BTreeMap::new();
    };
    reference_names(
        table,
        &["DeptNo", "Department", "DepartmentNo"],
        &["Department Name", "DeptName", "Name"],
    )
    .into_iter()
    .map(|((department, _), name)| (department, name))
    .collect()
}

fn group_names(tables: &[ReferenceTableSnapshot]) -> BTreeMap<(u32, u32), String> {
    let Some(table) = reference_table(tables, "Maingroup").filter(|table| table.present) else {
        return BTreeMap::new();
    };
    reference_names(
        table,
        &["Maingroup Dept", "Department", "DeptNo"],
        &["Maingroup Name", "Main Group Name", "GroupName", "Name"],
    )
}

fn reference_names(
    table: &ReferenceTableSnapshot,
    department_candidates: &[&str],
    name_candidates: &[&str],
) -> BTreeMap<(u32, u32), String> {
    let mut result = BTreeMap::new();
    let Some(department_column) = find_column(&table.columns, department_candidates) else {
        return result;
    };
    let group_column = find_column(
        &table.columns,
        &["MaingroupNo", "Main Group Code", "GroupNo", "GrpNo"],
    )
    .unwrap_or(department_column);
    let Some(name_column) = find_column(&table.columns, name_candidates) else {
        return result;
    };
    for row in &table.rows {
        let Some(department) = row.get(department_column).and_then(parse_u32_text) else {
            continue;
        };
        let Some(group) = row.get(group_column).and_then(parse_u32_text) else {
            continue;
        };
        let Some(name) = row
            .get(name_column)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            continue;
        };
        result
            .entry((department, group))
            .or_insert_with(|| name.to_string());
    }
    result
}

fn reference_table<'a>(
    tables: &'a [ReferenceTableSnapshot],
    name: &str,
) -> Option<&'a ReferenceTableSnapshot> {
    tables.iter().find(|table| table.name == name)
}

fn find_column<'a>(columns: &'a [String], candidates: &[&str]) -> Option<&'a str> {
    candidates
        .iter()
        .find_map(|candidate| columns.iter().find(|column| column.as_str() == *candidate))
        .map(String::as_str)
}

struct ActivityCounts {
    active: usize,
    inactive: usize,
    unknown: usize,
}

fn activity_counts(dataset: &SourceDataset) -> ActivityCounts {
    let mut counts = ActivityCounts {
        active: 0,
        inactive: 0,
        unknown: 0,
    };
    for row in &dataset.plu_rows {
        let value = first_raw(row, &["Active", "ACTIVE", "Status", "STATUS"]);
        match value.map(str::trim).map(str::to_ascii_uppercase).as_deref() {
            Some("1") | Some("Y") | Some("YES") | Some("TRUE") | Some("ACTIVE") => {
                counts.active += 1
            }
            Some("0") | Some("N") | Some("NO") | Some("FALSE") | Some("INACTIVE") => {
                counts.inactive += 1
            }
            _ => counts.unknown += 1,
        }
    }
    counts
}

fn first_raw<'a>(row: &'a SourceRow, columns: &[&str]) -> Option<&'a str> {
    columns.iter().find_map(|column| row.get(column))
}

fn parse_u64(value: &str) -> Option<u64> {
    value.trim().parse::<u64>().ok()
}

fn parse_u32_text(value: &str) -> Option<u32> {
    value.trim().parse::<u32>().ok()
}

fn push_example(field: &mut FieldDiscovery, category: &str, plu_number: Option<u64>, value: &str) {
    if field.examples.len() < EXAMPLE_LIMIT {
        field.examples.push(FieldExample {
            category: category.to_string(),
            plu_number,
            value: value.to_string(),
        });
    }
}

fn line(out: &mut String, text: impl AsRef<str>) {
    out.push_str(text.as_ref());
    out.push('\n');
}

fn kv(out: &mut String, key: &str, value: usize) {
    line(out, format!("  {key:<30} {value}"));
}

fn blank(out: &mut String) {
    out.push('\n');
}

fn yes_no(value: bool) -> &'static str {
    if value { "YES" } else { "NO" }
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

fn join_examples(numbers: &[u64], limit: usize) -> String {
    if numbers.is_empty() {
        "none".to_string()
    } else {
        let shown = numbers
            .iter()
            .take(limit)
            .map(|value| value.to_string())
            .collect::<Vec<_>>()
            .join(", ");
        if numbers.len() > limit {
            format!("{shown}, ...")
        } else {
            shown
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use rust_decimal::Decimal;

    use super::*;
    use crate::models::plu::{Plu, PriceMode};
    use crate::validation::validator::ValidationReport;

    fn row(plu: &str, group: &str, best_before: &str) -> SourceRow {
        SourceRow {
            table: "Pludata".to_string(),
            values: BTreeMap::from([
                ("Plucode".to_string(), plu.to_string()),
                ("Department".to_string(), "0001".to_string()),
                ("Main Group Code".to_string(), group.to_string()),
                ("Name 1".to_string(), "Apple".to_string()),
                ("Price".to_string(), "1.99".to_string()),
                ("Barcode".to_string(), plu.to_string()),
                ("Barcode Format".to_string(), "05".to_string()),
                ("Best Before".to_string(), best_before.to_string()),
            ]),
        }
    }

    fn valid_plu(plu_number: u64, label_format: u32) -> Plu {
        Plu {
            plu_number,
            store_number: 1,
            department_number: Some(1),
            group_number: Some(997),
            source_department: Some("0001".to_string()),
            source_group: Some("997".to_string()),
            group_default_applied: false,
            name: format!("PLU {plu_number}"),
            barcode: Some(format!("020{plu_number:05}")),
            barcode_type: Some("5".to_string()),
            barcode_ref_no: Some("5".to_string()),
            source_barcode: Some(plu_number.to_string()),
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
            expiration_days: None,
            label_format: Some(label_format),
            traceability: Some(0),
            short_description: None,
            key_label: None,
            ingredients: None,
            nutrition_facts: Vec::new(),
            source_pluing_row_count: 0,
        }
    }

    #[test]
    fn best_before_classifies_valid_and_invalid_values() {
        let dataset = SourceDataset {
            plu_rows: vec![
                row("1", "10", "0"),
                row("2", "10", "999"),
                row("3", "10", "1000"),
            ],
            ingredient_rows: Vec::new(),
            nutrition_rows: Vec::new(),
        };

        let field = field_discoveries(&dataset)
            .into_iter()
            .find(|field| field.field == "Best Before")
            .expect("best before");

        assert_eq!(field.zero, 1);
        assert_eq!(field.present_valid, 2);
        assert_eq!(field.out_of_supported_range, 1);
        assert_eq!(field.safe_profile_candidate, 1);
    }

    #[test]
    fn empty_group_requires_human_decision_not_active_rule() {
        let dataset = SourceDataset {
            plu_rows: vec![row("1", "", "0")],
            ingredient_rows: Vec::new(),
            nutrition_rows: Vec::new(),
        };

        let field = field_discoveries(&dataset)
            .into_iter()
            .find(|field| field.field == "Main Group Code")
            .expect("group");

        assert_eq!(field.empty, 1);
        assert_eq!(field.human_decision_required, 1);
    }

    #[test]
    fn discovery_report_confirms_no_network_or_submission() {
        let dataset = SourceDataset {
            plu_rows: vec![row("1", "10", "0")],
            ingredient_rows: Vec::new(),
            nutrition_rows: Vec::new(),
        };
        let now = Local::now();
        let report = build_discovery_report(DiscoveryInput {
            command: "discover",
            source_path: "plu.mdb",
            source_sha256: "abc",
            started_at: now,
            finished_at: now,
            dataset: &dataset,
            valid_plus: &[],
            all_normalized_plus: &[],
            row_issues: &[],
            validation_report: &ValidationReport { issues: Vec::new() },
            placeholder_ignored: 0,
            reference_tables: &[],
            timings: Vec::new(),
        });

        assert!(!report.safety.authentication_attempted);
        assert!(!report.safety.digiweb_api_requests_attempted);
        assert_eq!(report.safety.plus_submitted, 0);
    }

    #[test]
    fn label_formats_are_references_not_sanitization_candidates() {
        let dataset = SourceDataset {
            plu_rows: vec![row("18", "997", "0")],
            ingredient_rows: Vec::new(),
            nutrition_rows: Vec::new(),
        };
        let plus = vec![valid_plu(18, 6), valid_plu(19, 0)];
        let now = Local::now();
        let report = build_discovery_report(DiscoveryInput {
            command: "discover",
            source_path: "plu.mdb",
            source_sha256: "abc",
            started_at: now,
            finished_at: now,
            dataset: &dataset,
            valid_plus: &plus,
            all_normalized_plus: &plus,
            row_issues: &[],
            validation_report: &ValidationReport { issues: Vec::new() },
            placeholder_ignored: 0,
            reference_tables: &[],
            timings: Vec::new(),
        });
        let console = render_discovery_console(&report);
        let potential = console
            .split("Potential sanitization")
            .nth(1)
            .unwrap()
            .split("Result")
            .next()
            .unwrap();

        assert!(console.contains("Label Formats"));
        assert!(console.contains("Label Format 6"));
        assert!(console.contains("Label Format 1"));
        assert!(console.contains("Source normalization: 1 PLUs defaulted from raw Label Format 0"));
        assert!(!potential.contains("Label Format"));
        assert!(
            report
                .required_label_formats
                .iter()
                .any(|format| format.label_format == 1
                    && format.server_reference_required
                    && format.raw_zero_defaulted_count == 1)
        );
        assert!(
            !report
                .required_label_formats
                .iter()
                .any(|format| format.label_format == 0)
        );
    }

    #[test]
    fn discovery_console_bounds_dependent_plu_examples_but_text_keeps_full_list() {
        let dataset = SourceDataset {
            plu_rows: vec![row("1", "997", "0")],
            ingredient_rows: Vec::new(),
            nutrition_rows: Vec::new(),
        };
        let plus = (1..=12).map(|plu| valid_plu(plu, 6)).collect::<Vec<_>>();
        let now = Local::now();
        let report = build_discovery_report(DiscoveryInput {
            command: "discover",
            source_path: "plu.mdb",
            source_sha256: "abc",
            started_at: now,
            finished_at: now,
            dataset: &dataset,
            valid_plus: &plus,
            all_normalized_plus: &plus,
            row_issues: &[],
            validation_report: &ValidationReport { issues: Vec::new() },
            placeholder_ignored: 0,
            reference_tables: &[],
            timings: Vec::new(),
        });

        let console = render_discovery_console(&report);
        let text = render_discovery_text(&report);

        assert!(
            console.contains("Label Format 6 | PLUs: 12 | Used by: 1, 2, 3, 4, 5, 6, 7, 8, ...")
        );
        assert!(!console.contains("1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12"));
        assert!(text.contains("Used by: 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12"));
        assert_eq!(
            report.required_label_formats[0].plu_numbers,
            (1..=12).collect::<Vec<_>>()
        );
    }
}
