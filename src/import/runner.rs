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

    if config.import.send_only_first_plu && plus.len() > 1 {
        summary.skipped += plus.len() - 1;
        logger.warning(format!(
            "send_only_first_plu is enabled; {} PLU(s) will be skipped.",
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

    for plu in records_to_send {
        let started_at = Local::now();
        let timer = std::time::Instant::now();
        logger.kv("Importing PLU", &plu.plu_number.to_string())?;
        let payload = DigiwebPluPayload::from_plu(plu, &config.digiweb)?;
        if config.import.write_payload_preview {
            let preview = serde_json::to_string_pretty(&payload)
                .map_err(|err| AppError::Internal(format!("payload preview failed: {err}")))?;
            logger.line(format!(
                "Sanitized payload preview for PLU {}:",
                plu.plu_number
            ))?;
            logger.line(preview)?;
        }
        match client.upsert_plu(&token, &payload, logger).await {
            Ok((request_id, final_status, message))
                if final_status == ProcessingStatus::Success =>
            {
                summary.succeeded += 1;
                logger.kv("PLU result", &format!("{} SUCCESS", plu.plu_number))?;
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
                if final_status == ProcessingStatus::SubmittedStatusUnknown =>
            {
                summary.unknown += 1;
                let unknown_message = message.unwrap_or_else(|| {
                    "DIGIweb accepted the submission but the final status is unknown".to_string()
                });
                logger.warning(format!(
                    "PLU {} submitted with unknown final status: {}",
                    plu.plu_number, unknown_message
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
                    summary.skipped += plus
                        .len()
                        .saturating_sub(summary.succeeded + summary.failed + summary.unknown);
                    break;
                }
            }
            Ok((request_id, final_status, message)) => {
                summary.failed += 1;
                let failure = message
                    .unwrap_or_else(|| format!("DIGIweb final status {}", final_status.as_str()));
                logger.error(format!("PLU {} failed: {}", plu.plu_number, failure))?;
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
                    summary.skipped += plus
                        .len()
                        .saturating_sub(summary.succeeded + summary.failed + summary.unknown);
                    break;
                }
            }
            Err(err) => {
                summary.failed += 1;
                logger.error(format!("PLU {} failed: {}", plu.plu_number, err))?;
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
                    summary.skipped += plus
                        .len()
                        .saturating_sub(summary.succeeded + summary.failed + summary.unknown);
                    break;
                }
            }
        }
    }
    Ok(summary)
}

pub fn select_records_to_send(plus: &[Plu], send_only_first_plu: bool) -> Vec<&Plu> {
    if send_only_first_plu {
        plus.iter().take(1).collect()
    } else {
        plus.iter().collect()
    }
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
            barcode: None,
            price: Decimal::new(100, 2),
            price_mode: PriceMode::ByEach,
            price_calc_method: None,
            quantity: None,
            quantity_symbol: None,
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
}
