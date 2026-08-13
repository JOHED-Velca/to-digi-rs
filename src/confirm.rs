use std::collections::BTreeSet;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use chrono::Local;

use crate::cli::ConfirmTarget;
use crate::digiweb::preflight::{ReferenceConfirmationStatus, ReferenceReadiness};
use crate::error::AppError;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ConfirmationSet {
    pub departments: Vec<u32>,
    pub groups: Vec<String>,
    pub label_formats: Vec<u32>,
}

impl ConfirmationSet {
    pub fn is_empty(&self) -> bool {
        self.departments.is_empty() && self.groups.is_empty() && self.label_formats.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfirmationPlan {
    pub profile: String,
    pub source_plus: usize,
    pub target: ConfirmTarget,
    pub required: ConfirmationSet,
    pub existing: ConfirmationSet,
    pub additions: ConfirmationSet,
}

impl ConfirmationPlan {
    pub fn from_readiness(
        profile: String,
        source_plus: usize,
        target: ConfirmTarget,
        readiness: &ReferenceReadiness,
    ) -> Self {
        let mut required = ConfirmationSet::default();
        let mut existing = ConfirmationSet::default();

        if matches!(target, ConfirmTarget::Departments | ConfirmTarget::All) {
            for reference in &readiness.departments {
                required.departments.push(reference.number);
                if reference.status == ReferenceConfirmationStatus::ManuallyConfirmed {
                    existing.departments.push(reference.number);
                }
            }
        }
        if matches!(target, ConfirmTarget::Groups | ConfirmTarget::All) {
            for reference in &readiness.groups {
                let value = format!("{}:{}", reference.department, reference.number);
                required.groups.push(value.clone());
                if reference.status == ReferenceConfirmationStatus::ManuallyConfirmed {
                    existing.groups.push(value);
                }
            }
        }
        if matches!(target, ConfirmTarget::LabelFormats | ConfirmTarget::All) {
            for reference in &readiness.label_formats {
                required.label_formats.push(reference.number);
                if reference.status == ReferenceConfirmationStatus::ManuallyConfirmed {
                    existing.label_formats.push(reference.number);
                }
            }
        }

        normalize_set(&mut required);
        normalize_set(&mut existing);
        let additions = subtract_sets(&required, &existing);
        Self {
            profile,
            source_plus,
            target,
            required,
            existing,
            additions,
        }
    }
}

pub fn render_confirmation_preview(plan: &ConfirmationPlan, dry_run: bool) -> String {
    let mut out = String::new();
    out.push_str("REFERENCE CONFIRMATION\n\n");
    out.push_str(&format!("Profile: {}\n", plan.profile));
    out.push_str(&format!("Source PLUs: {}\n\n", plan.source_plus));
    render_values(&mut out, "Departments", &plan.required.departments);
    render_values(&mut out, "Groups", &plan.required.groups);
    render_values(&mut out, "Label Formats", &plan.required.label_formats);
    out.push('\n');
    render_values(&mut out, "Departments to add", &plan.additions.departments);
    render_values(&mut out, "Groups to add", &plan.additions.groups);
    render_values(
        &mut out,
        "Label Formats to add",
        &plan.additions.label_formats,
    );
    out.push('\n');
    out.push_str("This records operator confirmation only.\n");
    out.push_str("No DIGIweb server lookup has been performed.\n");
    if dry_run {
        out.push_str("Dry run: config.toml will not be changed.\n");
    }
    out
}

fn render_values<T: std::fmt::Display>(out: &mut String, heading: &str, values: &[T]) {
    out.push_str(heading);
    out.push_str(":\n");
    if values.is_empty() {
        out.push_str("  <none>\n");
    } else {
        for value in values {
            out.push_str(&format!("  {value}\n"));
        }
    }
    out.push('\n');
}

pub fn apply_confirmation_to_config_text(
    config_text: &str,
    additions: &ConfirmationSet,
) -> Result<String, AppError> {
    let current = parse_verification_from_config_text(config_text)?;
    let merged = merge_sets(&current, additions);
    let section = render_verification_section(&merged);

    let lines = config_text.lines().collect::<Vec<_>>();
    let verification_start = lines
        .iter()
        .position(|line| line.trim() == "[verification]");
    let mut output = Vec::<String>::new();
    match verification_start {
        Some(start) => {
            output.extend(lines[..start].iter().map(|line| (*line).to_string()));
            if output.last().is_some_and(|line| !line.trim().is_empty()) {
                output.push(String::new());
            }
            output.extend(section.lines().map(ToOwned::to_owned));
            let end = lines[start + 1..]
                .iter()
                .position(|line| is_section_header(line))
                .map(|offset| start + 1 + offset)
                .unwrap_or(lines.len());
            if end < lines.len() {
                output.push(String::new());
                output.extend(lines[end..].iter().map(|line| (*line).to_string()));
            }
        }
        None => {
            output.extend(lines.iter().map(|line| (*line).to_string()));
            if output.last().is_some_and(|line| !line.trim().is_empty()) {
                output.push(String::new());
            }
            output.extend(section.lines().map(ToOwned::to_owned));
        }
    }
    let mut text = output.join("\n");
    text.push('\n');
    Ok(text)
}

pub fn write_confirmed_config(path: &Path, new_text: &str) -> Result<PathBuf, AppError> {
    let backup = timestamped_backup_path(path)?;
    fs::copy(path, &backup).map_err(|err| {
        AppError::Config(format!(
            "failed to create config backup '{}': {err}",
            backup.display()
        ))
    })?;
    let temp_path = path.with_extension("toml.tmp");
    let mut temp = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&temp_path)
        .map_err(|err| {
            AppError::Config(format!(
                "failed to create temporary config '{}': {err}",
                temp_path.display()
            ))
        })?;
    temp.write_all(new_text.as_bytes()).map_err(|err| {
        AppError::Config(format!(
            "failed to write temporary config '{}': {err}",
            temp_path.display()
        ))
    })?;
    temp.sync_all().map_err(|err| {
        AppError::Config(format!(
            "failed to flush temporary config '{}': {err}",
            temp_path.display()
        ))
    })?;
    drop(temp);
    toml::from_str::<crate::config::AppConfig>(new_text)
        .map_err(|err| AppError::Config(format!("updated config.toml is invalid: {err}")))?;
    fs::rename(&temp_path, path).map_err(|err| {
        AppError::Config(format!(
            "failed to replace config.toml with '{}': {err}",
            temp_path.display()
        ))
    })?;
    Ok(backup)
}

fn timestamped_backup_path(path: &Path) -> Result<PathBuf, AppError> {
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| AppError::Config(format!("invalid config path '{}'", path.display())))?;
    Ok(path.with_file_name(format!(
        "{file_name}.{}.bak",
        Local::now().format("%Y%m%d%H%M%S")
    )))
}

fn parse_verification_from_config_text(text: &str) -> Result<ConfirmationSet, AppError> {
    let value = text
        .parse::<toml::Value>()
        .map_err(|err| AppError::Config(format!("config.toml is invalid: {err}")))?;
    let Some(table) = value.get("verification").and_then(|value| value.as_table()) else {
        return Ok(ConfirmationSet::default());
    };
    Ok(ConfirmationSet {
        departments: table
            .get("confirmed_departments")
            .and_then(|value| value.as_array())
            .map(|values| {
                values
                    .iter()
                    .filter_map(|value| {
                        value
                            .as_integer()
                            .and_then(|value| u32::try_from(value).ok())
                    })
                    .collect()
            })
            .unwrap_or_default(),
        groups: table
            .get("confirmed_groups")
            .and_then(|value| value.as_array())
            .map(|values| {
                values
                    .iter()
                    .filter_map(|value| value.as_str().map(ToOwned::to_owned))
                    .collect()
            })
            .unwrap_or_default(),
        label_formats: table
            .get("confirmed_label_formats")
            .and_then(|value| value.as_array())
            .map(|values| {
                values
                    .iter()
                    .filter_map(|value| {
                        value
                            .as_integer()
                            .and_then(|value| u32::try_from(value).ok())
                    })
                    .collect()
            })
            .unwrap_or_default(),
    })
}

fn render_verification_section(values: &ConfirmationSet) -> String {
    format!(
        "[verification]\n# Operator-attested DIGIweb references. No server lookup is implied.\nconfirmed_departments = [{}]\nconfirmed_groups = [{}]\nconfirmed_label_formats = [{}]",
        values
            .departments
            .iter()
            .map(u32::to_string)
            .collect::<Vec<_>>()
            .join(", "),
        values
            .groups
            .iter()
            .map(|value| format!("\"{value}\""))
            .collect::<Vec<_>>()
            .join(", "),
        values
            .label_formats
            .iter()
            .map(u32::to_string)
            .collect::<Vec<_>>()
            .join(", "),
    )
}

fn is_section_header(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.starts_with('[') && trimmed.ends_with(']')
}

fn merge_sets(existing: &ConfirmationSet, additions: &ConfirmationSet) -> ConfirmationSet {
    let mut merged = ConfirmationSet {
        departments: existing
            .departments
            .iter()
            .chain(additions.departments.iter())
            .copied()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect(),
        groups: existing
            .groups
            .iter()
            .chain(additions.groups.iter())
            .cloned()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect(),
        label_formats: existing
            .label_formats
            .iter()
            .chain(additions.label_formats.iter())
            .copied()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect(),
    };
    normalize_set(&mut merged);
    merged
}

fn subtract_sets(required: &ConfirmationSet, existing: &ConfirmationSet) -> ConfirmationSet {
    ConfirmationSet {
        departments: required
            .departments
            .iter()
            .copied()
            .filter(|value| !existing.departments.contains(value))
            .collect(),
        groups: required
            .groups
            .iter()
            .filter(|value| !existing.groups.contains(value))
            .cloned()
            .collect(),
        label_formats: required
            .label_formats
            .iter()
            .copied()
            .filter(|value| !existing.label_formats.contains(value))
            .collect(),
    }
}

fn normalize_set(values: &mut ConfirmationSet) {
    values.departments = values
        .departments
        .iter()
        .copied()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    values.groups = values
        .groups
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    values.label_formats = values
        .label_formats
        .iter()
        .copied()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::digiweb::preflight::{
        DepartmentReadiness, GroupReadiness, LabelFormatReadiness, ReadinessResult,
        ReferenceConfirmationStatus, ReferenceReadiness, evaluate_reference_readiness,
    };
    use crate::models::plu::{Plu, PriceMode};
    use rust_decimal::Decimal;

    fn readiness() -> ReferenceReadiness {
        ReferenceReadiness {
            departments: vec![
                DepartmentReadiness {
                    number: 2,
                    status: ReferenceConfirmationStatus::Unverified,
                    source_plu_numbers: vec![1],
                },
                DepartmentReadiness {
                    number: 1,
                    status: ReferenceConfirmationStatus::ManuallyConfirmed,
                    source_plu_numbers: vec![2],
                },
            ],
            groups: vec![
                GroupReadiness {
                    department: 2,
                    number: 997,
                    status: ReferenceConfirmationStatus::Unverified,
                    source_plu_numbers: vec![1],
                },
                GroupReadiness {
                    department: 1,
                    number: 12,
                    status: ReferenceConfirmationStatus::ManuallyConfirmed,
                    source_plu_numbers: vec![2],
                },
            ],
            label_formats: vec![
                LabelFormatReadiness {
                    number: 6,
                    status: ReferenceConfirmationStatus::Unverified,
                    source_plu_numbers: vec![1],
                },
                LabelFormatReadiness {
                    number: 1,
                    status: ReferenceConfirmationStatus::ManuallyConfirmed,
                    source_plu_numbers: vec![2],
                },
            ],
            stale_confirmations: Vec::new(),
            unverified_reference_count: 3,
            result: ReadinessResult::NotReady,
        }
    }

    #[test]
    fn plan_uses_readiness_and_target_filtering() {
        let plan = ConfirmationPlan::from_readiness(
            "bigway".to_string(),
            2,
            ConfirmTarget::Groups,
            &readiness(),
        );

        assert!(plan.required.departments.is_empty());
        assert_eq!(plan.required.groups, vec!["1:12", "2:997"]);
        assert_eq!(plan.existing.groups, vec!["1:12"]);
        assert_eq!(plan.additions.groups, vec!["2:997"]);
    }

    #[test]
    fn config_update_preserves_unrelated_settings_and_credentials() {
        let original = r#"[digiweb]
base_url = "https://example"
client_secret = "keep-me"

[verification]
confirmed_departments = [2, 1]
confirmed_groups = ["2:997"]
confirmed_label_formats = [6]

[import]
max_in_flight = 16
"#;
        let updated = apply_confirmation_to_config_text(
            original,
            &ConfirmationSet {
                departments: vec![1, 3],
                groups: vec!["1:12".to_string(), "2:997".to_string()],
                label_formats: vec![1, 6],
            },
        )
        .expect("update");

        assert!(updated.contains("client_secret = \"keep-me\""));
        assert!(updated.contains("[import]\nmax_in_flight = 16"));
        assert!(updated.contains("confirmed_departments = [1, 2, 3]"));
        assert!(updated.contains("confirmed_groups = [\"1:12\", \"2:997\"]"));
        assert!(updated.contains("confirmed_label_formats = [1, 6]"));
    }

    #[test]
    fn config_update_adds_missing_verification_section() {
        let updated = apply_confirmation_to_config_text(
            "[digiweb]\nclient_secret = \"keep\"\n",
            &ConfirmationSet {
                departments: vec![1],
                groups: vec!["1:997".to_string()],
                label_formats: vec![1],
            },
        )
        .expect("update");

        assert!(updated.contains("[verification]"));
        assert!(updated.contains("confirmed_departments = [1]"));
        assert!(updated.contains("client_secret = \"keep\""));
    }

    #[test]
    fn dry_run_preview_describes_no_file_change() {
        let plan = ConfirmationPlan::from_readiness(
            "bigway".to_string(),
            2,
            ConfirmTarget::All,
            &readiness(),
        );

        let rendered = render_confirmation_preview(&plan, true);

        assert!(rendered.contains("Profile: bigway"));
        assert!(rendered.contains("No DIGIweb server lookup has been performed."));
        assert!(rendered.contains("Dry run: config.toml will not be changed."));
    }

    #[test]
    fn write_confirmed_config_replaces_file_and_creates_backup() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        fs::write(
            &path,
            "[digiweb]\nbase_url = \"https://example\"\nclient_secret = \"keep\"\n",
        )
        .expect("write");
        let updated = "[digiweb]\nbase_url = \"https://example\"\nclient_secret = \"keep\"\n\n[verification]\nconfirmed_departments = [1]\nconfirmed_groups = [\"1:997\"]\nconfirmed_label_formats = [1]\n";

        let backup = write_confirmed_config(&path, updated).expect("write config");

        assert!(backup.is_file());
        assert_eq!(fs::read_to_string(&path).expect("updated"), updated);
        assert!(
            fs::read_to_string(&backup)
                .expect("backup")
                .contains("client_secret = \"keep\"")
        );
    }

    #[test]
    fn confirmed_config_makes_same_readiness_collector_ready() {
        let original = "[verification]\nconfirmed_departments = []\nconfirmed_groups = []\nconfirmed_label_formats = []\n";
        let updated = apply_confirmation_to_config_text(
            original,
            &ConfirmationSet {
                departments: vec![1],
                groups: vec!["1:997".to_string()],
                label_formats: vec![1],
            },
        )
        .expect("update");
        let config = toml::from_str::<crate::config::AppConfig>(&updated).expect("config");
        let mut plu = plu(1);
        plu.label_format = Some(1);

        let readiness =
            evaluate_reference_readiness(&[plu], &config.verification, 0).expect("readiness");

        assert_eq!(readiness.result, ReadinessResult::Ready);
        assert_eq!(readiness.unverified_reference_count, 0);
    }

    #[test]
    fn existing_confirmations_are_retained_without_duplicates() {
        let updated = apply_confirmation_to_config_text(
            "[verification]\nconfirmed_departments = [1]\nconfirmed_groups = [\"1:997\"]\nconfirmed_label_formats = [1]\n",
            &ConfirmationSet {
                departments: vec![1],
                groups: vec!["1:997".to_string()],
                label_formats: vec![1],
            },
        )
        .expect("update");

        assert!(updated.contains("confirmed_departments = [1]"));
        assert!(updated.contains("confirmed_groups = [\"1:997\"]"));
        assert!(updated.contains("confirmed_label_formats = [1]"));
    }

    fn plu(plu_number: u64) -> Plu {
        Plu {
            plu_number,
            store_number: 1,
            department_number: Some(1),
            group_number: Some(997),
            source_department: Some("1".to_string()),
            source_group: Some("997".to_string()),
            group_default_applied: false,
            name: "Bread".to_string(),
            barcode: Some("1".to_string()),
            barcode_type: Some("5".to_string()),
            barcode_ref_no: Some("5".to_string()),
            source_barcode: Some("1".to_string()),
            source_barcode_format: Some("05".to_string()),
            source_flag_data: Some("02".to_string()),
            price: Decimal::new(1, 0),
            price_mode: PriceMode::ByEach,
            price_calc_method: None,
            quantity: None,
            quantity_symbol: None,
            tare: None,
            source_tare: None,
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
            ingredients: None,
            nutrition_facts: Vec::new(),
            nutrition_profile: None,
            nutrition_remaps: Vec::new(),
            source_pluing_row_count: 0,
        }
    }
}
