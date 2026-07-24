use std::fs;
use std::path::Path;

use crate::analysis::model::{AnalysisReport, TableStatus};
use crate::error::AppError;

pub fn write_text_report(path: &Path, report: &AnalysisReport) -> Result<(), AppError> {
    fs::write(path, render_text_report(report)).map_err(|err| {
        AppError::Logging(format!(
            "failed to write {}: {err}",
            path.file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("analysis-report.txt")
        ))
    })
}

pub fn render_text_report(report: &AnalysisReport) -> String {
    let mut out = String::new();
    line(&mut out, "DIGIweb MDB Analysis Report");
    line(
        &mut out,
        format!("Application version: {}", report.application_version),
    );
    line(&mut out, format!("Generated at: {}", report.generated_at));
    line(
        &mut out,
        format!("Source file: {}", report.source.exact_filename),
    );
    line(
        &mut out,
        format!("Analysis status: {}", report.analysis_status.as_text()),
    );
    blank(&mut out);

    section(&mut out, "1. Source summary");
    line(
        &mut out,
        format!("Exact source filename: {}", report.source.exact_filename),
    );
    line(
        &mut out,
        format!("Source file size bytes: {}", report.source.file_size_bytes),
    );
    line(
        &mut out,
        format!("Symbolic link: {}", yes_no(report.source.is_symbolic_link)),
    );
    line(
        &mut out,
        format!(
            "Opened read-only: {}",
            yes_no(report.source.opened_read_only)
        ),
    );
    line(
        &mut out,
        format!(
            "Source database modified: {}",
            yes_no(report.source.source_modified)
        ),
    );
    line(
        &mut out,
        format!(
            "MDB tables discovered: {}",
            join_or_none(&report.source.mdb_tables_discovered)
        ),
    );
    blank(&mut out);

    section(&mut out, "2. Source tables");
    for table in &report.tables {
        let count = table
            .row_count
            .map(|value| format!(", {value} rows"))
            .unwrap_or_default();
        line(
            &mut out,
            format!("- {}: {}{}", table.name, table.status.as_text(), count),
        );
    }
    blank(&mut out);

    section(&mut out, "3. PLU validation");
    line(
        &mut out,
        format!("Source PLUs: {}", report.summary.source_plus_discovered),
    );
    line(
        &mut out,
        format!("Valid PLUs: {}", report.summary.valid_plus),
    );
    line(
        &mut out,
        format!(
            "Empty placeholders: {}",
            report.summary.empty_placeholder_rows
        ),
    );
    line(
        &mut out,
        format!("Invalid PLUs: {}", report.summary.invalid_plus),
    );
    line(&mut out, "Valid PLU numbers:");
    for plu in &report.summary.valid_plu_numbers {
        line(&mut out, format!("- {plu}"));
    }
    blank(&mut out);

    section(&mut out, "4. Required departments");
    line(
        &mut out,
        format!("Unique required departments: {}", report.departments.len()),
    );
    for department in &report.departments {
        line(
            &mut out,
            format!("Department {}", department.department_number),
        );
        line(
            &mut out,
            format!(
                "Source name: {}",
                display_optional_name(department.source_name.as_deref())
            ),
        );
        line(
            &mut out,
            format!(
                "Source representations: {}",
                quoted_join(&department.source_representations)
            ),
        );
        line(&mut out, format!("PLU count: {}", department.plu_count));
        line(
            &mut out,
            format!("PLUs: {}", join_numbers(&department.plu_numbers)),
        );
        line(
            &mut out,
            format!(
                "Normalization applied: {}",
                yes_no(department.normalization_applied)
            ),
        );
        line(
            &mut out,
            format!(
                "Source Department table: {}",
                department.source_table_status.as_text()
            ),
        );
        line(
            &mut out,
            format!(
                "Source-reference match: {}",
                department.source_reference_match.as_text()
            ),
        );
    }
    blank(&mut out);

    section(&mut out, "5. Required groups");
    line(
        &mut out,
        format!(
            "Unique required department/group combinations: {}",
            report.groups.len()
        ),
    );
    let explicit_groups: usize = report
        .groups
        .iter()
        .map(|group| group.explicit_source_group_count)
        .sum();
    let defaulted_groups: usize = report
        .groups
        .iter()
        .map(|group| group.default_group_applied_count)
        .sum();
    line(
        &mut out,
        format!("PLUs using explicit groups: {explicit_groups}"),
    );
    line(
        &mut out,
        format!("PLUs defaulted to group 997: {defaulted_groups}"),
    );
    for group in &report.groups {
        line(
            &mut out,
            format!(
                "Department {} / Group {}",
                group.department_number, group.group_number
            ),
        );
        line(
            &mut out,
            format!(
                "Source name: {}",
                display_optional_name(group.source_name.as_deref())
            ),
        );
        line(&mut out, format!("PLU count: {}", group.plu_count));
        line(
            &mut out,
            format!("PLUs: {}", join_numbers(&group.plu_numbers)),
        );
        line(
            &mut out,
            format!(
                "Explicit source group count: {}",
                group.explicit_source_group_count
            ),
        );
        line(
            &mut out,
            format!(
                "Default group applied count: {}",
                group.default_group_applied_count
            ),
        );
        line(
            &mut out,
            format!(
                "Source Maingroup table: {}",
                group.source_maingroup_table_status.as_text()
            ),
        );
        line(
            &mut out,
            format!(
                "Source-reference match: {}",
                group.source_reference_match.as_text()
            ),
        );
    }
    blank(&mut out);

    section(&mut out, "6. Barcode analysis");
    for barcode in &report.barcode_formats {
        line(
            &mut out,
            format!(
                "Format {}: {} PLUs",
                barcode.original_source_value, barcode.plu_count
            ),
        );
        line(
            &mut out,
            format!("PLUs: {}", join_numbers(&barcode.plu_numbers)),
        );
        line(
            &mut out,
            format!(
                "Derived DIGIweb barcode type: {}",
                barcode
                    .derived_digiweb_barcode_type
                    .as_deref()
                    .unwrap_or("missing")
            ),
        );
        line(
            &mut out,
            format!(
                "Derived DIGIweb barcode reference: {}",
                barcode
                    .derived_digiweb_barcode_reference
                    .as_deref()
                    .unwrap_or("missing")
            ),
        );
        line(
            &mut out,
            format!("Invalid derivations: {}", barcode.invalid_derivation_count),
        );
    }
    blank(&mut out);

    section(&mut out, "7. Price-category analysis");
    for category in &report.price_categories {
        line(
            &mut out,
            format!(
                "Category {}: {} PLUs",
                category.normalized_category, category.plu_count
            ),
        );
        line(
            &mut out,
            format!("PLUs: {}", join_numbers(&category.plu_numbers)),
        );
        line(
            &mut out,
            format!(
                "Derived DIGIweb price mode: {}",
                category.derived_price_mode
            ),
        );
        line(
            &mut out,
            format!(
                "Derived price calculation method: {}",
                category
                    .derived_price_calc_method
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "missing".to_string())
            ),
        );
        line(
            &mut out,
            format!(
                "Source quantity used: {}",
                yes_no(category.source_quantity_used)
            ),
        );
        line(
            &mut out,
            format!("Validation status: {}", category.validation_status),
        );
    }
    blank(&mut out);

    section(&mut out, "8. Ingredient and nutrition analysis");
    line(
        &mut out,
        format!(
            "Total PluIng rows: {}",
            report.ingredient_analysis.total_pluing_rows
        ),
    );
    line(
        &mut out,
        format!(
            "Rows matched to valid PLUs: {}",
            report.ingredient_analysis.rows_matched_to_valid_plus
        ),
    );
    line(
        &mut out,
        format!(
            "Unmatched PluIng rows: {}",
            report.ingredient_analysis.unmatched_rows
        ),
    );
    line(
        &mut out,
        format!(
            "Unique unmatched PLU codes: {}",
            report.ingredient_analysis.unique_unmatched_plu_codes
        ),
    );
    line(
        &mut out,
        format!(
            "Unmatched PLU code examples: {}{}",
            join_numbers(&report.ingredient_analysis.unmatched_plu_code_examples),
            if report.ingredient_analysis.unmatched_examples_truncated {
                " (truncated)"
            } else {
                ""
            }
        ),
    );
    line(
        &mut out,
        format!(
            "Valid PLUs with ingredient data: {}",
            report.ingredient_analysis.valid_plus_with_ingredient_data
        ),
    );
    line(
        &mut out,
        format!(
            "Valid PLUs without ingredient data: {}",
            report
                .ingredient_analysis
                .valid_plus_without_ingredient_data
        ),
    );
    line(
        &mut out,
        format!(
            "Nutrition source table used: {}",
            report.nutrition_analysis.source_table_used
        ),
    );
    line(
        &mut out,
        format!(
            "Nutrition fallback to PluIng used: {}",
            yes_no(report.nutrition_analysis.fallback_to_pluing_used)
        ),
    );
    line(
        &mut out,
        format!(
            "Valid PLUs with nutrition data: {}",
            report.nutrition_analysis.valid_plus_with_nutrition_data
        ),
    );
    line(
        &mut out,
        format!(
            "Valid PLUs without nutrition data: {}",
            report.nutrition_analysis.valid_plus_without_nutrition_data
        ),
    );
    blank(&mut out);

    section(&mut out, "9. Source-reference-table checks");
    for table in &report.tables {
        if table.name == "Department" || table.name == "Maingroup" {
            let note = if table.status == TableStatus::Empty {
                "present but empty"
            } else {
                table.status.as_text()
            };
            line(&mut out, format!("- {}: {}", table.name, note));
        }
    }
    blank(&mut out);

    section(&mut out, "10. Warnings");
    if report.warnings.is_empty() {
        line(&mut out, "None");
    } else {
        for warning in &report.warnings {
            line(&mut out, format!("WARN {}", warning.code));
            line(&mut out, &warning.message);
            line(
                &mut out,
                format!("Recommended action: {}", warning.recommended_action),
            );
        }
    }
    blank(&mut out);

    section(&mut out, "11. Blocking errors");
    if report.blocking_errors.is_empty() {
        line(&mut out, "None");
    } else {
        for error in &report.blocking_errors {
            line(&mut out, format!("{}: {}", error.code, error.message));
        }
    }
    blank(&mut out);

    section(&mut out, "12. Recommended installation actions");
    for (index, action) in report.recommended_actions.iter().enumerate() {
        line(&mut out, format!("{}. {}", index + 1, action));
    }
    blank(&mut out);

    section(&mut out, "13. Safety confirmation");
    line(&mut out, "ANALYSIS ONLY");
    line(
        &mut out,
        format!(
            "Network access permitted: {}",
            yes_no_upper(report.safety.network_access_permitted)
        ),
    );
    line(
        &mut out,
        format!(
            "Authentication attempted: {}",
            yes_no_upper(report.safety.authentication_attempted)
        ),
    );
    line(
        &mut out,
        format!(
            "DIGIweb API requests attempted: {}",
            yes_no_upper(report.safety.digiweb_api_requests_attempted)
        ),
    );
    line(
        &mut out,
        format!(
            "Source database modified: {}",
            yes_no_upper(report.safety.source_database_modified)
        ),
    );
    out
}

fn section(out: &mut String, title: &str) {
    line(out, title);
}

fn blank(out: &mut String) {
    out.push('\n');
}

fn line(out: &mut String, value: impl AsRef<str>) {
    out.push_str(value.as_ref());
    out.push('\n');
}

fn yes_no(value: bool) -> &'static str {
    if value { "yes" } else { "no" }
}

fn yes_no_upper(value: bool) -> &'static str {
    if value { "YES" } else { "NO" }
}

fn join_or_none(values: &[String]) -> String {
    if values.is_empty() {
        "None".to_string()
    } else {
        values.join(", ")
    }
}

fn quoted_join(values: &[String]) -> String {
    if values.is_empty() {
        "None".to_string()
    } else {
        values
            .iter()
            .map(|value| format!("{value:?}"))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

fn join_numbers(values: &[u64]) -> String {
    if values.is_empty() {
        "None".to_string()
    } else {
        values
            .iter()
            .map(u64::to_string)
            .collect::<Vec<_>>()
            .join(", ")
    }
}

fn display_optional_name(value: Option<&str>) -> &str {
    value.unwrap_or("Name unavailable in source MDB")
}

#[cfg(test)]
mod tests {
    use crate::analysis::model::{
        AnalysisReport, AnalysisStatus, PluClassification, SafetyConfirmation, SourceSummary,
    };

    use super::*;

    #[test]
    fn text_report_contains_safety_confirmation_and_no_secrets() {
        let report = AnalysisReport {
            schema_version: 1,
            application_version: "0.6.0".to_string(),
            generated_at: "2026-07-23T00:00:00-04:00".to_string(),
            analysis_status: AnalysisStatus::Pass,
            source: SourceSummary {
                exact_filename: "plu.mdb".to_string(),
                file_size_bytes: 10,
                is_symbolic_link: false,
                opened_read_only: true,
                source_modified: false,
                mdb_tables_discovered: vec!["Pludata".to_string()],
            },
            summary: PluClassification {
                total_pludata_rows: 1,
                total_pluing_rows: 0,
                source_plus_discovered: 1,
                empty_placeholder_rows: 0,
                normalized_plus: 1,
                valid_plus: 1,
                invalid_plus: 0,
                skipped_due_to_validation_errors: 0,
                valid_plu_numbers: vec![1],
                plus_with_ingredients: 0,
                plus_with_nutrition_data: 0,
            },
            tables: Vec::new(),
            departments: Vec::new(),
            groups: Vec::new(),
            barcode_formats: Vec::new(),
            price_categories: Vec::new(),
            ingredient_analysis: crate::analysis::model::IngredientAnalysis {
                total_pluing_rows: 0,
                rows_matched_to_valid_plus: 0,
                rows_matched_to_placeholders: 0,
                rows_matched_to_invalid_plus: 0,
                unmatched_rows: 0,
                unique_unmatched_plu_codes: 0,
                unmatched_plu_code_examples: Vec::new(),
                unmatched_examples_truncated: false,
                valid_plus_with_matching_pluing_row: 0,
                valid_plus_without_matching_pluing_row: 1,
                valid_plus_with_ingredient_data: 0,
                valid_plus_without_ingredient_data: 1,
                empty_ingredient_fields_ignored: 0,
                maximum_source_ingredient_field_number_observed: None,
                populated_ingredient_fields_per_plu: Vec::new(),
                duplicate_matches: Vec::new(),
            },
            nutrition_analysis: crate::analysis::model::NutritionAnalysis {
                source_table_used: "PluIng".to_string(),
                fallback_to_pluing_used: true,
                valid_plus_with_nutrition_data: 0,
                valid_plus_without_nutrition_data: 1,
                nutrition_rows_matched: 0,
                malformed_values: 0,
                recognized_fields: Vec::new(),
                ignored_empty_values: 0,
            },
            warnings: Vec::new(),
            blocking_errors: Vec::new(),
            recommended_actions: Vec::new(),
            safety: SafetyConfirmation {
                analysis_only: true,
                network_access_permitted: false,
                authentication_attempted: false,
                digiweb_api_requests_attempted: false,
                source_database_modified: false,
                opened_only_exact_plu_mdb: true,
            },
        };

        let text = render_text_report(&report);

        assert!(text.contains("Network access permitted: NO"));
        assert!(text.contains("Authentication attempted: NO"));
        assert!(!text.to_ascii_lowercase().contains("secret"));
        assert!(!text.to_ascii_lowercase().contains("token"));
    }
}
