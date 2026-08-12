use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use chrono::Local;
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::error::AppError;
use crate::recovery::SourceIdentity;
use crate::sanitization::engine::{
    SanitizationEngineReport, SanitizedFieldSummary, SanitizedRecordChange,
};
use crate::sanitization::profile::SanitizationProfile;

#[derive(Debug, Clone)]
pub struct SanitizationIntegration {
    pub profile: SanitizationProfile,
    pub profile_sha256: String,
    pub normalized_profile_toml: String,
    pub engine_report: SanitizationEngineReport,
    pub before_valid: usize,
    pub before_invalid: usize,
    pub after_valid: usize,
    pub after_invalid: usize,
    pub recovered_plus: usize,
    pub still_invalid_plus: Vec<u64>,
}

impl SanitizationIntegration {
    pub fn manifest_metadata(
        &self,
        snapshot_name: impl Into<String>,
    ) -> SanitizationManifestMetadata {
        SanitizationManifestMetadata {
            enabled: true,
            profile_name: Some(self.profile.profile_name.clone()),
            profile_version: Some(self.profile.profile_version),
            profile_sha256: Some(self.profile_sha256.clone()),
            profile_snapshot: Some(snapshot_name.into()),
            records_changed: Some(self.engine_report.changed_plus),
            fields_changed: self.engine_report.field_changes.clone(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct SanitizationManifestMetadata {
    pub enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile_version: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile_sha256: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile_snapshot: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub records_changed: Option<usize>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub fields_changed: BTreeMap<String, usize>,
}

#[derive(Debug, Clone)]
pub struct SanitizationReportInput<'a> {
    pub source: SourceIdentity,
    pub profile: &'a SanitizationProfile,
    pub profile_sha256: &'a str,
    pub engine_report: &'a SanitizationEngineReport,
    pub before_valid: usize,
    pub before_invalid: usize,
    pub after_valid: usize,
    pub after_invalid: usize,
    pub recovered_plus: usize,
    pub still_invalid_plus: &'a [u64],
}

#[derive(Debug, Clone, Serialize)]
pub struct SanitizationReport {
    pub schema_version: u32,
    pub application_version: String,
    pub generated_at: String,
    pub source: SourceIdentity,
    pub profile: SanitizationProfileSummary,
    pub summary: SanitizationSummary,
    pub field_changes: BTreeMap<String, usize>,
    pub field_summaries: BTreeMap<String, SanitizedFieldSummary>,
    pub records: Vec<SanitizedReportRecord>,
    pub safety: SanitizationSafetyReport,
}

#[derive(Debug, Clone, Serialize)]
pub struct SanitizationProfileSummary {
    pub name: String,
    pub version: u32,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct SanitizationSummary {
    pub source_plus: usize,
    pub changed_plus: usize,
    pub unchanged_plus: usize,
    pub placeholder_plus: usize,
    pub before_valid_plus: usize,
    pub before_invalid_plus: usize,
    pub after_valid_plus: usize,
    pub recovered_plus: usize,
    pub still_invalid_plus: usize,
    pub nonempty_values_changed: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct SanitizedReportRecord {
    pub plu_number: u64,
    pub fields_changed: Vec<crate::sanitization::engine::SanitizedFieldChange>,
    pub validation_result_after_sanitization: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct SanitizationSafetyReport {
    pub mdb_modified: bool,
    pub authentication_attempted: bool,
    pub api_requests_attempted: bool,
    pub nonempty_values_changed: usize,
}

pub fn integration_from_parts(
    profile: SanitizationProfile,
    engine_report: SanitizationEngineReport,
    before_valid: usize,
    before_invalid: usize,
    after_valid: usize,
    after_invalid: usize,
    before_valid_numbers: &[u64],
    after_valid_numbers: &[u64],
) -> Result<SanitizationIntegration, AppError> {
    let normalized_profile_toml = profile.normalized_toml()?;
    let profile_sha256 = sha256_text(&normalized_profile_toml);
    let before = before_valid_numbers
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    let recovered_plus = after_valid_numbers
        .iter()
        .filter(|plu| !before.contains(plu))
        .count();
    Ok(SanitizationIntegration {
        profile,
        profile_sha256,
        normalized_profile_toml,
        engine_report,
        before_valid,
        before_invalid,
        after_valid,
        after_invalid,
        recovered_plus,
        still_invalid_plus: Vec::new(),
    })
}

pub fn build_report(input: SanitizationReportInput<'_>) -> SanitizationReport {
    let still_invalid = input
        .still_invalid_plus
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    SanitizationReport {
        schema_version: 1,
        application_version: env!("CARGO_PKG_VERSION").to_string(),
        generated_at: Local::now().to_rfc3339(),
        source: input.source,
        profile: SanitizationProfileSummary {
            name: input.profile.profile_name.clone(),
            version: input.profile.profile_version,
            sha256: input.profile_sha256.to_string(),
        },
        summary: SanitizationSummary {
            source_plus: input.engine_report.source_plus,
            changed_plus: input.engine_report.changed_plus,
            unchanged_plus: input.engine_report.unchanged_plus,
            placeholder_plus: input.engine_report.placeholder_plus,
            before_valid_plus: input.before_valid,
            before_invalid_plus: input.before_invalid,
            after_valid_plus: input.after_valid,
            recovered_plus: input.recovered_plus,
            still_invalid_plus: input.after_invalid,
            nonempty_values_changed: input.engine_report.nonempty_values_changed,
        },
        field_changes: input.engine_report.field_changes.clone(),
        field_summaries: input.engine_report.field_summaries.clone(),
        records: input
            .engine_report
            .records
            .iter()
            .map(|record| sanitized_report_record(record, &still_invalid))
            .collect(),
        safety: SanitizationSafetyReport {
            mdb_modified: false,
            authentication_attempted: false,
            api_requests_attempted: false,
            nonempty_values_changed: input.engine_report.nonempty_values_changed,
        },
    }
}

pub fn write_sanitization_reports(
    text_path: &Path,
    json_path: &Path,
    snapshot_path: &Path,
    report: &SanitizationReport,
    normalized_profile_toml: &str,
) -> Result<(), AppError> {
    write_restricted(text_path, &render_text_report(report))?;
    let json = serde_json::to_string_pretty(report)
        .map_err(|err| AppError::Internal(format!("sanitization JSON failed: {err}")))?;
    write_restricted(json_path, &json)?;
    write_restricted(snapshot_path, normalized_profile_toml)?;
    Ok(())
}

pub fn render_text_report(report: &SanitizationReport) -> String {
    let mut lines = Vec::new();
    lines.push("PLU MDB Sanitization Report".to_string());
    lines.push(format!(
        "Application version: {}",
        report.application_version
    ));
    lines.push(String::new());
    lines.push("1. Source identity".to_string());
    lines.push(format!("Filename: {}", report.source.filename));
    lines.push(format!("Size bytes: {}", report.source.size_bytes));
    lines.push(format!("SHA-256: {}", report.source.sha256));
    lines.push(String::new());
    lines.push("2. Profile identity".to_string());
    lines.push(format!("Profile: {}", report.profile.name));
    lines.push(format!("Profile version: {}", report.profile.version));
    lines.push(format!("Profile SHA-256: {}", report.profile.sha256));
    lines.push(String::new());
    lines.push("3. Raw source summary".to_string());
    lines.push(format!("Source PLUs: {}", report.summary.source_plus));
    lines.push(format!(
        "Placeholder PLUs: {}",
        report.summary.placeholder_plus
    ));
    lines.push(String::new());
    lines.push("4. Sanitization field summary".to_string());
    for (field, summary) in &report.field_summaries {
        lines.push(format!(
            "{field}: changed={}, empty defaults applied={}, invalid nonempty values corrected={}, valid preserved={}",
            summary.changed,
            summary.empty_defaulted,
            summary.invalid_nonempty_corrected,
            summary.valid_preserved
        ));
    }
    for (field, count) in &report.field_changes {
        if !report.field_summaries.contains_key(field) {
            lines.push(format!("{field}: {count} empty value(s) would be filled"));
        }
    }
    lines.push(String::new());
    lines.push("5. Proposed changes".to_string());
    for record in &report.records {
        let fields = record
            .fields_changed
            .iter()
            .map(|field| {
                format!(
                    "{}: {:?} -> {} ({})",
                    field.field,
                    field.original_value,
                    field.sanitized_value,
                    field.reason.as_str()
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        lines.push(format!("PLU {}: {}", record.plu_number, fields));
    }
    lines.push(String::new());
    lines.push("6. Validation before sanitization".to_string());
    lines.push(format!("Valid PLUs: {}", report.summary.before_valid_plus));
    lines.push(format!(
        "Invalid PLUs: {}",
        report.summary.before_invalid_plus
    ));
    lines.push(String::new());
    lines.push("7. Validation after sanitization".to_string());
    lines.push(format!("Valid PLUs: {}", report.summary.after_valid_plus));
    lines.push(format!(
        "Still invalid: {}",
        report.summary.still_invalid_plus
    ));
    lines.push(String::new());
    lines.push("8. Recovered PLUs".to_string());
    lines.push(format!(
        "Recovered by profile: {}",
        report.summary.recovered_plus
    ));
    lines.push(String::new());
    lines.push("9. Still-invalid PLUs".to_string());
    lines.push(format!(
        "Still invalid: {}",
        report.summary.still_invalid_plus
    ));
    lines.push(String::new());
    lines.push("10. Per-rule summary".to_string());
    for (field, count) in &report.field_changes {
        lines.push(format!("{field}: {count}"));
    }
    lines.push(String::new());
    lines.push("11. Safety confirmation".to_string());
    lines.push("plu.mdb modified: NO".to_string());
    lines.push("Authentication attempted: NO".to_string());
    lines.push("DIGIweb API requests attempted: NO".to_string());
    lines.push(format!(
        "Existing nonempty values changed: {}",
        report.summary.nonempty_values_changed
    ));
    lines.push(String::new());
    lines.join("\n")
}

pub fn sha256_text(value: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(value.as_bytes());
    hex::encode(hasher.finalize())
}

fn sanitized_report_record(
    record: &SanitizedRecordChange,
    still_invalid: &BTreeSet<u64>,
) -> SanitizedReportRecord {
    SanitizedReportRecord {
        plu_number: record.plu_number,
        fields_changed: record.fields.clone(),
        validation_result_after_sanitization: if still_invalid.contains(&record.plu_number) {
            "invalid".to_string()
        } else {
            "valid".to_string()
        },
    }
}

fn write_restricted(path: &Path, contents: &str) -> Result<(), AppError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| {
            AppError::Logging(format!("failed to create '{}': {err}", parent.display()))
        })?;
    }
    fs::write(path, contents)
        .map_err(|err| AppError::Logging(format!("failed to write '{}': {err}", path.display())))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).map_err(|err| {
            AppError::Logging(format!(
                "failed to set permissions '{}': {err}",
                path.display()
            ))
        })?;
    }
    Ok(())
}

#[allow(dead_code)]
pub fn snapshot_path_in_manifest_dir(manifest_path: &Path) -> PathBuf {
    manifest_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("sanitization-profile.snapshot.toml")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sanitization::engine::{
        EmptyCategory, SanitizationEngineReport, SanitizedChangeReason, SanitizedFieldChange,
        SanitizedFieldSummary, SanitizedRecordChange,
    };
    use crate::sanitization::profile::{
        RuleAction, RuleCondition, SanitizationRule, SanitizationSafety, TargetField,
    };

    fn profile() -> SanitizationProfile {
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

    fn source() -> SourceIdentity {
        SourceIdentity {
            filename: "plu.mdb".to_string(),
            size_bytes: 10,
            sha256: "a".repeat(64),
        }
    }

    #[test]
    fn reports_are_written_and_contain_no_credentials_or_ingredients() {
        let temp = tempfile::tempdir().expect("temp");
        let profile = profile();
        let normalized = profile.normalized_toml().expect("toml");
        let report = build_report(SanitizationReportInput {
            source: source(),
            profile: &profile,
            profile_sha256: &sha256_text(&normalized),
            engine_report: &SanitizationEngineReport {
                profile_name: "test".to_string(),
                profile_version: 1,
                source_plus: 1,
                changed_plus: 1,
                unchanged_plus: 0,
                placeholder_plus: 0,
                nonempty_values_changed: 0,
                field_changes: BTreeMap::from([("department".to_string(), 1)]),
                field_summaries: BTreeMap::from([(
                    "department".to_string(),
                    SanitizedFieldSummary {
                        changed: 1,
                        empty_defaulted: 1,
                        invalid_nonempty_corrected: 0,
                        valid_preserved: 0,
                    },
                )]),
                records: vec![SanitizedRecordChange {
                    plu_number: 1,
                    fields: vec![SanitizedFieldChange {
                        field: "department".to_string(),
                        original_value: String::new(),
                        original_empty_category: EmptyCategory::EmptyString,
                        applied_rule: "set_constant".to_string(),
                        sanitized_value: "1".to_string(),
                        reason: SanitizedChangeReason::EmptyDefaulted,
                    }],
                }],
            },
            before_valid: 0,
            before_invalid: 1,
            after_valid: 1,
            after_invalid: 0,
            recovered_plus: 1,
            still_invalid_plus: &[],
        });

        write_sanitization_reports(
            &temp.path().join("sanitization-report.txt"),
            &temp.path().join("sanitization-report.json"),
            &temp.path().join("sanitization-profile.snapshot.toml"),
            &report,
            &normalized,
        )
        .expect("write");

        let json = fs::read_to_string(temp.path().join("sanitization-report.json")).expect("json");
        assert!(serde_json::from_str::<serde_json::Value>(&json).is_ok());
        assert!(!json.to_ascii_lowercase().contains("secret"));
        assert!(!json.to_ascii_lowercase().contains("ingredient"));
        assert!(
            temp.path()
                .join("sanitization-profile.snapshot.toml")
                .exists()
        );
    }
}
