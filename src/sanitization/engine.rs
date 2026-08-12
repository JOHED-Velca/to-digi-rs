use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

use crate::error::AppError;
use crate::sanitization::profile::{
    CopyNormalization, RuleAction, SanitizationProfile, SellingDateTermRule, SourceField,
    TargetField,
};
use crate::source::{SourceDataset, SourceRow};

const BEST_BEFORE_COLUMNS: &[&str] = &["BEST BEFORE", "BEST_BEFORE", "Best Before"];

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
    pub field_summaries: BTreeMap<String, SanitizedFieldSummary>,
    pub records: Vec<SanitizedRecordChange>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct SanitizedFieldSummary {
    pub changed: usize,
    pub empty_defaulted: usize,
    pub invalid_nonempty_corrected: usize,
    pub valid_preserved: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct SanitizedRecordChange {
    pub plu_number: u64,
    pub fields: Vec<SanitizedFieldChange>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SanitizedFieldChange {
    pub field: String,
    pub original_value: String,
    pub original_empty_category: EmptyCategory,
    pub applied_rule: String,
    pub sanitized_value: String,
    pub reason: SanitizedChangeReason,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EmptyCategory {
    EmptyString,
    WhitespaceOnly,
    PaddingOnly,
    NonEmpty,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SanitizedChangeReason {
    EmptyDefaulted,
    OutsideAllowedRange,
    MalformedValue,
}

impl SanitizedChangeReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::EmptyDefaulted => "empty_defaulted",
            Self::OutsideAllowedRange => "outside_allowed_range",
            Self::MalformedValue => "malformed_value",
        }
    }
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
            let summary = report
                .field_summaries
                .entry(rule.field.as_str().to_string())
                .or_default();
            summary.changed += 1;
            summary.empty_defaulted += 1;
            changes.push(SanitizedFieldChange {
                field: rule.field.as_str().to_string(),
                original_value: original,
                original_empty_category: empty_category,
                applied_rule: applied_rule_text(rule.action, rule.source_field),
                sanitized_value,
                reason: SanitizedChangeReason::EmptyDefaulted,
            });
        }
        if let Some(rule) = profile
            .selling_date_term
            .as_ref()
            .filter(|rule| rule.enabled)
        {
            if let Some(change) = apply_selling_date_term_rule(row, rule, &mut report)? {
                changes.push(change);
            }
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

fn apply_selling_date_term_rule(
    row: &mut SourceRow,
    rule: &SellingDateTermRule,
    report: &mut SanitizationEngineReport,
) -> Result<Option<SanitizedFieldChange>, AppError> {
    let Some(column) = existing_column(row, BEST_BEFORE_COLUMNS) else {
        return Err(AppError::Config(
            "selling_date_term profile rule is enabled but no Best Before source column exists"
                .to_string(),
        ));
    };
    let original = row.values.get(column).cloned().unwrap_or_default();
    let cleaned = clean_mdb_value(&original);
    let summary = report
        .field_summaries
        .entry("selling_date_term".to_string())
        .or_default();
    let outcome = selling_date_term_outcome(&cleaned, rule);
    match outcome {
        SellingDateTermOutcome::Preserve => {
            summary.valid_preserved += 1;
            if cleaned != original {
                row.values.insert(column.to_string(), cleaned);
            }
            Ok(None)
        }
        SellingDateTermOutcome::Sanitize { value, reason } => {
            let original_empty_category =
                empty_category(&original).unwrap_or(EmptyCategory::NonEmpty);
            if original_empty_category == EmptyCategory::NonEmpty {
                report.nonempty_values_changed += 1;
                summary.invalid_nonempty_corrected += 1;
            } else {
                summary.empty_defaulted += 1;
            }
            summary.changed += 1;
            *report
                .field_changes
                .entry("selling_date_term".to_string())
                .or_default() += 1;
            let sanitized_value = value.to_string();
            row.values
                .insert(column.to_string(), sanitized_value.clone());
            Ok(Some(SanitizedFieldChange {
                field: "selling_date_term".to_string(),
                original_value: original,
                original_empty_category,
                applied_rule: "normalize_range".to_string(),
                sanitized_value,
                reason,
            }))
        }
    }
}

enum SellingDateTermOutcome {
    Preserve,
    Sanitize {
        value: u32,
        reason: SanitizedChangeReason,
    },
}

fn selling_date_term_outcome(value: &str, rule: &SellingDateTermRule) -> SellingDateTermOutcome {
    if value.is_empty() {
        return SellingDateTermOutcome::Sanitize {
            value: rule.empty_value,
            reason: SanitizedChangeReason::EmptyDefaulted,
        };
    }
    let Ok(parsed) = value.parse::<i64>() else {
        return SellingDateTermOutcome::Sanitize {
            value: rule.invalid_value,
            reason: SanitizedChangeReason::MalformedValue,
        };
    };
    if parsed == 0 && rule.allow_zero {
        return SellingDateTermOutcome::Preserve;
    }
    if parsed >= i64::from(rule.minimum) && parsed <= i64::from(rule.maximum) {
        return SellingDateTermOutcome::Preserve;
    }
    SellingDateTermOutcome::Sanitize {
        value: rule.invalid_value,
        reason: SanitizedChangeReason::OutsideAllowedRange,
    }
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

fn clean_mdb_value(value: &str) -> String {
    value
        .trim_matches(|ch| ch == '\0' || char::is_whitespace(ch))
        .to_string()
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
    use crate::digiweb::payload::DigiwebPluPayload;
    use crate::sanitization::profile::{
        RuleCondition, SanitizationRule, SanitizationSafety, SellingDateTermRule,
    };
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
            selling_date_term: Some(SellingDateTermRule {
                enabled: true,
                allow_zero: true,
                minimum: 1,
                maximum: 999,
                invalid_value: 0,
                empty_value: 0,
            }),
            nutrition_remap: Vec::new(),
        }
    }

    fn profile_without_selling_date_rule() -> SanitizationProfile {
        let mut profile = starsky_profile();
        profile.selling_date_term = None;
        profile
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
                ("Best Before".to_string(), "0".to_string()),
            ]),
        }
    }

    fn row_with_best_before(plu: &str, best_before: &str) -> SourceRow {
        let mut row = row(plu, "1", plu, "05", "00", "Apples");
        row.values
            .insert("Best Before".to_string(), best_before.to_string());
        row
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
    fn selling_date_term_rule_normalizes_configured_values() {
        for (raw, expected, changed, reason) in [
            ("", "0", true, Some(SanitizedChangeReason::EmptyDefaulted)),
            (
                "   ",
                "0",
                true,
                Some(SanitizedChangeReason::EmptyDefaulted),
            ),
            (
                "\0\0",
                "0",
                true,
                Some(SanitizedChangeReason::EmptyDefaulted),
            ),
            ("0", "0", false, None),
            ("1", "1", false, None),
            ("365", "365", false, None),
            ("999", "999", false, None),
            (
                "1000",
                "0",
                true,
                Some(SanitizedChangeReason::OutsideAllowedRange),
            ),
            (
                "6851",
                "0",
                true,
                Some(SanitizedChangeReason::OutsideAllowedRange),
            ),
            (
                "-1",
                "0",
                true,
                Some(SanitizedChangeReason::OutsideAllowedRange),
            ),
            (
                "abc",
                "0",
                true,
                Some(SanitizedChangeReason::MalformedValue),
            ),
            (
                " 6851 ",
                "0",
                true,
                Some(SanitizedChangeReason::OutsideAllowedRange),
            ),
        ] {
            let dataset = SourceDataset {
                plu_rows: vec![row_with_best_before("6252", raw)],
                ingredient_rows: Vec::new(),
                nutrition_rows: Vec::new(),
            };

            let sanitized = apply_profile(&dataset, &starsky_profile()).expect("sanitize");

            assert_eq!(
                sanitized.dataset.plu_rows[0]
                    .values
                    .get("Best Before")
                    .map(String::as_str),
                Some(expected),
                "raw={raw:?}"
            );
            assert_eq!(
                sanitized.report.changed_plus,
                usize::from(changed),
                "raw={raw:?}"
            );
            if let Some(reason) = reason {
                let change = &sanitized.report.records[0].fields[0];
                assert_eq!(change.field, "selling_date_term");
                assert_eq!(change.original_value, raw);
                assert_eq!(change.sanitized_value, expected);
                assert_eq!(change.reason, reason);
            }
        }
    }

    #[test]
    fn selling_date_term_rule_disabled_leaves_values_untouched() {
        let dataset = SourceDataset {
            plu_rows: vec![row_with_best_before("6252", "6851")],
            ingredient_rows: Vec::new(),
            nutrition_rows: Vec::new(),
        };

        let sanitized =
            apply_profile(&dataset, &profile_without_selling_date_rule()).expect("sanitize");

        assert_eq!(
            sanitized.dataset.plu_rows[0]
                .values
                .get("Best Before")
                .map(String::as_str),
            Some("6851")
        );
        assert_eq!(sanitized.report.changed_plus, 0);
        assert!(sanitized.report.field_summaries.is_empty());
    }

    #[test]
    fn selling_date_term_report_counts_invalid_nonempty_correction() {
        let dataset = SourceDataset {
            plu_rows: vec![
                row_with_best_before("6251", "0"),
                row_with_best_before("6252", "6851"),
                row_with_best_before("6254", "30"),
            ],
            ingredient_rows: Vec::new(),
            nutrition_rows: Vec::new(),
        };

        let sanitized = apply_profile(&dataset, &starsky_profile()).expect("sanitize");
        let summary = sanitized
            .report
            .field_summaries
            .get("selling_date_term")
            .expect("summary");

        assert_eq!(sanitized.report.changed_plus, 1);
        assert_eq!(sanitized.report.nonempty_values_changed, 1);
        assert_eq!(
            sanitized.report.field_changes.get("selling_date_term"),
            Some(&1)
        );
        assert_eq!(summary.changed, 1);
        assert_eq!(summary.empty_defaulted, 0);
        assert_eq!(summary.invalid_nonempty_corrected, 1);
        assert_eq!(summary.valid_preserved, 2);
        assert_eq!(
            sanitized.report.records[0].fields[0].reason,
            SanitizedChangeReason::OutsideAllowedRange
        );
    }

    #[test]
    fn selling_date_term_sanitization_keeps_original_dataset_unchanged() {
        let dataset = SourceDataset {
            plu_rows: vec![row_with_best_before("6252", "6851")],
            ingredient_rows: Vec::new(),
            nutrition_rows: Vec::new(),
        };

        let sanitized = apply_profile(&dataset, &starsky_profile()).expect("sanitize");

        assert_eq!(
            dataset.plu_rows[0]
                .values
                .get("Best Before")
                .map(String::as_str),
            Some("6851")
        );
        assert_eq!(
            sanitized.dataset.plu_rows[0]
                .values
                .get("Best Before")
                .map(String::as_str),
            Some("0")
        );
    }

    #[test]
    fn strict_validation_detects_invalid_selling_date_term_before_submission() {
        let dataset = SourceDataset {
            plu_rows: vec![row_with_best_before("6252", "6851")],
            ingredient_rows: Vec::new(),
            nutrition_rows: Vec::new(),
        };

        let normalized =
            normalize_dataset(&dataset, &MappingConfig::default(), 1).expect("normalize");
        let validation = validate_plus(&normalized.plus);
        let valid = valid_plu_candidates(&normalized.plus, &validation);

        assert!(valid.is_empty());
        assert!(
            validation.issues.iter().any(|issue| {
                issue.plu_number == Some(6252) && issue.field == "selling_date_term"
            })
        );
    }

    #[test]
    fn sanitized_validation_accepts_plu_6252_and_payload_uses_zero_selling_term() {
        let dataset = SourceDataset {
            plu_rows: vec![row_with_best_before("6252", "6851")],
            ingredient_rows: Vec::new(),
            nutrition_rows: Vec::new(),
        };

        let sanitized = apply_profile(&dataset, &starsky_profile()).expect("sanitize");
        let normalized =
            normalize_dataset(&sanitized.dataset, &MappingConfig::default(), 1).expect("normalize");
        let validation = validate_plus(&normalized.plus);
        let valid = valid_plu_candidates(&normalized.plus, &validation);
        let payload =
            DigiwebPluPayload::from_plu(&valid[0], &crate::config::DigiwebConfig::default())
                .expect("payload");
        let json = serde_json::to_value(&payload).expect("json");

        assert_eq!(valid.len(), 1);
        assert_eq!(valid[0].plu_number, 6252);
        assert_eq!(valid[0].selling_date_term, Some(0));
        assert_eq!(
            json.get("plusellingdateterm").and_then(|v| v.as_u64()),
            Some(0)
        );
        assert_eq!(valid[0].selling_date_print, Some(0));
        assert_eq!(valid[0].expiration_days, None);
        assert_eq!(
            json.get("plusellingdateprint").and_then(|v| v.as_u64()),
            Some(0)
        );
        assert!(json.get("pluusingdateterm").is_none());
    }

    #[test]
    fn existing_empty_group_department_barcode_and_print_rules_still_work() {
        let mut source_row = row("6020", "", "", "", "", "Apples");
        source_row
            .values
            .insert("Main Group Code".to_string(), String::new());
        let dataset = SourceDataset {
            plu_rows: vec![source_row],
            ingredient_rows: Vec::new(),
            nutrition_rows: Vec::new(),
        };

        let sanitized = apply_profile(&dataset, &starsky_profile()).expect("sanitize");
        let values = &sanitized.dataset.plu_rows[0].values;
        let normalized =
            normalize_dataset(&sanitized.dataset, &MappingConfig::default(), 1).expect("normalize");

        assert_eq!(values.get("Department").map(String::as_str), Some("1"));
        assert_eq!(values.get("Barcode").map(String::as_str), Some("6020"));
        assert_eq!(values.get("Barcode Format").map(String::as_str), Some("05"));
        assert_eq!(
            values.get("Print Format Code").map(String::as_str),
            Some("00")
        );
        assert_eq!(normalized.plus[0].group_number, Some(997));
        assert!(normalized.plus[0].group_default_applied);
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
