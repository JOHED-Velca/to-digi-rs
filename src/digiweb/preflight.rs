use std::collections::BTreeMap;

use crate::models::plu::Plu;

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
            barcode: None,
            price: Decimal::new(100, 2),
            price_mode: PriceMode::ByEach,
            price_calc_method: None,
            quantity: None,
            quantity_symbol: None,
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
