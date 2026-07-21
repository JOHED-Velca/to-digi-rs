use std::collections::{HashMap, HashSet};

use rust_decimal::Decimal;

use crate::models::plu::{Plu, PriceMode};
use crate::validation::issue::{Severity, ValidationIssue};

const MAX_PLU_NUMBER: u64 = 999_999;
const MAX_COMMODITY_LINES: usize = 5;
const MAX_COMMODITY_LINE_LEN: usize = 50;
const MAX_SHORT_DESCRIPTION_LEN: usize = 255;
const MAX_KEY_LABEL_LEN: usize = 24;
const MAX_INGREDIENTS_LEN: usize = 5000;
const MAX_EXPIRATION_DAYS: u32 = 999;

#[derive(Debug, Clone, Default)]
pub struct ValidationReport {
    pub issues: Vec<ValidationIssue>,
}

impl ValidationReport {
    pub fn error_count(&self) -> usize {
        self.issues
            .iter()
            .filter(|issue| issue.severity == Severity::Error)
            .count()
    }

    pub fn warning_count(&self) -> usize {
        self.issues
            .iter()
            .filter(|issue| issue.severity == Severity::Warning)
            .count()
    }

    #[allow(dead_code)]
    pub fn has_blocking_errors(&self) -> bool {
        self.error_count() > 0
    }
}

pub fn validate_plus(plus: &[Plu]) -> ValidationReport {
    let mut issues = Vec::new();
    let mut seen_plu_numbers = HashSet::new();
    let mut seen_barcodes: HashMap<String, u64> = HashMap::new();

    for plu in plus {
        if plu.plu_number == 0 {
            issues.push(ValidationIssue::error(
                Some(plu.plu_number),
                "plu_number",
                "PLU number must be greater than zero",
            ));
        }
        if plu.plu_number > MAX_PLU_NUMBER {
            issues.push(ValidationIssue::error(
                Some(plu.plu_number),
                "plu_number",
                format!("PLU number exceeds DIGIweb maximum {MAX_PLU_NUMBER}"),
            ));
        }
        if !seen_plu_numbers.insert(plu.plu_number) {
            issues.push(ValidationIssue::error(
                Some(plu.plu_number),
                "plu_number",
                "duplicate PLU number",
            ));
        }
        if plu.name.trim().is_empty() {
            issues.push(ValidationIssue::error(
                Some(plu.plu_number),
                "name",
                "product name is required",
            ));
        }
        for issue in validate_commodity_name(plu) {
            issues.push(issue);
        }
        if plu.price < Decimal::ZERO {
            issues.push(ValidationIssue::error(
                Some(plu.plu_number),
                "price",
                "price must not be negative",
            ));
        }
        if matches!(plu.price_mode, PriceMode::Unknown) {
            issues.push(ValidationIssue::error(
                Some(plu.plu_number),
                "price_mode",
                "unsupported or missing price mode",
            ));
        }
        match plu.department_number {
            Some(department) if (1..=99).contains(&department) => {}
            Some(_) => issues.push(ValidationIssue::error(
                Some(plu.plu_number),
                "department_number",
                "DIGIweb pludepartmentno must be in range 1..99",
            )),
            None => issues.push(ValidationIssue::error(
                Some(plu.plu_number),
                "department_number",
                "DIGIweb pludepartmentno is mandatory for plus/write",
            )),
        }
        match plu.group_number {
            Some(group) if (1..=99).contains(&group) => {}
            Some(_) => issues.push(ValidationIssue::error(
                Some(plu.plu_number),
                "group_number",
                "DIGIweb plugroupno must be in range 1..99",
            )),
            None => issues.push(ValidationIssue::error(
                Some(plu.plu_number),
                "group_number",
                "DIGIweb plugroupno is mandatory for plus/write",
            )),
        }
        if let Some(barcode) = &plu.barcode {
            if !barcode.chars().all(|ch| ch.is_ascii_digit()) {
                issues.push(ValidationIssue::error(
                    Some(plu.plu_number),
                    "barcode",
                    "barcode must contain only digits",
                ));
            }
            if barcode.len() > 32 {
                issues.push(ValidationIssue::error(
                    Some(plu.plu_number),
                    "barcode",
                    "barcode exceeds 32 characters",
                ));
            }
            if let Some(previous_plu) = seen_barcodes.insert(barcode.clone(), plu.plu_number) {
                issues.push(ValidationIssue::error(
                    Some(plu.plu_number),
                    "barcode",
                    format!("duplicate barcode also used by PLU {previous_plu}"),
                ));
            }
        }
        if let Some(expiration_days) = plu.expiration_days {
            if expiration_days > MAX_EXPIRATION_DAYS {
                issues.push(ValidationIssue::error(
                    Some(plu.plu_number),
                    "expiration_days",
                    format!("expiration days exceeds {MAX_EXPIRATION_DAYS}"),
                ));
            }
        }
        if let Some(short_description) = &plu.short_description {
            if short_description.chars().count() > MAX_SHORT_DESCRIPTION_LEN {
                issues.push(ValidationIssue::warning(
                    Some(plu.plu_number),
                    "short_description",
                    format!("short description exceeds {MAX_SHORT_DESCRIPTION_LEN} characters"),
                ));
            }
        }
        if let Some(key_label) = &plu.key_label {
            if key_label.chars().count() > MAX_KEY_LABEL_LEN {
                issues.push(ValidationIssue::warning(
                    Some(plu.plu_number),
                    "key_label",
                    format!("key label exceeds {MAX_KEY_LABEL_LEN} characters"),
                ));
            }
        }
        if let Some(ingredients) = &plu.ingredients {
            if ingredients.chars().count() > MAX_INGREDIENTS_LEN {
                issues.push(ValidationIssue::error(
                    Some(plu.plu_number),
                    "ingredients",
                    format!("DIGIweb pluingredients exceeds {MAX_INGREDIENTS_LEN} characters"),
                ));
            }
        }
        for fact in &plu.nutrition_facts {
            if fact.name.trim().is_empty() {
                issues.push(ValidationIssue::error(
                    Some(plu.plu_number),
                    "nutrition_facts",
                    "nutrition fact name is required when a fact is present",
                ));
            }
        }
    }

    ValidationReport { issues }
}

fn validate_commodity_name(plu: &Plu) -> Vec<ValidationIssue> {
    let mut issues = Vec::new();
    let lines = split_html_lines(&plu.name);
    if lines.len() > MAX_COMMODITY_LINES {
        issues.push(ValidationIssue::error(
            Some(plu.plu_number),
            "name",
            format!("DIGIweb plucommname exceeds {MAX_COMMODITY_LINES} lines"),
        ));
    }
    for line in lines {
        if line.chars().count() > MAX_COMMODITY_LINE_LEN {
            issues.push(ValidationIssue::error(
                Some(plu.plu_number),
                "name",
                format!("DIGIweb plucommname line exceeds {MAX_COMMODITY_LINE_LEN} characters"),
            ));
        }
    }
    issues
}

fn split_html_lines(value: &str) -> Vec<&str> {
    value.split("<br>").flat_map(|part| part.lines()).collect()
}

#[cfg(test)]
mod tests {
    use rust_decimal::Decimal;

    use super::*;
    use crate::models::plu::PriceMode;

    fn valid_plu(plu_number: u64) -> Plu {
        Plu {
            plu_number,
            store_number: 1,
            department_number: Some(1),
            group_number: Some(1),
            name: "Apples".to_string(),
            barcode: None,
            price: Decimal::new(199, 2),
            price_mode: PriceMode::ByWeight,
            short_description: None,
            key_label: None,
            expiration_days: None,
            ingredients: None,
            nutrition_facts: Vec::new(),
            source_pluing_row_count: 0,
        }
    }

    #[test]
    fn duplicate_plu_is_error() {
        let report = validate_plus(&[valid_plu(100), valid_plu(100)]);
        assert!(report.has_blocking_errors());
    }

    #[test]
    fn negative_price_is_error() {
        let mut plu = valid_plu(100);
        plu.price = Decimal::new(-1, 0);

        let report = validate_plus(&[plu]);

        assert!(
            report
                .issues
                .iter()
                .any(|issue| issue.field == "price" && issue.severity == Severity::Error)
        );
    }

    #[test]
    fn missing_group_is_error_for_digiweb_write() {
        let mut plu = valid_plu(100);
        plu.group_number = None;

        let report = validate_plus(&[plu]);

        assert!(
            report
                .issues
                .iter()
                .any(|issue| issue.field == "group_number" && issue.severity == Severity::Error)
        );
    }
}
