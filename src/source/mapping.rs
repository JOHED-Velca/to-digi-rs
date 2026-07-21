use std::collections::HashMap;
use std::str::FromStr;

use rust_decimal::Decimal;

use crate::config::MappingConfig;
use crate::error::AppError;
use crate::models::nutrition::NutritionFact;
use crate::models::plu::{Plu, PriceMode};
use crate::source::{SourceDataset, SourceRow};

const PLU_NUMBER_COLUMNS: &[&str] = &["Plucode", "PLUNo", "PluNo", "PLU", "PLU_NO", "plu_number"];
const DEPARTMENT_COLUMNS: &[&str] = &[
    "Department",
    "DeptNo",
    "DepartmentNo",
    "DEPT",
    "department_number",
];
const GROUP_COLUMNS: &[&str] = &[
    "Category",
    "Main Group Code",
    "GroupNo",
    "GrpNo",
    "GROUP",
    "group_number",
];
const BARCODE_COLUMNS: &[&str] = &["Barcode", "BarCode", "JAN", "UPC", "barcode"];
const NAME_COLUMNS: &[&str] = &[
    "Name 1",
    "Name",
    "ProductName",
    "CommodityName",
    "PLUName",
    "name",
];
const NAME_LINE_COLUMNS: &[&str] = &["Name 1", "Name 2", "Name 3", "Name 4"];
const PRICE_COLUMNS: &[&str] = &["Price", "UnitPrice", "SellPrice", "price"];
const PRICE_MODE_COLUMNS: &[&str] = &[
    "PriceMode",
    "Price Mode",
    "UnitPriceFlag",
    "SalesMode",
    "price_mode",
];
const SHORT_DESCRIPTION_COLUMNS: &[&str] = &[
    "ShortDescription",
    "ShortDesc",
    "Description",
    "short_description",
];
const KEY_LABEL_COLUMNS: &[&str] = &["KeyLabel", "ButtonLabel", "KeyName", "key_label"];
const EXPIRATION_COLUMNS: &[&str] = &[
    "Use By Date",
    "Best Before",
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
const PLUING_NUTRITION_COLUMNS: &[(&str, &str, Option<&str>)] = &[
    ("Serving Size", "Serving Size", None),
    ("Servings Per Container", "Serving Container", None),
    ("Calories", "Calories", None),
    ("Calories From Fat", "Calories Fat", None),
    ("Total Fat", "Total Fat", Some("g")),
    ("Percent Total Fat", "Total Fat Daily Value", Some("%")),
    ("Saturated Fat", "Saturated Fat", Some("g")),
    (
        "Percent Saturated Fat",
        "Saturated Fat Daily Value",
        Some("%"),
    ),
    ("Cholesterol", "Cholesterol", Some("mg")),
    ("Percent Cholesterol", "Cholesterol Daily Value", Some("%")),
    ("Sodium", "Sodium", Some("mg")),
    ("Percent Sodium", "Sodium Daily Value", Some("%")),
    ("Total Carbohydrate", "Carbohydrate", Some("g")),
    (
        "Percent Total Carbohydrate",
        "Carbohydrate Daily Value",
        Some("%"),
    ),
    ("Dietary Fiber", "Fiber", Some("g")),
    ("Percent Dietary Fiber", "Fiber Daily Value", Some("%")),
    ("Sugar", "Sugar", Some("g")),
    ("Protein", "Protein", Some("g")),
    ("Iron", "Iron", None),
    ("Niacin", "Niacin", None),
    ("Riboflavin", "Riboflavin", None),
    ("Thiamin", "Thiamin", None),
    ("Calcium", "Calcium", None),
    ("Vitamin A", "Vitamin A", None),
    ("Vitamin C", "Vitamin C", None),
    ("Trans fat", "Trans Fat", Some("g")),
];

/// Source mapping assumptions:
///
/// - The main table defaults to `Pludata`; `PluIng` supplies both ingredient text and nutrition values.
/// - The exact table names are configurable in `config.toml`.
/// - Column mappings are intentionally limited to observed/common DCA-style names listed in the constants above.
/// - If required PLU number, name, or price columns are absent/empty, the row is not given a fabricated default.
/// - `Pludata` names are assembled from non-empty `Name 1` through `Name 4` values with DIGIweb `<br>` line breaks.
/// - `PluIng` ingredients are assembled from non-empty `Ing Name 1` through `Ing Name 99` values in numeric order.
/// - `PluIng` nutrition values are text in the inspected MDB and may contain zero padding. They are parsed as written with no unit conversion or decimal scaling.
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
        name: required_name(row)?,
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
        let ordered_parts = ordered_ingredient_parts(row);
        if ordered_parts.is_empty() {
            if let Some(text) = optional_text(row, INGREDIENT_TEXT_COLUMNS) {
                by_plu.entry(plu_number).or_default().push(text);
            }
        } else {
            by_plu.entry(plu_number).or_default().extend(ordered_parts);
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
        let plu_ing_facts = nutrition_from_pluing(row)?;
        if !plu_ing_facts.is_empty() {
            by_plu.entry(plu_number).or_default().extend(plu_ing_facts);
            continue;
        }

        if let Some(name) = optional_text(row, NUTRITION_NAME_COLUMNS) {
            let amount = optional_text(row, NUTRITION_AMOUNT_COLUMNS);
            by_plu.entry(plu_number).or_default().push(NutritionFact {
                name,
                amount,
                unit: optional_text(row, NUTRITION_UNIT_COLUMNS),
            });
        }
    }
    Ok(by_plu)
}

fn required_name(row: &SourceRow) -> Result<String, AppError> {
    let parts = NAME_LINE_COLUMNS
        .iter()
        .filter_map(|column| row.get(column))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    if !parts.is_empty() {
        return Ok(parts.join("<br>"));
    }
    required_text(row, NAME_COLUMNS, "product name")
}

fn ordered_ingredient_parts(row: &SourceRow) -> Vec<String> {
    (1..=99)
        .filter_map(|index| row.get(&format!("Ing Name {index}")).map(str::trim))
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn nutrition_from_pluing(row: &SourceRow) -> Result<Vec<NutritionFact>, AppError> {
    let mut facts = Vec::new();
    for (column, name, unit) in PLUING_NUTRITION_COLUMNS {
        let Some(value) = row
            .get(column)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            continue;
        };
        facts.push(NutritionFact {
            name: (*name).to_string(),
            amount: Some(value.to_string()),
            unit: unit.map(ToOwned::to_owned),
        });
    }
    Ok(facts)
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

    #[test]
    fn pluing_supplies_ordered_ingredients_and_nutrition() {
        let plu_row = SourceRow {
            table: "Pludata".to_string(),
            values: BTreeMap::from([
                ("Plucode".to_string(), "1001".to_string()),
                ("Department".to_string(), "2".to_string()),
                ("Category".to_string(), "3".to_string()),
                ("Name 1".to_string(), "Apple".to_string()),
                ("Name 2".to_string(), "Slices".to_string()),
                ("Price".to_string(), "1.99".to_string()),
                ("PriceMode".to_string(), "each".to_string()),
            ]),
        };
        let pluing_row = SourceRow {
            table: "PluIng".to_string(),
            values: BTreeMap::from([
                ("Plucode".to_string(), "1001".to_string()),
                ("Ing Name 1".to_string(), "Apples".to_string()),
                ("Ing Name 2".to_string(), " ".to_string()),
                ("Ing Name 3".to_string(), "Water".to_string()),
                ("Calories".to_string(), "008".to_string()),
                ("Sodium".to_string(), "690".to_string()),
            ]),
        };
        let dataset = SourceDataset {
            plu_rows: vec![plu_row],
            ingredient_rows: vec![pluing_row.clone()],
            nutrition_rows: vec![pluing_row],
        };

        let plus = normalize_dataset(&dataset, &MappingConfig::default(), 1).expect("normalize");

        assert_eq!(plus[0].name, "Apple<br>Slices");
        assert_eq!(plus[0].department_number, Some(2));
        assert_eq!(plus[0].group_number, Some(3));
        assert_eq!(plus[0].ingredients.as_deref(), Some("Apples\nWater"));
        assert!(
            plus[0]
                .nutrition_facts
                .iter()
                .any(|fact| fact.name == "Calories" && fact.amount.is_some())
        );
        assert!(
            plus[0]
                .nutrition_facts
                .iter()
                .any(|fact| fact.name == "Sodium" && fact.unit.as_deref() == Some("mg"))
        );
    }
}
