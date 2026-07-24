use crate::analysis::model::{AnalysisReport, AnalysisWarning};

const SETUP_SEPARATOR: &str = "============================";
const DISPLAY_LIMIT: usize = 100;

pub fn render_console_summary(
    report: &AnalysisReport,
    text_report_path: &str,
    json_report_path: &str,
) -> String {
    let mut out = String::new();
    line(&mut out, "MDB ANALYSIS COMPLETE");
    blank(&mut out);
    line(
        &mut out,
        format!("Analysis status: {}", report.analysis_status.as_text()),
    );
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
    line(&mut out, format!("Warnings: {}", report.warnings.len()));
    line(
        &mut out,
        format!("Blocking errors: {}", report.blocking_errors.len()),
    );
    blank(&mut out);

    render_departments(report, &mut out);
    blank(&mut out);
    render_groups(report, &mut out);
    blank(&mut out);
    render_installation_warnings(report, &mut out);
    blank(&mut out);
    render_setup(report, &mut out);
    blank(&mut out);
    line(&mut out, "Detailed text report:");
    line(&mut out, text_report_path);
    blank(&mut out);
    line(&mut out, "JSON report:");
    line(&mut out, json_report_path);
    blank(&mut out);
    line(
        &mut out,
        "No authentication or DIGIweb API requests were attempted.",
    );
    out
}

fn render_departments(report: &AnalysisReport, out: &mut String) {
    line(
        out,
        format!("Required departments: {}", report.departments.len()),
    );
    if report.departments.is_empty() {
        line(out, "No department prerequisites were identified.");
        return;
    }
    if report.departments.len() > DISPLAY_LIMIT {
        line(
            out,
            format!(
                "Showing first {DISPLAY_LIMIT} of {} required departments.",
                report.departments.len()
            ),
        );
        line(out, "Complete list: ./analysis-report.txt");
    }
    for department in report.departments.iter().take(DISPLAY_LIMIT) {
        line(
            out,
            format!(
                "- Department {} - {}",
                department.department_number,
                display_source_name(department.source_name.as_deref())
            ),
        );
        line(out, format!("  Used by {} PLUs", department.plu_count));
    }
}

fn render_groups(report: &AnalysisReport, out: &mut String) {
    line(out, format!("Required groups: {}", report.groups.len()));
    if report.groups.is_empty() {
        line(out, "No group prerequisites were identified.");
        return;
    }
    if report.groups.len() > DISPLAY_LIMIT {
        line(
            out,
            format!(
                "Showing first {DISPLAY_LIMIT} of {} required groups.",
                report.groups.len()
            ),
        );
        line(out, "Complete list: ./analysis-report.txt");
    }
    for group in report.groups.iter().take(DISPLAY_LIMIT) {
        line(
            out,
            format!(
                "- Department {} / Group {} - {}",
                group.department_number,
                group.group_number,
                display_source_name(group.source_name.as_deref())
            ),
        );
        line(out, format!("  Used by {} PLUs", group.plu_count));
        if group.default_group_applied_count > 0 {
            line(
                out,
                format!(
                    "  Default group 997 applied for {} PLUs",
                    group.default_group_applied_count
                ),
            );
        }
    }
}

fn render_installation_warnings(report: &AnalysisReport, out: &mut String) {
    let warnings = installation_warnings(&report.warnings);
    if warnings.is_empty() {
        line(out, "Warnings: none requiring installation action");
        return;
    }
    line(out, "Warnings:");
    for warning in warnings {
        line(out, format!("- {warning}"));
    }
}

fn installation_warnings(warnings: &[AnalysisWarning]) -> Vec<String> {
    let mut values = Vec::new();
    for warning in warnings {
        match warning.code.as_str() {
            "SOURCE_MAINGROUP_EMPTY" => values.push(
                "Maingroup is empty; required group names could not be confirmed.".to_string(),
            ),
            "SOURCE_DEPARTMENT_EMPTY" => values.push(
                "Department table is empty; required department names could not be confirmed."
                    .to_string(),
            ),
            "UNMATCHED_PLUING_ROWS" => values.push(format!(
                "{} unmatched PluIng rows.",
                format_count(warning.count.unwrap_or_default())
            )),
            "DEFAULT_GROUP_APPLIED" => values.push(format!(
                "{} PLUs defaulted to group 997.",
                format_count(warning.count.unwrap_or_default())
            )),
            _ => {}
        }
    }
    values
}

fn render_setup(report: &AnalysisReport, out: &mut String) {
    line(out, SETUP_SEPARATOR);
    line(out, "REQUIRED DIGIWEB SETUP");
    blank(out);
    line(out, "Create or confirm these departments:");
    if report.departments.is_empty() {
        blank(out);
        line(out, "No department prerequisites were identified.");
    } else {
        blank(out);
        for (index, department) in report.departments.iter().take(DISPLAY_LIMIT).enumerate() {
            line(
                out,
                format!(
                    "{}. Department ID: {}",
                    index + 1,
                    department.department_number
                ),
            );
            line(
                out,
                format!(
                    "   Name: {}",
                    display_setup_name(department.source_name.as_deref())
                ),
            );
        }
        if report.departments.len() > DISPLAY_LIMIT {
            line(
                out,
                format!(
                    "... {} more departments omitted. Complete list: ./analysis-report.txt",
                    report.departments.len() - DISPLAY_LIMIT
                ),
            );
        }
    }
    blank(out);
    line(out, "Create or confirm these groups:");
    if report.groups.is_empty() {
        blank(out);
        line(out, "No group prerequisites were identified.");
    } else {
        blank(out);
        for (index, group) in report.groups.iter().take(DISPLAY_LIMIT).enumerate() {
            line(
                out,
                format!("{}. Department ID: {}", index + 1, group.department_number),
            );
            line(out, format!("   Group ID: {}", group.group_number));
            line(
                out,
                format!(
                    "   Name: {}",
                    display_setup_name(group.source_name.as_deref())
                ),
            );
        }
        if report.groups.len() > DISPLAY_LIMIT {
            line(
                out,
                format!(
                    "... {} more groups omitted. Complete list: ./analysis-report.txt",
                    report.groups.len() - DISPLAY_LIMIT
                ),
            );
        }
    }
    blank(out);
    line(out, "This analysis reads only plu.mdb.");
    line(
        out,
        "It does not confirm whether these departments or groups already exist in DIGIweb.",
    );
    blank(out);
    line(out, "Next:");
    line(out, "./import.sh verify");
    blank(out);
    line(out, "Test one PLU:");
    line(out, "./import.sh import --limit 1");
    line(out, SETUP_SEPARATOR);
}

fn display_source_name(value: Option<&str>) -> &str {
    value.unwrap_or("Name unavailable in source MDB")
}

fn display_setup_name(value: Option<&str>) -> &str {
    value.unwrap_or("unavailable in source MDB")
}

fn format_count(value: usize) -> String {
    let text = value.to_string();
    let mut out = String::new();
    for (index, ch) in text.chars().rev().enumerate() {
        if index > 0 && index % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    out.chars().rev().collect()
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
    use crate::analysis::model::{
        AnalysisReport, AnalysisStatus, DepartmentRequirement, GroupRequirement,
        IngredientAnalysis, NutritionAnalysis, PluClassification, ReferenceMatchStatus,
        SafetyConfirmation, SourceSummary, TableStatus,
    };

    use super::*;

    fn report() -> AnalysisReport {
        AnalysisReport {
            schema_version: 1,
            application_version: "0.7.0".to_string(),
            generated_at: "2026-07-23T00:00:00-04:00".to_string(),
            analysis_status: AnalysisStatus::PassWithWarnings,
            source: SourceSummary {
                exact_filename: "plu.mdb".to_string(),
                file_size_bytes: 10,
                is_symbolic_link: false,
                opened_read_only: true,
                source_modified: false,
                mdb_tables_discovered: Vec::new(),
            },
            summary: PluClassification {
                total_pludata_rows: 5,
                total_pluing_rows: 6458,
                source_plus_discovered: 5,
                empty_placeholder_rows: 1,
                normalized_plus: 4,
                valid_plus: 4,
                invalid_plus: 0,
                skipped_due_to_validation_errors: 0,
                valid_plu_numbers: vec![1, 2, 3, 4],
                plus_with_ingredients: 4,
                plus_with_nutrition_data: 3,
            },
            tables: Vec::new(),
            departments: vec![DepartmentRequirement {
                department_number: 1,
                source_name: Some("STORE".to_string()),
                source_representations: vec!["0001".to_string()],
                plu_count: 4,
                plu_numbers: vec![1, 2, 3, 4],
                normalization_applied: true,
                source_table_status: TableStatus::Present,
                source_reference_match: ReferenceMatchStatus::NotChecked,
            }],
            groups: vec![GroupRequirement {
                department_number: 1,
                group_number: 997,
                source_name: None,
                plu_count: 4,
                plu_numbers: vec![1, 2, 3, 4],
                explicit_source_group_count: 4,
                default_group_applied_count: 0,
                source_maingroup_table_status: TableStatus::Empty,
                source_reference_match: ReferenceMatchStatus::EmptyTable,
            }],
            barcode_formats: Vec::new(),
            price_categories: Vec::new(),
            ingredient_analysis: IngredientAnalysis {
                total_pluing_rows: 6458,
                rows_matched_to_valid_plus: 4,
                rows_matched_to_placeholders: 0,
                rows_matched_to_invalid_plus: 0,
                unmatched_rows: 6454,
                unique_unmatched_plu_codes: 6454,
                unmatched_plu_code_examples: Vec::new(),
                unmatched_examples_truncated: true,
                valid_plus_with_matching_pluing_row: 4,
                valid_plus_without_matching_pluing_row: 0,
                valid_plus_with_ingredient_data: 4,
                valid_plus_without_ingredient_data: 0,
                empty_ingredient_fields_ignored: 0,
                maximum_source_ingredient_field_number_observed: Some(99),
                populated_ingredient_fields_per_plu: Vec::new(),
                duplicate_matches: Vec::new(),
            },
            nutrition_analysis: NutritionAnalysis {
                source_table_used: "PluIng".to_string(),
                fallback_to_pluing_used: true,
                valid_plus_with_nutrition_data: 3,
                valid_plus_without_nutrition_data: 1,
                nutrition_rows_matched: 3,
                malformed_values: 0,
                recognized_fields: Vec::new(),
                ignored_empty_values: 0,
            },
            warnings: vec![
                crate::analysis::model::AnalysisWarning {
                    code: "SOURCE_MAINGROUP_EMPTY".to_string(),
                    severity: "WARNING".to_string(),
                    message: "Maingroup is present but contains no records.".to_string(),
                    affected_plus: Vec::new(),
                    count: Some(0),
                    recommended_action: String::new(),
                },
                crate::analysis::model::AnalysisWarning {
                    code: "UNMATCHED_PLUING_ROWS".to_string(),
                    severity: "WARNING".to_string(),
                    message: "6454 unmatched".to_string(),
                    affected_plus: Vec::new(),
                    count: Some(6454),
                    recommended_action: String::new(),
                },
            ],
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
        }
    }

    #[test]
    fn console_summary_contains_operational_analysis_setup_and_paths() {
        let output = render_console_summary(
            &report(),
            "/tmp/analysis-report.txt",
            "/tmp/analysis-report.json",
        );

        assert!(output.contains("MDB ANALYSIS COMPLETE"));
        assert!(output.contains("Analysis status: PASS_WITH_WARNINGS"));
        assert!(output.contains("Source PLUs: 5"));
        assert!(output.contains("Valid PLUs: 4"));
        assert!(output.contains("Empty placeholders: 1"));
        assert!(output.contains("Invalid PLUs: 0"));
        assert!(output.contains("Warnings: 2"));
        assert!(output.contains("Blocking errors: 0"));
        assert!(output.contains("Required departments: 1"));
        assert!(output.contains("- Department 1 - STORE"));
        assert!(output.contains("Required groups: 1"));
        assert!(output.contains("- Department 1 / Group 997 - Name unavailable in source MDB"));
        assert!(output.contains(SETUP_SEPARATOR));
        assert!(output.contains("REQUIRED DIGIWEB SETUP"));
        assert!(output.contains("Create or confirm these departments:"));
        assert!(output.contains("Create or confirm these groups:"));
        assert!(output.contains(
            "It does not confirm whether these departments or groups already exist in DIGIweb."
        ));
        assert!(output.contains("./import.sh verify"));
        assert!(output.contains("./import.sh import --limit 1"));
        assert!(output.contains("/tmp/analysis-report.txt"));
        assert!(output.contains("/tmp/analysis-report.json"));
        assert!(output.contains("No authentication or DIGIweb API requests were attempted."));
        assert!(!output.contains("MDB tables discovered"));
        assert!(!output.contains("Derived DIGIweb barcode"));
    }

    #[test]
    fn empty_prerequisite_lists_are_clear() {
        let mut report = report();
        report.departments.clear();
        report.groups.clear();

        let output = render_console_summary(&report, "analysis-report.txt", "analysis-report.json");

        assert!(output.contains("Required departments: 0"));
        assert!(output.contains("No department prerequisites were identified."));
        assert!(output.contains("Required groups: 0"));
        assert!(output.contains("No group prerequisites were identified."));
    }

    #[test]
    fn large_result_sets_are_not_silently_truncated() {
        let mut report = report();
        report.groups = (1..=101)
            .map(|group| GroupRequirement {
                department_number: 1,
                group_number: group,
                source_name: None,
                plu_count: 1,
                plu_numbers: vec![group as u64],
                explicit_source_group_count: 1,
                default_group_applied_count: 0,
                source_maingroup_table_status: TableStatus::Present,
                source_reference_match: ReferenceMatchStatus::NotChecked,
            })
            .collect();

        let output = render_console_summary(&report, "analysis-report.txt", "analysis-report.json");

        assert!(output.contains("Showing first 100 of 101 required groups."));
        assert!(output.contains("1 more groups omitted"));
    }
}
