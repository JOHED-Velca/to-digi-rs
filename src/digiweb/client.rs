use std::time::{Duration, Instant};

use reqwest::StatusCode;
use serde::Deserialize;
use tokio::time::sleep;

use crate::config::AppConfig;
use crate::digiweb::auth::AccessToken;
use crate::digiweb::payload::DigiwebPluPayload;
use crate::digiweb::status::ProcessingStatus;
use crate::error::AppError;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DigiwebWriteResponse {
    pub request_id: Option<String>,
    pub status: Option<ProcessingStatus>,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DigiwebStatusResponse {
    pub status: ProcessingStatus,
    pub message: Option<String>,
}

#[derive(Clone)]
pub struct DigiwebClient {
    http: reqwest::Client,
    config: AppConfig,
}

impl DigiwebClient {
    pub fn new(config: AppConfig) -> Result<Self, AppError> {
        let mut builder = reqwest::Client::builder()
            .timeout(Duration::from_secs(config.timeouts.request_seconds));
        if config.digiweb.allow_invalid_certificates {
            builder = builder.danger_accept_invalid_certs(true);
        }
        let http = builder
            .build()
            .map_err(|err| AppError::Network(err.to_string()))?;
        Ok(Self { http, config })
    }

    pub fn http(&self) -> &reqwest::Client {
        &self.http
    }

    pub async fn upsert_plu(
        &self,
        token: &AccessToken,
        payload: &DigiwebPluPayload,
    ) -> Result<(Option<String>, ProcessingStatus, Option<String>), AppError> {
        let url = self.join_base_path(self.config.plu_upsert_path()?)?;
        let response = self
            .http
            .post(url)
            .header("Authorization", token.bearer_value())
            .json(payload)
            .send()
            .await
            .map_err(|err| AppError::Network(err.to_string()))?;
        let status = response.status();
        if !status.is_success() {
            return Err(AppError::Http(format!(
                "PLU {} returned HTTP {status}",
                payload.plu_number
            )));
        }
        let body = if status == StatusCode::NO_CONTENT {
            DigiwebWriteResponse {
                request_id: None,
                status: Some(ProcessingStatus::Success),
                message: None,
            }
        } else {
            response
                .json::<DigiwebWriteResponse>()
                .await
                .map_err(|err| AppError::Http(format!("invalid PLU response body: {err}")))?
        };
        let initial_status = body.status.unwrap_or(ProcessingStatus::Success);
        if initial_status == ProcessingStatus::Processing {
            let request_id = body.request_id.clone().ok_or_else(|| {
                AppError::DigiwebProcessing(
                    "PROCESSING response did not include requestId".to_string(),
                )
            })?;
            let final_status = self.poll_request_status(&request_id).await?;
            Ok((Some(request_id), final_status.status, final_status.message))
        } else {
            Ok((body.request_id, initial_status, body.message))
        }
    }

    pub async fn poll_request_status(
        &self,
        request_id: &str,
    ) -> Result<DigiwebStatusResponse, AppError> {
        let template = self.config.digiweb.request_status_path_template.trim();
        if template.is_empty() {
            return Err(AppError::Config(
                "digiweb.request_status_path_template is required for PROCESSING responses"
                    .to_string(),
            ));
        }
        let path = template.replace("{request_id}", request_id);
        let url = self.join_base_path(&path)?;
        let deadline =
            Instant::now() + Duration::from_secs(self.config.timeouts.poll_timeout_seconds);
        loop {
            let response = self
                .http
                .get(url.clone())
                .send()
                .await
                .map_err(|err| AppError::Network(err.to_string()))?;
            if !response.status().is_success() {
                return Err(AppError::Http(format!(
                    "status request {request_id} returned HTTP {}",
                    response.status()
                )));
            }
            let status_response = response
                .json::<DigiwebStatusResponse>()
                .await
                .map_err(|err| AppError::Http(format!("invalid status response body: {err}")))?;
            if status_response.status != ProcessingStatus::Processing {
                return Ok(status_response);
            }
            if Instant::now() >= deadline {
                return Ok(DigiwebStatusResponse {
                    status: ProcessingStatus::UnknownOrTimeout,
                    message: Some(format!(
                        "request did not complete within {} seconds",
                        self.config.timeouts.poll_timeout_seconds
                    )),
                });
            }
            sleep(Duration::from_secs(
                self.config.timeouts.poll_interval_seconds,
            ))
            .await;
        }
    }

    fn join_base_path(&self, path: &str) -> Result<String, AppError> {
        if path.starts_with("http://") || path.starts_with("https://") {
            return Ok(path.to_string());
        }
        if !path.starts_with('/') {
            return Err(AppError::Config(format!(
                "endpoint path '{path}' must start with '/'"
            )));
        }
        Ok(format!(
            "{}{}",
            self.config.digiweb.base_url.trim_end_matches('/'),
            path
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{AppConfig, DigiwebConfig, ImportConfig, MappingConfig, TimeoutConfig};

    #[test]
    fn api_success_response_parses() {
        let response: DigiwebWriteResponse =
            serde_json::from_str(r#"{"requestId":"abc","status":"SUCCESS","message":"done"}"#)
                .expect("parse");

        assert_eq!(response.request_id.as_deref(), Some("abc"));
        assert_eq!(response.status, Some(ProcessingStatus::Success));
    }

    #[test]
    fn api_failure_response_parses() {
        let response: DigiwebWriteResponse =
            serde_json::from_str(r#"{"requestId":"abc","status":"FAIL","message":"bad PLU"}"#)
                .expect("parse");

        assert_eq!(response.status, Some(ProcessingStatus::Fail));
        assert_eq!(response.message.as_deref(), Some("bad PLU"));
    }

    #[tokio::test]
    async fn status_polling_success() {
        let server = TestServer::start(vec![r#"{"status":"SUCCESS","message":"done"}"#]).await;
        let client = DigiwebClient::new(test_config(&server.base_url, 1, 5)).expect("client");

        let response = client.poll_request_status("abc").await.expect("poll");

        assert_eq!(response.status, ProcessingStatus::Success);
        assert_eq!(response.message.as_deref(), Some("done"));
    }

    #[tokio::test]
    async fn status_polling_failure() {
        let server = TestServer::start(vec![r#"{"status":"FAIL","message":"bad"}"#]).await;
        let client = DigiwebClient::new(test_config(&server.base_url, 1, 5)).expect("client");

        let response = client.poll_request_status("abc").await.expect("poll");

        assert_eq!(response.status, ProcessingStatus::Fail);
        assert_eq!(response.message.as_deref(), Some("bad"));
    }

    #[tokio::test]
    async fn status_polling_timeout() {
        let server = TestServer::start(vec![r#"{"status":"PROCESSING","message":"wait"}"#]).await;
        let client = DigiwebClient::new(test_config(&server.base_url, 1, 0)).expect("client");

        let response = client.poll_request_status("abc").await.expect("poll");

        assert_eq!(response.status, ProcessingStatus::UnknownOrTimeout);
    }

    fn test_config(
        base_url: &str,
        poll_interval_seconds: u64,
        poll_timeout_seconds: u64,
    ) -> AppConfig {
        AppConfig {
            digiweb: DigiwebConfig {
                base_url: base_url.to_string(),
                client_id: "digi".to_string(),
                token_url: format!("{base_url}/token"),
                store_number: 1,
                allow_invalid_certificates: false,
                plu_upsert_path: "/plu".to_string(),
                request_status_path_template: "/status/{request_id}".to_string(),
            },
            timeouts: TimeoutConfig {
                request_seconds: 5,
                poll_interval_seconds,
                poll_timeout_seconds,
            },
            import: ImportConfig::default(),
            mapping: MappingConfig::default(),
        }
    }

    struct TestServer {
        base_url: String,
    }

    impl TestServer {
        async fn start(responses: Vec<&'static str>) -> Self {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            use tokio::net::TcpListener;

            let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
            let addr = listener.local_addr().expect("addr");
            tokio::spawn(async move {
                for response_body in responses {
                    let Ok((mut stream, _peer)) = listener.accept().await else {
                        return;
                    };
                    let mut buffer = [0_u8; 1024];
                    let _ = stream.read(&mut buffer).await;
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        response_body.len(),
                        response_body
                    );
                    let _ = stream.write_all(response.as_bytes()).await;
                }
            });
            Self {
                base_url: format!("http://{addr}"),
            }
        }
    }
}
