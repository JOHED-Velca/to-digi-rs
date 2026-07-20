use rust_decimal::Decimal;

#[derive(Debug, Clone, PartialEq)]
pub struct NutritionFact {
    pub name: String,
    pub amount: Option<Decimal>,
    pub unit: Option<String>,
}
