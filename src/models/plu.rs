use rust_decimal::Decimal;

use super::nutrition::NutritionFact;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PriceMode {
    ByWeight,
    ByEach,
    FixedWeight,
    Unknown,
}

impl PriceMode {
    pub fn from_source(value: Option<&str>) -> Self {
        let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
            return Self::Unknown;
        };
        match value.to_ascii_lowercase().as_str() {
            "0" | "weight" | "byweight" | "by_weight" | "weighed" => Self::ByWeight,
            "1" | "each" | "byeach" | "by_each" | "count" => Self::ByEach,
            "2" | "fixed" | "fixedweight" | "fixed_weight" => Self::FixedWeight,
            _ => Self::Unknown,
        }
    }

    pub fn as_api_code(self) -> &'static str {
        match self {
            Self::ByWeight => "BY_WEIGHT",
            Self::ByEach => "BY_EACH",
            Self::FixedWeight => "FIXED_WEIGHT",
            Self::Unknown => "UNKNOWN",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Plu {
    pub plu_number: u64,
    pub store_number: u32,
    pub department_number: Option<u32>,
    pub group_number: Option<u32>,
    pub name: String,
    pub barcode: Option<String>,
    pub price: Decimal,
    pub price_mode: PriceMode,
    pub short_description: Option<String>,
    pub key_label: Option<String>,
    pub expiration_days: Option<u32>,
    pub ingredients: Option<String>,
    pub nutrition_facts: Vec<NutritionFact>,
}
