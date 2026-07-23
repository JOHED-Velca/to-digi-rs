use std::collections::{BTreeMap, BTreeSet};

use chrono::Local;

use crate::analysis::model::{
    AnalysisBlockingError, AnalysisReport, AnalysisStatus, AnalysisWarning, BarcodeFormatAnalysis,
    DepartmentRequirement, GroupRequirement, IngredientAnalysis, NutritionAnalysis,
    PluClassification, PluFieldCount, PriceCategoryAnalysis, ReferenceMatchStatus,
    ReferenceTableSnapshot, SafetyConfirmation, SourceSummary, TableAnalysis, TableStatus,
};
use crate::models::plu::{Plu, PriceMode};
use crate::source::{SourceDataset, SourceRow};
use crate::validation::issue::{Severity, ValidationIssue};
use crate::validation::validator::ValidationReport;

const UNMATCHED_EXAMPLE_LIMIT: usize = 20;

pub struct AnalysisInput<'a> {
    pub source_filename: &'a str,
    pub source_file_size_bytes: u64,
    pub source_is_symlink: bool,
    pub source_opened_read_only: bool,
    pub mdb_tables: &'a [String],
    pub dataset: &'a SourceDataset,
    pub valid_plus: &'a [Plu],
    pub all_normalized_plus: &'a [Plu],
    pub row_issues: &'a [ValidationIssue],
    pub validation_report: &'a ValidationReport,
    pub placeholder_ignored: usize,
    pub invalid_source_rows: usize,
    pub validation_skipped: usize,
    pub orphan_pluing_rows: usize,
    pub explicit_group_references: usize,
    pub defaulted_group_references: usize,
    pub invalid_group_values: usize,
    pub reference_tables: &'a [ReferenceTableSnapshot],
    pub nutrition_fallback_to_pluing: bool,
    pub nutrition_source_table: &'a str,
}

pub fn collect_analysis(input: AnalysisInput<'_>) -> AnalysisReport {
    let source = SourceSummary {
        exact_filename: input.source_filename.to_string(),
        file_size_bytes: input.source_file_size_bytes,
        is_symbolic_link: input.source_is_symlink,
        opened_read_only: input.source_opened_read_only,
        source_modified: false,
        mdb_tables_discovered: sorted_strings(input.mdb_tables.iter().cloned()),
    };
    let valid_plu_numbers = sorted_plu_numbers(input.valid_plus);
    let ingredient_analysis = ingredient_analysis(&input);
    let nutrition_analysis = nutrition_analysis(&input);
    let tables = table_analysis(&input);
    let departments = department_requirements(&input);
    let groups = group_requirements(&input);
    let barcode_formats = barcode_analysis(&input);
    let price_categories = price_analysis(&input);
    let mut warnings = warnings(&input, &ingredient_analysis);
    let mut blocking_errors = blocking_errors(&input);
    warnings.sort_by(|left, right| {
        left.code
            .cmp(&right.code)
            .then(left.message.cmp(&right.message))
    });
    blocking_errors.sort_by(|left, right| left.code.cmp(&right.code));
    let analysis_status = if !blocking_errors.is_empty() {
        AnalysisStatus::Fail
    } else if !warnings.is_empty() {
        AnalysisStatus::PassWithWarnings
    } else {
        AnalysisStatus::Pass
    };
    let summary = PluClassification {
        total_pludata_rows: input.dataset.plu_rows.len(),
        total_pluing_rows: input.dataset.ingredient_rows.len(),
        source_plus_discovered: input.dataset.plu_rows.len(),
        empty_placeholder_rows: input.placeholder_ignored,
        normalized_plus: input.all_normalized_plus.len(),
        valid_plus: input.valid_plus.len(),
        invalid_plus: input.invalid_source_rows,
        skipped_due_to_validation_errors: input.validation_skipped,
        valid_plu_numbers,
        plus_with_ingredients: input
            .valid_plus
            .iter()
            .filter(|plu| plu.ingredients.is_some())
            .count(),
        plus_with_nutrition_data: input
            .valid_plus
            .iter()
            .filter(|plu| !plu.nutrition_facts.is_empty())
            .count(),
    };
    let recommended_actions =
        recommended_actions(&departments, &groups, &warnings, analysis_status);

    AnalysisReport {
        schema_version: 1,
        application_version: env!("CARGO_PKG_VERSION").to_string(),
        generated_at: Local::now().to_rfc3339(),
        analysis_status,
        source,
        summary,
        tables,
        departments,
        groups,
        barcode_formats,
        price_categories,
        ingredient_analysis,
        nutrition_analysis,
        warnings,
        blocking_errors,
        recommended_actions,
        safety: SafetyConfirmation {
            analysis_only: true,
            network_access_permitted: false,
            authentication_attempted: false,
            digiweb_api_requests_attempted: false,
            source_database_modified: false,
            opened_only_exact_plu_mdb: true,
        },
    }
}

fn table_analysis(input: &AnalysisInput<'_>) -> Vec<TableAnalysis> {
    let mut tables = Vec::new();
    tables.push(TableAnalysis {
        name: "Pludata".to_string(),
        role: "main_plu_table".to_string(),
        status: table_status(
            input.mdb_tables,
            Some(input.dataset.plu_rows.len()),
            "Pludata",
        ),
        row_count: Some(input.dataset.plu_rows.len()),
    });
    tables.push(TableAnalysis {
        name: "PluIng".to_string(),
        role: "ingredient_and_fallback_nutrition_table".to_string(),
        status: table_status(
            input.mdb_tables,
            Some(input.dataset.ingredient_rows.len()),
            "PluIng",
        ),
        row_count: Some(input.dataset.ingredient_rows.len()),
    });
    for table_name in ["Department", "Maingroup"] {
        let snapshot = reference_table(input, table_name);
        tables.push(TableAnalysis {
            name: table_name.to_string(),
            role: "source_reference_table".to_string(),
            status: snapshot_to_status(snapshot),
            row_count: snapshot.and_then(|table| table.present.then_some(table.row_count)),
        });
    }
    tables.push(TableAnalysis {
        name: if input.nutrition_fallback_to_pluing {
            "Separate nutrition table".to_string()
        } else {
            input.nutrition_source_table.to_string()
        },
        role: "nutrition_table".to_string(),
        status: if input.nutrition_fallback_to_pluing {
            TableStatus::NotUsed
        } else {
            table_status(
                input.mdb_tables,
                Some(input.dataset.nutrition_rows.len()),
                input.nutrition_source_table,
            )
        },
        row_count: if input.nutrition_fallback_to_pluing {
            None
        } else {
            Some(input.dataset.nutrition_rows.len())
        },
    });
    tables.sort_by(|left, right| left.name.cmp(&right.name).then(left.role.cmp(&right.role)));
    tables
}

fn department_requirements(input: &AnalysisInput<'_>) -> Vec<DepartmentRequirement> {
    let table_status = snapshot_to_status(reference_table(input, "Department"));
    let mut by_department: BTreeMap<u32, (BTreeSet<String>, BTreeSet<u64>, bool)> = BTreeMap::new();
    for plu in input.valid_plus {
        if let Some(department) = plu.department_number {
            let entry = by_department.entry(department).or_default();
            if let Some(raw) = plu
                .source_department
                .as_ref()
                .filter(|value| !value.trim().is_empty())
            {
                entry.0.insert(raw.clone());
                if raw.trim() != department.to_string() {
                    entry.2 = true;
                }
            }
            entry.1.insert(plu.plu_number);
        }
    }
    by_department
        .into_iter()
        .map(
            |(department_number, (sources, plus, normalization_applied))| DepartmentRequirement {
                department_number,
                source_representations: sources.into_iter().collect(),
                plu_count: plus.len(),
                plu_numbers: plus.into_iter().collect(),
                normalization_applied,
                source_table_status: table_status,
                source_reference_match: reference_match_status(table_status),
            },
        )
        .collect()
}

fn group_requirements(input: &AnalysisInput<'_>) -> Vec<GroupRequirement> {
    let table_status = snapshot_to_status(reference_table(input, "Maingroup"));
    let mut groups: BTreeMap<(u32, u32), (BTreeSet<u64>, usize, usize)> = BTreeMap::new();
    for plu in input.valid_plus {
        if let (Some(department), Some(group)) = (plu.department_number, plu.group_number) {
            let entry = groups.entry((department, group)).or_default();
            entry.0.insert(plu.plu_number);
            if plu.group_default_applied {
                entry.2 += 1;
            } else {
                entry.1 += 1;
            }
        }
    }
    groups
        .into_iter()
        .map(
            |((department_number, group_number), (plus, explicit, defaulted))| GroupRequirement {
                department_number,
                group_number,
                plu_count: plus.len(),
                plu_numbers: plus.into_iter().collect(),
                explicit_source_group_count: explicit,
                default_group_applied_count: defaulted,
                source_maingroup_table_status: table_status,
                source_reference_match: reference_match_status(table_status),
            },
        )
        .collect()
}

fn barcode_analysis(input: &AnalysisInput<'_>) -> Vec<BarcodeFormatAnalysis> {
    let mut formats: BTreeMap<String, Vec<&Plu>> = BTreeMap::new();
    for plu in input.valid_plus {
        formats
            .entry(plu.source_barcode_format.clone().unwrap_or_default())
            .or_default()
            .push(plu);
    }
    formats
        .into_iter()
        .map(|(raw, plus)| {
            let plu_numbers = sorted_numbers(plus.iter().map(|plu| plu.plu_number));
            let barcode_type = single_value(plus.iter().filter_map(|plu| plu.barcode_type.clone()));
            let barcode_ref =
                single_value(plus.iter().filter_map(|plu| plu.barcode_ref_no.clone()));
            BarcodeFormatAnalysis {
                normalized_format: normalize_numeric_text(&raw, "5"),
                original_source_value: raw,
                plu_count: plu_numbers.len(),
                plu_numbers,
                derived_digiweb_barcode_type: barcode_type,
                derived_digiweb_barcode_reference: barcode_ref,
                valid_derivation_count: plus
                    .iter()
                    .filter(|plu| plu.barcode_type.is_some() && plu.barcode_ref_no.is_some())
                    .count(),
                invalid_derivation_count: 0,
                missing_barcode_count: plus.iter().filter(|plu| plu.barcode.is_none()).count(),
            }
        })
        .collect()
}

fn price_analysis(input: &AnalysisInput<'_>) -> Vec<PriceCategoryAnalysis> {
    let mut by_category: BTreeMap<String, Vec<&Plu>> = BTreeMap::new();
    let row_category = raw_categories_by_key(input.dataset);
    for plu in input.valid_plus {
        let key = plu_key(plu);
        let raw = key
            .and_then(|key| row_category.get(&key).cloned())
            .unwrap_or_else(|| "not present".to_string());
        by_category.entry(raw).or_default().push(plu);
    }
    by_category
        .into_iter()
        .map(|(raw, plus)| {
            let normalized = if raw.trim().is_empty() {
                "0".to_string()
            } else {
                raw.trim().to_string()
            };
            let first = plus.first().copied();
            PriceCategoryAnalysis {
                raw_category: raw,
                normalized_category: normalized,
                plu_count: plus.len(),
                plu_numbers: sorted_numbers(plus.iter().map(|plu| plu.plu_number)),
                derived_price_mode: first
                    .map(|plu| price_mode_name(plu.price_mode).to_string())
                    .unwrap_or_else(|| "unknown".to_string()),
                derived_price_calc_method: first.and_then(|plu| plu.price_calc_method),
                source_quantity_used: first.is_some_and(|plu| plu.quantity.unwrap_or_default() > 0),
                validation_status: if first
                    .is_some_and(|plu| matches!(plu.price_mode, PriceMode::Unknown))
                {
                    "INVALID".to_string()
                } else {
                    "VALID".to_string()
                },
            }
        })
        .collect()
}

fn ingredient_analysis(input: &AnalysisInput<'_>) -> IngredientAnalysis {
    let valid_keys = valid_keys(input.valid_plus);
    let placeholder_keys = placeholder_keys(input.dataset, input.row_issues);
    let invalid_keys = invalid_normalized_keys(input.all_normalized_plus, input.valid_plus);
    let mut matched_valid = 0;
    let mut matched_placeholder = 0;
    let mut matched_invalid = 0;
    let mut unmatched = 0;
    let mut unmatched_codes = BTreeSet::new();
    let mut duplicate_matches = Vec::new();
    let mut counts_by_key: BTreeMap<(u64, u32), usize> = BTreeMap::new();
    for row in &input.dataset.ingredient_rows {
        if let Some(key) = row_key(row) {
            *counts_by_key.entry(key).or_default() += 1;
            if valid_keys.contains(&key) {
                matched_valid += 1;
            } else if placeholder_keys.contains(&key) {
                matched_placeholder += 1;
            } else if invalid_keys.contains(&key) {
                matched_invalid += 1;
            } else {
                unmatched += 1;
                unmatched_codes.insert(key.0);
            }
        } else {
            unmatched += 1;
        }
    }
    for ((plu_number, department_number), count) in counts_by_key {
        if count > 1 && valid_keys.contains(&(plu_number, department_number)) {
            duplicate_matches.push(PluFieldCount {
                plu_number,
                department_number,
                count,
            });
        }
    }
    let examples = unmatched_codes
        .iter()
        .take(UNMATCHED_EXAMPLE_LIMIT)
        .copied()
        .collect::<Vec<_>>();
    IngredientAnalysis {
        total_pluing_rows: input.dataset.ingredient_rows.len(),
        rows_matched_to_valid_plus: matched_valid,
        rows_matched_to_placeholders: matched_placeholder,
        rows_matched_to_invalid_plus: matched_invalid,
        unmatched_rows: input.orphan_pluing_rows.max(unmatched),
        unique_unmatched_plu_codes: unmatched_codes.len(),
        unmatched_plu_code_examples: examples,
        unmatched_examples_truncated: unmatched_codes.len() > UNMATCHED_EXAMPLE_LIMIT,
        valid_plus_with_matching_pluing_row: input
            .valid_plus
            .iter()
            .filter(|plu| plu.source_pluing_row_count > 0)
            .count(),
        valid_plus_without_matching_pluing_row: input
            .valid_plus
            .iter()
            .filter(|plu| plu.source_pluing_row_count == 0)
            .count(),
        valid_plus_with_ingredient_data: input
            .valid_plus
            .iter()
            .filter(|plu| plu.ingredients.is_some())
            .count(),
        valid_plus_without_ingredient_data: input
            .valid_plus
            .iter()
            .filter(|plu| plu.ingredients.is_none())
            .count(),
        empty_ingredient_fields_ignored: count_empty_ingredient_fields(input.dataset),
        maximum_source_ingredient_field_number_observed: max_ing_name_field(input.dataset),
        populated_ingredient_fields_per_plu: populated_ingredient_counts(
            input.dataset,
            &valid_keys,
        ),
        duplicate_matches,
    }
}

fn nutrition_analysis(input: &AnalysisInput<'_>) -> NutritionAnalysis {
    let recognized = [
        "calories",
        "calories fat",
        "total fat",
        "saturated fat",
        "cholesterol",
        "sodium",
        "carbohydrate",
        "fiber",
        "sugar",
        "iron",
        "protein",
        "niacin",
        "riboflavin",
        "thiamin",
        "calcium",
        "vitamin a",
        "vitamin c",
        "serving size",
        "serving container",
        "trans fat",
    ];
    NutritionAnalysis {
        source_table_used: input.nutrition_source_table.to_string(),
        fallback_to_pluing_used: input.nutrition_fallback_to_pluing,
        valid_plus_with_nutrition_data: input
            .valid_plus
            .iter()
            .filter(|plu| !plu.nutrition_facts.is_empty())
            .count(),
        valid_plus_without_nutrition_data: input
            .valid_plus
            .iter()
            .filter(|plu| plu.nutrition_facts.is_empty())
            .count(),
        nutrition_rows_matched: input
            .valid_plus
            .iter()
            .filter(|plu| !plu.nutrition_facts.is_empty())
            .count(),
        malformed_values: input
            .validation_report
            .issues
            .iter()
            .filter(|issue| issue.field == "nutrition_facts")
            .count(),
        recognized_fields: recognized.iter().map(|value| value.to_string()).collect(),
        ignored_empty_values: count_empty_nutrition_values(input.dataset),
    }
}

fn warnings(input: &AnalysisInput<'_>, ingredient: &IngredientAnalysis) -> Vec<AnalysisWarning> {
    let mut warnings = Vec::new();
    let _explicit_group_references = input.explicit_group_references;
    if reference_table(input, "Maingroup")
        .is_some_and(|table| table.present && table.row_count == 0)
    {
        warnings.push(AnalysisWarning {
            code: "SOURCE_MAINGROUP_EMPTY".to_string(),
            severity: "WARNING".to_string(),
            message: "Maingroup is present but contains no records.".to_string(),
            affected_plus: sorted_plu_numbers(input.valid_plus),
            count: Some(0),
            recommended_action:
                "Required groups must be created or confirmed separately before import.".to_string(),
        });
    }
    if reference_table(input, "Department")
        .is_some_and(|table| table.present && table.row_count == 0)
    {
        warnings.push(AnalysisWarning {
            code: "SOURCE_DEPARTMENT_EMPTY".to_string(),
            severity: "WARNING".to_string(),
            message: "Department is present but contains no records.".to_string(),
            affected_plus: sorted_plu_numbers(input.valid_plus),
            count: Some(0),
            recommended_action:
                "Required departments must be created or confirmed separately before import."
                    .to_string(),
        });
    }
    if ingredient.unmatched_rows > 0 {
        warnings.push(AnalysisWarning {
            code: "UNMATCHED_PLUING_ROWS".to_string(),
            severity: "WARNING".to_string(),
            message: format!(
                "{} PluIng row(s) did not match a valid active PLU by Plucode and Department.",
                ingredient.unmatched_rows
            ),
            affected_plus: Vec::new(),
            count: Some(ingredient.unmatched_rows),
            recommended_action:
                "Review unmatched PluIng rows if they are expected to contain active product data."
                    .to_string(),
        });
    }
    if !ingredient.duplicate_matches.is_empty() {
        warnings.push(AnalysisWarning {
            code: "PLUING_DUPLICATE_MATCH".to_string(),
            severity: "WARNING".to_string(),
            message: "One or more valid PLUs have multiple matching PluIng rows.".to_string(),
            affected_plus: ingredient
                .duplicate_matches
                .iter()
                .map(|entry| entry.plu_number)
                .collect(),
            count: Some(ingredient.duplicate_matches.len()),
            recommended_action: "Confirm duplicate PluIng matches are expected.".to_string(),
        });
    }
    if input.placeholder_ignored > 0 {
        warnings.push(AnalysisWarning {
            code: "PLACEHOLDER_ROWS_IGNORED".to_string(),
            severity: "WARNING".to_string(),
            message: format!(
                "{} empty placeholder PLU row(s) were ignored.",
                input.placeholder_ignored
            ),
            affected_plus: Vec::new(),
            count: Some(input.placeholder_ignored),
            recommended_action: "No action is needed when these are expected scale placeholders."
                .to_string(),
        });
    }
    if input.defaulted_group_references > 0 {
        warnings.push(AnalysisWarning {
            code: "DEFAULT_GROUP_APPLIED".to_string(),
            severity: "WARNING".to_string(),
            message: format!(
                "{} PLU(s) used default group 997 because the source group was empty.",
                input.defaulted_group_references
            ),
            affected_plus: input
                .valid_plus
                .iter()
                .filter(|plu| plu.group_default_applied)
                .map(|plu| plu.plu_number)
                .collect(),
            count: Some(input.defaulted_group_references),
            recommended_action: "Confirm group 997 exists under the required departments."
                .to_string(),
        });
    }
    if input.invalid_group_values > 0 {
        warnings.push(AnalysisWarning {
            code: "INVALID_GROUP_VALUES".to_string(),
            severity: "WARNING".to_string(),
            message: format!(
                "{} PLU row(s) contained invalid source group values.",
                input.invalid_group_values
            ),
            affected_plus: Vec::new(),
            count: Some(input.invalid_group_values),
            recommended_action: "Correct invalid source group values before attempting import."
                .to_string(),
        });
    }
    warnings
}

fn blocking_errors(input: &AnalysisInput<'_>) -> Vec<AnalysisBlockingError> {
    let mut errors = Vec::new();
    if input.valid_plus.is_empty() {
        errors.push(AnalysisBlockingError {
            code: "NO_VALID_PLUS".to_string(),
            message: "No valid normalized PLUs are available for import.".to_string(),
        });
    }
    if input
        .validation_report
        .issues
        .iter()
        .any(|issue| issue.severity == Severity::Error && issue.plu_number.is_none())
    {
        errors.push(AnalysisBlockingError {
            code: "GLOBAL_VALIDATION_ERROR".to_string(),
            message: "A global validation error prevents safe analysis.".to_string(),
        });
    }
    errors
}

fn recommended_actions(
    departments: &[DepartmentRequirement],
    groups: &[GroupRequirement],
    warnings: &[AnalysisWarning],
    status: AnalysisStatus,
) -> Vec<String> {
    let mut actions = Vec::new();
    for department in departments {
        actions.push(format!(
            "Confirm that Department {} exists in DIGIweb.",
            department.department_number
        ));
    }
    for group in groups {
        actions.push(format!(
            "Confirm that Group {} exists under Department {}.",
            group.group_number, group.department_number
        ));
    }
    if warnings
        .iter()
        .any(|warning| warning.code == "SOURCE_MAINGROUP_EMPTY")
    {
        actions.push("Review the empty Maingroup source table.".to_string());
    }
    if let Some(warning) = warnings
        .iter()
        .find(|warning| warning.code == "UNMATCHED_PLUING_ROWS")
    {
        actions.push(format!(
            "Review {} unmatched PluIng rows if they are expected to contain active product data.",
            warning.count.unwrap_or_default()
        ));
    }
    if status != AnalysisStatus::Fail {
        actions
            .push("Run `to-digi-rs verify` after DIGIweb configuration is complete.".to_string());
        actions.push("Run `to-digi-rs import --limit 1` before the full import.".to_string());
    }
    actions
}

fn table_status(tables: &[String], row_count: Option<usize>, table: &str) -> TableStatus {
    if !tables.iter().any(|candidate| candidate == table) {
        TableStatus::Absent
    } else if row_count == Some(0) {
        TableStatus::Empty
    } else {
        TableStatus::Present
    }
}

fn snapshot_to_status(snapshot: Option<&ReferenceTableSnapshot>) -> TableStatus {
    match snapshot {
        Some(table) if table.present && table.row_count == 0 => TableStatus::Empty,
        Some(table) if table.present => TableStatus::Present,
        Some(_) | None => TableStatus::Absent,
    }
}

fn reference_match_status(table_status: TableStatus) -> ReferenceMatchStatus {
    match table_status {
        TableStatus::Present => ReferenceMatchStatus::NotChecked,
        TableStatus::Empty => ReferenceMatchStatus::EmptyTable,
        TableStatus::Absent => ReferenceMatchStatus::TableMissing,
        TableStatus::SchemaUnsupported => ReferenceMatchStatus::SchemaUnsupported,
        TableStatus::NotUsed => ReferenceMatchStatus::NotChecked,
    }
}

fn reference_table<'a>(
    input: &'a AnalysisInput<'_>,
    name: &str,
) -> Option<&'a ReferenceTableSnapshot> {
    input
        .reference_tables
        .iter()
        .find(|table| table.name == name)
}

fn sorted_strings(values: impl Iterator<Item = String>) -> Vec<String> {
    values.collect::<BTreeSet<_>>().into_iter().collect()
}

fn sorted_plu_numbers(plus: &[Plu]) -> Vec<u64> {
    sorted_numbers(plus.iter().map(|plu| plu.plu_number))
}

fn sorted_numbers(values: impl Iterator<Item = u64>) -> Vec<u64> {
    values.collect::<BTreeSet<_>>().into_iter().collect()
}

fn single_value(values: impl Iterator<Item = String>) -> Option<String> {
    values.collect::<BTreeSet<_>>().into_iter().next()
}

fn normalize_numeric_text(raw: &str, default_value: &str) -> String {
    let trimmed = raw.trim();
    let effective = if trimmed.is_empty() {
        default_value
    } else {
        trimmed
    };
    effective
        .parse::<u32>()
        .map(|value| value.to_string())
        .unwrap_or_else(|_| effective.to_string())
}

fn raw_categories_by_key(dataset: &SourceDataset) -> BTreeMap<(u64, u32), String> {
    let mut categories = BTreeMap::new();
    for row in &dataset.plu_rows {
        if let Some(key) = row_key(row) {
            categories.insert(
                key,
                optional_value(row, &["Category", "CATEGORY", "category"]),
            );
        }
    }
    categories
}

fn valid_keys(plus: &[Plu]) -> BTreeSet<(u64, u32)> {
    plus.iter().filter_map(plu_key).collect()
}

fn invalid_normalized_keys(all_plus: &[Plu], valid_plus: &[Plu]) -> BTreeSet<(u64, u32)> {
    let valid = valid_keys(valid_plus);
    all_plus
        .iter()
        .filter_map(plu_key)
        .filter(|key| !valid.contains(key))
        .collect()
}

fn placeholder_keys(
    dataset: &SourceDataset,
    row_issues: &[ValidationIssue],
) -> BTreeSet<(u64, u32)> {
    let placeholder_numbers = row_issues
        .iter()
        .filter(|issue| {
            issue.plu_number == Some(0) && issue.message.contains("missing product name")
        })
        .filter_map(|issue| issue.plu_number)
        .collect::<BTreeSet<_>>();
    dataset
        .plu_rows
        .iter()
        .filter_map(row_key)
        .filter(|(plu_number, _)| placeholder_numbers.contains(plu_number))
        .collect()
}

fn plu_key(plu: &Plu) -> Option<(u64, u32)> {
    Some((plu.plu_number, plu.department_number?))
}

fn row_key(row: &SourceRow) -> Option<(u64, u32)> {
    let plu = optional_value(row, &["Plucode", "PLUNo", "PluNo", "PLU", "PLU_NO"])
        .trim()
        .parse()
        .ok()?;
    let department = normalize_department(&optional_value(
        row,
        &["Department", "DeptNo", "DepartmentNo", "DEPT"],
    ))?;
    Some((plu, department))
}

fn normalize_department(raw: &str) -> Option<u32> {
    let value = raw.trim().parse::<u32>().ok()?;
    (value > 0).then_some(value)
}

fn optional_value(row: &SourceRow, names: &[&str]) -> String {
    names
        .iter()
        .find_map(|name| row.get(name).map(str::to_string))
        .unwrap_or_default()
}

fn count_empty_ingredient_fields(dataset: &SourceDataset) -> usize {
    dataset
        .ingredient_rows
        .iter()
        .flat_map(|row| row.values.iter())
        .filter(|(key, value)| key.starts_with("Ing Name ") && value.trim().is_empty())
        .count()
}

fn max_ing_name_field(dataset: &SourceDataset) -> Option<u32> {
    dataset
        .ingredient_rows
        .iter()
        .flat_map(|row| row.values.keys())
        .filter_map(|key| key.strip_prefix("Ing Name "))
        .filter_map(|value| value.parse::<u32>().ok())
        .max()
}

fn populated_ingredient_counts(
    dataset: &SourceDataset,
    valid_keys: &BTreeSet<(u64, u32)>,
) -> Vec<PluFieldCount> {
    let mut counts: BTreeMap<(u64, u32), usize> = BTreeMap::new();
    for row in &dataset.ingredient_rows {
        let Some(key) = row_key(row) else {
            continue;
        };
        if !valid_keys.contains(&key) {
            continue;
        }
        let count = row
            .values
            .iter()
            .filter(|(field, value)| field.starts_with("Ing Name ") && !value.trim().is_empty())
            .count();
        *counts.entry(key).or_default() += count;
    }
    counts
        .into_iter()
        .map(|((plu_number, department_number), count)| PluFieldCount {
            plu_number,
            department_number,
            count,
        })
        .collect()
}

fn count_empty_nutrition_values(dataset: &SourceDataset) -> usize {
    let names = [
        "Calories",
        "Calories Fat",
        "Total Fat",
        "Saturated Fat",
        "Cholesterol",
        "Sodium",
        "Carbohydrate",
        "Fiber",
        "Sugar",
        "Protein",
    ];
    dataset
        .nutrition_rows
        .iter()
        .flat_map(|row| row.values.iter())
        .filter(|(field, value)| {
            names.iter().any(|name| field.eq_ignore_ascii_case(name)) && value.trim().is_empty()
        })
        .count()
}

fn price_mode_name(mode: PriceMode) -> &'static str {
    match mode {
        PriceMode::ByWeight => "by_weight",
        PriceMode::ByEach => "by_each",
        PriceMode::FixedWeight => "fixed_weight",
        PriceMode::Unknown => "unknown",
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use rust_decimal::Decimal;

    use super::*;
    use crate::models::plu::Plu;
    use crate::source::SourceRow;

    fn valid_plu(plu_number: u64, raw_department: &str, group: u32) -> Plu {
        Plu {
            plu_number,
            store_number: 1,
            department_number: Some(raw_department.parse::<u32>().unwrap_or(1)),
            group_number: Some(group),
            source_department: Some(raw_department.to_string()),
            source_group: Some(group.to_string()),
            group_default_applied: false,
            name: "hidden".to_string(),
            barcode: Some(format!("020{plu_number:05}")),
            barcode_type: Some("5".to_string()),
            barcode_ref_no: Some("5".to_string()),
            source_barcode: Some(plu_number.to_string()),
            source_barcode_format: Some("05".to_string()),
            source_flag_data: Some("02".to_string()),
            price: Decimal::new(100, 2),
            price_mode: PriceMode::ByWeight,
            price_calc_method: Some(0),
            quantity: Some(0),
            quantity_symbol: Some(0),
            tare: None,
            discount_type: None,
            packing_date_print: None,
            packing_time_print: None,
            selling_date_print: None,
            selling_date_term: None,
            label_format: None,
            traceability: None,
            short_description: None,
            key_label: None,
            expiration_days: None,
            ingredients: Some("do not report this".to_string()),
            nutrition_facts: Vec::new(),
            source_pluing_row_count: 1,
        }
    }

    fn row(plu: &str, department: &str, category: &str) -> SourceRow {
        SourceRow {
            table: "Pludata".to_string(),
            values: BTreeMap::from([
                ("Plucode".to_string(), plu.to_string()),
                ("Department".to_string(), department.to_string()),
                ("Category".to_string(), category.to_string()),
            ]),
        }
    }

    fn pluing(plu: &str, department: &str) -> SourceRow {
        SourceRow {
            table: "PluIng".to_string(),
            values: BTreeMap::from([
                ("Plucode".to_string(), plu.to_string()),
                ("Department".to_string(), department.to_string()),
                ("Ing Name 1".to_string(), "private ingredient".to_string()),
                ("Ing Name 2".to_string(), "".to_string()),
            ]),
        }
    }

    fn report_for(
        dataset: &SourceDataset,
        valid: &[Plu],
        row_issues: &[ValidationIssue],
        refs: &[ReferenceTableSnapshot],
    ) -> AnalysisReport {
        let tables = vec![
            "Pludata".to_string(),
            "PluIng".to_string(),
            "Maingroup".to_string(),
        ];
        let validation_report = ValidationReport::default();
        collect_analysis(AnalysisInput {
            source_filename: "plu.mdb",
            source_file_size_bytes: 123,
            source_is_symlink: false,
            source_opened_read_only: true,
            mdb_tables: &tables,
            dataset,
            valid_plus: valid,
            all_normalized_plus: valid,
            row_issues,
            validation_report: &validation_report,
            placeholder_ignored: row_issues.len(),
            invalid_source_rows: 0,
            validation_skipped: 0,
            orphan_pluing_rows: 1,
            explicit_group_references: valid.len(),
            defaulted_group_references: 0,
            invalid_group_values: 0,
            reference_tables: refs,
            nutrition_fallback_to_pluing: true,
            nutrition_source_table: "PluIng",
        })
    }

    #[test]
    fn departments_and_groups_are_deduplicated_and_sorted() {
        let dataset = SourceDataset {
            plu_rows: vec![row("2", "2", "0"), row("1", "1", "0")],
            ingredient_rows: Vec::new(),
            nutrition_rows: Vec::new(),
        };
        let valid = vec![valid_plu(2, "2", 997), valid_plu(1, "1", 997)];
        let refs = vec![ReferenceTableSnapshot {
            name: "Maingroup".to_string(),
            present: true,
            row_count: 0,
            columns: Vec::new(),
        }];

        let report = report_for(&dataset, &valid, &[], &refs);

        assert_eq!(
            report
                .departments
                .iter()
                .map(|department| department.department_number)
                .collect::<Vec<_>>(),
            vec![1, 2]
        );
        assert_eq!(report.groups[0].department_number, 1);
        assert_eq!(report.groups[1].department_number, 2);
    }

    #[test]
    fn pluing_matching_counts_unmatched_and_caps_examples() {
        let mut ingredient_rows = vec![pluing("1", "1")];
        for plu in 10..=40 {
            ingredient_rows.push(pluing(&plu.to_string(), "1"));
        }
        let dataset = SourceDataset {
            plu_rows: vec![row("1", "1", "0")],
            ingredient_rows,
            nutrition_rows: Vec::new(),
        };
        let valid = vec![valid_plu(1, "1", 997)];

        let report = report_for(&dataset, &valid, &[], &[]);

        assert_eq!(report.ingredient_analysis.rows_matched_to_valid_plus, 1);
        assert_eq!(
            report.ingredient_analysis.unmatched_plu_code_examples.len(),
            20
        );
        assert!(report.ingredient_analysis.unmatched_examples_truncated);
    }

    #[test]
    fn reports_do_not_include_full_ingredient_text_in_json_model() {
        let dataset = SourceDataset {
            plu_rows: vec![row("1", "1", "0")],
            ingredient_rows: vec![pluing("1", "1")],
            nutrition_rows: Vec::new(),
        };
        let valid = vec![valid_plu(1, "1", 997)];

        let report = report_for(&dataset, &valid, &[], &[]);
        let json = serde_json::to_string(&report).expect("json");

        assert!(!json.contains("private ingredient"));
        assert!(!json.contains("do not report this"));
    }
}
