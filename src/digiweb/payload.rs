use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use crate::config::DigiwebConfig;
use crate::error::AppError;
use crate::models::nutrition::NutritionFact;
use crate::models::plu::{Plu, PriceMode};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DigiwebPluPayload {
    pub storeno: u32,
    pub pluno: u64,
    pub pludepartmentno: u32,
    pub plugroupno: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plubarcodetype: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plubarcoderefno: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plubarcodedata: Option<String>,
    pub plucommname: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub plutexts: Vec<DigiwebPluTextPayload>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pluingredients: Option<String>,
    pub plupricemode: u8,
    #[serde(with = "rust_decimal::serde::float")]
    pub pluunitprice: Decimal,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pluusingdateprint: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pluusingdateterm: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pluadditionaldatas: Option<DigiwebPluAdditionalDataPayload>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plunft: Option<DigiwebPluNftPayload>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DigiwebPluTextPayload {
    pub plutextindex: u8,
    pub plutextval: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DigiwebPluAdditionalDataPayload {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub keylabel: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DigiwebPluNftPayload {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub data: Vec<DigiwebPluNftDataPayload>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DigiwebPluNftDataPayload {
    pub row: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data1: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data2: Option<String>,
    pub name: String,
}

impl DigiwebPluPayload {
    pub fn from_plu(plu: &Plu, config: &DigiwebConfig) -> Result<Self, AppError> {
        let pludepartmentno = plu.department_number.ok_or_else(|| {
            AppError::ValidationPayload(format!(
                "PLU {} is missing required pludepartmentno",
                plu.plu_number
            ))
        })?;
        let plugroupno = plu.group_number.ok_or_else(|| {
            AppError::ValidationPayload(format!(
                "PLU {} is missing required plugroupno",
                plu.plu_number
            ))
        })?;
        let pluadditionaldatas = optional_additional_data(plu);
        let plunft = optional_nft(&plu.nutrition_facts);
        let barcode = plu.barcode.clone();

        Ok(Self {
            storeno: plu.store_number,
            pluno: plu.plu_number,
            pludepartmentno,
            plugroupno,
            plubarcodetype: optional_config_text(&config.plu_barcode_type)
                .filter(|_| barcode.is_some()),
            plubarcoderefno: optional_config_text(&config.plu_barcode_ref_no)
                .filter(|_| barcode.is_some()),
            plubarcodedata: barcode,
            plucommname: plu.name.clone(),
            plutexts: optional_texts(plu),
            pluingredients: plu.ingredients.clone(),
            plupricemode: price_mode_to_digiweb(plu.price_mode)?,
            pluunitprice: plu.price,
            pluusingdateprint: plu.expiration_days.map(|_| 1),
            pluusingdateterm: plu.expiration_days,
            pluadditionaldatas,
            plunft,
        })
    }
}

fn price_mode_to_digiweb(price_mode: PriceMode) -> Result<u8, AppError> {
    match price_mode {
        PriceMode::ByWeight => Ok(0),
        PriceMode::ByEach | PriceMode::FixedWeight => Ok(1),
        PriceMode::Unknown => Err(AppError::ValidationPayload(
            "unsupported DIGIweb plupricemode".to_string(),
        )),
    }
}

fn optional_texts(plu: &Plu) -> Vec<DigiwebPluTextPayload> {
    plu.short_description
        .as_ref()
        .map(|text| {
            vec![DigiwebPluTextPayload {
                plutextindex: 1,
                plutextval: text.clone(),
            }]
        })
        .unwrap_or_default()
}

fn optional_additional_data(plu: &Plu) -> Option<DigiwebPluAdditionalDataPayload> {
    plu.key_label
        .as_ref()
        .map(|key_label| DigiwebPluAdditionalDataPayload {
            keylabel: Some(key_label.clone()),
        })
}

fn optional_nft(facts: &[NutritionFact]) -> Option<DigiwebPluNftPayload> {
    if facts.is_empty() {
        return None;
    }
    Some(DigiwebPluNftPayload {
        data: facts
            .iter()
            .enumerate()
            .map(|(index, fact)| DigiwebPluNftDataPayload {
                row: (index + 1) as u32,
                data1: fact.amount.clone(),
                data2: fact.unit.clone(),
                name: fact.name.clone(),
            })
            .collect(),
    })
}

fn optional_config_text(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

#[cfg(test)]
mod tests {
    use rust_decimal::Decimal;

    use super::*;

    #[test]
    fn payload_serializes_with_digiweb_field_names_and_no_nulls() {
        let plu = Plu {
            plu_number: 10,
            store_number: 1,
            department_number: Some(2),
            group_number: Some(3),
            name: "Apples".to_string(),
            barcode: Some("12345".to_string()),
            price: Decimal::new(199, 2),
            price_mode: PriceMode::ByEach,
            short_description: Some("Fresh".to_string()),
            key_label: Some("APPLE".to_string()),
            expiration_days: Some(5),
            ingredients: Some("Apples".to_string()),
            nutrition_facts: vec![NutritionFact {
                name: "Sugar".to_string(),
                amount: Some("1.0".to_string()),
                unit: Some("g".to_string()),
            }],
            source_pluing_row_count: 1,
        };
        let mut config = DigiwebConfig::default();
        config.plu_barcode_ref_no = "29".to_string();

        let json =
            serde_json::to_string(&DigiwebPluPayload::from_plu(&plu, &config).expect("payload"))
                .expect("json");

        assert!(json.contains("\"storeno\":1"));
        assert!(json.contains("\"pluno\":10"));
        assert!(json.contains("\"pludepartmentno\":2"));
        assert!(json.contains("\"plugroupno\":3"));
        assert!(json.contains("\"plucommname\":\"Apples\""));
        assert!(json.contains("\"plupricemode\":1"));
        assert!(json.contains("\"pluunitprice\":1.99"));
        assert!(json.contains("\"pluingredients\":\"Apples\""));
        assert!(json.contains("\"plubarcoderefno\":\"29\""));
        assert!(!json.contains("null"));
    }
}
