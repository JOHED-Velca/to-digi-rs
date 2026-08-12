use std::collections::BTreeMap;
use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::config::VerificationConfig;
use crate::error::AppError;
use crate::models::plu::Plu;
use crate::models::plu::effective_label_format;

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReferenceStatus {
    Confirmed,
    NotFound,
    NotChecked,
    Ambiguous,
}

impl ReferenceStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Confirmed => "CONFIRMED",
            Self::NotFound => "NOT_FOUND",
            Self::NotChecked => "NOT_CHECKED",
            Self::Ambiguous => "AMBIGUOUS",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequiredReference {
    pub department_number: u32,
    pub group_number: u32,
    pub source_plu_numbers: Vec<u64>,
    pub status: ReferenceStatus,
}

impl RequiredReference {
    #[allow(dead_code)]
    pub fn missing_group_message(&self) -> String {
        format!(
            "Required DIGIweb group was not found:\nDepartment reference: {}\nGroup reference: {}\n\nCreate or import this group in DIGIweb before running the PLU import.",
            self.department_number, self.group_number
        )
    }
}

pub fn collect_required_references(plus: &[Plu]) -> Vec<RequiredReference> {
    let mut by_reference: BTreeMap<(u32, u32), Vec<u64>> = BTreeMap::new();
    for plu in plus {
        let (Some(department), Some(group)) = (plu.department_number, plu.group_number) else {
            continue;
        };
        by_reference
            .entry((department, group))
            .or_default()
            .push(plu.plu_number);
    }
    by_reference
        .into_iter()
        .map(
            |((department_number, group_number), source_plu_numbers)| RequiredReference {
                department_number,
                group_number,
                source_plu_numbers,
                status: ReferenceStatus::NotChecked,
            },
        )
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ReferenceConfirmationStatus {
    ApiConfirmed,
    ManuallyConfirmed,
    Unverified,
    Missing,
}

impl ReferenceConfirmationStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ApiConfirmed => "API_CONFIRMED",
            Self::ManuallyConfirmed => "MANUALLY_CONFIRMED",
            Self::Unverified => "UNVERIFIED",
            Self::Missing => "MISSING",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum AuthenticationReadinessStatus {
    NotAttempted,
    Passed,
    Failed,
}

impl AuthenticationReadinessStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NotAttempted => "NOT_ATTEMPTED",
            Self::Passed => "PASSED",
            Self::Failed => "FAILED",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ReadinessResult {
    Ready,
    ReadyWithSkips,
    NotReady,
}

impl ReadinessResult {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ready => "READY",
            Self::ReadyWithSkips => "READY_WITH_SKIPS",
            Self::NotReady => "NOT_READY",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DepartmentReadiness {
    pub number: u32,
    pub status: ReferenceConfirmationStatus,
    pub source_plu_numbers: Vec<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GroupReadiness {
    pub department: u32,
    pub number: u32,
    pub status: ReferenceConfirmationStatus,
    pub source_plu_numbers: Vec<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LabelFormatReadiness {
    pub number: u32,
    pub status: ReferenceConfirmationStatus,
    pub source_plu_numbers: Vec<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StaleConfirmation {
    pub reference_type: String,
    pub reference: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReferenceReadiness {
    pub departments: Vec<DepartmentReadiness>,
    pub groups: Vec<GroupReadiness>,
    pub label_formats: Vec<LabelFormatReadiness>,
    pub stale_confirmations: Vec<StaleConfirmation>,
    pub unverified_reference_count: usize,
    pub result: ReadinessResult,
}

impl ReferenceReadiness {
    pub fn is_ready(&self) -> bool {
        matches!(
            self.result,
            ReadinessResult::Ready | ReadinessResult::ReadyWithSkips
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManualConfirmations {
    departments: BTreeSet<u32>,
    groups: BTreeSet<(u32, u32)>,
    label_formats: BTreeSet<u32>,
}

impl ManualConfirmations {
    pub fn from_config(config: &VerificationConfig) -> Result<Self, AppError> {
        let groups = config
            .confirmed_groups
            .iter()
            .map(|value| parse_group_confirmation(value))
            .collect::<Result<BTreeSet<_>, AppError>>()?;
        Ok(Self {
            departments: config.confirmed_departments.iter().copied().collect(),
            groups,
            label_formats: config.confirmed_label_formats.iter().copied().collect(),
        })
    }
}

pub fn evaluate_reference_readiness(
    plus: &[Plu],
    verification: &VerificationConfig,
    excluded_source_records: usize,
) -> Result<ReferenceReadiness, AppError> {
    let confirmations = ManualConfirmations::from_config(verification)?;
    let required_departments = required_departments(plus);
    let required_groups = required_groups(plus);
    let required_label_formats = required_label_formats(plus);

    let departments = required_departments
        .iter()
        .map(|(number, source_plu_numbers)| DepartmentReadiness {
            number: *number,
            status: if confirmations.departments.contains(number) {
                ReferenceConfirmationStatus::ManuallyConfirmed
            } else {
                ReferenceConfirmationStatus::Unverified
            },
            source_plu_numbers: source_plu_numbers.clone(),
        })
        .collect::<Vec<_>>();
    let groups = required_groups
        .iter()
        .map(
            |((department, number), source_plu_numbers)| GroupReadiness {
                department: *department,
                number: *number,
                status: if confirmations.groups.contains(&(*department, *number)) {
                    ReferenceConfirmationStatus::ManuallyConfirmed
                } else {
                    ReferenceConfirmationStatus::Unverified
                },
                source_plu_numbers: source_plu_numbers.clone(),
            },
        )
        .collect::<Vec<_>>();
    let label_formats = required_label_formats
        .iter()
        .map(|(number, source_plu_numbers)| LabelFormatReadiness {
            number: *number,
            status: if confirmations.label_formats.contains(number) {
                ReferenceConfirmationStatus::ManuallyConfirmed
            } else {
                ReferenceConfirmationStatus::Unverified
            },
            source_plu_numbers: source_plu_numbers.clone(),
        })
        .collect::<Vec<_>>();

    let stale_confirmations = stale_confirmations(
        &confirmations,
        &required_departments,
        &required_groups,
        &required_label_formats,
    );
    let unverified_reference_count = departments
        .iter()
        .filter(|reference| reference.status == ReferenceConfirmationStatus::Unverified)
        .count()
        + groups
            .iter()
            .filter(|reference| reference.status == ReferenceConfirmationStatus::Unverified)
            .count()
        + label_formats
            .iter()
            .filter(|reference| reference.status == ReferenceConfirmationStatus::Unverified)
            .count();
    let result = if unverified_reference_count > 0 {
        ReadinessResult::NotReady
    } else if excluded_source_records > 0 {
        ReadinessResult::ReadyWithSkips
    } else {
        ReadinessResult::Ready
    };

    Ok(ReferenceReadiness {
        departments,
        groups,
        label_formats,
        stale_confirmations,
        unverified_reference_count,
        result,
    })
}

fn required_departments(plus: &[Plu]) -> BTreeMap<u32, Vec<u64>> {
    let mut by_department = BTreeMap::new();
    for plu in plus {
        if let Some(department) = plu.department_number {
            by_department
                .entry(department)
                .or_insert_with(Vec::new)
                .push(plu.plu_number);
        }
    }
    by_department
}

fn required_groups(plus: &[Plu]) -> BTreeMap<(u32, u32), Vec<u64>> {
    let mut by_group = BTreeMap::new();
    for plu in plus {
        if let (Some(department), Some(group)) = (plu.department_number, plu.group_number) {
            by_group
                .entry((department, group))
                .or_insert_with(Vec::new)
                .push(plu.plu_number);
        }
    }
    by_group
}

fn required_label_formats(plus: &[Plu]) -> BTreeMap<u32, Vec<u64>> {
    let mut by_label_format = BTreeMap::new();
    for plu in plus {
        if let Some(label_format) = effective_label_format(plu.label_format) {
            by_label_format
                .entry(label_format)
                .or_insert_with(Vec::new)
                .push(plu.plu_number);
        }
    }
    by_label_format
}

fn parse_group_confirmation(value: &str) -> Result<(u32, u32), AppError> {
    let (department, group) = value.trim().split_once(':').ok_or_else(|| {
        AppError::Config(format!(
            "verification.confirmed_groups value '{value}' must use department:group"
        ))
    })?;
    let department = parse_positive_u32("department", department, value)?;
    let group = parse_positive_u32("group", group, value)?;
    Ok((department, group))
}

fn parse_positive_u32(label: &str, raw: &str, original: &str) -> Result<u32, AppError> {
    let parsed = raw.trim().parse::<u32>().map_err(|err| {
        AppError::Config(format!(
            "verification.confirmed_groups value '{original}' has invalid {label}: {err}"
        ))
    })?;
    if parsed == 0 {
        return Err(AppError::Config(format!(
            "verification.confirmed_groups value '{original}' has invalid {label}: must be positive"
        )));
    }
    Ok(parsed)
}

fn stale_confirmations(
    confirmations: &ManualConfirmations,
    required_departments: &BTreeMap<u32, Vec<u64>>,
    required_groups: &BTreeMap<(u32, u32), Vec<u64>>,
    required_label_formats: &BTreeMap<u32, Vec<u64>>,
) -> Vec<StaleConfirmation> {
    let mut stale = Vec::new();
    for department in confirmations
        .departments
        .iter()
        .filter(|department| !required_departments.contains_key(department))
    {
        stale.push(StaleConfirmation {
            reference_type: "department".to_string(),
            reference: department.to_string(),
            message: format!(
                "Configured confirmation not required by this MDB: Department {department}"
            ),
        });
    }
    for (department, group) in confirmations
        .groups
        .iter()
        .filter(|reference| !required_groups.contains_key(reference))
    {
        stale.push(StaleConfirmation {
            reference_type: "group".to_string(),
            reference: format!("{department}:{group}"),
            message: format!(
                "Configured confirmation not required by this MDB: Department {department} / Group {group}"
            ),
        });
    }
    for label_format in confirmations
        .label_formats
        .iter()
        .filter(|label_format| !required_label_formats.contains_key(label_format))
    {
        stale.push(StaleConfirmation {
            reference_type: "label_format".to_string(),
            reference: label_format.to_string(),
            message: format!(
                "Configured confirmation not required by this MDB: Label Format {label_format}"
            ),
        });
    }
    stale
}

#[cfg(test)]
mod tests {
    use rust_decimal::Decimal;

    use super::*;
    use crate::models::plu::{Plu, PriceMode};

    fn plu(plu_number: u64, department_number: u32, group_number: u32) -> Plu {
        Plu {
            plu_number,
            store_number: 1,
            department_number: Some(department_number),
            group_number: Some(group_number),
            source_department: Some(format!("{department_number:04}")),
            source_group: Some(format!("{group_number}   ")),
            group_default_applied: false,
            name: format!("PLU {plu_number}"),
            barcode: Some(format!("020{plu_number:05}")),
            barcode_type: Some("5".to_string()),
            barcode_ref_no: Some("5".to_string()),
            source_barcode: Some(plu_number.to_string()),
            source_barcode_format: Some("05".to_string()),
            source_flag_data: Some("02".to_string()),
            price: Decimal::new(100, 2),
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
            source_pluing_row_count: 0,
        }
    }

    #[test]
    fn logical_group_identity_includes_department_and_group_reference() {
        let refs = collect_required_references(&[plu(1, 1, 997), plu(2, 2, 997)]);

        assert_eq!(refs.len(), 2);
        assert!(
            refs.iter()
                .any(|reference| reference.department_number == 1 && reference.group_number == 997)
        );
        assert!(
            refs.iter()
                .any(|reference| reference.department_number == 2 && reference.group_number == 997)
        );
    }

    #[test]
    fn reference_collection_does_not_invent_uuid_or_confirmation() {
        let refs = collect_required_references(&[plu(1, 1, 997)]);

        assert_eq!(refs[0].status, ReferenceStatus::NotChecked);
    }

    #[test]
    fn required_reference_manually_confirmed_is_not_api_confirmed() {
        let config = VerificationConfig {
            confirmed_departments: vec![1],
            confirmed_groups: vec!["1:997".to_string()],
            confirmed_label_formats: vec![6],
        };
        let mut plu = plu(1, 1, 997);
        plu.label_format = Some(6);

        let readiness = evaluate_reference_readiness(&[plu], &config, 0).expect("readiness");

        assert_eq!(
            readiness.departments[0].status,
            ReferenceConfirmationStatus::ManuallyConfirmed
        );
        assert_ne!(
            readiness.groups[0].status,
            ReferenceConfirmationStatus::ApiConfirmed
        );
        assert_eq!(readiness.result, ReadinessResult::Ready);
    }

    #[test]
    fn absent_confirmation_keeps_reference_unverified() {
        let config = VerificationConfig::default();
        let mut plu = plu(1, 1, 997);
        plu.label_format = Some(6);

        let readiness = evaluate_reference_readiness(&[plu], &config, 0).expect("readiness");

        assert_eq!(readiness.unverified_reference_count, 3);
        assert_eq!(
            readiness.departments[0].status,
            ReferenceConfirmationStatus::Unverified
        );
        assert_eq!(
            readiness.groups[0].status,
            ReferenceConfirmationStatus::Unverified
        );
        assert_eq!(
            readiness.label_formats[0].status,
            ReferenceConfirmationStatus::Unverified
        );
        assert_eq!(readiness.result, ReadinessResult::NotReady);
    }

    #[test]
    fn one_missing_label_format_keeps_verify_not_ready() {
        let config = VerificationConfig {
            confirmed_departments: vec![1],
            confirmed_groups: vec!["1:997".to_string()],
            confirmed_label_formats: vec![6],
        };
        let mut a = plu(1, 1, 997);
        a.label_format = Some(6);
        let mut b = plu(2, 1, 997);
        b.label_format = Some(21);

        let readiness = evaluate_reference_readiness(&[a, b], &config, 0).expect("readiness");

        assert_eq!(readiness.result, ReadinessResult::NotReady);
        assert_eq!(
            readiness
                .label_formats
                .iter()
                .find(|reference| reference.number == 21)
                .expect("label 21")
                .status,
            ReferenceConfirmationStatus::Unverified
        );
    }

    #[test]
    fn one_missing_group_keeps_verify_not_ready() {
        let config = VerificationConfig {
            confirmed_departments: vec![1],
            confirmed_groups: vec!["1:997".to_string()],
            confirmed_label_formats: vec![],
        };

        let readiness = evaluate_reference_readiness(&[plu(1, 1, 997), plu(2, 1, 998)], &config, 0)
            .expect("readiness");

        assert_eq!(readiness.result, ReadinessResult::NotReady);
        assert_eq!(
            readiness
                .groups
                .iter()
                .find(|reference| reference.number == 998)
                .expect("group 998")
                .status,
            ReferenceConfirmationStatus::Unverified
        );
    }

    #[test]
    fn one_missing_department_keeps_verify_not_ready() {
        let config = VerificationConfig {
            confirmed_departments: vec![1],
            confirmed_groups: vec!["1:997".to_string(), "2:997".to_string()],
            confirmed_label_formats: vec![],
        };

        let readiness = evaluate_reference_readiness(&[plu(1, 1, 997), plu(2, 2, 997)], &config, 0)
            .expect("readiness");

        assert_eq!(readiness.result, ReadinessResult::NotReady);
        assert_eq!(
            readiness
                .departments
                .iter()
                .find(|reference| reference.number == 2)
                .expect("department 2")
                .status,
            ReferenceConfirmationStatus::Unverified
        );
    }

    #[test]
    fn all_required_refs_confirmed_with_excluded_records_is_ready_with_skips() {
        let config = VerificationConfig {
            confirmed_departments: vec![1],
            confirmed_groups: vec!["1:997".to_string()],
            confirmed_label_formats: vec![],
        };

        let readiness =
            evaluate_reference_readiness(&[plu(1, 1, 997)], &config, 4).expect("readiness");

        assert_eq!(readiness.result, ReadinessResult::ReadyWithSkips);
        assert!(readiness.is_ready());
    }

    #[test]
    fn raw_label_format_zero_requires_effective_label_format_one_confirmation() {
        let config = VerificationConfig {
            confirmed_departments: vec![1],
            confirmed_groups: vec!["1:997".to_string()],
            confirmed_label_formats: vec![1],
        };
        let mut plu = plu(1, 1, 997);
        plu.label_format = Some(0);

        let readiness = evaluate_reference_readiness(&[plu], &config, 0).expect("readiness");

        assert_eq!(readiness.result, ReadinessResult::Ready);
        assert_eq!(readiness.label_formats[0].number, 1);
        assert_eq!(
            readiness.label_formats[0].status,
            ReferenceConfirmationStatus::ManuallyConfirmed
        );
    }

    #[test]
    fn stale_confirmations_are_warnings_only() {
        let config = VerificationConfig {
            confirmed_departments: vec![1, 99],
            confirmed_groups: vec!["1:997".to_string(), "1:998".to_string()],
            confirmed_label_formats: vec![6, 99],
        };
        let mut plu = plu(1, 1, 997);
        plu.label_format = Some(6);

        let readiness = evaluate_reference_readiness(&[plu], &config, 0).expect("readiness");

        assert_eq!(readiness.result, ReadinessResult::Ready);
        assert_eq!(readiness.stale_confirmations.len(), 3);
        assert!(
            readiness
                .stale_confirmations
                .iter()
                .any(|warning| warning.message.contains("Label Format 99"))
        );
    }

    #[test]
    fn missing_prerequisite_group_message_is_actionable() {
        let reference = RequiredReference {
            department_number: 1,
            group_number: 997,
            source_plu_numbers: vec![1],
            status: ReferenceStatus::NotFound,
        };

        let message = reference.missing_group_message();

        assert!(message.contains("Department reference: 1"));
        assert!(message.contains("Group reference: 997"));
        assert!(message.contains("Create or import this group in DIGIweb"));
    }

    #[test]
    fn empty_maingroup_source_does_not_result_in_fabricated_group_name() {
        let refs = collect_required_references(&[plu(1, 1, 997)]);
        let message = refs[0].missing_group_message();

        assert!(!message.contains("Group 997"));
        assert!(!message.contains("grp997"));
        assert!(!message.contains("Unknown Group"));
    }
}
