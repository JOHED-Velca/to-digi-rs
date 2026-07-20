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

    logger.line("Authenticating with DIGIweb.")?;
    let token = authenticate(client.http(), &config, &client_secret).await?;
    logger.kv("Authentication result", "SUCCESS")?;

    let mut summary = ImportSummary {
        discovered: plus.len(),
        ..ImportSummary::default()
    };
    let records_to_send = if config.import.send_only_first_plu {
        plus.iter().take(1).collect::<Vec<_>>()
    } else {
        plus.iter().collect::<Vec<_>>()
    };

    if config.import.send_only_first_plu && plus.len() > 1 {
        summary.skipped += plus.len() - 1;
        logger.warning(format!(
            "send_only_first_plu is enabled; {} PLU(s) will be skipped.",
            plus.len() - 1
        ))?;
    }

    for plu in records_to_send {
        let started_at = Local::now();
        let timer = std::time::Instant::now();
        logger.kv("Importing PLU", &plu.plu_number.to_string())?;
        let payload = DigiwebPluPayload::from(plu);
        match client.upsert_plu(&token, &payload).await {
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
                        .saturating_sub(summary.succeeded + summary.failed);
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
                        .saturating_sub(summary.succeeded + summary.failed);
                    break;
                }
            }
        }
    }
    Ok(summary)
}
