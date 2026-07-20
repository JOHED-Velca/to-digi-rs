use std::time::{Duration, Instant};

use reqwest::header::LOCATION;
use reqwest::{StatusCode, Url};
use serde::Deserialize;
use tokio::time::sleep;

use crate::config::AppConfig;
use crate::digiweb::auth::AccessToken;
use crate::digiweb::payload::DigiwebPluPayload;
use crate::digiweb::status::ProcessingStatus;
use crate::error::AppError;

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct DigiwebStatusResponse {
    pub id: Option<String>,
    pub status: ProcessingStatus,
    pub method: Option<String>,
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
                payload.pluno
            )));
        }
        if status == StatusCode::CREATED {
            let location = response
                .headers()
                .get(LOCATION)
                .ok_or_else(|| {
                    AppError::DigiwebProcessing(
                        "DIGIweb returned 201 Created without Location header".to_string(),
                    )
                })?
                .to_str()
                .map_err(|err| AppError::Http(format!("invalid Location header: {err}")))?
                .to_string();
            let final_status = self.poll_location(&location).await?;
            return Ok((Some(location), final_status.status, final_status.message));
        }
        if status == StatusCode::NO_CONTENT {
            return Ok((None, ProcessingStatus::Success, None));
        }
        let status_response = response
            .json::<DigiwebStatusResponse>()
            .await
            .map_err(|err| AppError::Http(format!("invalid PLU response body: {err}")))?;
        Ok((
            status_response.id.clone(),
            status_response.status,
            status_response.message,
        ))
    }

    #[allow(dead_code)]
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
        self.poll_url(&url, request_id).await
    }

    pub async fn poll_location(&self, location: &str) -> Result<DigiwebStatusResponse, AppError> {
        let url = self.resolve_location(location)?;
        self.poll_url(&url, location).await
    }

    async fn poll_url(
        &self,
        url: &str,
        status_reference: &str,
    ) -> Result<DigiwebStatusResponse, AppError> {
        let deadline =
            Instant::now() + Duration::from_secs(self.config.timeouts.poll_timeout_seconds);
        loop {
            let response = self
                .http
                .get(url)
                .send()
                .await
                .map_err(|err| AppError::Network(err.to_string()))?;
            if !response.status().is_success() {
                return Err(AppError::Http(format!(
                    "status request {status_reference} returned HTTP {}",
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
                    id: None,
                    status: ProcessingStatus::UnknownOrTimeout,
                    method: None,
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

    fn resolve_location(&self, location: &str) -> Result<String, AppError> {
        if location.starts_with("http://") || location.starts_with("https://") {
            return Ok(location.to_string());
        }
        if location.starts_with('/') {
            return self.join_base_path(location);
        }
        let base = Url::parse(self.config.digiweb.base_url.trim_end_matches('/'))
            .map_err(|err| AppError::Config(format!("invalid digiweb.base_url: {err}")))?;
        base.join(location)
            .map(|url| url.to_string())
            .map_err(|err| AppError::Http(format!("invalid Location header: {err}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{AppConfig, DigiwebConfig, ImportConfig, MappingConfig, TimeoutConfig};

    #[test]
    fn api_success_response_parses() {
        let response: DigiwebStatusResponse =
            serde_json::from_str(r#"{"id":"abc","status":"SUCCESS","message":"done"}"#)
                .expect("parse");

        assert_eq!(response.id.as_deref(), Some("abc"));
        assert_eq!(response.status, ProcessingStatus::Success);
    }

    #[test]
    fn api_failure_response_parses() {
        let response: DigiwebStatusResponse =
            serde_json::from_str(r#"{"id":"abc","status":"FAIL","message":"bad PLU"}"#)
                .expect("parse");

        assert_eq!(response.status, ProcessingStatus::Fail);
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
                client_secret: String::new(),
                log_credentials_for_testing: false,
                token_url: format!("{base_url}/token"),
                store_number: 1,
                allow_invalid_certificates: false,
                plu_upsert_path: "/api/v1/third-party/plus/write".to_string(),
                request_status_path_template: "/status/{request_id}".to_string(),
                plu_barcode_type: String::new(),
                plu_barcode_ref_no: String::new(),
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
