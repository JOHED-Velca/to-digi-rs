use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AnalysisStatus {
    Pass,
    PassWithWarnings,
    Fail,
}

impl AnalysisStatus {
    pub fn as_text(self) -> &'static str {
        match self {
            Self::Pass => "PASS",
            Self::PassWithWarnings => "PASS_WITH_WARNINGS",
            Self::Fail => "FAIL",
        }
    }

    pub fn exit_code(self) -> i32 {
        match self {
            Self::Pass | Self::PassWithWarnings => 0,
            Self::Fail => 2,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AnalysisReport {
    pub schema_version: u32,
    pub application_version: String,
    pub generated_at: String,
    pub analysis_status: AnalysisStatus,
    pub source: SourceSummary,
    pub summary: PluClassification,
    pub tables: Vec<TableAnalysis>,
    pub departments: Vec<DepartmentRequirement>,
    pub groups: Vec<GroupRequirement>,
    pub barcode_formats: Vec<BarcodeFormatAnalysis>,
    pub price_categories: Vec<PriceCategoryAnalysis>,
    pub ingredient_analysis: IngredientAnalysis,
    pub nutrition_analysis: NutritionAnalysis,
    pub warnings: Vec<AnalysisWarning>,
    pub blocking_errors: Vec<AnalysisBlockingError>,
    pub recommended_actions: Vec<String>,
    pub safety: SafetyConfirmation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SourceSummary {
    pub exact_filename: String,
    pub file_size_bytes: u64,
    pub is_symbolic_link: bool,
    pub opened_read_only: bool,
    pub source_modified: bool,
    pub mdb_tables_discovered: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PluClassification {
    pub total_pludata_rows: usize,
    pub total_pluing_rows: usize,
    pub source_plus_discovered: usize,
    pub empty_placeholder_rows: usize,
    pub normalized_plus: usize,
    pub valid_plus: usize,
    pub invalid_plus: usize,
    pub skipped_due_to_validation_errors: usize,
    pub valid_plu_numbers: Vec<u64>,
    pub plus_with_ingredients: usize,
    pub plus_with_nutrition_data: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TableAnalysis {
    pub name: String,
    pub role: String,
    pub status: TableStatus,
    pub row_count: Option<usize>,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TableStatus {
    Present,
    Empty,
    Absent,
    NotUsed,
    SchemaUnsupported,
}

impl TableStatus {
    pub fn as_text(self) -> &'static str {
        match self {
            Self::Present => "PRESENT",
            Self::Empty => "EMPTY",
            Self::Absent => "ABSENT",
            Self::NotUsed => "NOT_USED",
            Self::SchemaUnsupported => "SCHEMA_UNSUPPORTED",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DepartmentRequirement {
    pub department_number: u32,
    pub source_representations: Vec<String>,
    pub plu_count: usize,
    pub plu_numbers: Vec<u64>,
    pub normalization_applied: bool,
    pub source_table_status: TableStatus,
    pub source_reference_match: ReferenceMatchStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GroupRequirement {
    pub department_number: u32,
    pub group_number: u32,
    pub plu_count: usize,
    pub plu_numbers: Vec<u64>,
    pub explicit_source_group_count: usize,
    pub default_group_applied_count: usize,
    pub source_maingroup_table_status: TableStatus,
    pub source_reference_match: ReferenceMatchStatus,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReferenceMatchStatus {
    Matched,
    NotFound,
    EmptyTable,
    TableMissing,
    SchemaUnsupported,
    NotChecked,
}

impl ReferenceMatchStatus {
    pub fn as_text(self) -> &'static str {
        match self {
            Self::Matched => "MATCHED",
            Self::NotFound => "NOT_FOUND",
            Self::EmptyTable => "EMPTY_TABLE",
            Self::TableMissing => "TABLE_MISSING",
            Self::SchemaUnsupported => "SCHEMA_UNSUPPORTED",
            Self::NotChecked => "NOT_CHECKED",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BarcodeFormatAnalysis {
    pub original_source_value: String,
    pub normalized_format: String,
    pub plu_count: usize,
    pub plu_numbers: Vec<u64>,
    pub derived_digiweb_barcode_type: Option<String>,
    pub derived_digiweb_barcode_reference: Option<String>,
    pub valid_derivation_count: usize,
    pub invalid_derivation_count: usize,
    pub missing_barcode_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PriceCategoryAnalysis {
    pub raw_category: String,
    pub normalized_category: String,
    pub plu_count: usize,
    pub plu_numbers: Vec<u64>,
    pub derived_price_mode: String,
    pub derived_price_calc_method: Option<u8>,
    pub source_quantity_used: bool,
    pub validation_status: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct IngredientAnalysis {
    pub total_pluing_rows: usize,
    pub rows_matched_to_valid_plus: usize,
    pub rows_matched_to_placeholders: usize,
    pub rows_matched_to_invalid_plus: usize,
    pub unmatched_rows: usize,
    pub unique_unmatched_plu_codes: usize,
    pub unmatched_plu_code_examples: Vec<u64>,
    pub unmatched_examples_truncated: bool,
    pub valid_plus_with_matching_pluing_row: usize,
    pub valid_plus_without_matching_pluing_row: usize,
    pub valid_plus_with_ingredient_data: usize,
    pub valid_plus_without_ingredient_data: usize,
    pub empty_ingredient_fields_ignored: usize,
    pub maximum_source_ingredient_field_number_observed: Option<u32>,
    pub populated_ingredient_fields_per_plu: Vec<PluFieldCount>,
    pub duplicate_matches: Vec<PluFieldCount>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NutritionAnalysis {
    pub source_table_used: String,
    pub fallback_to_pluing_used: bool,
    pub valid_plus_with_nutrition_data: usize,
    pub valid_plus_without_nutrition_data: usize,
    pub nutrition_rows_matched: usize,
    pub malformed_values: usize,
    pub recognized_fields: Vec<String>,
    pub ignored_empty_values: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PluFieldCount {
    pub plu_number: u64,
    pub department_number: u32,
    pub count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AnalysisWarning {
    pub code: String,
    pub severity: String,
    pub message: String,
    pub affected_plus: Vec<u64>,
    pub count: Option<usize>,
    pub recommended_action: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AnalysisBlockingError {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SafetyConfirmation {
    pub analysis_only: bool,
    pub network_access_permitted: bool,
    pub authentication_attempted: bool,
    pub digiweb_api_requests_attempted: bool,
    pub source_database_modified: bool,
    pub opened_only_exact_plu_mdb: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReferenceTableSnapshot {
    pub name: String,
    pub present: bool,
    pub row_count: usize,
    pub columns: Vec<String>,
}
