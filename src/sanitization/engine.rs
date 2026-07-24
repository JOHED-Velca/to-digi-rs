use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

use crate::error::AppError;
use crate::sanitization::profile::{
    CopyNormalization, RuleAction, SanitizationProfile, SourceField, TargetField,
};
use crate::source::{SourceDataset, SourceRow};

#[derive(Debug, Clone)]
pub struct SanitizedDataset {
    pub dataset: SourceDataset,
    pub report: SanitizationEngineReport,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct SanitizationEngineReport {
    pub profile_name: String,
    pub profile_version: u32,
    pub source_plus: usize,
    pub changed_plus: usize,
    pub unchanged_plus: usize,
    pub placeholder_plus: usize,
    pub nonempty_values_changed: usize,
    pub field_changes: BTreeMap<String, usize>,
    pub records: Vec<SanitizedRecordChange>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SanitizedRecordChange {
    pub plu_number: u64,
    pub fields: Vec<SanitizedFieldChange>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SanitizedFieldChange {
    pub field: String,
    pub original_empty_category: EmptyCategory,
    pub applied_rule: String,
    pub sanitized_value: String,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EmptyCategory {
    EmptyString,
    WhitespaceOnly,
    PaddingOnly,
}

pub fn apply_profile(
    dataset: &SourceDataset,
    profile: &SanitizationProfile,
) -> Result<SanitizedDataset, AppError> {
    profile.validate()?;
    let mut sanitized = dataset.clone();
    let mut report = SanitizationEngineReport {
        profile_name: profile.profile_name.clone(),
        profile_version: profile.profile_version,
        source_plus: dataset.plu_rows.len(),
        unchanged_plus: dataset.plu_rows.len(),
        ..SanitizationEngineReport::default()
    };
    let mut changed_numbers = BTreeSet::new();
    let mut record_changes = Vec::new();
    for row in &mut sanitized.plu_rows {
        if !is_candidate_product(row) {
            report.placeholder_plus += 1;
            continue;
        }
        let plu_number = normalized_numeric_field(row, SourceField::PluCode.source_columns())
            .ok_or_else(|| AppError::Internal("candidate product lacks PLU code".to_string()))?;
        let plu_number = plu_number
            .parse::<u64>()
            .map_err(|err| AppError::Internal(format!("invalid normalized PLU code: {err}")))?;
        let mut changes = Vec::new();
        for rule in &profile.rules {
            let Some(column) = existing_column(row, rule.field.source_columns()) else {
                return Err(AppError::Config(format!(
                    "sanitization profile targets field '{}' but no matching source column exists",
                    rule.field.as_str()
                )));
            };
            let original = row.values.get(column).cloned().unwrap_or_default();
            let Some(empty_category) = empty_category(&original) else {
                continue;
            };
            let sanitized_value = rule_value(
                row,
                profile,
                rule.field,
                rule.action,
                rule.value.as_deref(),
                rule.source_field,
                rule.normalization,
            )?;
            if is_empty_source_value(&sanitized_value) {
                return Err(AppError::Config(format!(
                    "sanitization rule for '{}' produced an empty value",
                    rule.field.as_str()
                )));
            }
            row.values
                .insert(column.to_string(), sanitized_value.clone());
            *report
                .field_changes
                .entry(rule.field.as_str().to_string())
                .or_default() += 1;
            changes.push(SanitizedFieldChange {
                field: rule.field.as_str().to_string(),
                original_empty_category: empty_category,
                applied_rule: applied_rule_text(rule.action, rule.source_field),
                sanitized_value,
            });
        }
        if !changes.is_empty() {
            changed_numbers.insert(plu_number);
            record_changes.push(SanitizedRecordChange {
                plu_number,
                fields: changes,
            });
        }
    }
    record_changes.sort_by_key(|record| record.plu_number);
    report.changed_plus = changed_numbers.len();
    report.unchanged_plus = report.source_plus.saturating_sub(report.changed_plus);
    report.records = record_changes;
    Ok(SanitizedDataset {
        dataset: sanitized,
        report,
    })
}

pub fn is_empty_source_value(value: &str) -> bool {
    empty_category(value).is_some()
}

fn empty_category(value: &str) -> Option<EmptyCategory> {
    if value.is_empty() {
        return Some(EmptyCategory::EmptyString);
    }
    if value.chars().all(|ch| ch.is_whitespace()) {
        return Some(EmptyCategory::WhitespaceOnly);
    }
    let trimmed_nuls = value.trim_matches('\0');
    if trimmed_nuls.is_empty() {
        return Some(EmptyCategory::PaddingOnly);
    }
    if value.chars().all(|ch| ch == '\0' || ch.is_whitespace()) {
        return Some(EmptyCategory::PaddingOnly);
    }
    None
}

fn is_candidate_product(row: &SourceRow) -> bool {
    let Some(plu_code) = normalized_numeric_field(row, SourceField::PluCode.source_columns())
    else {
        return false;
    };
    if plu_code.parse::<u64>().ok().is_none_or(|value| value == 0) {
        return false;
    }
    find_value(
        row,
        &[
            "Name 1",
            "Name",
            "ProductName",
            "CommodityName",
            "PLUName",
            "name",
        ],
    )
    .is_some_and(|value| !is_empty_source_value(value))
}

fn rule_value(
    row: &SourceRow,
    profile: &SanitizationProfile,
    field: TargetField,
    action: RuleAction,
    value: Option<&str>,
    source_field: Option<SourceField>,
    normalization: Option<CopyNormalization>,
) -> Result<String, AppError> {
    match action {
        RuleAction::SetConstant => Ok(value.unwrap_or_default().trim().to_string()),
        RuleAction::CopyField => {
            if source_field != Some(SourceField::PluCode)
                || normalization != Some(CopyNormalization::NumericNoPadding)
            {
                return Err(AppError::Config(format!(
                    "profile '{}' has unsupported copy_field rule for {}",
                    profile.profile_name,
                    field.as_str()
                )));
            }
            normalized_numeric_field(row, SourceField::PluCode.source_columns()).ok_or_else(|| {
                AppError::Config(format!(
                    "copy_field rule for '{}' could not read a numeric PLU code",
                    field.as_str()
                ))
            })
        }
    }
}

fn applied_rule_text(action: RuleAction, source_field: Option<SourceField>) -> String {
    match (action, source_field) {
        (RuleAction::SetConstant, _) => "set_constant".to_string(),
        (RuleAction::CopyField, Some(SourceField::PluCode)) => "copy_field:plu_code".to_string(),
        (RuleAction::CopyField, None) => "copy_field".to_string(),
    }
}

fn normalized_numeric_field(row: &SourceRow, columns: &[&str]) -> Option<String> {
    let value = find_value(row, columns)?.trim();
    if value.is_empty() || !value.chars().all(|ch| ch.is_ascii_digit()) {
        return None;
    }
    value.parse::<u64>().ok().map(|value| value.to_string())
}

fn existing_column<'a>(row: &'a SourceRow, columns: &[&'a str]) -> Option<&'a str> {
    columns
        .iter()
        .find(|column| row.values.contains_key(**column))
        .copied()
}

fn find_value<'a>(row: &'a SourceRow, columns: &[&str]) -> Option<&'a str> {
    columns.iter().find_map(|column| row.get(column))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::MappingConfig;
    use crate::sanitization::profile::{RuleCondition, SanitizationRule, SanitizationSafety};
    use crate::source::mapping::normalize_dataset;
    use crate::validation::validator::{valid_plu_candidates, validate_plus};

    fn starsky_profile() -> SanitizationProfile {
        SanitizationProfile {
            profile_version: 1,
            profile_name: "starsky".to_string(),
            description: String::new(),
            safety: SanitizationSafety {
                fill_empty_only: true,
                preserve_nonempty_values: true,
                reject_invalid_results: true,
            },
            rules: vec![
                SanitizationRule {
                    field: TargetField::Department,
                    when: RuleCondition::Empty,
                    action: RuleAction::SetConstant,
                    value: Some("1".to_string()),
                    source_field: None,
                    normalization: None,
                },
                SanitizationRule {
                    field: TargetField::Barcode,
                    when: RuleCondition::Empty,
                    action: RuleAction::CopyField,
                    value: None,
                    source_field: Some(SourceField::PluCode),
                    normalization: Some(CopyNormalization::NumericNoPadding),
                },
                SanitizationRule {
                    field: TargetField::BarcodeFormat,
                    when: RuleCondition::Empty,
                    action: RuleAction::SetConstant,
                    value: Some("05".to_string()),
                    source_field: None,
                    normalization: None,
                },
                SanitizationRule {
                    field: TargetField::PrintFormatCode,
                    when: RuleCondition::Empty,
                    action: RuleAction::SetConstant,
                    value: Some("00".to_string()),
                    source_field: None,
                    normalization: None,
                },
            ],
        }
    }

    fn row(
        plu: &str,
        department: &str,
        barcode: &str,
        format: &str,
        print: &str,
        name: &str,
    ) -> SourceRow {
        SourceRow {
            table: "Pludata".to_string(),
            values: BTreeMap::from([
                ("Plucode".to_string(), plu.to_string()),
                ("Department".to_string(), department.to_string()),
                ("Barcode".to_string(), barcode.to_string()),
                ("Barcode Format".to_string(), format.to_string()),
                ("Print Format Code".to_string(), print.to_string()),
                ("Name 1".to_string(), name.to_string()),
                ("Price".to_string(), "1.00".to_string()),
                ("Category".to_string(), "0".to_string()),
                ("Flag Data".to_string(), "02".to_string()),
                ("Main Group Code".to_string(), "997".to_string()),
            ]),
        }
    }

    #[test]
    fn empty_detection_matches_supported_values() {
        assert!(is_empty_source_value(""));
        assert!(is_empty_source_value("   "));
        assert!(is_empty_source_value("\0\0"));
        assert!(is_empty_source_value(" \0 "));
        assert!(!is_empty_source_value("0"));
        assert!(!is_empty_source_value("abc"));
    }

    #[test]
    fn starsky_rules_fill_empty_fields_and_preserve_nonempty_values() {
        let dataset = SourceDataset {
            plu_rows: vec![
                row("1", "", "", "", "", "Apples"),
                row("2", "0002", "999", "04", "12", "Bread"),
            ],
            ingredient_rows: Vec::new(),
            nutrition_rows: Vec::new(),
        };

        let sanitized = apply_profile(&dataset, &starsky_profile()).expect("sanitize");

        let first = &sanitized.dataset.plu_rows[0].values;
        assert_eq!(first.get("Department").map(String::as_str), Some("1"));
        assert_eq!(first.get("Barcode").map(String::as_str), Some("1"));
        assert_eq!(first.get("Barcode Format").map(String::as_str), Some("05"));
        assert_eq!(
            first.get("Print Format Code").map(String::as_str),
            Some("00")
        );
        let second = &sanitized.dataset.plu_rows[1].values;
        assert_eq!(second.get("Department").map(String::as_str), Some("0002"));
        assert_eq!(second.get("Barcode").map(String::as_str), Some("999"));
        assert_eq!(sanitized.report.nonempty_values_changed, 0);
    }

    #[test]
    fn copied_barcode_uses_numeric_plu_without_padding() {
        for (plu, expected) in [("0001", "1"), ("37", "37"), ("6020", "6020")] {
            let dataset = SourceDataset {
                plu_rows: vec![row(plu, "1", "", "05", "00", "Apples")],
                ingredient_rows: Vec::new(),
                nutrition_rows: Vec::new(),
            };
            let sanitized = apply_profile(&dataset, &starsky_profile()).expect("sanitize");
            assert_eq!(
                sanitized.dataset.plu_rows[0]
                    .values
                    .get("Barcode")
                    .map(String::as_str),
                Some(expected)
            );
        }
    }

    #[test]
    fn placeholder_plu_zero_is_not_recovered() {
        let dataset = SourceDataset {
            plu_rows: vec![row("0", "", "", "", "", "")],
            ingredient_rows: Vec::new(),
            nutrition_rows: Vec::new(),
        };

        let sanitized = apply_profile(&dataset, &starsky_profile()).expect("sanitize");

        assert_eq!(
            sanitized.dataset.plu_rows[0]
                .values
                .get("Department")
                .map(String::as_str),
            Some("")
        );
        assert_eq!(sanitized.report.placeholder_plus, 1);
        assert_eq!(sanitized.report.changed_plus, 0);
    }

    #[test]
    fn no_product_is_deleted() {
        let dataset = SourceDataset {
            plu_rows: vec![
                row("1", "", "", "", "", "Apples"),
                row("0", "", "", "", "", ""),
            ],
            ingredient_rows: Vec::new(),
            nutrition_rows: Vec::new(),
        };

        let sanitized = apply_profile(&dataset, &starsky_profile()).expect("sanitize");

        assert_eq!(sanitized.dataset.plu_rows.len(), 2);
    }

    #[test]
    fn product_missing_only_department_can_be_recovered_before_validation() {
        let dataset = SourceDataset {
            plu_rows: vec![row("1", "", "1", "05", "00", "Apples")],
            ingredient_rows: Vec::new(),
            nutrition_rows: Vec::new(),
        };
        let before = normalize_dataset(&dataset, &MappingConfig::default(), 1).expect("before");
        assert!(before.plus.is_empty());

        let sanitized = apply_profile(&dataset, &starsky_profile()).expect("sanitize");
        let after =
            normalize_dataset(&sanitized.dataset, &MappingConfig::default(), 1).expect("after");
        let validation = validate_plus(&after.plus);
        let valid = valid_plu_candidates(&after.plus, &validation);

        assert_eq!(valid.len(), 1);
        assert_eq!(valid[0].department_number, Some(1));
    }

    #[test]
    fn product_missing_only_barcode_can_be_recovered_before_validation() {
        let dataset = SourceDataset {
            plu_rows: vec![row("37", "1", "", "05", "00", "Apples")],
            ingredient_rows: Vec::new(),
            nutrition_rows: Vec::new(),
        };
        let before = normalize_dataset(&dataset, &MappingConfig::default(), 1).expect("before");
        assert!(before.plus.is_empty());

        let sanitized = apply_profile(&dataset, &starsky_profile()).expect("sanitize");
        let after =
            normalize_dataset(&sanitized.dataset, &MappingConfig::default(), 1).expect("after");
        let validation = validate_plus(&after.plus);
        let valid = valid_plu_candidates(&after.plus, &validation);

        assert_eq!(valid.len(), 1);
        assert_eq!(valid[0].source_barcode.as_deref(), Some("37"));
    }

    #[test]
    fn product_missing_multiple_configured_fields_can_be_recovered() {
        let dataset = SourceDataset {
            plu_rows: vec![row("6020", "", "", "", "", "Apples")],
            ingredient_rows: Vec::new(),
            nutrition_rows: Vec::new(),
        };

        let sanitized = apply_profile(&dataset, &starsky_profile()).expect("sanitize");
        let after =
            normalize_dataset(&sanitized.dataset, &MappingConfig::default(), 1).expect("after");
        let validation = validate_plus(&after.plus);
        let valid = valid_plu_candidates(&after.plus, &validation);

        assert_eq!(valid.len(), 1);
        assert_eq!(sanitized.report.changed_plus, 1);
        assert_eq!(sanitized.report.field_changes.get("department"), Some(&1));
        assert_eq!(sanitized.report.field_changes.get("barcode"), Some(&1));
        assert_eq!(
            sanitized.report.field_changes.get("barcode_format"),
            Some(&1)
        );
        assert_eq!(
            sanitized.report.field_changes.get("print_format_code"),
            Some(&1)
        );
    }

    #[test]
    fn unrelated_fatal_validation_error_remains_invalid() {
        let mut bad_price = row("1", "", "", "", "", "Apples");
        bad_price
            .values
            .insert("Price".to_string(), "-1.00".to_string());
        let dataset = SourceDataset {
            plu_rows: vec![bad_price],
            ingredient_rows: Vec::new(),
            nutrition_rows: Vec::new(),
        };

        let sanitized = apply_profile(&dataset, &starsky_profile()).expect("sanitize");
        let after =
            normalize_dataset(&sanitized.dataset, &MappingConfig::default(), 1).expect("after");
        let validation = validate_plus(&after.plus);
        let valid = valid_plu_candidates(&after.plus, &validation);

        assert!(valid.is_empty());
        assert!(validation.issues.iter().any(|issue| issue.field == "price"));
    }
}
