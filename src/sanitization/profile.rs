use std::collections::HashSet;
use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::AppError;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SanitizationProfile {
    pub profile_version: u32,
    pub profile_name: String,
    #[serde(default)]
    pub description: String,
    pub safety: SanitizationSafety,
    #[serde(default)]
    pub rules: Vec<SanitizationRule>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selling_date_term: Option<SellingDateTermRule>,
    #[serde(default)]
    pub nutrition_remap: Vec<NutritionRemapRule>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SanitizationSafety {
    pub fill_empty_only: bool,
    pub preserve_nonempty_values: bool,
    pub reject_invalid_results: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SanitizationRule {
    pub field: TargetField,
    pub when: RuleCondition,
    pub action: RuleAction,
    #[serde(default)]
    pub value: Option<String>,
    #[serde(default)]
    pub source_field: Option<SourceField>,
    #[serde(default)]
    pub normalization: Option<CopyNormalization>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SellingDateTermRule {
    pub enabled: bool,
    pub allow_zero: bool,
    pub minimum: u32,
    pub maximum: u32,
    pub invalid_value: u32,
    pub empty_value: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct NutritionRemapRule {
    pub source_field: String,
    pub nutrient: String,
    pub value_role: NutritionValueRole,
    #[serde(default)]
    pub suppress_from_ingredients: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum NutritionValueRole {
    Amount,
    Percent,
}

impl NutritionValueRole {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Amount => "amount",
            Self::Percent => "percent",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum TargetField {
    Department,
    Barcode,
    BarcodeFormat,
    PrintFormatCode,
}

impl TargetField {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Department => "department",
            Self::Barcode => "barcode",
            Self::BarcodeFormat => "barcode_format",
            Self::PrintFormatCode => "print_format_code",
        }
    }

    pub fn source_columns(self) -> &'static [&'static str] {
        match self {
            Self::Department => &["Department", "DeptNo", "DepartmentNo", "DEPT"],
            Self::Barcode => &["Barcode", "BarCode", "JAN", "UPC", "barcode"],
            Self::BarcodeFormat => &["Barcode Format", "BARCODE_FORMAT", "BarcodeFormat"],
            Self::PrintFormatCode => &[
                "PRINT FORMAT CODE",
                "PRINT_FORMAT_CODE",
                "Print Format Code",
            ],
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RuleCondition {
    Empty,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RuleAction {
    SetConstant,
    CopyField,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SourceField {
    PluCode,
}

impl SourceField {
    pub fn source_columns(self) -> &'static [&'static str] {
        match self {
            Self::PluCode => &["Plucode", "PLUNo", "PluNo", "PLU", "PLU_NO", "plu_number"],
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CopyNormalization {
    NumericNoPadding,
}

pub fn load_profile_from_safe_path(path: &Path) -> Result<SanitizationProfile, AppError> {
    validate_profile_path(path)?;
    let contents = fs::read_to_string(path)
        .map_err(|err| AppError::Config(format!("failed to read sanitization profile: {err}")))?;
    let profile: SanitizationProfile = toml::from_str(&contents)
        .map_err(|err| AppError::Config(format!("invalid sanitization profile TOML: {err}")))?;
    profile.validate()?;
    Ok(profile)
}

pub fn validate_profile_path(path: &Path) -> Result<(), AppError> {
    let current_dir = std::env::current_dir().map_err(|err| {
        AppError::Config(format!("failed to resolve deployment directory: {err}"))
    })?;
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        current_dir.join(path)
    };
    let parent = path.parent().ok_or_else(|| {
        AppError::Config("sanitization profile path must have a parent directory".to_string())
    })?;
    let canonical_parent = parent.canonicalize().map_err(|err| {
        AppError::Config(format!(
            "failed to resolve sanitization profile directory '{}': {err}",
            parent.display()
        ))
    })?;
    let canonical_root = current_dir.canonicalize().map_err(|err| {
        AppError::Config(format!("failed to resolve deployment directory: {err}"))
    })?;
    if !canonical_parent.starts_with(&canonical_root) {
        return Err(AppError::Config(
            "Sanitization profiles must be located inside the deployment directory.".to_string(),
        ));
    }
    let checked =
        canonical_parent.join(path.file_name().ok_or_else(|| {
            AppError::Config("sanitization profile filename is missing".to_string())
        })?);
    let metadata = fs::symlink_metadata(&checked).map_err(|err| {
        AppError::Config(format!(
            "failed to inspect sanitization profile '{}': {err}",
            checked.display()
        ))
    })?;
    if metadata.file_type().is_symlink() {
        return Err(AppError::Config(
            "sanitization profile must be a regular file, not a symbolic link".to_string(),
        ));
    }
    if !metadata.is_file() {
        return Err(AppError::Config(
            "sanitization profile must be a regular file".to_string(),
        ));
    }
    Ok(())
}

impl SanitizationProfile {
    pub fn validate(&self) -> Result<(), AppError> {
        if self.profile_version != 1 {
            return Err(AppError::Config(format!(
                "unsupported sanitization profile version {}",
                self.profile_version
            )));
        }
        if self.profile_name.trim().is_empty() {
            return Err(AppError::Config(
                "sanitization profile_name must not be empty".to_string(),
            ));
        }
        if !self.safety.fill_empty_only {
            return Err(AppError::Config(
                "sanitization profiles must keep fill_empty_only enabled".to_string(),
            ));
        }
        if !self.safety.preserve_nonempty_values {
            return Err(AppError::Config(
                "sanitization profiles must preserve nonempty values".to_string(),
            ));
        }
        let mut fields = HashSet::new();
        for rule in &self.rules {
            if !fields.insert(rule.field) {
                return Err(AppError::Config(format!(
                    "duplicate sanitization rule for field {}",
                    rule.field.as_str()
                )));
            }
            match rule.action {
                RuleAction::SetConstant => validate_constant(rule)?,
                RuleAction::CopyField => {
                    if rule.source_field != Some(SourceField::PluCode) {
                        return Err(AppError::Config(
                            "copy_field rules support only source_field = \"plu_code\"".to_string(),
                        ));
                    }
                    if rule.normalization != Some(CopyNormalization::NumericNoPadding) {
                        return Err(AppError::Config(
                            "copy_field rules require normalization = \"numeric_no_padding\""
                                .to_string(),
                        ));
                    }
                }
            }
        }
        if let Some(rule) = &self.selling_date_term {
            validate_selling_date_term_rule(rule)?;
        }
        validate_nutrition_remaps(&self.nutrition_remap)?;
        Ok(())
    }

    pub fn normalized_toml(&self) -> Result<String, AppError> {
        toml::to_string_pretty(self)
            .map_err(|err| AppError::Internal(format!("profile serialization failed: {err}")))
    }
}

fn validate_nutrition_remaps(rules: &[NutritionRemapRule]) -> Result<(), AppError> {
    let mut source_fields = HashSet::new();
    let mut nutrient_roles = HashSet::new();
    for rule in rules {
        let source_field = rule.source_field.trim();
        if source_field.is_empty() {
            return Err(AppError::Config(
                "nutrition_remap.source_field must not be empty".to_string(),
            ));
        }
        validate_nutrition_source_field(source_field)?;
        if !source_fields.insert(source_field.to_ascii_lowercase()) {
            return Err(AppError::Config(format!(
                "duplicate nutrition remap source field '{source_field}'"
            )));
        }
        let nutrient = rule.nutrient.trim();
        if nutrient.is_empty() {
            return Err(AppError::Config(
                "nutrition_remap.nutrient must not be empty".to_string(),
            ));
        }
        let role = rule.value_role.as_str();
        if !nutrient_roles.insert((nutrient.to_ascii_lowercase(), role)) {
            return Err(AppError::Config(format!(
                "duplicate nutrition remap for nutrient '{nutrient}' role '{role}'"
            )));
        }
    }
    Ok(())
}

fn validate_nutrition_source_field(source_field: &str) -> Result<(), AppError> {
    if matches!(source_field, "Calcium" | "Iron") {
        return Ok(());
    }
    let Some(index) = source_field.strip_prefix("Ing Name ") else {
        return Err(AppError::Config(format!(
            "unsupported nutrition_remap.source_field '{source_field}'; expected Ing Name 1..99, Calcium, or Iron"
        )));
    };
    let index = index.parse::<u8>().map_err(|err| {
        AppError::Config(format!(
            "unsupported nutrition_remap.source_field '{source_field}'; expected Ing Name 1..99, Calcium, or Iron: {err}"
        ))
    })?;
    if (1..=99).contains(&index) {
        Ok(())
    } else {
        Err(AppError::Config(format!(
            "unsupported nutrition_remap.source_field '{source_field}'; expected Ing Name 1..99, Calcium, or Iron"
        )))
    }
}

fn validate_selling_date_term_rule(rule: &SellingDateTermRule) -> Result<(), AppError> {
    if rule.minimum == 0 {
        return Err(AppError::Config(
            "selling_date_term.minimum must be greater than zero".to_string(),
        ));
    }
    if rule.maximum < rule.minimum {
        return Err(AppError::Config(
            "selling_date_term.maximum must be greater than or equal to minimum".to_string(),
        ));
    }
    for (field, value) in [
        ("empty_value", rule.empty_value),
        ("invalid_value", rule.invalid_value),
    ] {
        if !selling_date_result_allowed(rule, value) {
            return Err(AppError::Config(format!(
                "selling_date_term.{field} must be 0 when allow_zero is true or in the configured allowed range"
            )));
        }
    }
    Ok(())
}

fn selling_date_result_allowed(rule: &SellingDateTermRule, value: u32) -> bool {
    (rule.allow_zero && value == 0) || (rule.minimum..=rule.maximum).contains(&value)
}

fn validate_constant(rule: &SanitizationRule) -> Result<(), AppError> {
    let value = rule.value.as_deref().unwrap_or("").trim();
    if value.is_empty() {
        return Err(AppError::Config(format!(
            "set_constant rule for {} requires a nonempty value",
            rule.field.as_str()
        )));
    }
    if rule.source_field.is_some() {
        return Err(AppError::Config(
            "set_constant rules must not specify source_field".to_string(),
        ));
    }
    match rule.field {
        TargetField::Department => {
            let department = value.parse::<u32>().map_err(|err| {
                AppError::Config(format!("invalid department constant '{value}': {err}"))
            })?;
            if department == 0 {
                return Err(AppError::Config(
                    "department constant must be greater than zero".to_string(),
                ));
            }
        }
        TargetField::Barcode => {
            if !value.chars().all(|ch| ch.is_ascii_digit()) {
                return Err(AppError::Config(
                    "barcode constants must contain only digits".to_string(),
                ));
            }
        }
        TargetField::BarcodeFormat | TargetField::PrintFormatCode => {
            if !value.chars().all(|ch| ch.is_ascii_digit()) {
                return Err(AppError::Config(format!(
                    "{} constants must contain only digits",
                    rule.field.as_str()
                )));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_profile() -> SanitizationProfile {
        SanitizationProfile {
            profile_version: 1,
            profile_name: "test".to_string(),
            description: String::new(),
            safety: SanitizationSafety {
                fill_empty_only: true,
                preserve_nonempty_values: true,
                reject_invalid_results: true,
            },
            rules: vec![SanitizationRule {
                field: TargetField::Department,
                when: RuleCondition::Empty,
                action: RuleAction::SetConstant,
                value: Some("1".to_string()),
                source_field: None,
                normalization: None,
            }],
            selling_date_term: None,
            nutrition_remap: Vec::new(),
        }
    }

    #[test]
    fn valid_profile_version_one_parses() {
        let toml = r#"
profile_version = 1
profile_name = "starsky"

[safety]
fill_empty_only = true
preserve_nonempty_values = true
reject_invalid_results = true

[[rules]]
field = "department"
when = "empty"
action = "set_constant"
value = "1"
"#;
        let profile: SanitizationProfile = toml::from_str(toml).expect("parse");
        profile.validate().expect("valid");
        assert!(profile.selling_date_term.is_none());
        assert!(profile.nutrition_remap.is_empty());
    }

    #[test]
    fn nutrition_remap_rules_parse_and_validate() {
        let toml = r#"
profile_version = 1
profile_name = "bigway"

[safety]
fill_empty_only = true
preserve_nonempty_values = true
reject_invalid_results = true

[[nutrition_remap]]
source_field = "Calcium"
nutrient = "Calcium"
value_role = "percent"
suppress_from_ingredients = false

[[nutrition_remap]]
source_field = "Ing Name 95"
nutrient = "Calcium"
value_role = "amount"
suppress_from_ingredients = true

[[nutrition_remap]]
source_field = "Iron"
nutrient = "Iron"
value_role = "amount"
suppress_from_ingredients = false

[[nutrition_remap]]
source_field = "Ing Name 99"
nutrient = "Potassium"
value_role = "percent"
suppress_from_ingredients = true
"#;
        let profile: SanitizationProfile = toml::from_str(toml).expect("parse");
        profile.validate().expect("valid");

        assert_eq!(profile.nutrition_remap.len(), 4);
        assert_eq!(
            profile.nutrition_remap[0].value_role,
            NutritionValueRole::Percent
        );
    }

    #[test]
    fn duplicate_nutrition_source_fields_fail() {
        let mut profile = base_profile();
        profile.nutrition_remap = vec![
            NutritionRemapRule {
                source_field: "Ing Name 96".to_string(),
                nutrient: "Iron".to_string(),
                value_role: NutritionValueRole::Amount,
                suppress_from_ingredients: true,
            },
            NutritionRemapRule {
                source_field: " ing name 96 ".to_string(),
                nutrient: "Sugar".to_string(),
                value_role: NutritionValueRole::Amount,
                suppress_from_ingredients: true,
            },
        ];

        assert!(profile.validate().is_err());
    }

    #[test]
    fn empty_nutrition_nutrient_fails() {
        let mut profile = base_profile();
        profile.nutrition_remap = vec![NutritionRemapRule {
            source_field: "Ing Name 96".to_string(),
            nutrient: " ".to_string(),
            value_role: NutritionValueRole::Amount,
            suppress_from_ingredients: true,
        }];

        assert!(profile.validate().is_err());
    }

    #[test]
    fn conflicting_nutrition_nutrient_role_fails() {
        let mut profile = base_profile();
        profile.nutrition_remap = vec![
            NutritionRemapRule {
                source_field: "Ing Name 96".to_string(),
                nutrient: "Potassium".to_string(),
                value_role: NutritionValueRole::Percent,
                suppress_from_ingredients: true,
            },
            NutritionRemapRule {
                source_field: "Ing Name 97".to_string(),
                nutrient: " potassium ".to_string(),
                value_role: NutritionValueRole::Percent,
                suppress_from_ingredients: true,
            },
        ];

        assert!(profile.validate().is_err());
    }

    #[test]
    fn unsupported_nutrition_value_role_fails_toml_parse() {
        let toml = r#"
profile_version = 1
profile_name = "bad"

[safety]
fill_empty_only = true
preserve_nonempty_values = true
reject_invalid_results = true

[[nutrition_remap]]
source_field = "Ing Name 96"
nutrient = "Iron"
value_role = "daily"
"#;
        assert!(toml::from_str::<SanitizationProfile>(toml).is_err());
    }

    #[test]
    fn invalid_nutrition_source_field_fails_validation() {
        let mut profile = base_profile();
        profile.nutrition_remap = vec![NutritionRemapRule {
            source_field: "Ing Name 100".to_string(),
            nutrient: "Iron".to_string(),
            value_role: NutritionValueRole::Amount,
            suppress_from_ingredients: true,
        }];

        assert!(profile.validate().is_err());

        profile.nutrition_remap[0].source_field = "Sugar".to_string();
        assert!(profile.validate().is_err());
    }

    #[test]
    fn selling_date_term_section_parses_and_serializes() {
        let toml = r#"
profile_version = 1
profile_name = "starsky"

[safety]
fill_empty_only = true
preserve_nonempty_values = true
reject_invalid_results = true

[selling_date_term]
enabled = true
allow_zero = true
minimum = 1
maximum = 999
invalid_value = 0
empty_value = 0
"#;
        let profile: SanitizationProfile = toml::from_str(toml).expect("parse");
        profile.validate().expect("valid");
        let normalized = profile.normalized_toml().expect("toml");
        assert!(normalized.contains("[selling_date_term]"));
        assert!(normalized.contains("maximum = 999"));
    }

    #[test]
    fn normalized_profile_changes_when_selling_date_rule_changes() {
        let mut profile = base_profile();
        profile.selling_date_term = Some(SellingDateTermRule {
            enabled: true,
            allow_zero: true,
            minimum: 1,
            maximum: 999,
            invalid_value: 0,
            empty_value: 0,
        });
        let original = profile.normalized_toml().expect("toml");

        profile.selling_date_term.as_mut().expect("rule").maximum = 365;
        let changed = profile.normalized_toml().expect("toml");

        assert_ne!(
            crate::sanitization::report::sha256_text(&original),
            crate::sanitization::report::sha256_text(&changed)
        );
    }

    #[test]
    fn invalid_selling_date_term_configuration_is_rejected() {
        let mut profile = base_profile();
        profile.selling_date_term = Some(SellingDateTermRule {
            enabled: true,
            allow_zero: false,
            minimum: 1,
            maximum: 999,
            invalid_value: 0,
            empty_value: 0,
        });

        let err = profile.validate().expect_err("invalid");

        assert!(err.to_string().contains("selling_date_term"));
    }

    #[test]
    fn unknown_profile_setting_is_rejected() {
        let toml = r#"
profile_version = 1
profile_name = "bad"
unexpected = true

[safety]
fill_empty_only = true
preserve_nonempty_values = true
reject_invalid_results = true
"#;
        assert!(toml::from_str::<SanitizationProfile>(toml).is_err());
    }

    #[test]
    fn missing_profile_name_fails() {
        let mut profile = base_profile();
        profile.profile_name = " ".to_string();
        assert!(profile.validate().is_err());
    }

    #[test]
    fn unsupported_version_fails() {
        let mut profile = base_profile();
        profile.profile_version = 2;
        assert!(profile.validate().is_err());
    }

    #[test]
    fn unsupported_target_field_fails_toml_parse() {
        let toml = r#"
profile_version = 1
profile_name = "bad"
[safety]
fill_empty_only = true
preserve_nonempty_values = true
reject_invalid_results = true
[[rules]]
field = "name"
when = "empty"
action = "set_constant"
value = "x"
"#;
        assert!(toml::from_str::<SanitizationProfile>(toml).is_err());
    }

    #[test]
    fn unsupported_action_fails_toml_parse() {
        let toml = r#"
profile_version = 1
profile_name = "bad"
[safety]
fill_empty_only = true
preserve_nonempty_values = true
reject_invalid_results = true
[[rules]]
field = "department"
when = "empty"
action = "clear"
value = "1"
"#;
        assert!(toml::from_str::<SanitizationProfile>(toml).is_err());
    }

    #[test]
    fn unsupported_source_field_fails_toml_parse() {
        let toml = r#"
profile_version = 1
profile_name = "bad"
[safety]
fill_empty_only = true
preserve_nonempty_values = true
reject_invalid_results = true
[[rules]]
field = "barcode"
when = "empty"
action = "copy_field"
source_field = "name"
normalization = "numeric_no_padding"
"#;
        assert!(toml::from_str::<SanitizationProfile>(toml).is_err());
    }

    #[test]
    fn duplicate_target_rules_fail() {
        let mut profile = base_profile();
        profile.rules.push(profile.rules[0].clone());
        assert!(profile.validate().is_err());
    }

    #[test]
    fn missing_constant_fails() {
        let mut profile = base_profile();
        profile.rules[0].value = None;
        assert!(profile.validate().is_err());
    }

    #[test]
    fn invalid_constants_fail() {
        for (field, value) in [
            (TargetField::Department, "0"),
            (TargetField::BarcodeFormat, "xx"),
            (TargetField::PrintFormatCode, "xx"),
        ] {
            let mut profile = base_profile();
            profile.rules[0].field = field;
            profile.rules[0].value = Some(value.to_string());
            assert!(profile.validate().is_err(), "{field:?}");
        }
    }

    #[test]
    fn profile_permitting_overwrites_fails() {
        let mut profile = base_profile();
        profile.safety.fill_empty_only = false;
        assert!(profile.validate().is_err());
        profile.safety.fill_empty_only = true;
        profile.safety.preserve_nonempty_values = false;
        assert!(profile.validate().is_err());
    }
}
