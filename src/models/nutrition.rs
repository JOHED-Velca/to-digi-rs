#[derive(Debug, Clone, PartialEq)]
pub struct NutritionFact {
    pub name: String,
    pub amount: Option<String>,
    pub unit: Option<String>,
}
