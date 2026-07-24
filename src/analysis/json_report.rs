use std::fs;
use std::path::Path;

use crate::analysis::model::AnalysisReport;
use crate::error::AppError;

pub fn write_json_report(path: &Path, report: &AnalysisReport) -> Result<(), AppError> {
    let json = serde_json::to_string_pretty(report)
        .map_err(|err| AppError::Internal(format!("analysis JSON serialization failed: {err}")))?;
    fs::write(path, json).map_err(|err| {
        AppError::Logging(format!(
            "failed to write {}: {err}",
            path.file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("analysis-report.json")
        ))
    })
}

#[cfg(test)]
mod tests {
    use crate::analysis::model::{
        AnalysisReport, AnalysisStatus, IngredientAnalysis, NutritionAnalysis, PluClassification,
        SafetyConfirmation, SourceSummary,
    };

    #[test]
    fn json_report_is_parseable_and_contains_schema_version() {
        let report = AnalysisReport {
            schema_version: 1,
            application_version: "0.7.0".to_string(),
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
            ingredient_analysis: IngredientAnalysis {
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
            nutrition_analysis: NutritionAnalysis {
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

        let json = serde_json::to_string_pretty(&report).expect("json");
        let parsed: serde_json::Value = serde_json::from_str(&json).expect("parse");

        assert_eq!(parsed["schema_version"], 1);
        assert_eq!(parsed["application_version"], "0.7.0");
        assert!(!json.contains("Authorization"));
    }
}
