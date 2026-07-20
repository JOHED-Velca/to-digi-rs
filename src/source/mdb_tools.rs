use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

use crate::config::MappingConfig;
use crate::error::AppError;
use crate::logging::AuditLogger;
use crate::source::schema::MdbSchema;
use crate::source::{SourceDataset, SourceRow};

const REQUIRED_COMMANDS: &[&str] = &["mdb-tables", "mdb-schema", "mdb-export"];

pub struct MdbTools;

impl MdbTools {
    pub fn verify_required_commands() -> Result<(), AppError> {
        for command in REQUIRED_COMMANDS {
            match Command::new(command).arg("--help").output() {
                Ok(_) => {}
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                    return Err(AppError::MdbToolsUnavailable(format!(
                        "{command} was not found. Install with: sudo apt install mdbtools"
                    )));
                }
                Err(err) => {
                    return Err(AppError::MdbToolsUnavailable(format!("{command}: {err}")));
                }
            }
        }
        Ok(())
    }

    pub fn inspect_schema(path: &Path) -> Result<MdbSchema, AppError> {
        let output = Command::new("mdb-tables")
            .arg("-1")
            .arg(path)
            .output()
            .map_err(|err| AppError::MdbSchema(err.to_string()))?;
        if !output.status.success() {
            return Err(AppError::MdbSchema(stderr_or_status(&output)));
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        let tables = stdout
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(ToOwned::to_owned)
            .collect();
        Ok(MdbSchema {
            tables,
            columns: BTreeMap::new(),
        })
    }

    pub fn export_table(
        path: &Path,
        table: &str,
    ) -> Result<(Vec<String>, Vec<SourceRow>), AppError> {
        let output = Command::new("mdb-export")
            .arg(path)
            .arg(table)
            .output()
            .map_err(|err| AppError::MdbExport(err.to_string()))?;
        if !output.status.success() {
            return Err(AppError::MdbExport(stderr_or_status(&output)));
        }
        let mut reader = csv::Reader::from_reader(output.stdout.as_slice());
        let headers = reader
            .headers()?
            .iter()
            .map(ToOwned::to_owned)
            .collect::<Vec<_>>();
        let mut rows = Vec::new();
        for record in reader.records() {
            let record = record?;
            let values = headers
                .iter()
                .cloned()
                .zip(record.iter().map(ToOwned::to_owned))
                .collect::<BTreeMap<_, _>>();
            rows.push(SourceRow {
                table: table.to_string(),
                values,
            });
        }
        Ok((headers, rows))
    }

    pub fn read_dataset(
        path: &Path,
        mapping: &MappingConfig,
        logger: &mut AuditLogger,
    ) -> Result<(MdbSchema, SourceDataset), AppError> {
        let mut schema = Self::inspect_schema(path)?;
        logger.kv("MDB tables discovered", &schema.tables.join(", "))?;

        if !schema.has_table(&mapping.main_plu_table) {
            return Err(AppError::MdbSchema(format!(
                "main PLU table '{}' was not found",
                mapping.main_plu_table
            )));
        }

        let (columns, plu_rows) = Self::export_table(path, &mapping.main_plu_table)?;
        schema.set_columns(&mapping.main_plu_table, columns.clone());
        logger.kv(
            &format!("Columns in {}", mapping.main_plu_table),
            &columns.join(", "),
        )?;

        let ingredient_rows = if schema.has_table(&mapping.ingredient_table) {
            let (columns, rows) = Self::export_table(path, &mapping.ingredient_table)?;
            schema.set_columns(&mapping.ingredient_table, columns.clone());
            logger.kv(
                &format!("Columns in {}", mapping.ingredient_table),
                &columns.join(", "),
            )?;
            rows
        } else {
            logger.warning(format!(
                "Optional ingredient table '{}' was not found; continuing without ingredients.",
                mapping.ingredient_table
            ))?;
            Vec::new()
        };

        let nutrition_rows = if schema.has_table(&mapping.nutrition_table) {
            let (columns, rows) = Self::export_table(path, &mapping.nutrition_table)?;
            schema.set_columns(&mapping.nutrition_table, columns.clone());
            logger.kv(
                &format!("Columns in {}", mapping.nutrition_table),
                &columns.join(", "),
            )?;
            rows
        } else {
            logger.warning(format!(
                "Optional nutrition table '{}' was not found; continuing without nutrition facts.",
                mapping.nutrition_table
            ))?;
            Vec::new()
        };

        Ok((
            schema,
            SourceDataset {
                plu_rows,
                ingredient_rows,
                nutrition_rows,
            },
        ))
    }
}

fn stderr_or_status(output: &std::process::Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if stderr.is_empty() {
        format!("command exited with status {}", output.status)
    } else {
        stderr
    }
}
