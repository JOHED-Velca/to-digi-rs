use serde::{Deserialize, Serialize};

use crate::error::AppError;
use crate::models::plu::Plu;
use crate::validation::issue::{Severity, ValidationIssue};
use crate::validation::validator::ValidationReport;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SelectionMode {
    All,
    Limit,
    Test,
    Plu,
}

impl SelectionMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Limit => "limit",
            Self::Test => "test",
            Self::Plu => "plu",
        }
    }
}

impl Default for SelectionMode {
    fn default() -> Self {
        Self::All
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SelectionCriteria {
    pub limit: Option<usize>,
    pub requested_plu: Option<u64>,
    pub test_mode: bool,
}

impl SelectionCriteria {
    pub fn mode(self) -> SelectionMode {
        if self.requested_plu.is_some() {
            SelectionMode::Plu
        } else if self.test_mode {
            SelectionMode::Test
        } else if self.limit.is_some() {
            SelectionMode::Limit
        } else {
            SelectionMode::All
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SelectionFailure {
    NotFound {
        plu_number: u64,
    },
    NotEligible {
        plu_number: u64,
        reasons: Vec<String>,
    },
}

impl SelectionFailure {
    pub fn message(&self) -> String {
        match self {
            Self::NotFound { plu_number } => {
                format!("requested PLU {plu_number} was not found in plu.mdb")
            }
            Self::NotEligible {
                plu_number,
                reasons,
            } => format!(
                "requested PLU {plu_number} is not eligible for import: {}",
                if reasons.is_empty() {
                    "no eligibility reason was recorded".to_string()
                } else {
                    reasons.join("; ")
                }
            ),
        }
    }
}

pub fn select_eligible_plus<'a>(
    all_plus: &'a [Plu],
    valid_plus: &'a [Plu],
    row_issues: &[ValidationIssue],
    validation_report: &ValidationReport,
    criteria: SelectionCriteria,
) -> Result<Vec<&'a Plu>, SelectionFailure> {
    if let Some(plu_number) = criteria.requested_plu {
        if let Some(plu) = valid_plus.iter().find(|plu| plu.plu_number == plu_number) {
            return Ok(vec![plu]);
        }
        let exists = all_plus.iter().any(|plu| plu.plu_number == plu_number)
            || row_issues
                .iter()
                .any(|issue| issue.plu_number == Some(plu_number))
            || validation_report
                .issues
                .iter()
                .any(|issue| issue.plu_number == Some(plu_number));
        if exists {
            return Err(SelectionFailure::NotEligible {
                plu_number,
                reasons: reasons_for_plu(plu_number, row_issues, validation_report),
            });
        }
        return Err(SelectionFailure::NotFound { plu_number });
    }
    Ok(valid_plus
        .iter()
        .take(criteria.limit.unwrap_or(usize::MAX))
        .collect())
}

pub fn selection_error(failure: &SelectionFailure) -> AppError {
    AppError::ValidationPayload(failure.message())
}

fn reasons_for_plu(
    plu_number: u64,
    row_issues: &[ValidationIssue],
    validation_report: &ValidationReport,
) -> Vec<String> {
    let mut reasons = row_issues
        .iter()
        .filter(|issue| issue.plu_number == Some(plu_number))
        .map(issue_reason)
        .chain(
            validation_report
                .issues
                .iter()
                .filter(|issue| {
                    issue.plu_number == Some(plu_number) && issue.severity == Severity::Error
                })
                .map(issue_reason),
        )
        .collect::<Vec<_>>();
    reasons.sort();
    reasons.dedup();
    reasons
}

fn issue_reason(issue: &ValidationIssue) -> String {
    format!("{}: {}", issue.field, issue.message)
}

#[cfg(test)]
mod tests {
    use rust_decimal::Decimal;

    use super::*;
    use crate::models::plu::PriceMode;
    use crate::validation::issue::ValidationIssue;

    fn plu(plu_number: u64) -> Plu {
        Plu {
            plu_number,
            store_number: 1,
            department_number: Some(2),
            group_number: Some(997),
            source_department: Some("0002".to_string()),
            source_group: Some("997".to_string()),
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
            nutrition_profile: None,
            nutrition_remaps: Vec::new(),
            source_pluing_row_count: 0,
        }
    }

    #[test]
    fn exact_plu_selection_does_not_depend_on_order() {
        let plus = vec![plu(18), plu(721), plu(1)];

        let selected = select_eligible_plus(
            &plus,
            &plus,
            &[],
            &ValidationReport::default(),
            SelectionCriteria {
                limit: None,
                requested_plu: Some(721),
                test_mode: false,
            },
        )
        .expect("selected");

        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].plu_number, 721);
    }

    #[test]
    fn nonexistent_plu_reports_not_found() {
        let plus = vec![plu(18)];

        let err = select_eligible_plus(
            &plus,
            &plus,
            &[],
            &ValidationReport::default(),
            SelectionCriteria {
                limit: None,
                requested_plu: Some(721),
                test_mode: false,
            },
        )
        .expect_err("not found");

        assert_eq!(err.message(), "requested PLU 721 was not found in plu.mdb");
    }

    #[test]
    fn invalid_target_reports_reason_without_substitution() {
        let plus = vec![plu(20), plu(21)];
        let valid = vec![plus[0].clone()];
        let validation_report = ValidationReport {
            issues: vec![ValidationIssue::error(
                Some(21),
                "barcode",
                "duplicate barcode with PLU 20",
            )],
        };

        let err = select_eligible_plus(
            &plus,
            &valid,
            &[],
            &validation_report,
            SelectionCriteria {
                limit: None,
                requested_plu: Some(21),
                test_mode: false,
            },
        )
        .expect_err("invalid");

        assert!(err.message().contains("requested PLU 21 is not eligible"));
        assert!(err.message().contains("duplicate barcode"));
    }
}
