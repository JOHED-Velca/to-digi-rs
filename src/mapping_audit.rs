use std::fs;
use std::path::Path;
use std::time::Duration;

use chrono::{DateTime, Local};
use serde::Serialize;

use crate::config::DigiwebConfig;
use crate::digiweb::payload::DigiwebPluPayload;
use crate::discovery::PhaseTiming;
use crate::error::AppError;
use crate::models::plu::{Plu, effective_label_format};
use crate::source::SourceDataset;
use crate::source::mapping::{
    BARCODE_COLUMNS, BARCODE_FORMAT_COLUMNS, BEST_BEFORE_COLUMNS, BEST_BEFORE_FLAG_COLUMNS,
    CATEGORY_COLUMNS, DEPARTMENT_COLUMNS, EXPIRATION_COLUMNS, FLAG_DATA_COLUMNS,
    INGREDIENT_TEXT_COLUMNS, KEY_LABEL_COLUMNS, NAME_LINE_COLUMNS, PLU_NUMBER_COLUMNS,
    PLUING_NUTRITION_COLUMNS, PRICE_COLUMNS, PRINT_FORMAT_COLUMNS, QUANTITY_COLUMNS,
    QUANTITY_SYMBOL_COLUMNS, TARE_COLUMNS,
};

const DEFAULT_SAMPLE_LIMIT: usize = 5;

pub struct MappingAuditInput<'a> {
    pub source_path: &'a str,
    pub source_sha256: &'a str,
    pub started_at: DateTime<Local>,
    pub finished_at: DateTime<Local>,
    pub dataset: &'a SourceDataset,
    pub valid_plus: &'a [Plu],
    pub config: &'a DigiwebConfig,
    pub sample_limit: usize,
    pub target_plu: Option<u64>,
    pub timings: Vec<PhaseTiming>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MappingAuditReport {
    pub schema_version: u32,
    pub application_version: String,
    pub command: String,
    pub generated_at: String,
    pub started_at: String,
    pub finished_at: String,
    pub source_path: String,
    pub source_sha256: String,
    pub safety: MappingSafety,
    pub status: MappingAuditStatus,
    pub selected_plu_count: usize,
    pub destinations: Vec<MappingDestination>,
    pub ingredient_nutrition_separation: SeparationAudit,
    pub warnings: Vec<String>,
    pub samples: Vec<PayloadSample>,
    pub timings: Vec<PhaseTiming>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MappingAuditStatus {
    Pass,
    Warning,
    Fail,
}

impl MappingAuditStatus {
    pub fn as_text(self) -> &'static str {
        match self {
            Self::Pass => "PASS",
            Self::Warning => "WARNING",
            Self::Fail => "FAIL",
        }
    }

    pub fn exit_code(self) -> i32 {
        match self {
            Self::Pass | Self::Warning => 0,
            Self::Fail => 2,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MappingSafety {
    pub authentication_attempted: bool,
    pub digiweb_api_requests_attempted: bool,
    pub source_database_modified: bool,
    pub plus_submitted: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MappingDestination {
    pub destination: String,
    pub source_table: String,
    pub source_columns: Vec<String>,
    pub transformation: String,
    pub plus_with_source_data: usize,
    pub plus_with_populated_target: usize,
    pub rejected_or_skipped: usize,
    pub representative_plus: Vec<u64>,
    pub warning: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SeparationAudit {
    pub status: MappingAuditStatus,
    pub ingredients_source_table: String,
    pub ingredients_source_fields: Vec<String>,
    pub ingredients_destination: String,
    pub nutrition_source_table: String,
    pub nutrition_source_fields: Vec<String>,
    pub nutrition_destination: String,
    pub plus_with_ingredient_source: usize,
    pub plus_with_ingredient_payload: usize,
    pub plus_with_nutrition_source: usize,
    pub plus_with_nutrition_payload: usize,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PayloadSample {
    pub plu_number: u64,
    pub populated_destinations: Vec<String>,
    pub ingredient_present: bool,
    pub nutrition_fact_count: usize,
    pub raw_label_format: Option<u32>,
    pub effective_label_format: Option<u32>,
}

pub fn default_sample_limit() -> usize {
    DEFAULT_SAMPLE_LIMIT
}

pub fn build_mapping_audit_report(
    input: MappingAuditInput<'_>,
) -> Result<MappingAuditReport, AppError> {
    let started = std::time::Instant::now();
    let selected_plus = input
        .valid_plus
        .iter()
        .filter(|plu| {
            input
                .target_plu
                .is_none_or(|target| plu.plu_number == target)
        })
        .collect::<Vec<_>>();
    let payloads = selected_plus
        .iter()
        .map(|plu| DigiwebPluPayload::from_plu(plu, input.config).map(|payload| (*plu, payload)))
        .collect::<Result<Vec<_>, AppError>>()?;
    let destinations = destinations(input.dataset, &payloads);
    let separation = separation_audit(input.dataset, &payloads, &destinations);
    let mut warnings = destinations
        .iter()
        .filter_map(|destination| destination.warning.clone())
        .collect::<Vec<_>>();
    if input.target_plu.is_some() && selected_plus.is_empty() {
        warnings.push(format!(
            "PLU {} was requested but is not present among valid normalized PLUs",
            input.target_plu.unwrap_or_default()
        ));
    }
    warnings.extend(separation.warnings.clone());
    warnings.sort();
    warnings.dedup();
    let status = if separation.status == MappingAuditStatus::Fail {
        MappingAuditStatus::Fail
    } else if !warnings.is_empty() || separation.status == MappingAuditStatus::Warning {
        MappingAuditStatus::Warning
    } else {
        MappingAuditStatus::Pass
    };
    let mut timings = input.timings;
    timings.push(PhaseTiming::from_duration(
        "Mapping audit",
        started.elapsed(),
    ));
    Ok(MappingAuditReport {
        schema_version: 1,
        application_version: env!("CARGO_PKG_VERSION").to_string(),
        command: "map-audit".to_string(),
        generated_at: Local::now().to_rfc3339(),
        started_at: input.started_at.to_rfc3339(),
        finished_at: input.finished_at.to_rfc3339(),
        source_path: input.source_path.to_string(),
        source_sha256: input.source_sha256.to_string(),
        safety: MappingSafety {
            authentication_attempted: false,
            digiweb_api_requests_attempted: false,
            source_database_modified: false,
            plus_submitted: 0,
        },
        status,
        selected_plu_count: payloads.len(),
        destinations,
        ingredient_nutrition_separation: separation,
        warnings,
        samples: payloads
            .iter()
            .take(input.sample_limit)
            .map(|(plu, payload)| sample(plu, payload))
            .collect(),
        timings,
    })
}

pub fn write_mapping_reports(
    text_path: &Path,
    json_path: &Path,
    report: &MappingAuditReport,
) -> Result<(), AppError> {
    fs::write(text_path, render_mapping_text(report))
        .map_err(|err| AppError::Internal(format!("failed to write mapping report: {err}")))?;
    let json = serde_json::to_string_pretty(report)
        .map_err(|err| AppError::Internal(format!("mapping JSON serialization failed: {err}")))?;
    fs::write(json_path, json)
        .map_err(|err| AppError::Internal(format!("failed to write mapping JSON: {err}")))?;
    Ok(())
}

pub fn render_mapping_console(report: &MappingAuditReport) -> String {
    let mut out = String::new();
    line(&mut out, "SOURCE TO DIGIWEB MAPPING AUDIT");
    line(&mut out, format!("Result: {}", report.status.as_text()));
    line(
        &mut out,
        format!("Destinations audited: {}", report.destinations.len()),
    );
    line(
        &mut out,
        format!("Selected valid PLUs: {}", report.selected_plu_count),
    );
    line(
        &mut out,
        format!(
            "Ingredient/nutrition separation: {}",
            report.ingredient_nutrition_separation.status.as_text()
        ),
    );
    line(&mut out, "Detailed report:");
    line(&mut out, "./mapping-report.txt");
    line(&mut out, "Machine-readable report:");
    line(&mut out, "./mapping-report.json");
    out
}

fn render_mapping_text(report: &MappingAuditReport) -> String {
    let mut out = render_mapping_console(report);
    blank(&mut out);
    line(&mut out, "SAFETY");
    line(&mut out, "Authentication attempted: NO");
    line(&mut out, "DIGIweb API requests attempted: NO");
    line(&mut out, "Source database modified: NO");
    line(&mut out, "PLUs submitted: 0");
    blank(&mut out);
    if !report.warnings.is_empty() {
        line(&mut out, "WARNINGS");
        for warning in &report.warnings {
            line(&mut out, format!("WARNING: {warning}"));
        }
        blank(&mut out);
    }
    line(&mut out, "DESTINATIONS");
    for destination in &report.destinations {
        line(&mut out, &destination.destination);
        line(
            &mut out,
            format!("  Source table: {}", destination.source_table),
        );
        line(
            &mut out,
            format!(
                "  Source columns: {}",
                destination.source_columns.join(", ")
            ),
        );
        line(
            &mut out,
            format!("  Transformation: {}", destination.transformation),
        );
        line(
            &mut out,
            format!(
                "  PLUs with source data: {}",
                destination.plus_with_source_data
            ),
        );
        line(
            &mut out,
            format!(
                "  PLUs with populated target: {}",
                destination.plus_with_populated_target
            ),
        );
        if let Some(warning) = &destination.warning {
            line(&mut out, format!("  WARNING: {warning}"));
        }
    }
    blank(&mut out);
    line(&mut out, "INGREDIENTS VS NUTRITION");
    let separation = &report.ingredient_nutrition_separation;
    line(
        &mut out,
        format!(
            "Ingredients: {} -> {}",
            separation.ingredients_source_table, separation.ingredients_destination
        ),
    );
    line(
        &mut out,
        format!(
            "Nutrition: {} -> {}",
            separation.nutrition_source_table, separation.nutrition_destination
        ),
    );
    for warning in &separation.warnings {
        line(&mut out, format!("WARNING: {warning}"));
    }
    blank(&mut out);
    line(&mut out, "SAMPLES");
    for sample in &report.samples {
        line(
            &mut out,
            format!(
                "PLU {} | destinations={} | ingredient={} | nutrition_facts={}",
                sample.plu_number,
                sample.populated_destinations.join(", "),
                sample.ingredient_present,
                sample.nutrition_fact_count
            ),
        );
        line(
            &mut out,
            format!(
                "  Label Format raw/effective: {}/{}",
                sample
                    .raw_label_format
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "none".to_string()),
                sample
                    .effective_label_format
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "none".to_string())
            ),
        );
    }
    blank(&mut out);
    line(&mut out, "PERFORMANCE");
    for timing in &report.timings {
        line(
            &mut out,
            format!("{}: {} ms", timing.phase, timing.milliseconds),
        );
    }
    out
}

fn destinations(
    dataset: &SourceDataset,
    payloads: &[(&Plu, DigiwebPluPayload)],
) -> Vec<MappingDestination> {
    vec![
        dest(
            "pluno",
            "Pludata",
            PLU_NUMBER_COLUMNS,
            "parse unsigned integer",
            dataset.plu_rows.len(),
            payloads,
            |p| p.pluno > 0,
        ),
        dest(
            "pludepartmentno",
            "Pludata",
            DEPARTMENT_COLUMNS,
            "normalize positive department number",
            dataset.plu_rows.len(),
            payloads,
            |p| p.pludepartmentno > 0,
        ),
        dest(
            "plugroupno",
            "Pludata",
            crate::source::mapping::GROUP_COLUMNS,
            "normalize Main Group Code; empty source currently defaults in importer",
            dataset.plu_rows.len(),
            payloads,
            |p| p.plugroupno > 0,
        ),
        dest(
            "plubarcodetype/plubarcoderefno/plubarcodedata",
            "Pludata",
            &[BARCODE_COLUMNS, BARCODE_FORMAT_COLUMNS, FLAG_DATA_COLUMNS].concat(),
            "VB.NET-compatible DCA barcode derivation",
            dataset.plu_rows.len(),
            payloads,
            |p| !p.plubarcodedata.is_empty(),
        ),
        dest(
            "plucommname",
            "Pludata",
            NAME_LINE_COLUMNS,
            "join non-empty name lines with <br>",
            dataset.plu_rows.len(),
            payloads,
            |p| !p.plucommname.is_empty(),
        ),
        dest(
            "pluunitprice/plupricemode",
            "Pludata",
            &[PRICE_COLUMNS, CATEGORY_COLUMNS].concat(),
            "parse price and DCA category",
            dataset.plu_rows.len(),
            payloads,
            |p| p.pluunitprice >= rust_decimal::Decimal::ZERO,
        ),
        dest(
            "pluquantity/pluquantitysymbol",
            "Pludata",
            &[QUANTITY_COLUMNS, QUANTITY_SYMBOL_COLUMNS].concat(),
            "category-dependent optional quantity fields",
            dataset.plu_rows.len(),
            payloads,
            |p| p.pluquantity.is_some() || p.pluquantitysymbol.is_some(),
        ),
        dest(
            "plutare",
            "Pludata",
            TARE_COLUMNS,
            "optional decimal defaulted by mapper",
            dataset.plu_rows.len(),
            payloads,
            |p| p.plutare.is_some(),
        ),
        dest(
            "plusellingdateprint/plusellingdateterm",
            "Pludata",
            &[BEST_BEFORE_FLAG_COLUMNS, BEST_BEFORE_COLUMNS].concat(),
            "Best Before flag and term",
            dataset.plu_rows.len(),
            payloads,
            |p| p.plusellingdateprint.is_some() || p.plusellingdateterm.is_some(),
        ),
        dest(
            "pluusingdateprint/pluusingdateterm",
            "Pludata",
            EXPIRATION_COLUMNS,
            "Use By Date term with print flag derived when present",
            dataset.plu_rows.len(),
            payloads,
            |p| p.pluusingdateterm.is_some(),
        ),
        dest(
            "plulabelformat",
            "Pludata",
            PRINT_FORMAT_COLUMNS,
            "parse Label Format; source 0 defaults to effective Label Format 1",
            dataset.plu_rows.len(),
            payloads,
            |p| p.plulabelformat.is_some(),
        ),
        dest(
            "pluadditionaldatas.keylabel",
            "Pludata",
            KEY_LABEL_COLUMNS,
            "optional key label; fallback dot when absent",
            dataset.plu_rows.len(),
            payloads,
            |p| p.pluadditionaldatas.is_some(),
        ),
        dest(
            "pluingredients",
            "PluIng",
            ingredient_source_fields().as_slice(),
            "assemble Ing Name 1..99 or ingredient text columns, then DCA markup",
            dataset.ingredient_rows.len(),
            payloads,
            |p| p.pluingredients.is_some(),
        ),
        dest(
            "plunft.data",
            "PluIng",
            nutrition_source_fields().as_slice(),
            "recognized nutrition columns become NFT data rows",
            dataset.nutrition_rows.len(),
            payloads,
            |p| p.plunft.as_ref().is_some_and(|nft| !nft.data.is_empty()),
        ),
    ]
}

fn dest(
    destination: &str,
    source_table: &str,
    source_columns: &[&str],
    transformation: &str,
    source_rows: usize,
    payloads: &[(&Plu, DigiwebPluPayload)],
    populated: fn(&DigiwebPluPayload) -> bool,
) -> MappingDestination {
    let populated_plus = payloads
        .iter()
        .filter(|(_, payload)| populated(payload))
        .map(|(plu, _)| plu.plu_number)
        .collect::<Vec<_>>();
    MappingDestination {
        destination: destination.to_string(),
        source_table: source_table.to_string(),
        source_columns: source_columns
            .iter()
            .map(|value| (*value).to_string())
            .collect(),
        transformation: transformation.to_string(),
        plus_with_source_data: source_rows,
        plus_with_populated_target: populated_plus.len(),
        rejected_or_skipped: payloads.len().saturating_sub(populated_plus.len()),
        representative_plus: populated_plus.into_iter().take(20).collect(),
        warning: None,
    }
}

fn separation_audit(
    dataset: &SourceDataset,
    payloads: &[(&Plu, DigiwebPluPayload)],
    destinations: &[MappingDestination],
) -> SeparationAudit {
    let ingredient_payload = payloads
        .iter()
        .filter(|(_, payload)| payload.pluingredients.is_some())
        .count();
    let nutrition_payload = payloads
        .iter()
        .filter(|(_, payload)| {
            payload
                .plunft
                .as_ref()
                .is_some_and(|nft| !nft.data.is_empty())
        })
        .count();
    let ingredient_source = dataset
        .ingredient_rows
        .iter()
        .filter(|row| has_any(row, &ingredient_source_fields()))
        .count();
    let nutrition_source = dataset
        .nutrition_rows
        .iter()
        .filter(|row| has_any(row, &nutrition_source_fields()))
        .count();
    let mut warnings = Vec::new();
    if nutrition_source > 0 && nutrition_payload == 0 {
        warnings.push(
            "nutrition source values exist but no nutrition payload section was generated"
                .to_string(),
        );
    }
    if ingredient_source > 0 && ingredient_payload == 0 {
        warnings.push(
            "ingredient source values exist but no ingredient payload section was generated"
                .to_string(),
        );
    }
    warnings.extend(provenance_warnings(destinations));
    warnings.sort();
    warnings.dedup();
    let status = if warnings
        .iter()
        .any(|warning| warning.contains("wrong provenance"))
    {
        MappingAuditStatus::Fail
    } else if warnings.is_empty() {
        MappingAuditStatus::Pass
    } else {
        MappingAuditStatus::Warning
    };
    SeparationAudit {
        status,
        ingredients_source_table: "PluIng".to_string(),
        ingredients_source_fields: ingredient_source_fields()
            .into_iter()
            .map(ToOwned::to_owned)
            .collect(),
        ingredients_destination: "pluingredients".to_string(),
        nutrition_source_table: "PluIng".to_string(),
        nutrition_source_fields: nutrition_source_fields()
            .into_iter()
            .map(ToOwned::to_owned)
            .collect(),
        nutrition_destination: "plunft.data".to_string(),
        plus_with_ingredient_source: ingredient_source,
        plus_with_ingredient_payload: ingredient_payload,
        plus_with_nutrition_source: nutrition_source,
        plus_with_nutrition_payload: nutrition_payload,
        warnings,
    }
}

fn sample(plu: &Plu, payload: &DigiwebPluPayload) -> PayloadSample {
    let mut destinations = vec![
        "pluno",
        "pludepartmentno",
        "plugroupno",
        "plubarcodedata",
        "plucommname",
        "pluunitprice",
    ]
    .into_iter()
    .map(ToOwned::to_owned)
    .collect::<Vec<_>>();
    if payload.pluingredients.is_some() {
        destinations.push("pluingredients".to_string());
    }
    if payload.plulabelformat.is_some() {
        destinations.push("plulabelformat".to_string());
    }
    let nutrition_fact_count = payload
        .plunft
        .as_ref()
        .map(|nft| nft.data.len())
        .unwrap_or_default();
    if nutrition_fact_count > 0 {
        destinations.push("plunft.data".to_string());
    }
    PayloadSample {
        plu_number: plu.plu_number,
        populated_destinations: destinations,
        ingredient_present: payload.pluingredients.is_some(),
        nutrition_fact_count,
        raw_label_format: plu.label_format,
        effective_label_format: effective_label_format(plu.label_format),
    }
}

fn ingredient_source_fields() -> Vec<&'static str> {
    let mut fields = (1..=99)
        .map(|index| format!("Ing Name {index}"))
        .collect::<Vec<_>>();
    fields.extend(
        INGREDIENT_TEXT_COLUMNS
            .iter()
            .map(|value| (*value).to_string()),
    );
    fields
        .into_iter()
        .map(|value| Box::leak(value.into_boxed_str()) as &'static str)
        .collect()
}

fn nutrition_source_fields() -> Vec<&'static str> {
    PLUING_NUTRITION_COLUMNS
        .iter()
        .flat_map(|(_, amount, pct)| [Some(*amount), *pct])
        .flatten()
        .collect()
}

fn has_any(row: &crate::source::SourceRow, columns: &[&str]) -> bool {
    columns
        .iter()
        .filter_map(|column| row.get(column))
        .any(|value| !value.trim().is_empty())
}

fn provenance_warnings(destinations: &[MappingDestination]) -> Vec<String> {
    let ingredient_fields = ingredient_source_fields()
        .into_iter()
        .map(ToOwned::to_owned)
        .collect::<std::collections::BTreeSet<_>>();
    let nutrition_fields = nutrition_source_fields()
        .into_iter()
        .map(ToOwned::to_owned)
        .collect::<std::collections::BTreeSet<_>>();
    let mut warnings = Vec::new();
    for destination in destinations {
        if destination.destination == "pluingredients" {
            for column in &destination.source_columns {
                if nutrition_fields.contains(column) {
                    warnings.push(format!(
                        "wrong provenance: PluIng.{column} -> pluingredients"
                    ));
                }
            }
        }
        if destination.destination == "plunft.data" {
            for column in &destination.source_columns {
                if ingredient_fields.contains(column) {
                    warnings.push(format!("wrong provenance: PluIng.{column} -> plunft.data"));
                }
            }
        }
    }
    warnings
}

fn line(out: &mut String, text: impl AsRef<str>) {
    out.push_str(text.as_ref());
    out.push('\n');
}

fn blank(out: &mut String) {
    out.push('\n');
}

#[allow(dead_code)]
fn _duration_ms(duration: Duration) -> u128 {
    duration.as_millis()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use rust_decimal::Decimal;

    use super::*;
    use crate::models::plu::{Plu, PriceMode};
    use crate::source::SourceRow;

    fn plu() -> Plu {
        Plu {
            plu_number: 1,
            store_number: 1,
            department_number: Some(1),
            group_number: Some(10),
            source_department: Some("1".to_string()),
            source_group: Some("10".to_string()),
            group_default_applied: false,
            name: "Apple".to_string(),
            barcode: Some("0200001".to_string()),
            barcode_type: Some("5".to_string()),
            barcode_ref_no: Some("5".to_string()),
            source_barcode: Some("1".to_string()),
            source_barcode_format: Some("05".to_string()),
            source_flag_data: Some("02".to_string()),
            price: Decimal::new(199, 2),
            price_mode: PriceMode::ByEach,
            price_calc_method: Some(0),
            quantity: Some(0),
            quantity_symbol: Some(0),
            tare: Some(Decimal::ZERO),
            discount_type: Some(0),
            packing_date_print: Some(0),
            packing_time_print: Some(0),
            selling_date_print: Some(0),
            selling_date_term: Some(0),
            expiration_days: None,
            label_format: Some(0),
            traceability: Some(0),
            short_description: None,
            key_label: None,
            ingredients: Some("Apples".to_string()),
            nutrition_facts: vec![crate::models::nutrition::NutritionFact {
                name: "calories".to_string(),
                amount: Some("10".to_string()),
                unit: None,
            }],
            source_pluing_row_count: 1,
        }
    }

    #[test]
    fn ingredients_and_nutrition_destinations_are_separate() {
        let dataset = SourceDataset {
            plu_rows: Vec::new(),
            ingredient_rows: vec![SourceRow {
                table: "PluIng".to_string(),
                values: BTreeMap::from([
                    ("Ing Name 1".to_string(), "Apples".to_string()),
                    ("Calories".to_string(), "10".to_string()),
                ]),
            }],
            nutrition_rows: vec![SourceRow {
                table: "PluIng".to_string(),
                values: BTreeMap::from([
                    ("Ing Name 1".to_string(), "Apples".to_string()),
                    ("Calories".to_string(), "10".to_string()),
                ]),
            }],
        };
        let now = Local::now();
        let report = build_mapping_audit_report(MappingAuditInput {
            source_path: "plu.mdb",
            source_sha256: "abc",
            started_at: now,
            finished_at: now,
            dataset: &dataset,
            valid_plus: &[plu()],
            config: &DigiwebConfig::default(),
            sample_limit: 5,
            target_plu: None,
            timings: Vec::new(),
        })
        .expect("audit");

        assert_eq!(
            report
                .ingredient_nutrition_separation
                .ingredients_destination,
            "pluingredients"
        );
        assert_eq!(
            report.ingredient_nutrition_separation.nutrition_destination,
            "plunft.data"
        );
        assert!(report.samples[0].ingredient_present);
        assert_eq!(report.samples[0].nutrition_fact_count, 1);
    }

    #[test]
    fn ingredient_text_with_nutrition_word_is_not_cross_mapping_warning() {
        let mut plu = plu();
        plu.ingredients = Some("Pea protein, salt".to_string());
        plu.nutrition_facts = Vec::new();
        let dataset = SourceDataset {
            plu_rows: Vec::new(),
            ingredient_rows: vec![SourceRow {
                table: "PluIng".to_string(),
                values: BTreeMap::from([(
                    "Ing Name 1".to_string(),
                    "Pea protein, salt".to_string(),
                )]),
            }],
            nutrition_rows: Vec::new(),
        };
        let now = Local::now();
        let report = build_mapping_audit_report(MappingAuditInput {
            source_path: "plu.mdb",
            source_sha256: "abc",
            started_at: now,
            finished_at: now,
            dataset: &dataset,
            valid_plus: &[plu],
            config: &DigiwebConfig::default(),
            sample_limit: 5,
            target_plu: None,
            timings: Vec::new(),
        })
        .expect("audit");

        assert_eq!(
            report.ingredient_nutrition_separation.status,
            MappingAuditStatus::Pass
        );
        assert!(
            !report
                .warnings
                .iter()
                .any(|warning| warning.contains("nutrition token"))
        );
    }

    #[test]
    fn plulabelformat_audit_reports_raw_and_effective_zero_default() {
        let dataset = SourceDataset {
            plu_rows: Vec::new(),
            ingredient_rows: Vec::new(),
            nutrition_rows: Vec::new(),
        };
        let plu = plu();
        let now = Local::now();
        let report = build_mapping_audit_report(MappingAuditInput {
            source_path: "plu.mdb",
            source_sha256: "abc",
            started_at: now,
            finished_at: now,
            dataset: &dataset,
            valid_plus: &[plu],
            config: &DigiwebConfig::default(),
            sample_limit: 5,
            target_plu: None,
            timings: Vec::new(),
        })
        .expect("audit");

        let destination = report
            .destinations
            .iter()
            .find(|destination| destination.destination == "plulabelformat")
            .expect("label destination");
        assert!(
            destination
                .transformation
                .contains("source 0 defaults to effective Label Format 1")
        );
        assert_eq!(report.samples[0].raw_label_format, Some(0));
        assert_eq!(report.samples[0].effective_label_format, Some(1));
        assert!(
            report.samples[0]
                .populated_destinations
                .contains(&"plulabelformat".to_string())
        );
    }

    #[test]
    fn wrong_provenance_mapping_is_detected() {
        let warnings = provenance_warnings(&[
            MappingDestination {
                destination: "pluingredients".to_string(),
                source_table: "PluIng".to_string(),
                source_columns: vec!["Calories".to_string()],
                transformation: "bad".to_string(),
                plus_with_source_data: 1,
                plus_with_populated_target: 1,
                rejected_or_skipped: 0,
                representative_plus: vec![1],
                warning: None,
            },
            MappingDestination {
                destination: "plunft.data".to_string(),
                source_table: "PluIng".to_string(),
                source_columns: vec!["Ing Name 1".to_string()],
                transformation: "bad".to_string(),
                plus_with_source_data: 1,
                plus_with_populated_target: 1,
                rejected_or_skipped: 0,
                representative_plus: vec![1],
                warning: None,
            },
        ]);

        assert!(
            warnings.contains(&"wrong provenance: PluIng.Calories -> pluingredients".to_string())
        );
        assert!(
            warnings.contains(&"wrong provenance: PluIng.Ing Name 1 -> plunft.data".to_string())
        );
    }

    #[test]
    fn sample_limit_is_respected() {
        let dataset = SourceDataset::default();
        let plus = vec![
            plu(),
            Plu {
                plu_number: 2,
                ..plu()
            },
        ];
        let now = Local::now();
        let report = build_mapping_audit_report(MappingAuditInput {
            source_path: "plu.mdb",
            source_sha256: "abc",
            started_at: now,
            finished_at: now,
            dataset: &dataset,
            valid_plus: &plus,
            config: &DigiwebConfig::default(),
            sample_limit: 1,
            target_plu: None,
            timings: Vec::new(),
        })
        .expect("audit");

        assert_eq!(report.samples.len(), 1);
    }

    #[test]
    fn missing_target_plu_is_reported_without_submission() {
        let dataset = SourceDataset::default();
        let plus = vec![plu()];
        let now = Local::now();
        let report = build_mapping_audit_report(MappingAuditInput {
            source_path: "plu.mdb",
            source_sha256: "abc",
            started_at: now,
            finished_at: now,
            dataset: &dataset,
            valid_plus: &plus,
            config: &DigiwebConfig::default(),
            sample_limit: 5,
            target_plu: Some(99),
            timings: Vec::new(),
        })
        .expect("audit");

        assert_eq!(report.selected_plu_count, 0);
        assert_eq!(report.safety.plus_submitted, 0);
        assert!(
            report
                .warnings
                .iter()
                .any(|warning| warning.contains("PLU 99"))
        );
    }
}
