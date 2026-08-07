use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Local};
use serde::Serialize;

use crate::discovery::{FieldDiscovery, PhaseTiming, field_discoveries};
use crate::error::AppError;
use crate::source::SourceDataset;

pub struct ProfileSuggestionInput<'a> {
    pub profile_name: &'a str,
    pub source_path: &'a str,
    pub source_sha256: &'a str,
    pub started_at: DateTime<Local>,
    pub finished_at: DateTime<Local>,
    pub dataset: &'a SourceDataset,
    pub timings: Vec<PhaseTiming>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProfileSuggestionReport {
    pub schema_version: u32,
    pub application_version: String,
    pub command: String,
    pub generated_at: String,
    pub started_at: String,
    pub finished_at: String,
    pub source_path: String,
    pub source_sha256: String,
    pub profile_name: String,
    pub draft_path: String,
    pub recommendations_path: String,
    pub active_rules: Vec<SuggestedRule>,
    pub human_review: Vec<String>,
    pub safety: SuggestionSafety,
    pub timings: Vec<PhaseTiming>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SuggestedRule {
    pub field: String,
    pub rule: String,
    pub affected_values: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SuggestionSafety {
    pub authentication_attempted: bool,
    pub digiweb_api_requests_attempted: bool,
    pub source_database_modified: bool,
    pub plus_submitted: usize,
    pub overwrites_existing_profile: bool,
}

pub fn suggest_profile(
    input: ProfileSuggestionInput<'_>,
) -> Result<ProfileSuggestionReport, AppError> {
    let started = std::time::Instant::now();
    let safe_name = sanitize_profile_name(input.profile_name)?;
    let profile_dir = Path::new("profiles");
    let draft_path = profile_dir.join(format!("{safe_name}.draft.toml"));
    if draft_path.exists() {
        return Err(AppError::Config(format!(
            "profile suggestion refused to overwrite existing file '{}'",
            draft_path.display()
        )));
    }

    let fields = field_discoveries(input.dataset);
    let mut active_rules = Vec::new();
    if let Some(best_before) = fields.iter().find(|field| field.field == "Best Before") {
        let affected = best_before.safe_profile_candidate;
        if affected > 0 {
            active_rules.push(SuggestedRule {
                field: "selling_date_term".to_string(),
                rule: "empty, malformed, negative, or >999 Best Before values -> 0; preserve 0 and 1..999".to_string(),
                affected_values: affected,
            });
        }
    }

    let human_review = human_review_items(&fields);
    let mut timings = input.timings;
    timings.push(PhaseTiming::from_duration(
        "Profile suggestion",
        started.elapsed(),
    ));

    Ok(ProfileSuggestionReport {
        schema_version: 1,
        application_version: env!("CARGO_PKG_VERSION").to_string(),
        command: "profile suggest".to_string(),
        generated_at: Local::now().to_rfc3339(),
        started_at: input.started_at.to_rfc3339(),
        finished_at: input.finished_at.to_rfc3339(),
        source_path: input.source_path.to_string(),
        source_sha256: input.source_sha256.to_string(),
        profile_name: safe_name.clone(),
        draft_path: draft_path.display().to_string(),
        recommendations_path: "profile-recommendations.txt".to_string(),
        active_rules,
        human_review,
        safety: SuggestionSafety {
            authentication_attempted: false,
            digiweb_api_requests_attempted: false,
            source_database_modified: false,
            plus_submitted: 0,
            overwrites_existing_profile: false,
        },
        timings,
    })
}

pub fn write_profile_suggestion(report: &ProfileSuggestionReport) -> Result<(), AppError> {
    let draft_path = PathBuf::from(&report.draft_path);
    if draft_path.exists() {
        return Err(AppError::Config(format!(
            "profile suggestion refused to overwrite existing file '{}'",
            draft_path.display()
        )));
    }
    fs::create_dir_all(draft_path.parent().unwrap_or_else(|| Path::new(".")))
        .map_err(|err| AppError::Internal(format!("failed to create profiles directory: {err}")))?;
    fs::write(&draft_path, render_draft_profile(report))
        .map_err(|err| AppError::Internal(format!("failed to write draft profile: {err}")))?;
    fs::write(
        Path::new(&report.recommendations_path),
        render_recommendations(report),
    )
    .map_err(|err| AppError::Internal(format!("failed to write profile recommendations: {err}")))?;
    Ok(())
}

pub fn render_profile_suggestion_console(report: &ProfileSuggestionReport) -> String {
    let mut out = String::new();
    line(&mut out, "PROFILE SUGGESTION");
    line(&mut out, format!("Profile draft: {}", report.draft_path));
    line(
        &mut out,
        format!("Active deterministic rules: {}", report.active_rules.len()),
    );
    line(
        &mut out,
        format!(
            "Human-review recommendations: {}",
            report.human_review.len()
        ),
    );
    line(&mut out, "Recommendations:");
    line(&mut out, &report.recommendations_path);
    out
}

fn render_draft_profile(report: &ProfileSuggestionReport) -> String {
    let mut out = String::new();
    line(&mut out, "profile_version = 1");
    line(
        &mut out,
        format!("profile_name = {:?}", report.profile_name),
    );
    line(
        &mut out,
        "description = \"Draft generated from raw MDB discovery; review before customer use\"",
    );
    blank(&mut out);
    line(&mut out, "[safety]");
    line(&mut out, "fill_empty_only = true");
    line(&mut out, "preserve_nonempty_values = true");
    line(&mut out, "reject_invalid_results = true");
    blank(&mut out);
    if report
        .active_rules
        .iter()
        .any(|rule| rule.field == "selling_date_term")
    {
        line(&mut out, "[selling_date_term]");
        line(&mut out, "enabled = true");
        line(&mut out, "allow_zero = true");
        line(&mut out, "minimum = 1");
        line(&mut out, "maximum = 999");
        line(&mut out, "invalid_value = 0");
        line(&mut out, "empty_value = 0");
        blank(&mut out);
    } else {
        line(
            &mut out,
            "# No active sanitization rules were generated from deterministic findings.",
        );
        blank(&mut out);
    }
    for item in &report.human_review {
        line(&mut out, format!("# REVIEW: {item}"));
    }
    out
}

fn render_recommendations(report: &ProfileSuggestionReport) -> String {
    let mut out = render_profile_suggestion_console(report);
    blank(&mut out);
    line(&mut out, "SAFETY");
    line(&mut out, "Authentication attempted: NO");
    line(&mut out, "DIGIweb API requests attempted: NO");
    line(&mut out, "Source database modified: NO");
    line(&mut out, "PLUs submitted: 0");
    blank(&mut out);
    line(&mut out, "ACTIVE RULES");
    if report.active_rules.is_empty() {
        line(&mut out, "None");
    }
    for rule in &report.active_rules {
        line(
            &mut out,
            format!(
                "{} | affected values: {} | {}",
                rule.field, rule.affected_values, rule.rule
            ),
        );
    }
    blank(&mut out);
    line(&mut out, "HUMAN REVIEW");
    if report.human_review.is_empty() {
        line(&mut out, "None");
    }
    for item in &report.human_review {
        line(&mut out, item);
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

fn sanitize_profile_name(name: &str) -> Result<String, AppError> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(AppError::Config(
            "profile name must not be empty".to_string(),
        ));
    }
    if trimmed
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
    {
        Ok(trimmed.to_ascii_lowercase())
    } else {
        Err(AppError::Config(
            "profile name may contain only letters, numbers, '-' and '_'".to_string(),
        ))
    }
}

fn human_review_items(fields: &[FieldDiscovery]) -> Vec<String> {
    let mut items = Vec::new();
    if let Some(group) = fields.iter().find(|field| field.field == "Main Group Code") {
        if group.human_decision_required > 0 {
            items.push(format!(
                "Main Group Code has {} empty, invalid, or ambiguous values; no active profile rule was generated.",
                group.human_decision_required
            ));
        }
    }
    for field in fields {
        if field.human_decision_required > 0 && field.field != "Main Group Code" {
            items.push(format!(
                "{} has {} values requiring manual review.",
                field.field, field.human_decision_required
            ));
        }
    }
    items
}

fn line(out: &mut String, text: impl AsRef<str>) {
    out.push_str(text.as_ref());
    out.push('\n');
}

fn blank(out: &mut String) {
    out.push('\n');
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::{Mutex, OnceLock};

    use tempfile::tempdir;

    use super::*;
    use crate::source::SourceRow;

    static CURRENT_DIR_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

    fn cwd_lock() -> std::sync::MutexGuard<'static, ()> {
        CURRENT_DIR_LOCK
            .get_or_init(|| Mutex::new(()))
            .lock()
            .expect("cwd lock")
    }

    fn row(plu: &str, group: &str, best_before: &str) -> SourceRow {
        SourceRow {
            table: "Pludata".to_string(),
            values: BTreeMap::from([
                ("Plucode".to_string(), plu.to_string()),
                ("Department".to_string(), "1".to_string()),
                ("Main Group Code".to_string(), group.to_string()),
                ("Best Before".to_string(), best_before.to_string()),
            ]),
        }
    }

    #[test]
    fn ambiguous_group_does_not_create_active_rule() {
        let dataset = SourceDataset {
            plu_rows: vec![row("1", "", "1000")],
            ingredient_rows: Vec::new(),
            nutrition_rows: Vec::new(),
        };
        let now = Local::now();
        let report = suggest_profile(ProfileSuggestionInput {
            profile_name: "bigway",
            source_path: "plu.mdb",
            source_sha256: "abc",
            started_at: now,
            finished_at: now,
            dataset: &dataset,
            timings: Vec::new(),
        })
        .expect("suggestion");

        assert!(report.active_rules.iter().all(|rule| rule.field != "group"));
        assert!(
            report
                .human_review
                .iter()
                .any(|item| item.contains("Main Group Code"))
        );
        assert!(
            report
                .active_rules
                .iter()
                .any(|rule| rule.field == "selling_date_term")
        );
    }

    #[test]
    fn suggestion_refuses_to_overwrite_existing_draft() {
        let _guard = cwd_lock();
        let dir = tempdir().expect("tempdir");
        let original = std::env::current_dir().expect("cwd");
        std::env::set_current_dir(dir.path()).expect("chdir");
        fs::create_dir("profiles").expect("profiles");
        fs::write("profiles/bigway.draft.toml", "existing").expect("write");
        let dataset = SourceDataset::default();
        let now = Local::now();

        let result = suggest_profile(ProfileSuggestionInput {
            profile_name: "bigway",
            source_path: "plu.mdb",
            source_sha256: "abc",
            started_at: now,
            finished_at: now,
            dataset: &dataset,
            timings: Vec::new(),
        });

        std::env::set_current_dir(original).expect("restore");
        assert!(result.is_err());
    }
}
