use std::collections::HashMap;
use std::str::FromStr;

use rust_decimal::Decimal;

use crate::config::MappingConfig;
use crate::error::AppError;
use crate::models::nutrition::NutritionFact;
use crate::models::plu::{Plu, PriceMode};
use crate::source::{SourceDataset, SourceRow};

const PLU_NUMBER_COLUMNS: &[&str] = &["PLUNo", "PluNo", "PLU", "PLU_NO", "plu_number"];
const DEPARTMENT_COLUMNS: &[&str] = &["DeptNo", "DepartmentNo", "DEPT", "department_number"];
const GROUP_COLUMNS: &[&str] = &["GroupNo", "GrpNo", "GROUP", "group_number"];
const BARCODE_COLUMNS: &[&str] = &["Barcode", "BarCode", "JAN", "UPC", "barcode"];
const NAME_COLUMNS: &[&str] = &["Name", "ProductName", "CommodityName", "PLUName", "name"];
const PRICE_COLUMNS: &[&str] = &["Price", "UnitPrice", "SellPrice", "price"];
const PRICE_MODE_COLUMNS: &[&str] = &["PriceMode", "UnitPriceFlag", "SalesMode", "price_mode"];
const SHORT_DESCRIPTION_COLUMNS: &[&str] = &[
    "ShortDescription",
    "ShortDesc",
    "Description",
    "short_description",
];
const KEY_LABEL_COLUMNS: &[&str] = &["KeyLabel", "ButtonLabel", "KeyName", "key_label"];
const EXPIRATION_COLUMNS: &[&str] = &[
    "ExpirationDays",
    "UseByDays",
    "ShelfLife",
    "expiration_days",
];
const INGREDIENT_TEXT_COLUMNS: &[&str] = &[
    "Ingredients",
    "Ingredient",
    "Text",
    "IngText",
    "ingredients",
];
const NUTRITION_NAME_COLUMNS: &[&str] = &["Name", "Nutrient", "NutritionName", "name"];
const NUTRITION_AMOUNT_COLUMNS: &[&str] = &["Amount", "Value", "Qty", "amount"];
const NUTRITION_UNIT_COLUMNS: &[&str] = &["Unit", "Uom", "unit"];

/// Source mapping assumptions:
///
/// - The main table defaults to `Pludata`; ingredients default to optional `PluIng`; nutrition defaults to optional `PluNut`.
/// - The exact table names are configurable in `config.toml`.
/// - Column mappings are intentionally limited to observed/common DCA-style names listed in the constants above.
/// - If required PLU number, name, or price columns are absent/empty, the row is not given a fabricated default.
/// - Unknown DIGIweb-specific field limits are enforced in validation with conservative defaults only where documented in code.
pub fn normalize_dataset(
    dataset: &SourceDataset,
    mapping: &MappingConfig,
    store_number: u32,
) -> Result<Vec<Plu>, AppError> {
    let ingredients = normalize_ingredients(&dataset.ingredient_rows)?;
    let nutrition = normalize_nutrition(&dataset.nutrition_rows)?;
    let mut plus = Vec::with_capacity(dataset.plu_rows.len());
    for row in &dataset.plu_rows {
        plus.push(normalize_plu(
            row,
            mapping,
            store_number,
            &ingredients,
            &nutrition,
        )?);
    }
    Ok(plus)
}

fn normalize_plu(
    row: &SourceRow,
    _mapping: &MappingConfig,
    store_number: u32,
    ingredients: &HashMap<u64, String>,
    nutrition: &HashMap<u64, Vec<NutritionFact>>,
) -> Result<Plu, AppError> {
    let plu_number = parse_required_u64(row, PLU_NUMBER_COLUMNS, "PLU number")?;
    let price = parse_required_decimal(row, PRICE_COLUMNS, "price")?;
    let price_mode = PriceMode::from_source(find_value(row, PRICE_MODE_COLUMNS));
    Ok(Plu {
        plu_number,
        store_number,
        department_number: parse_optional_u32(row, DEPARTMENT_COLUMNS, "department number")?,
        group_number: parse_optional_u32(row, GROUP_COLUMNS, "group number")?,
        name: required_text(row, NAME_COLUMNS, "product name")?,
        barcode: optional_text(row, BARCODE_COLUMNS),
        price,
        price_mode,
        short_description: optional_text(row, SHORT_DESCRIPTION_COLUMNS),
        key_label: optional_text(row, KEY_LABEL_COLUMNS),
        expiration_days: parse_optional_u32(row, EXPIRATION_COLUMNS, "expiration days")?,
        ingredients: ingredients.get(&plu_number).cloned(),
        nutrition_facts: nutrition.get(&plu_number).cloned().unwrap_or_default(),
    })
}

fn normalize_ingredients(rows: &[SourceRow]) -> Result<HashMap<u64, String>, AppError> {
    let mut by_plu: HashMap<u64, Vec<String>> = HashMap::new();
    for row in rows {
        let plu_number = parse_required_u64(row, PLU_NUMBER_COLUMNS, "ingredient PLU number")?;
        if let Some(text) = optional_text(row, INGREDIENT_TEXT_COLUMNS) {
            by_plu.entry(plu_number).or_default().push(text);
        }
    }
    Ok(by_plu
        .into_iter()
        .map(|(plu, parts)| (plu, parts.join("\n")))
        .collect())
}

fn normalize_nutrition(rows: &[SourceRow]) -> Result<HashMap<u64, Vec<NutritionFact>>, AppError> {
    let mut by_plu: HashMap<u64, Vec<NutritionFact>> = HashMap::new();
    for row in rows {
        let plu_number = parse_required_u64(row, PLU_NUMBER_COLUMNS, "nutrition PLU number")?;
        let name = optional_text(row, NUTRITION_NAME_COLUMNS);
        if let Some(name) = name {
            let amount = match optional_text(row, NUTRITION_AMOUNT_COLUMNS) {
                Some(value) => Some(parse_decimal_value(&value, "nutrition amount")?),
                None => None,
            };
            by_plu.entry(plu_number).or_default().push(NutritionFact {
                name,
                amount,
                unit: optional_text(row, NUTRITION_UNIT_COLUMNS),
            });
        }
    }
    Ok(by_plu)
}

fn find_value<'a>(row: &'a SourceRow, candidates: &[&str]) -> Option<&'a str> {
    candidates
        .iter()
        .find_map(|candidate| row.get(candidate))
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn required_text(row: &SourceRow, candidates: &[&str], field: &str) -> Result<String, AppError> {
    optional_text(row, candidates).ok_or_else(|| {
        AppError::MdbExport(format!(
            "{} is missing in table '{}'; checked columns: {}",
            field,
            row.table,
            candidates.join(", ")
        ))
    })
}

fn optional_text(row: &SourceRow, candidates: &[&str]) -> Option<String> {
    find_value(row, candidates).map(ToOwned::to_owned)
}

fn parse_required_u64(row: &SourceRow, candidates: &[&str], field: &str) -> Result<u64, AppError> {
    let value = required_text(row, candidates, field)?;
    value.parse::<u64>().map_err(|err| {
        AppError::MdbExport(format!(
            "{} value '{}' in table '{}' is invalid: {}",
            field, value, row.table, err
        ))
    })
}

fn parse_optional_u32(
    row: &SourceRow,
    candidates: &[&str],
    field: &str,
) -> Result<Option<u32>, AppError> {
    match optional_text(row, candidates) {
        Some(value) => value.parse::<u32>().map(Some).map_err(|err| {
            AppError::MdbExport(format!(
                "{} value '{}' in table '{}' is invalid: {}",
                field, value, row.table, err
            ))
        }),
        None => Ok(None),
    }
}

fn parse_required_decimal(
    row: &SourceRow,
    candidates: &[&str],
    field: &str,
) -> Result<Decimal, AppError> {
    let value = required_text(row, candidates, field)?;
    parse_decimal_value(&value, field)
}

fn parse_decimal_value(value: &str, field: &str) -> Result<Decimal, AppError> {
    Decimal::from_str(value.trim()).map_err(|err| {
        AppError::MdbExport(format!(
            "{} value '{}' is not a valid decimal: {}",
            field, value, err
        ))
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    #[test]
    fn normalizes_source_row() {
        let row = SourceRow {
            table: "Pludata".to_string(),
            values: BTreeMap::from([
                ("PLUNo".to_string(), "1001".to_string()),
                ("DeptNo".to_string(), "10".to_string()),
                ("Name".to_string(), "Apples".to_string()),
                ("Price".to_string(), "1.99".to_string()),
                ("PriceMode".to_string(), "weight".to_string()),
            ]),
        };
        let dataset = SourceDataset {
            plu_rows: vec![row],
            ingredient_rows: Vec::new(),
            nutrition_rows: Vec::new(),
        };

        let plus = normalize_dataset(&dataset, &MappingConfig::default(), 1).expect("normalize");

        assert_eq!(plus[0].plu_number, 1001);
        assert_eq!(plus[0].department_number, Some(10));
        assert_eq!(plus[0].price_mode, PriceMode::ByWeight);
    }
}
