use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use crate::models::nutrition::NutritionFact;
use crate::models::plu::Plu;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DigiwebPluPayload {
    pub plu_number: u64,
    pub store_number: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub department_number: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group_number: Option<u32>,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub barcode: Option<String>,
    #[serde(with = "rust_decimal::serde::str")]
    pub price: Decimal,
    pub price_mode: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub short_description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key_label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expiration_days: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ingredients: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub nutrition_facts: Vec<DigiwebNutritionFactPayload>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DigiwebNutritionFactPayload {
    pub name: String,
    #[serde(
        with = "rust_decimal::serde::str_option",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub amount: Option<Decimal>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
}

impl From<&Plu> for DigiwebPluPayload {
    fn from(value: &Plu) -> Self {
        Self {
            plu_number: value.plu_number,
            store_number: value.store_number,
            department_number: value.department_number,
            group_number: value.group_number,
            name: value.name.clone(),
            barcode: value.barcode.clone(),
            price: value.price,
            price_mode: value.price_mode.as_api_code().to_string(),
            short_description: value.short_description.clone(),
            key_label: value.key_label.clone(),
            expiration_days: value.expiration_days,
            ingredients: value.ingredients.clone(),
            nutrition_facts: value
                .nutrition_facts
                .iter()
                .map(DigiwebNutritionFactPayload::from)
                .collect(),
        }
    }
}

impl From<&NutritionFact> for DigiwebNutritionFactPayload {
    fn from(value: &NutritionFact) -> Self {
        Self {
            name: value.name.clone(),
            amount: value.amount,
            unit: value.unit.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use rust_decimal::Decimal;

    use super::*;
    use crate::models::plu::{Plu, PriceMode};

    #[test]
    fn payload_serializes_price_as_string() {
        let plu = Plu {
            plu_number: 10,
            store_number: 1,
            department_number: None,
            group_number: None,
            name: "Apples".to_string(),
            barcode: Some("12345".to_string()),
            price: Decimal::new(199, 2),
            price_mode: PriceMode::ByEach,
            short_description: None,
            key_label: None,
            expiration_days: None,
            ingredients: None,
            nutrition_facts: Vec::new(),
        };

        let json = serde_json::to_string(&DigiwebPluPayload::from(&plu)).expect("json");

        assert!(json.contains("\"price\":\"1.99\""));
        assert!(json.contains("\"priceMode\":\"BY_EACH\""));
    }
}
