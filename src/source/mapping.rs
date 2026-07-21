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
const CATEGORY_COLUMNS: &[&str] = &["Category", "CATEGORY", "category"];
const QUANTITY_COLUMNS: &[&str] = &["Quantity", "QUANTITY", "quantity"];
const QUANTITY_SYMBOL_COLUMNS: &[&str] = &[
    "Quantity Symbol",
    "QUANTITY SYMBOL",
    "QuantitySymbol",
    "QUANTITY_SYMBOL",
    "quantity_symbol",
];
const TARE_COLUMNS: &[&str] = &["TARE", "Tare", "tare"];
const DISCOUNT_COLUMNS: &[&str] = &["DISCOUNT", "Discount", "discount"];
const PACK_DATE_FLAG_COLUMNS: &[&str] = &["PACK DATE FLAG", "PACK_DATE_FLAG", "PackDateFlag"];
const BEST_BEFORE_COLUMNS: &[&str] = &["BEST BEFORE", "BEST_BEFORE", "Best Before"];
const BEST_BEFORE_FLAG_COLUMNS: &[&str] =
    &["BEST BEFORE FLAG", "BEST_BEFORE_FLAG", "Best Before Flag"];
const PRINT_FORMAT_COLUMNS: &[&str] = &[
    "PRINT FORMAT CODE",
    "PRINT_FORMAT_CODE",
    "Print Format Code",
];
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
    ("calories", "Calories", None),
    ("calories fat", "Calories From Fat", None),
    ("total fat", "Total Fat", Some("Percent Total Fat")),
    (
        "saturated fat",
        "Saturated Fat",
        Some("Percent Saturated Fat"),
    ),
    ("cholesterol", "Cholesterol", Some("Percent Cholesterol")),
    ("sodium", "Sodium", Some("Percent Sodium")),
    (
        "carbohydrate",
        "Total Carbohydrate",
        Some("Percent Total Carbohydrate"),
    ),
    ("fiber", "Dietary Fiber", Some("Percent Dietary Fiber")),
    ("sugar", "Sugar", None),
    ("iron", "Iron", None),
    ("protein", "Protein", None),
    ("niacin", "Niacin", None),
    ("riboflavin", "Riboflavin", None),
    ("thiamin", "Thiamin", None),
    ("calcium", "Calcium", None),
    ("vitamin a", "Vitamin A", None),
    ("vitamin c", "Vitamin C", None),
    ("serving size", "Serving Size", None),
    ("serving container", "Servings Per Container", None),
    ("trans fat", "Trans fat", None),
];
const DEFAULT_GROUP_REFERENCE: u32 = 997;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct NormalizedPriceMode {
    price_mode: PriceMode,
    price_calc_method: Option<u8>,
    quantity: Option<u32>,
    quantity_symbol: Option<u32>,
}

/// Source mapping assumptions:
///
/// - The main table defaults to `Pludata`; `PluIng` supplies both ingredient text and nutrition values.
/// - The exact table names are configurable in `config.toml`.
/// - Column mappings are intentionally limited to observed/common DCA-style names listed in the constants above.
/// - If required PLU number, name, or price columns are absent/empty, the row is not given a fabricated default.
/// - `Pludata` names are assembled from non-empty `Name 1` through `Name 4` values with DIGIweb `<br>` line breaks.
/// - `Pludata`.`Main Group Code` is the preferred external DIGIweb group reference. Empty values default to group reference 997.
/// - DCA `Category` is the source of truth for price mode when present: 0/blank = weight per kg, 1 = fixed price, 3 = weight per 100g.
/// - `PluIng` ingredients are assembled from non-empty `Ing Name 1` through `Ing Name 99` values in numeric order.
/// - `PluIng` nutrition values are text in the inspected MDB and may contain zero padding. They are parsed as written with no unit conversion or decimal scaling.
/// - Unknown DIGIweb-specific field limits are enforced in validation with conservative defaults only where documented in code.
#[derive(Debug, Clone, Default)]
pub struct NormalizationReport {
    pub plus: Vec<Plu>,
    pub row_issues: Vec<ValidationIssue>,
    pub orphan_pluing_rows: usize,
    pub explicit_group_references: usize,
    pub defaulted_group_references: usize,
    pub invalid_group_values: usize,
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
    require_source_column(columns, "Main Group Code", &mapping.main_plu_table)?;
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
    let explicit_group_references = plus.iter().filter(|plu| !plu.group_default_applied).count();
    let defaulted_group_references = plus.iter().filter(|plu| plu.group_default_applied).count();
    let invalid_group_values = row_issues
        .iter()
        .filter(|issue| issue.field == "group_number")
        .count();
    Ok(NormalizationReport {
        plus,
        row_issues,
        orphan_pluing_rows,
        explicit_group_references,
        defaulted_group_references,
        invalid_group_values,
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
    let department_number =
        parse_required_positive_u32(row, DEPARTMENT_COLUMNS, "department number").map_err(
            |err| {
                row_issue(
                    row,
                    Some(plu_number),
                    "department_number",
                    format!("invalid department: {err}"),
                )
            },
        )?;
    let key = Some(JoinKey {
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
    let normalized_price_mode = normalize_price_mode(row).map_err(|err| {
        row_issue(
            row,
            Some(plu_number),
            "price_mode",
            format!("invalid price mode fields: {err}"),
        )
    })?;
    let name = required_name(row)
        .ok_or_else(|| row_issue(row, Some(plu_number), "name", "missing product name"))?;
    let normalized_group = normalize_main_group(row).map_err(|err| {
        row_issue(
            row,
            Some(plu_number),
            "group_number",
            format!("invalid group: {err}"),
        )
    })?;
    Ok(Plu {
        plu_number,
        store_number,
        department_number: Some(department_number),
        group_number: Some(normalized_group.value),
        source_department: optional_raw_text(row, DEPARTMENT_COLUMNS),
        source_group: optional_raw_text(row, GROUP_COLUMNS),
        group_default_applied: normalized_group.default_applied,
        name,
        barcode: optional_text(row, BARCODE_COLUMNS),
        price,
        price_mode: normalized_price_mode.price_mode,
        price_calc_method: normalized_price_mode.price_calc_method,
        quantity: normalized_price_mode.quantity,
        quantity_symbol: normalized_price_mode.quantity_symbol,
        tare: parse_optional_decimal_default_zero(row, TARE_COLUMNS, "TARE").map_err(|err| {
            row_issue(
                row,
                Some(plu_number),
                "tare",
                format!("invalid tare: {err}"),
            )
        })?,
        discount_type: Some(
            parse_optional_u32_default_zero(row, DISCOUNT_COLUMNS, "DISCOUNT").map_err(|err| {
                row_issue(
                    row,
                    Some(plu_number),
                    "discount_type",
                    format!("invalid discount: {err}"),
                )
            })?,
        ),
        packing_date_print: Some(flag_to_print_value(row, PACK_DATE_FLAG_COLUMNS)),
        packing_time_print: Some(flag_to_print_value(row, PACK_DATE_FLAG_COLUMNS)),
        selling_date_print: Some(flag_to_print_value(row, BEST_BEFORE_FLAG_COLUMNS)),
        selling_date_term: Some(
            parse_optional_u32_default_zero(row, BEST_BEFORE_COLUMNS, "BEST BEFORE").map_err(
                |err| {
                    row_issue(
                        row,
                        Some(plu_number),
                        "selling_date_term",
                        format!("invalid best-before value: {err}"),
                    )
                },
            )?,
        ),
        label_format: parse_optional_u32(row, PRINT_FORMAT_COLUMNS, "PRINT FORMAT CODE").map_err(
            |err| {
                row_issue(
                    row,
                    Some(plu_number),
                    "label_format",
                    format!("invalid print format: {err}"),
                )
            },
        )?,
        traceability: Some(0),
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

fn normalize_price_mode(row: &SourceRow) -> Result<NormalizedPriceMode, AppError> {
    if let Some(category) = optional_raw_text(row, CATEGORY_COLUMNS) {
        let category = category.trim();
        let category = if category.is_empty() { "0" } else { category };
        let quantity_symbol = Some(parse_optional_u32_default_zero(
            row,
            QUANTITY_SYMBOL_COLUMNS,
            "Quantity Symbol",
        )?);
        return match category {
            "0" => Ok(NormalizedPriceMode {
                price_mode: PriceMode::ByWeight,
                price_calc_method: Some(0),
                quantity: Some(0),
                quantity_symbol,
            }),
            "1" => Ok(NormalizedPriceMode {
                price_mode: PriceMode::ByEach,
                price_calc_method: Some(0),
                quantity: Some(parse_optional_u32_default_zero(
                    row,
                    QUANTITY_COLUMNS,
                    "Quantity",
                )?),
                quantity_symbol,
            }),
            "3" => Ok(NormalizedPriceMode {
                price_mode: PriceMode::ByWeight,
                price_calc_method: Some(1),
                quantity: Some(0),
                quantity_symbol,
            }),
            _ => Ok(NormalizedPriceMode {
                price_mode: PriceMode::Unknown,
                price_calc_method: None,
                quantity: None,
                quantity_symbol,
            }),
        };
    }

    Ok(NormalizedPriceMode {
        price_mode: PriceMode::from_source(find_value(row, PRICE_MODE_COLUMNS)),
        price_calc_method: None,
        quantity: None,
        quantity_symbol: None,
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
    let department_number =
        parse_required_positive_u32(row, DEPARTMENT_COLUMNS, "department number").ok()?;
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct NormalizedGroup {
    value: u32,
    default_applied: bool,
}

fn normalize_main_group(row: &SourceRow) -> Result<NormalizedGroup, AppError> {
    let Some(raw) = optional_raw_text(row, GROUP_COLUMNS) else {
        return Ok(NormalizedGroup {
            value: DEFAULT_GROUP_REFERENCE,
            default_applied: true,
        });
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(NormalizedGroup {
            value: DEFAULT_GROUP_REFERENCE,
            default_applied: true,
        });
    }
    let value = parse_positive_u32(trimmed, "Main Group Code").map_err(|_| {
        AppError::MdbExport(format!("invalid non-numeric Main Group Code {:?}", trimmed))
    })?;
    Ok(NormalizedGroup {
        value,
        default_applied: false,
    })
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
        .map(|(plu, parts)| (plu, apply_dca_ingredient_markup(&parts.join(" "))))
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

fn apply_dca_ingredient_markup(value: &str) -> String {
    let patterns = [
        ("peut contenir", "<br><b>Peut contenir</b>"),
        ("may contain", "<br><b>May contain</b>"),
        ("ingrédients", "<b>Ingrédients</b>"),
        ("contient", "<br><b>Contient</b>"),
        ("ingredient", "<b>Ingredient</b>"),
        ("contain", "<br><b>Contain</b>"),
    ];
    let trimmed = value.trim();
    let lower = trimmed.to_lowercase();
    let mut result = String::new();
    let mut index = 0;
    while index < trimmed.len() {
        if let Some((matched, replacement)) = patterns
            .iter()
            .find(|(pattern, _)| lower[index..].starts_with(pattern))
        {
            result.push_str(replacement);
            index += byte_len_for_chars(&trimmed[index..], matched.chars().count());
        } else {
            let next = trimmed[index..]
                .chars()
                .next()
                .expect("index is on a char boundary");
            result.push(next);
            index += next.len_utf8();
        }
    }
    result
}

fn byte_len_for_chars(value: &str, chars: usize) -> usize {
    value.chars().take(chars).map(char::len_utf8).sum()
}

fn nutrition_from_pluing(row: &SourceRow) -> Result<Vec<NutritionFact>, AppError> {
    let mut facts = Vec::new();
    for (name, amount_column, data2_column) in PLUING_NUTRITION_COLUMNS {
        let Some(amount) = row.get(*amount_column).and_then(normalize_nutrition_value) else {
            continue;
        };
        facts.push(NutritionFact {
            name: (*name).to_string(),
            amount: Some(amount),
            unit: data2_column
                .and_then(|column| row.get(column).and_then(normalize_nutrition_value)),
        });
    }
    Ok(facts)
}

fn normalize_nutrition_value(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    if !trimmed.contains('.') {
        if let Ok(number) = trimmed.parse::<i64>() {
            return Some(number.to_string());
        }
    }
    Some(trimmed.to_string())
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

fn optional_raw_text(row: &SourceRow, candidates: &[&str]) -> Option<String> {
    candidates
        .iter()
        .find_map(|candidate| row.get(candidate))
        .map(ToOwned::to_owned)
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

fn parse_optional_u32_default_zero(
    row: &SourceRow,
    candidates: &[&str],
    field: &str,
) -> Result<u32, AppError> {
    match optional_text(row, candidates) {
        Some(value) => value.parse::<u32>().map_err(|err| {
            AppError::MdbExport(format!(
                "{} value '{}' in table '{}' is invalid: {}",
                field, value, row.table, err
            ))
        }),
        None => Ok(0),
    }
}

fn parse_optional_decimal_default_zero(
    row: &SourceRow,
    candidates: &[&str],
    field: &str,
) -> Result<Option<Decimal>, AppError> {
    match optional_text(row, candidates) {
        Some(value) => parse_decimal_value(&value, field).map(Some),
        None => Ok(Some(Decimal::ZERO)),
    }
}

fn flag_to_print_value(row: &SourceRow, candidates: &[&str]) -> u8 {
    match optional_raw_text(row, candidates)
        .unwrap_or_default()
        .trim()
        .to_ascii_uppercase()
        .as_str()
    {
        "Y" | "YES" | "TRUE" | "1" => 1,
        _ => 0,
    }
}

fn parse_required_positive_u32(
    row: &SourceRow,
    candidates: &[&str],
    field: &str,
) -> Result<u32, AppError> {
    let value = required_text(row, candidates, field)?;
    parse_positive_u32(&value, field)
}

fn parse_positive_u32(value: &str, field: &str) -> Result<u32, AppError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(AppError::MdbExport(format!("{field} is empty")));
    }
    let parsed = trimmed.parse::<u32>().map_err(|err| {
        AppError::MdbExport(format!("{field} value '{trimmed}' is invalid: {err}"))
    })?;
    if parsed == 0 {
        return Err(AppError::MdbExport(format!("{field} must be positive")));
    }
    Ok(parsed)
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
    use crate::validation::validator::validate_plus;

    fn pludata_row(plucode: &str, department: &str, name_1: &str) -> SourceRow {
        pludata_row_with_group(plucode, department, "1", name_1)
    }

    fn pludata_row_with_group(
        plucode: &str,
        department: &str,
        group: &str,
        name_1: &str,
    ) -> SourceRow {
        SourceRow {
            table: "Pludata".to_string(),
            values: BTreeMap::from([
                ("Plucode".to_string(), plucode.to_string()),
                ("Department".to_string(), department.to_string()),
                ("Main Group Code".to_string(), group.to_string()),
                ("Name 1".to_string(), name_1.to_string()),
                ("Price".to_string(), "1.99".to_string()),
                ("PriceMode".to_string(), "each".to_string()),
            ]),
        }
    }

    fn dca_pludata_row(
        plucode: &str,
        category: &str,
        price: &str,
        quantity: &str,
        quantity_symbol: &str,
    ) -> SourceRow {
        SourceRow {
            table: "Pludata".to_string(),
            values: BTreeMap::from([
                ("Plucode".to_string(), plucode.to_string()),
                ("Department".to_string(), "1".to_string()),
                ("Main Group Code".to_string(), "1".to_string()),
                ("Category".to_string(), category.to_string()),
                ("Name 1".to_string(), format!("PLU {plucode}")),
                ("Price".to_string(), price.to_string()),
                ("Quantity".to_string(), quantity.to_string()),
                ("Quantity Symbol".to_string(), quantity_symbol.to_string()),
                ("Barcode Format".to_string(), "05".to_string()),
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
                "Main Group Code".to_string(),
                "Price".to_string(),
            ],
        );

        let result = validate_source_schema(&schema, &MappingConfig::default());

        assert!(matches!(result, Err(AppError::MdbSchema(_))));
    }

    #[test]
    fn missing_main_group_code_column_is_schema_failure() {
        let mut schema = MdbSchema::default();
        schema.tables = vec!["Pludata".to_string()];
        schema.set_columns(
            "Pludata",
            vec![
                "Plucode".to_string(),
                "Department".to_string(),
                "Name 1".to_string(),
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
    fn whitespace_padded_main_group_code_997_normalizes_to_997() {
        let dataset = SourceDataset {
            plu_rows: vec![pludata_row_with_group("1", "0001", "997   ", "Apples")],
            ingredient_rows: Vec::new(),
            nutrition_rows: Vec::new(),
        };

        let report = normalize_dataset(&dataset, &MappingConfig::default(), 1).expect("normalize");

        assert_eq!(report.plus[0].group_number, Some(997));
        assert_eq!(report.plus[0].source_group.as_deref(), Some("997   "));
    }

    #[test]
    fn padded_department_0001_normalizes_to_1() {
        let dataset = SourceDataset {
            plu_rows: vec![pludata_row_with_group("1", "0001", "997", "Apples")],
            ingredient_rows: Vec::new(),
            nutrition_rows: Vec::new(),
        };

        let report = normalize_dataset(&dataset, &MappingConfig::default(), 1).expect("normalize");

        assert_eq!(report.plus[0].department_number, Some(1));
        assert_eq!(report.plus[0].source_department.as_deref(), Some("0001"));
    }

    #[test]
    fn department_values_normalize_leading_zeroes_and_whitespace() {
        for (raw, expected) in [
            ("0001", 1),
            ("0010", 10),
            ("1", 1),
            ("10", 10),
            (" 0001 ", 1),
        ] {
            let dataset = SourceDataset {
                plu_rows: vec![pludata_row_with_group("1", raw, "997", "Apples")],
                ingredient_rows: Vec::new(),
                nutrition_rows: Vec::new(),
            };

            let report =
                normalize_dataset(&dataset, &MappingConfig::default(), 1).expect("normalize");

            assert_eq!(report.plus[0].department_number, Some(expected), "{raw}");
        }
    }

    #[test]
    fn invalid_department_values_create_row_issues() {
        for raw in ["", "   ", "ABC", "0", "-1"] {
            let dataset = SourceDataset {
                plu_rows: vec![pludata_row_with_group("1", raw, "997", "Apples")],
                ingredient_rows: Vec::new(),
                nutrition_rows: Vec::new(),
            };

            let report =
                normalize_dataset(&dataset, &MappingConfig::default(), 1).expect("normalize");

            assert!(report.plus.is_empty(), "{raw}");
            assert!(
                report
                    .row_issues
                    .iter()
                    .any(|issue| issue.field == "department_number"),
                "{raw}"
            );
        }
    }

    #[test]
    fn main_group_values_normalize_and_empty_values_default_to_997() {
        for (raw, expected, default_applied) in [
            ("", 997, true),
            ("   ", 997, true),
            ("997", 997, false),
            ("997   ", 997, false),
            ("0001", 1, false),
            ("1", 1, false),
            ("25", 25, false),
            ("995", 995, false),
            ("100", 100, false),
        ] {
            let dataset = SourceDataset {
                plu_rows: vec![pludata_row_with_group("1", "0001", raw, "Apples")],
                ingredient_rows: Vec::new(),
                nutrition_rows: Vec::new(),
            };

            let report =
                normalize_dataset(&dataset, &MappingConfig::default(), 1).expect("normalize");

            assert_eq!(report.plus[0].group_number, Some(expected), "{raw}");
            assert_eq!(
                report.plus[0].group_default_applied, default_applied,
                "{raw}"
            );
        }
    }

    #[test]
    fn explicit_group_values_are_not_counted_as_defaults() {
        let dataset = SourceDataset {
            plu_rows: vec![
                pludata_row_with_group("1", "0001", "", "Defaulted"),
                pludata_row_with_group("2", "0001", "997", "Explicit 997"),
                pludata_row_with_group("3", "0001", "1", "Explicit 1"),
            ],
            ingredient_rows: Vec::new(),
            nutrition_rows: Vec::new(),
        };

        let report = normalize_dataset(&dataset, &MappingConfig::default(), 1).expect("normalize");

        assert_eq!(report.defaulted_group_references, 1);
        assert_eq!(report.explicit_group_references, 2);
        assert_eq!(report.invalid_group_values, 0);
        assert_eq!(report.plus[0].group_number, Some(997));
        assert!(report.plus[0].group_default_applied);
        assert_eq!(report.plus[1].group_number, Some(997));
        assert!(!report.plus[1].group_default_applied);
        assert_eq!(report.plus[2].group_number, Some(1));
    }

    #[test]
    fn invalid_non_empty_group_values_do_not_default_to_997() {
        for raw in ["ABC", "-1", "0", "1.5", "99X"] {
            let dataset = SourceDataset {
                plu_rows: vec![pludata_row_with_group("1", "0001", raw, "Apples")],
                ingredient_rows: Vec::new(),
                nutrition_rows: Vec::new(),
            };

            let report =
                normalize_dataset(&dataset, &MappingConfig::default(), 1).expect("normalize");

            assert!(report.plus.is_empty(), "{raw}");
            assert_eq!(report.invalid_group_values, 1, "{raw}");
            assert!(
                report
                    .row_issues
                    .iter()
                    .any(|issue| issue.field == "group_number"),
                "{raw}"
            );
        }
    }

    #[test]
    fn non_numeric_group_source_value_creates_row_issue() {
        let dataset = SourceDataset {
            plu_rows: vec![pludata_row_with_group("1", "0001", "ABC", "Apples")],
            ingredient_rows: Vec::new(),
            nutrition_rows: Vec::new(),
        };

        let report = normalize_dataset(&dataset, &MappingConfig::default(), 1).expect("normalize");

        assert!(report.plus.is_empty());
        assert!(
            report
                .row_issues
                .iter()
                .any(|issue| issue.field == "group_number")
        );
    }

    #[test]
    fn negative_group_source_value_creates_row_issue() {
        let dataset = SourceDataset {
            plu_rows: vec![pludata_row_with_group("1", "0001", "-1", "Apples")],
            ingredient_rows: Vec::new(),
            nutrition_rows: Vec::new(),
        };

        let report = normalize_dataset(&dataset, &MappingConfig::default(), 1).expect("normalize");

        assert!(report.plus.is_empty());
        assert!(
            report
                .row_issues
                .iter()
                .any(|issue| issue.field == "group_number")
        );
    }

    #[test]
    fn normalized_department_values_are_used_for_pluing_join() {
        let dataset = SourceDataset {
            plu_rows: vec![pludata_row("1", "0001", "Apples")],
            ingredient_rows: vec![pluing_row("1", "1", "Matched")],
            nutrition_rows: Vec::new(),
        };

        let report = normalize_dataset(&dataset, &MappingConfig::default(), 1).expect("normalize");

        assert_eq!(report.plus[0].ingredients.as_deref(), Some("Matched"));
        assert_eq!(report.plus[0].source_pluing_row_count, 1);
        assert_eq!(report.orphan_pluing_rows, 0);
    }

    #[test]
    fn different_normalized_department_values_do_not_join() {
        let dataset = SourceDataset {
            plu_rows: vec![pludata_row("1", "0001", "Apples")],
            ingredient_rows: vec![pluing_row("1", "2", "Wrong department")],
            nutrition_rows: Vec::new(),
        };

        let report = normalize_dataset(&dataset, &MappingConfig::default(), 1).expect("normalize");

        assert_eq!(report.plus[0].ingredients, None);
        assert_eq!(report.plus[0].source_pluing_row_count, 0);
        assert_eq!(report.orphan_pluing_rows, 1);
    }

    #[test]
    fn group_summary_counts_invalid_non_empty_values_without_defaulting() {
        let dataset = SourceDataset {
            plu_rows: vec![
                pludata_row_with_group("1", "0001", "", "Defaulted"),
                pludata_row_with_group("2", "0001", "25", "Explicit"),
                pludata_row_with_group("3", "0001", "99X", "Invalid"),
            ],
            ingredient_rows: Vec::new(),
            nutrition_rows: Vec::new(),
        };

        let report = normalize_dataset(&dataset, &MappingConfig::default(), 1).expect("normalize");

        assert_eq!(report.defaulted_group_references, 1);
        assert_eq!(report.explicit_group_references, 1);
        assert_eq!(report.invalid_group_values, 1);
        assert_eq!(report.plus.len(), 2);
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
    fn dca_category_0_maps_to_weight_price_per_kg() {
        let dataset = SourceDataset {
            plu_rows: vec![dca_pludata_row("1001", "0", "16.9", "0", "")],
            ingredient_rows: Vec::new(),
            nutrition_rows: Vec::new(),
        };

        let report = normalize_dataset(&dataset, &MappingConfig::default(), 1).expect("normalize");
        let plu = &report.plus[0];

        assert_eq!(plu.price, Decimal::new(169, 1));
        assert_eq!(plu.price_mode, PriceMode::ByWeight);
        assert_eq!(plu.price_calc_method, Some(0));
        assert_eq!(plu.quantity, Some(0));
        assert_eq!(plu.quantity_symbol, Some(0));
    }

    #[test]
    fn dca_category_1_maps_to_fixed_price_with_source_quantity() {
        let dataset = SourceDataset {
            plu_rows: vec![dca_pludata_row("1002", "1", "2.50", "6", "1")],
            ingredient_rows: Vec::new(),
            nutrition_rows: Vec::new(),
        };

        let report = normalize_dataset(&dataset, &MappingConfig::default(), 1).expect("normalize");
        let plu = &report.plus[0];

        assert_eq!(plu.price_mode, PriceMode::ByEach);
        assert_eq!(plu.price_calc_method, Some(0));
        assert_eq!(plu.quantity, Some(6));
        assert_eq!(plu.quantity_symbol, Some(1));
    }

    #[test]
    fn dca_category_3_maps_to_weight_price_per_100g() {
        let dataset = SourceDataset {
            plu_rows: vec![dca_pludata_row("1003", "3", "1.49", "9", "5")],
            ingredient_rows: Vec::new(),
            nutrition_rows: Vec::new(),
        };

        let report = normalize_dataset(&dataset, &MappingConfig::default(), 1).expect("normalize");
        let plu = &report.plus[0];

        assert_eq!(plu.price_mode, PriceMode::ByWeight);
        assert_eq!(plu.price_calc_method, Some(1));
        assert_eq!(plu.quantity, Some(0));
        assert_eq!(plu.quantity_symbol, Some(5));
    }

    #[test]
    fn blank_dca_category_defaults_to_category_0_weight_mode() {
        let dataset = SourceDataset {
            plu_rows: vec![dca_pludata_row("1004", "", "3.25", "", "")],
            ingredient_rows: Vec::new(),
            nutrition_rows: Vec::new(),
        };

        let report = normalize_dataset(&dataset, &MappingConfig::default(), 1).expect("normalize");
        let plu = &report.plus[0];

        assert_eq!(plu.price_mode, PriceMode::ByWeight);
        assert_eq!(plu.price_calc_method, Some(0));
        assert_eq!(plu.quantity, Some(0));
        assert_eq!(plu.quantity_symbol, Some(0));
    }

    #[test]
    fn unsupported_dca_category_still_fails_price_mode_validation() {
        let dataset = SourceDataset {
            plu_rows: vec![dca_pludata_row("1005", "2", "3.25", "", "")],
            ingredient_rows: Vec::new(),
            nutrition_rows: Vec::new(),
        };

        let report = normalize_dataset(&dataset, &MappingConfig::default(), 1).expect("normalize");
        let validation_report = validate_plus(&report.plus);

        assert_eq!(report.plus[0].price_mode, PriceMode::Unknown);
        assert_eq!(report.plus[0].price_calc_method, None);
        assert!(validation_report.issues.iter().any(|issue| {
            issue.field == "price_mode" && issue.message == "unsupported or missing price mode"
        }));
    }

    #[test]
    fn pluing_supplies_ordered_ingredients_and_nutrition() {
        let plu_row = SourceRow {
            table: "Pludata".to_string(),
            values: BTreeMap::from([
                ("Plucode".to_string(), "1001".to_string()),
                ("Department".to_string(), "2".to_string()),
                ("Main Group Code".to_string(), "3".to_string()),
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
                ("Percent Sodium".to_string(), "029".to_string()),
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
        assert_eq!(plus[0].ingredients.as_deref(), Some("Apples Water"));
        assert_eq!(plus[0].source_pluing_row_count, 1);
        assert!(
            plus[0]
                .nutrition_facts
                .iter()
                .any(|fact| fact.name == "calories" && fact.amount.as_deref() == Some("8"))
        );
        assert!(
            plus[0]
                .nutrition_facts
                .iter()
                .any(|fact| fact.name == "sodium"
                    && fact.amount.as_deref() == Some("690")
                    && fact.unit.as_deref() == Some("29"))
        );
    }

    #[test]
    fn dca_ingredients_use_vb_html_marker_formatting() {
        let row = pluing_row("1", "1", "Ingredient wheat May contain milk");
        let dataset = SourceDataset {
            plu_rows: vec![pludata_row("1", "1", "Bread")],
            ingredient_rows: vec![row],
            nutrition_rows: Vec::new(),
        };

        let report = normalize_dataset(&dataset, &MappingConfig::default(), 1).expect("normalize");

        assert_eq!(
            report.plus[0].ingredients.as_deref(),
            Some("<b>Ingredient</b> wheat <br><b>May contain</b> milk")
        );
    }
}
