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
    pub plubarcodetype: String,
    pub plubarcoderefno: String,
    pub plubarcodedata: String,
    pub plucommname: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub plutexts: Vec<DigiwebPluTextPayload>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pluingredients: Option<String>,
    pub plupricemode: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plupricecalcmethod: Option<u8>,
    #[serde(with = "rust_decimal::serde::float")]
    pub pluunitprice: Decimal,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pluquantity: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pluquantitysymbol: Option<u32>,
    #[serde(
        with = "optional_decimal_float",
        skip_serializing_if = "Option::is_none"
    )]
    pub plutare: Option<Decimal>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pludiscounttype: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plupackingdateprint: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plupackingtimeprint: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plusellingdateprint: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plusellingdateterm: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pluusingdateprint: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pluusingdateterm: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plulabelformat: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plutraceability: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pluadditionaldatas: Option<DigiwebPluAdditionalDataPayload>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pluimages: Option<DigiwebPluImagesPayload>,
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct DigiwebPluImagesPayload {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pluimage1: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pluimage2: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pluimage3: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pluimage4: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pluimage5: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pluimage6: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pluimage7: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pluimage8: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pluimage9: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pluimage10: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DigiwebPluNftPayload {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text1: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text2: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text3: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text4: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text5: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub data: Vec<DigiwebPluNftDataPayload>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DigiwebPluNftDataPayload {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data1: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data2: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data3: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data4: Option<String>,
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
        let barcode = required_barcode_value(plu, "plubarcodedata", plu.barcode.as_deref())?;
        let barcode_type = optional_config_text(&config.plu_barcode_type)
            .or_else(|| plu.barcode_type.clone())
            .ok_or_else(|| {
                AppError::ValidationPayload(format!(
                    "PLU {} is missing required plubarcodetype",
                    plu.plu_number
                ))
            })?;
        let barcode_ref_no = optional_config_text(&config.plu_barcode_ref_no)
            .or_else(|| plu.barcode_ref_no.clone())
            .ok_or_else(|| {
                AppError::ValidationPayload(format!(
                    "PLU {} is missing required plubarcoderefno",
                    plu.plu_number
                ))
            })?;

        Ok(Self {
            storeno: plu.store_number,
            pluno: plu.plu_number,
            pludepartmentno,
            plugroupno,
            plubarcodetype: barcode_type,
            plubarcoderefno: barcode_ref_no,
            plubarcodedata: barcode,
            plucommname: plu.name.clone(),
            plutexts: optional_texts(plu),
            pluingredients: plu.ingredients.clone(),
            plupricemode: price_mode_to_digiweb(plu.price_mode)?,
            plupricecalcmethod: plu.price_calc_method,
            pluunitprice: plu.price,
            pluquantity: plu.quantity,
            pluquantitysymbol: plu.quantity_symbol,
            plutare: plu.tare,
            pludiscounttype: plu.discount_type,
            plupackingdateprint: plu.packing_date_print,
            plupackingtimeprint: plu.packing_time_print,
            plusellingdateprint: plu.selling_date_print,
            plusellingdateterm: plu.selling_date_term,
            pluusingdateprint: plu.expiration_days.map(|_| 1),
            pluusingdateterm: plu.expiration_days,
            plulabelformat: plu.label_format,
            plutraceability: plu.traceability,
            pluadditionaldatas,
            pluimages: Some(DigiwebPluImagesPayload::default()),
            plunft,
        })
    }
}

fn required_barcode_value(plu: &Plu, field: &str, value: Option<&str>) -> Result<String, AppError> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .ok_or_else(|| {
            AppError::ValidationPayload(format!(
                "PLU {} is missing required {field}",
                plu.plu_number
            ))
        })
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
    Some(DigiwebPluAdditionalDataPayload {
        keylabel: Some(plu.key_label.clone().unwrap_or_else(|| ".".to_string())),
    })
}

fn optional_nft(facts: &[NutritionFact]) -> Option<DigiwebPluNftPayload> {
    if facts.is_empty() {
        return None;
    }
    Some(DigiwebPluNftPayload {
        image: Some(String::new()),
        text1: Some(String::new()),
        text2: Some(String::new()),
        text3: Some(String::new()),
        text4: Some(String::new()),
        text5: Some(String::new()),
        data: facts
            .iter()
            .map(|fact| DigiwebPluNftDataPayload {
                data1: fact.amount.clone(),
                data2: fact.unit.clone(),
                data3: None,
                data4: None,
                name: fact.name.clone(),
            })
            .collect(),
    })
}

mod optional_decimal_float {
    use rust_decimal::Decimal;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(value: &Option<Decimal>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match value {
            Some(value) => rust_decimal::serde::float::serialize(value, serializer),
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Option<Decimal>, D::Error>
    where
        D: Deserializer<'de>,
    {
        Option::<f64>::deserialize(deserializer)?
            .map(|value| {
                Decimal::try_from(value).map_err(|err| serde::de::Error::custom(err.to_string()))
            })
            .transpose()
    }
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
            source_department: Some("0002".to_string()),
            source_group: Some("3".to_string()),
            group_default_applied: false,
            name: "Apples".to_string(),
            barcode: Some("12345".to_string()),
            barcode_type: Some("5".to_string()),
            barcode_ref_no: Some("5".to_string()),
            source_barcode: Some("12345".to_string()),
            source_barcode_format: Some("05".to_string()),
            source_flag_data: Some("0".to_string()),
            price: Decimal::new(199, 2),
            price_mode: PriceMode::ByEach,
            price_calc_method: Some(0),
            quantity: Some(2),
            quantity_symbol: Some(1),
            tare: Some(Decimal::ZERO),
            discount_type: Some(0),
            packing_date_print: Some(1),
            packing_time_print: Some(1),
            selling_date_print: Some(1),
            selling_date_term: Some(5),
            label_format: Some(4),
            traceability: Some(0),
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
        let config = DigiwebConfig::default();

        let json =
            serde_json::to_string(&DigiwebPluPayload::from_plu(&plu, &config).expect("payload"))
                .expect("json");

        assert!(json.contains("\"storeno\":1"));
        assert!(json.contains("\"pluno\":10"));
        assert!(json.contains("\"pludepartmentno\":2"));
        assert!(json.contains("\"plugroupno\":3"));
        assert!(json.contains("\"plucommname\":\"Apples\""));
        assert!(json.contains("\"plupricemode\":1"));
        assert!(json.contains("\"plupricecalcmethod\":0"));
        assert!(json.contains("\"pluunitprice\":1.99"));
        assert!(json.contains("\"pluquantity\":2"));
        assert!(json.contains("\"pluquantitysymbol\":1"));
        assert!(json.contains("\"plutare\":0.0"));
        assert!(json.contains("\"pludiscounttype\":0"));
        assert!(json.contains("\"plupackingdateprint\":1"));
        assert!(json.contains("\"plupackingtimeprint\":1"));
        assert!(json.contains("\"plusellingdateprint\":1"));
        assert!(json.contains("\"plusellingdateterm\":5"));
        assert!(json.contains("\"plulabelformat\":4"));
        assert!(json.contains("\"plutraceability\":0"));
        assert!(json.contains("\"pluingredients\":\"Apples\""));
        assert!(json.contains("\"plubarcodetype\":\"5\""));
        assert!(json.contains("\"plubarcoderefno\":\"5\""));
        assert!(json.contains("\"plubarcodedata\":\"12345\""));
        assert!(json.contains("\"pluadditionaldatas\":{\"keylabel\":\"APPLE\"}"));
        assert!(json.contains("\"pluimages\":{}"));
        assert!(json.contains("\"plunft\":{\"image\":\"\",\"text1\":\"\""));
        assert!(!json.contains("\"row\""));
        assert!(!json.contains("null"));
    }

    #[test]
    fn payload_top_level_shape_matches_vb_plu_object_without_wrapper() {
        let plu = Plu {
            plu_number: 1,
            store_number: 1,
            department_number: Some(1),
            group_number: Some(997),
            source_department: Some("0001".to_string()),
            source_group: Some("997".to_string()),
            group_default_applied: false,
            name: "BALERON".to_string(),
            barcode: Some("0200001".to_string()),
            barcode_type: Some("5".to_string()),
            barcode_ref_no: Some("5".to_string()),
            source_barcode: Some("1".to_string()),
            source_barcode_format: Some("05".to_string()),
            source_flag_data: Some("02".to_string()),
            price: Decimal::new(1690, 2),
            price_mode: PriceMode::ByWeight,
            price_calc_method: Some(0),
            quantity: Some(0),
            quantity_symbol: Some(0),
            tare: Some(Decimal::ZERO),
            discount_type: Some(0),
            packing_date_print: Some(0),
            packing_time_print: Some(0),
            selling_date_print: Some(0),
            selling_date_term: Some(0),
            label_format: None,
            traceability: Some(0),
            short_description: None,
            key_label: None,
            expiration_days: Some(0),
            ingredients: Some("<b>Ingredient</b> pork".to_string()),
            nutrition_facts: vec![NutritionFact {
                name: "sodium".to_string(),
                amount: Some("690".to_string()),
                unit: Some("29".to_string()),
            }],
            source_pluing_row_count: 1,
        };
        let config = DigiwebConfig {
            plu_barcode_type: "1".to_string(),
            plu_barcode_ref_no: "1".to_string(),
            ..DigiwebConfig::default()
        };

        let value =
            serde_json::to_value(DigiwebPluPayload::from_plu(&plu, &config).expect("payload"))
                .expect("json value");

        assert!(value.get("data").is_none());
        assert!(value.as_array().is_none());
        assert_eq!(value["plucommname"], "BALERON");
        assert_eq!(value["plubarcodetype"], "1");
        assert_eq!(value["plubarcoderefno"], "1");
        assert_eq!(value["plubarcodedata"], "0200001");
        assert_eq!(value["plunft"]["data"][0]["name"], "sodium");
        assert_eq!(value["plunft"]["data"][0]["data1"], "690");
        assert_eq!(value["plunft"]["data"][0]["data2"], "29");
        assert!(value["plunft"]["data"][0].get("row").is_none());
        assert_eq!(value["pluadditionaldatas"]["keylabel"], ".");
    }

    #[test]
    fn empty_config_override_does_not_erase_derived_barcode_type() {
        let plu = Plu {
            plu_number: 1,
            store_number: 1,
            department_number: Some(1),
            group_number: Some(997),
            source_department: Some("0001".to_string()),
            source_group: Some("997".to_string()),
            group_default_applied: false,
            name: "BALERON".to_string(),
            barcode: Some("0200001".to_string()),
            barcode_type: Some("5".to_string()),
            barcode_ref_no: Some("5".to_string()),
            source_barcode: Some("1".to_string()),
            source_barcode_format: Some("05".to_string()),
            source_flag_data: Some("02".to_string()),
            price: Decimal::new(1690, 2),
            price_mode: PriceMode::ByWeight,
            price_calc_method: Some(0),
            quantity: Some(0),
            quantity_symbol: Some(0),
            tare: Some(Decimal::ZERO),
            discount_type: Some(0),
            packing_date_print: Some(0),
            packing_time_print: Some(0),
            selling_date_print: Some(0),
            selling_date_term: Some(0),
            label_format: None,
            traceability: Some(0),
            short_description: None,
            key_label: None,
            expiration_days: Some(0),
            ingredients: None,
            nutrition_facts: Vec::new(),
            source_pluing_row_count: 0,
        };
        let config = DigiwebConfig::default();

        let payload = DigiwebPluPayload::from_plu(&plu, &config).expect("payload");

        assert_eq!(payload.plubarcodetype, "5");
        assert_eq!(payload.plubarcoderefno, "5");
    }

    #[test]
    fn required_barcode_type_never_serializes_as_null() {
        let plu = Plu {
            plu_number: 1,
            store_number: 1,
            department_number: Some(1),
            group_number: Some(997),
            source_department: Some("0001".to_string()),
            source_group: Some("997".to_string()),
            group_default_applied: false,
            name: "BALERON".to_string(),
            barcode: Some("0200001".to_string()),
            barcode_type: None,
            barcode_ref_no: Some("5".to_string()),
            source_barcode: Some("1".to_string()),
            source_barcode_format: Some("05".to_string()),
            source_flag_data: Some("02".to_string()),
            price: Decimal::new(1690, 2),
            price_mode: PriceMode::ByWeight,
            price_calc_method: Some(0),
            quantity: Some(0),
            quantity_symbol: Some(0),
            tare: Some(Decimal::ZERO),
            discount_type: Some(0),
            packing_date_print: Some(0),
            packing_time_print: Some(0),
            selling_date_print: Some(0),
            selling_date_term: Some(0),
            label_format: None,
            traceability: Some(0),
            short_description: None,
            key_label: None,
            expiration_days: Some(0),
            ingredients: None,
            nutrition_facts: Vec::new(),
            source_pluing_row_count: 0,
        };

        let err = DigiwebPluPayload::from_plu(&plu, &DigiwebConfig::default())
            .expect_err("missing barcode type");

        assert!(err.to_string().contains("plubarcodetype"));
    }
}
