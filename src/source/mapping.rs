use std::collections::{HashMap, HashSet};
use std::str::FromStr;

use rust_decimal::Decimal;

use crate::config::MappingConfig;
use crate::error::AppError;
use crate::models::nutrition::NutritionFact;
use crate::models::plu::{Plu, PriceMode};
use crate::source::schema::MdbSchema;
use crate::source::{SourceDataset, SourceRow};
use crate::validation::issue::ValidationIssue;

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
const NAME_COLUMNS: &[&str] = &["Name", "ProductName", "CommodityName", "PLUName", "name"];
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
#[derive(Debug, Clone, Default)]
pub struct NormalizationReport {
    pub plus: Vec<Plu>,
    pub row_issues: Vec<ValidationIssue>,
    pub orphan_pluing_rows: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct JoinKey {
    plu_number: u64,
    department_number: u32,
}

pub fn validate_source_schema(schema: &MdbSchema, mapping: &MappingConfig) -> Result<(), AppError> {
    let columns = schema.columns.get(&mapping.main_plu_table).ok_or_else(|| {
        AppError::MdbSchema(format!(
            "columns for main PLU table '{}' were not inspected",
            mapping.main_plu_table
        ))
    })?;
    require_source_column(columns, "Plucode", &mapping.main_plu_table)?;
    require_source_column(columns, "Department", &mapping.main_plu_table)?;
    require_source_column(columns, "Name 1", &mapping.main_plu_table)?;
    require_source_column(columns, "Price", &mapping.main_plu_table)?;
    Ok(())
}

pub fn normalize_dataset(
    dataset: &SourceDataset,
    mapping: &MappingConfig,
    store_number: u32,
) -> Result<NormalizationReport, AppError> {
    let ingredients = normalize_ingredients(&dataset.ingredient_rows)?;
    let nutrition = normalize_nutrition(&dataset.nutrition_rows)?;
    let pluing_counts = pluing_counts_by_key(&dataset.ingredient_rows);
    let mut plus = Vec::with_capacity(dataset.plu_rows.len());
    let mut row_issues = Vec::new();
    for row in &dataset.plu_rows {
        match normalize_plu(
            row,
            mapping,
            store_number,
            &ingredients,
            &nutrition,
            &pluing_counts,
        ) {
            Ok(plu) => plus.push(plu),
            Err(issue) => row_issues.push(issue),
        }
    }
    let active_keys = plus
        .iter()
        .filter_map(|plu| {
            plu.department_number.map(|department_number| JoinKey {
                plu_number: plu.plu_number,
                department_number,
            })
        })
        .collect::<HashSet<_>>();
    let orphan_pluing_rows = dataset
        .ingredient_rows
        .iter()
        .filter(|row| row_join_key(row).is_none_or(|key| !active_keys.contains(&key)))
        .count();
    Ok(NormalizationReport {
        plus,
        row_issues,
        orphan_pluing_rows,
    })
}

fn normalize_plu(
    row: &SourceRow,
    _mapping: &MappingConfig,
    store_number: u32,
    ingredients: &HashMap<JoinKey, String>,
    nutrition: &HashMap<JoinKey, Vec<NutritionFact>>,
    pluing_counts: &HashMap<JoinKey, usize>,
) -> Result<Plu, ValidationIssue> {
    let plu_number = parse_required_u64(row, PLU_NUMBER_COLUMNS, "PLU number").map_err(|err| {
        row_issue(
            row,
            None,
            "plu_number",
            format!("invalid PLU number: {err}"),
        )
    })?;
    let department_number = parse_optional_u32(row, DEPARTMENT_COLUMNS, "department number")
        .map_err(|err| {
            row_issue(
                row,
                Some(plu_number),
                "department_number",
                format!("invalid department: {err}"),
            )
        })?;
    let key = department_number.map(|department_number| JoinKey {
        plu_number,
        department_number,
    });
    let price = parse_required_decimal(row, PRICE_COLUMNS, "price").map_err(|err| {
        row_issue(
            row,
            Some(plu_number),
            "price",
            format!("invalid price: {err}"),
        )
    })?;
    let price_mode = PriceMode::from_source(find_value(row, PRICE_MODE_COLUMNS));
    let name = required_name(row)
        .ok_or_else(|| row_issue(row, Some(plu_number), "name", "missing product name"))?;
    Ok(Plu {
        plu_number,
        store_number,
        department_number,
        group_number: parse_optional_u32(row, GROUP_COLUMNS, "group number").map_err(|err| {
            row_issue(
                row,
                Some(plu_number),
                "group_number",
                format!("invalid group: {err}"),
            )
        })?,
        name,
        barcode: optional_text(row, BARCODE_COLUMNS),
        price,
        price_mode,
        short_description: optional_text(row, SHORT_DESCRIPTION_COLUMNS),
        key_label: optional_text(row, KEY_LABEL_COLUMNS),
        expiration_days: parse_optional_u32(row, EXPIRATION_COLUMNS, "expiration days")
            .ok()
            .flatten(),
        ingredients: key.and_then(|key| ingredients.get(&key).cloned()),
        nutrition_facts: key
            .and_then(|key| nutrition.get(&key).cloned())
            .unwrap_or_default(),
        source_pluing_row_count: key
            .and_then(|key| pluing_counts.get(&key).copied())
            .unwrap_or_default(),
    })
}

fn require_source_column(columns: &[String], column: &str, table: &str) -> Result<(), AppError> {
    if columns.iter().any(|candidate| candidate == column) {
        Ok(())
    } else {
        Err(AppError::MdbSchema(format!(
            "required source column '{column}' was not found in table '{table}'"
        )))
    }
}

fn row_join_key(row: &SourceRow) -> Option<JoinKey> {
    let plu_number = parse_required_u64(row, PLU_NUMBER_COLUMNS, "PLU number").ok()?;
    let department_number = parse_optional_u32(row, DEPARTMENT_COLUMNS, "department number")
        .ok()
        .flatten()?;
    Some(JoinKey {
        plu_number,
        department_number,
    })
}

fn pluing_counts_by_key(rows: &[SourceRow]) -> HashMap<JoinKey, usize> {
    let mut counts = HashMap::new();
    for row in rows {
        if let Some(key) = row_join_key(row) {
            *counts.entry(key).or_default() += 1;
        }
    }
    counts
}

fn row_issue(
    row: &SourceRow,
    plu_number: Option<u64>,
    field: impl Into<String>,
    message: impl Into<String>,
) -> ValidationIssue {
    let department =
        optional_text(row, DEPARTMENT_COLUMNS).unwrap_or_else(|| "unknown".to_string());
    ValidationIssue::error(
        plu_number,
        field,
        format!("{}; Department={department}", message.into()),
    )
}

fn normalize_ingredients(rows: &[SourceRow]) -> Result<HashMap<JoinKey, String>, AppError> {
    let mut by_plu: HashMap<JoinKey, Vec<String>> = HashMap::new();
    for row in rows {
        let Some(key) = row_join_key(row) else {
            continue;
        };
        let ordered_parts = ordered_ingredient_parts(row);
        if ordered_parts.is_empty() {
            if let Some(text) = optional_text(row, INGREDIENT_TEXT_COLUMNS) {
                by_plu.entry(key).or_default().push(text);
            }
        } else {
            by_plu.entry(key).or_default().extend(ordered_parts);
        }
    }
    Ok(by_plu
        .into_iter()
        .map(|(plu, parts)| (plu, parts.join("\n")))
        .collect())
}

fn normalize_nutrition(
    rows: &[SourceRow],
) -> Result<HashMap<JoinKey, Vec<NutritionFact>>, AppError> {
    let mut by_plu: HashMap<JoinKey, Vec<NutritionFact>> = HashMap::new();
    for row in rows {
        let Some(key) = row_join_key(row) else {
            continue;
        };
        let plu_ing_facts = nutrition_from_pluing(row)?;
        if !plu_ing_facts.is_empty() {
            by_plu.entry(key).or_default().extend(plu_ing_facts);
            continue;
        }

        if let Some(name) = optional_text(row, NUTRITION_NAME_COLUMNS) {
            let amount = optional_text(row, NUTRITION_AMOUNT_COLUMNS);
            by_plu.entry(key).or_default().push(NutritionFact {
                name,
                amount,
                unit: optional_text(row, NUTRITION_UNIT_COLUMNS),
            });
        }
    }
    Ok(by_plu)
}

fn required_name(row: &SourceRow) -> Option<String> {
    let parts = NAME_LINE_COLUMNS
        .iter()
        .filter_map(|column| row.get(column))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    if !parts.is_empty() {
        return Some(parts.join("<br>"));
    }
    optional_text(row, NAME_COLUMNS)
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

    fn pludata_row(plucode: &str, department: &str, name_1: &str) -> SourceRow {
        SourceRow {
            table: "Pludata".to_string(),
            values: BTreeMap::from([
                ("Plucode".to_string(), plucode.to_string()),
                ("Department".to_string(), department.to_string()),
                ("Category".to_string(), "1".to_string()),
                ("Name 1".to_string(), name_1.to_string()),
                ("Price".to_string(), "1.99".to_string()),
                ("PriceMode".to_string(), "each".to_string()),
            ]),
        }
    }

    fn pluing_row(plucode: &str, department: &str, ingredient: &str) -> SourceRow {
        SourceRow {
            table: "PluIng".to_string(),
            values: BTreeMap::from([
                ("Plucode".to_string(), plucode.to_string()),
                ("Department".to_string(), department.to_string()),
                ("Ing Name 1".to_string(), ingredient.to_string()),
                ("Calories".to_string(), "008".to_string()),
            ]),
        }
    }

    #[test]
    fn missing_name_1_column_is_schema_failure() {
        let mut schema = MdbSchema::default();
        schema.tables = vec!["Pludata".to_string()];
        schema.set_columns(
            "Pludata",
            vec![
                "Plucode".to_string(),
                "Department".to_string(),
                "Price".to_string(),
            ],
        );

        let result = validate_source_schema(&schema, &MappingConfig::default());

        assert!(matches!(result, Err(AppError::MdbSchema(_))));
    }

    #[test]
    fn present_name_1_with_empty_row_creates_row_issue_and_valid_rows_continue() {
        let dataset = SourceDataset {
            plu_rows: vec![
                pludata_row("0", "0001", ""),
                pludata_row("1", "0001", "Apples"),
            ],
            ingredient_rows: Vec::new(),
            nutrition_rows: Vec::new(),
        };

        let report = normalize_dataset(&dataset, &MappingConfig::default(), 1).expect("normalize");

        assert_eq!(report.plus.len(), 1);
        assert_eq!(report.plus[0].plu_number, 1);
        assert_eq!(report.row_issues.len(), 1);
        assert_eq!(report.row_issues[0].plu_number, Some(0));
        assert!(
            report.row_issues[0]
                .message
                .contains("missing product name")
        );
        assert!(report.row_issues[0].message.contains("Department=0001"));
    }

    #[test]
    fn plu_zero_with_empty_name_is_not_normalized_for_sending() {
        let dataset = SourceDataset {
            plu_rows: vec![
                pludata_row("0", "0001", ""),
                pludata_row("2", "0001", "Bananas"),
            ],
            ingredient_rows: Vec::new(),
            nutrition_rows: Vec::new(),
        };

        let report = normalize_dataset(&dataset, &MappingConfig::default(), 1).expect("normalize");

        assert!(report.plus.iter().all(|plu| plu.plu_number != 0));
        assert_eq!(report.plus[0].plu_number, 2);
    }

    #[test]
    fn pluing_join_uses_plucode_and_department() {
        let dataset = SourceDataset {
            plu_rows: vec![pludata_row("1", "0001", "Apples")],
            ingredient_rows: vec![
                pluing_row("1", "0001", "Matched"),
                pluing_row("1", "0002", "Wrong department"),
            ],
            nutrition_rows: vec![
                pluing_row("1", "0001", "Matched"),
                pluing_row("1", "0002", "Wrong department"),
            ],
        };

        let report = normalize_dataset(&dataset, &MappingConfig::default(), 1).expect("normalize");

        assert_eq!(report.plus[0].ingredients.as_deref(), Some("Matched"));
        assert_eq!(report.plus[0].source_pluing_row_count, 1);
        assert_eq!(report.orphan_pluing_rows, 1);
    }

    #[test]
    fn unmatched_pluing_rows_are_not_attached_to_valid_plus() {
        let dataset = SourceDataset {
            plu_rows: vec![pludata_row("3", "0001", "Oranges")],
            ingredient_rows: vec![pluing_row("9", "0001", "Unmatched")],
            nutrition_rows: vec![pluing_row("9", "0001", "Unmatched")],
        };

        let report = normalize_dataset(&dataset, &MappingConfig::default(), 1).expect("normalize");

        assert_eq!(report.plus[0].ingredients, None);
        assert!(report.plus[0].nutrition_facts.is_empty());
        assert_eq!(report.orphan_pluing_rows, 1);
    }

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

        let report = normalize_dataset(&dataset, &MappingConfig::default(), 1).expect("normalize");
        let plus = report.plus;

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
                ("Department".to_string(), "2".to_string()),
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

        let report = normalize_dataset(&dataset, &MappingConfig::default(), 1).expect("normalize");
        let plus = report.plus;

        assert_eq!(plus[0].name, "Apple<br>Slices");
        assert_eq!(plus[0].department_number, Some(2));
        assert_eq!(plus[0].group_number, Some(3));
        assert_eq!(plus[0].ingredients.as_deref(), Some("Apples\nWater"));
        assert_eq!(plus[0].source_pluing_row_count, 1);
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
