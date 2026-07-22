use std::fs;
use std::path::{Path, PathBuf};

use chrono::Local;
use secrecy::SecretString;

use crate::config::AppConfig;
use crate::digiweb::auth::authenticate;
use crate::digiweb::client::DigiwebClient;
use crate::digiweb::payload::DigiwebPluPayload;
use crate::digiweb::status::ProcessingStatus;
use crate::error::AppError;
use crate::import::result::{ImportSummary, RecordImportResult};
use crate::logging::AuditLogger;
use crate::models::plu::Plu;

pub async fn run_import(
    config: AppConfig,
    client_secret: SecretString,
    plus: &[Plu],
    logger: &mut AuditLogger,
) -> Result<ImportSummary, AppError> {
    config.token_url()?;
    config.plu_upsert_path()?;
    let client = DigiwebClient::new(config.clone())?;

    let mut summary = ImportSummary {
        discovered: plus.len(),
        ..ImportSummary::default()
    };
    let records_to_send = select_records_to_send(plus, config.import.send_only_first_plu);
    let selected_count = records_to_send.len();
    summary.selected = selected_count;

    prepare_payload_preview_dir(config.import.write_payload_preview)?;

    if config.import.send_only_first_plu && plus.len() > 1 {
        summary.intentionally_skipped_by_limit += plus.len() - 1;
        logger.warning(format!(
            "send_only_first_plu is enabled; {} PLU(s) will be intentionally skipped by the first-PLU limit.",
            plus.len() - 1
        ))?;
    }
    if let Some(first) = records_to_send.first() {
        logger.kv("Selected first valid PLU", &first.plu_number.to_string())?;
        logger.kv(
            "Matching PluIng rows for selected PLU",
            &first.source_pluing_row_count.to_string(),
        )?;
        logger.kv(
            "Selected PLU group default applied",
            if first.group_default_applied {
                "yes"
            } else {
                "no"
            },
        )?;
    }

    logger.line("Authenticating with DIGIweb.")?;
    let token = authenticate(client.http(), &config, &client_secret).await?;
    logger.kv("Authentication result", "SUCCESS")?;

    for (index, plu) in records_to_send.iter().enumerate() {
        let progress = format!("[{}/{}]", index + 1, selected_count);
        let started_at = Local::now();
        let timer = std::time::Instant::now();
        logger.line(format!("{progress} Importing PLU {}", plu.plu_number))?;
        let payload = DigiwebPluPayload::from_plu(plu, &config.digiweb)?;
        if config.import.write_payload_preview {
            let path = write_payload_preview(plu.plu_number, &payload)?;
            logger.line(format!(
                "{progress} Payload preview written: {}",
                path.display()
            ))?;
        }
        match client
            .upsert_plu_with_progress(&token, &payload, logger, &progress)
            .await
        {
            Ok((request_id, final_status, message))
                if final_status == ProcessingStatus::Success =>
            {
                summary.submitted += 1;
                summary.succeeded += 1;
                logger.line(format!("{progress} Final status: SUCCESS"))?;
                logger.line(format!(
                    "{progress} Duration ms: {}",
                    timer.elapsed().as_millis()
                ))?;
                summary.records.push(RecordImportResult {
                    plu_number: plu.plu_number,
                    started_at,
                    api_request_id: request_id,
                    http_result: "2xx".to_string(),
                    final_status,
                    failure_message: message,
                    duration_ms: timer.elapsed().as_millis(),
                });
            }
            Ok((request_id, final_status, message))
                if matches!(
                    final_status,
                    ProcessingStatus::SubmittedStatusUnknown | ProcessingStatus::UnknownOrTimeout
                ) =>
            {
                summary.submitted += 1;
                summary.unknown += 1;
                let unknown_message = message.unwrap_or_else(|| {
                    "DIGIweb accepted the submission but the final status is unknown".to_string()
                });
                logger.warning(format!(
                    "{progress} PLU {} submitted with unknown final status: {}",
                    plu.plu_number, unknown_message
                ))?;
                logger.line(format!(
                    "{progress} Final status: {}",
                    final_status.as_str()
                ))?;
                logger.line(format!(
                    "{progress} Duration ms: {}",
                    timer.elapsed().as_millis()
                ))?;
                summary.records.push(RecordImportResult {
                    plu_number: plu.plu_number,
                    started_at,
                    api_request_id: request_id,
                    http_result: "2xx".to_string(),
                    final_status,
                    failure_message: Some(unknown_message),
                    duration_ms: timer.elapsed().as_millis(),
                });
                if !config.import.continue_after_record_failure {
                    summary.not_attempted_after_stop += skipped_after_stop(
                        selected_count,
                        summary.succeeded,
                        summary.failed,
                        summary.unknown,
                    );
                    break;
                }
            }
            Ok((request_id, final_status, message)) => {
                summary.submitted += 1;
                summary.failed += 1;
                let failure = message
                    .unwrap_or_else(|| format!("DIGIweb final status {}", final_status.as_str()));
                logger.error(format!(
                    "{progress} PLU {} failed: {}",
                    plu.plu_number, failure
                ))?;
                logger.line(format!(
                    "{progress} Final status: {}",
                    final_status.as_str()
                ))?;
                logger.line(format!(
                    "{progress} Duration ms: {}",
                    timer.elapsed().as_millis()
                ))?;
                summary.records.push(RecordImportResult {
                    plu_number: plu.plu_number,
                    started_at,
                    api_request_id: request_id,
                    http_result: "2xx".to_string(),
                    final_status,
                    failure_message: Some(failure),
                    duration_ms: timer.elapsed().as_millis(),
                });
                if !config.import.continue_after_record_failure {
                    summary.not_attempted_after_stop += skipped_after_stop(
                        selected_count,
                        summary.succeeded,
                        summary.failed,
                        summary.unknown,
                    );
                    break;
                }
            }
            Err(err) => {
                summary.failed += 1;
                logger.error(format!("{progress} PLU {} failed: {}", plu.plu_number, err))?;
                logger.line(format!("{progress} Final status: FAIL"))?;
                logger.line(format!(
                    "{progress} Duration ms: {}",
                    timer.elapsed().as_millis()
                ))?;
                summary.records.push(RecordImportResult {
                    plu_number: plu.plu_number,
                    started_at,
                    api_request_id: None,
                    http_result: err.to_string(),
                    final_status: ProcessingStatus::Fail,
                    failure_message: Some(err.to_string()),
                    duration_ms: timer.elapsed().as_millis(),
                });
                if !config.import.continue_after_record_failure {
                    summary.not_attempted_after_stop += skipped_after_stop(
                        selected_count,
                        summary.succeeded,
                        summary.failed,
                        summary.unknown,
                    );
                    break;
                }
            }
        }
    }
    Ok(summary)
}

fn prepare_payload_preview_dir(enabled: bool) -> Result<(), AppError> {
    prepare_payload_preview_dir_in_dir(Path::new("."), enabled)
}

fn prepare_payload_preview_dir_in_dir(base_dir: &Path, enabled: bool) -> Result<(), AppError> {
    if !enabled {
        return Ok(());
    }
    let dir = base_dir.join("payload-previews");
    if !dir.exists() {
        return Ok(());
    }
    let entries = fs::read_dir(&dir).map_err(|err| {
        AppError::Logging(format!(
            "failed to inspect payload preview directory '{}': {err}",
            dir.display()
        ))
    })?;
    for entry in entries {
        let entry = entry.map_err(|err| {
            AppError::Logging(format!(
                "failed to inspect payload preview directory '{}': {err}",
                dir.display()
            ))
        })?;
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) == Some("json") {
            fs::remove_file(&path).map_err(|err| {
                AppError::Logging(format!(
                    "failed to remove old payload preview '{}': {err}",
                    path.display()
                ))
            })?;
        }
    }
    Ok(())
}

fn write_payload_preview(
    plu_number: u64,
    payload: &DigiwebPluPayload,
) -> Result<PathBuf, AppError> {
    write_payload_preview_in_dir(Path::new("."), plu_number, payload)
}

fn write_payload_preview_in_dir(
    base_dir: &Path,
    plu_number: u64,
    payload: &DigiwebPluPayload,
) -> Result<PathBuf, AppError> {
    let dir = base_dir.join("payload-previews");
    fs::create_dir_all(&dir).map_err(|err| {
        AppError::Logging(format!(
            "failed to create payload preview directory '{}': {err}",
            dir.display()
        ))
    })?;
    let path = dir.join(format!("plu-{plu_number}.json"));
    let preview = serde_json::to_string_pretty(payload)
        .map_err(|err| AppError::Internal(format!("payload preview failed: {err}")))?;
    fs::write(&path, preview).map_err(|err| {
        AppError::Logging(format!(
            "failed to write payload preview '{}': {err}",
            path.display()
        ))
    })?;
    Ok(fs::canonicalize(&path).unwrap_or(path))
}

pub fn select_records_to_send(plus: &[Plu], send_only_first_plu: bool) -> Vec<&Plu> {
    if send_only_first_plu {
        plus.iter().take(1).collect()
    } else {
        plus.iter().collect()
    }
}

pub fn skipped_after_stop(
    selected_count: usize,
    succeeded: usize,
    failed: usize,
    unknown: usize,
) -> usize {
    selected_count.saturating_sub(succeeded + failed + unknown)
}

#[cfg(test)]
mod tests {
    use rust_decimal::Decimal;

    use super::*;
    use crate::models::plu::PriceMode;

    fn plu(plu_number: u64) -> Plu {
        Plu {
            plu_number,
            store_number: 1,
            department_number: Some(1),
            group_number: Some(1),
            source_department: Some("0001".to_string()),
            source_group: Some("1".to_string()),
            group_default_applied: false,
            name: format!("PLU {plu_number}"),
            barcode: Some(format!("020{plu_number:05}")),
            barcode_type: Some("5".to_string()),
            barcode_ref_no: Some("5".to_string()),
            source_barcode: Some(plu_number.to_string()),
            source_barcode_format: Some("05".to_string()),
            source_flag_data: Some("02".to_string()),
            price: Decimal::new(100, 2),
            price_mode: PriceMode::ByEach,
            price_calc_method: None,
            quantity: None,
            quantity_symbol: None,
            tare: None,
            discount_type: None,
            packing_date_print: None,
            packing_time_print: None,
            selling_date_print: None,
            selling_date_term: None,
            label_format: None,
            traceability: None,
            short_description: None,
            key_label: None,
            expiration_days: None,
            ingredients: None,
            nutrition_facts: Vec::new(),
            source_pluing_row_count: 0,
        }
    }

    #[test]
    fn send_only_first_plu_limits_selection_to_one_record() {
        let records = vec![plu(1), plu(2), plu(3)];

        let selected = select_records_to_send(&records, true);

        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].plu_number, 1);
    }

    #[test]
    fn send_only_first_plu_selects_first_valid_normalized_plu() {
        let valid_after_row_skips = vec![plu(1), plu(2), plu(3)];

        let selected = select_records_to_send(&valid_after_row_skips, true);

        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].plu_number, 1);
    }

    #[test]
    fn stop_after_first_selected_failure_does_not_double_count_unselected_plus() {
        let all_valid = vec![plu(1), plu(2), plu(3), plu(4)];
        let selected = select_records_to_send(&all_valid, true);
        let skipped_by_first_plu_mode = all_valid.len() - selected.len();
        let skipped_after_failure = skipped_after_stop(selected.len(), 0, 1, 0);

        assert_eq!(skipped_by_first_plu_mode, 3);
        assert_eq!(skipped_after_failure, 0);
    }

    #[test]
    fn send_only_first_plu_false_selects_all_valid_plus() {
        let records = vec![plu(1), plu(4), plu(2), plu(3)];

        let selected = select_records_to_send(&records, false);

        assert_eq!(
            selected
                .iter()
                .map(|plu| plu.plu_number)
                .collect::<Vec<_>>(),
            vec![1, 4, 2, 3]
        );
    }

    #[test]
    fn preview_file_is_written_for_payload() {
        let temp = tempfile::tempdir().expect("tempdir");
        let payload =
            DigiwebPluPayload::from_plu(&plu(1), &crate::config::DigiwebConfig::default())
                .expect("payload");

        let path = write_payload_preview_in_dir(temp.path(), 1, &payload).expect("preview");
        let contents = fs::read_to_string(&path).expect("read");

        assert!(contents.contains("\"pluno\": 1"));
        assert!(contents.contains("\"plubarcodetype\": \"5\""));
        assert!(!contents.to_ascii_lowercase().contains("secret"));
        assert!(!contents.to_ascii_lowercase().contains("token"));
    }

    #[test]
    fn preview_file_matches_submitted_json() {
        let temp = tempfile::tempdir().expect("tempdir");
        let payload =
            DigiwebPluPayload::from_plu(&plu(4), &crate::config::DigiwebConfig::default())
                .expect("payload");

        let path = write_payload_preview_in_dir(temp.path(), 4, &payload).expect("preview");
        let contents = fs::read_to_string(&path).expect("read");
        let expected = serde_json::to_string_pretty(&payload).expect("json");

        assert_eq!(contents, expected);
    }

    #[test]
    fn old_preview_json_files_are_cleaned_when_enabled() {
        let temp = tempfile::tempdir().expect("tempdir");
        let dir = temp.path().join("payload-previews");
        fs::create_dir_all(&dir).expect("dir");
        fs::write(dir.join("plu-1.json"), "{}").expect("old json");
        fs::write(dir.join("keep.txt"), "keep").expect("old txt");

        prepare_payload_preview_dir_in_dir(temp.path(), true).expect("clean");

        assert!(!dir.join("plu-1.json").exists());
        assert!(dir.join("keep.txt").exists());
    }

    #[test]
    fn preview_directory_is_not_created_when_disabled() {
        let temp = tempfile::tempdir().expect("tempdir");

        prepare_payload_preview_dir_in_dir(temp.path(), false).expect("disabled");

        assert!(!temp.path().join("payload-previews").exists());
    }
}
