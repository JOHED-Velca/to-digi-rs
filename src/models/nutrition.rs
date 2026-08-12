use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NutritionFact {
    pub name: String,
    pub amount: Option<String>,
    pub unit: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NutritionRemapDetail {
    pub source_field: String,
    pub nutrient: String,
    pub value_role: String,
    pub effective_value: String,
    pub suppressed_from_ingredients: bool,
}
