use std::time::{Duration, Instant};

use reqwest::header::{HeaderMap, LOCATION};
use reqwest::{StatusCode, Url};
use serde::Deserialize;
use serde_json::Value;
use tokio::time::sleep;

use crate::config::AppConfig;
use crate::digiweb::auth::AccessToken;
use crate::digiweb::payload::DigiwebPluPayload;
use crate::digiweb::status::ProcessingStatus;
use crate::error::AppError;
use crate::logging::AuditLogger;

#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
pub struct DigiwebStatusResponse {
    pub id: Option<String>,
    pub status: ProcessingStatus,
    pub method: Option<String>,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[allow(dead_code)]
pub struct PluSubmissionResponse {
    pub id: Option<String>,
    pub request_id: Option<String>,
    pub request_id_camel: Option<String>,
    pub status: Option<String>,
    pub state: Option<String>,
    pub message: Option<String>,
    pub status_url: Option<String>,
    pub status_url_camel: Option<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[allow(dead_code)]
pub struct AsyncRequestStatusResponse {
    pub id: Option<String>,
    pub status: Option<String>,
    pub state: Option<String>,
    pub method: Option<String>,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[allow(dead_code)]
pub struct FinalSynchronousResponse {
    pub status: Option<String>,
    pub state: Option<String>,
    pub message: Option<String>,
}

#[derive(Debug, Clone)]
struct CapturedHttpResponse {
    status: StatusCode,
    content_type: Option<String>,
    location: Option<String>,
    request_id_header: Option<String>,
    body: String,
    body_empty: bool,
}

#[derive(Debug, Clone)]
enum SubmissionInterpretation {
    Final {
        request_id: Option<String>,
        status: ProcessingStatus,
        message: Option<String>,
    },
    Async {
        request_id: String,
        status_path: Option<String>,
        message: Option<String>,
    },
    Unknown {
        request_id: Option<String>,
        message: String,
    },
}

#[derive(Clone)]
pub struct DigiwebClient {
    http: reqwest::Client,
    config: AppConfig,
}

impl DigiwebClient {
    pub fn new(config: AppConfig) -> Result<Self, AppError> {
        let mut builder = reqwest::Client::builder()
            .timeout(Duration::from_secs(config.timeouts.request_seconds))
            .redirect(reqwest::redirect::Policy::none());
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

    #[allow(dead_code)]
    pub async fn upsert_plu(
        &self,
        token: &AccessToken,
        payload: &DigiwebPluPayload,
        logger: &mut AuditLogger,
    ) -> Result<(Option<String>, ProcessingStatus, Option<String>), AppError> {
        self.upsert_plu_internal(token, payload, logger, None).await
    }

    pub async fn upsert_plu_with_progress(
        &self,
        token: &AccessToken,
        payload: &DigiwebPluPayload,
        logger: &mut AuditLogger,
        progress_label: &str,
    ) -> Result<(Option<String>, ProcessingStatus, Option<String>), AppError> {
        self.upsert_plu_internal(token, payload, logger, Some(progress_label))
            .await
    }

    async fn upsert_plu_internal(
        &self,
        token: &AccessToken,
        payload: &DigiwebPluPayload,
        logger: &mut AuditLogger,
        progress_label: Option<&str>,
    ) -> Result<(Option<String>, ProcessingStatus, Option<String>), AppError> {
        let url = self.join_base_path(self.config.plu_upsert_path()?)?;
        let response = self
            .http
            .post(&url)
            .header("Authorization", token.bearer_value())
            .json(payload)
            .send()
            .await
            .map_err(|err| AppError::Network(err.to_string()))?;
        let captured = capture_response(
            "PLU submission response",
            "POST",
            &url,
            true,
            true,
            response,
            logger,
        )
        .await?;

        if !captured.status.is_success() {
            return Err(http_error("PLU submission", &captured));
        }

        match interpret_plu_submission(&captured)? {
            SubmissionInterpretation::Final {
                request_id,
                status,
                message,
            } => Ok((request_id, status, message)),
            SubmissionInterpretation::Async {
                request_id,
                status_path,
                message,
            } => {
                if let Some(label) = progress_label {
                    logger.line(format!("{label} Request submitted: {request_id}"))?;
                }
                if self.has_configured_status_path() {
                    return match self
                        .poll_request_status_internal(token, &request_id, logger, progress_label)
                        .await
                    {
                        Ok(final_status) => {
                            Ok((Some(request_id), final_status.status, final_status.message))
                        }
                        Err(err) => Ok(submitted_status_unknown(
                            request_id,
                            format!(
                                "PLU submission returned a request ID, but status polling failed: {err}. The PLU may have been accepted; do not retry blindly."
                            ),
                        )),
                    };
                }
                if let Some(path) = status_path.or_else(|| captured.location.clone()) {
                    return match self
                        .poll_location_internal(token, &path, logger, progress_label)
                        .await
                    {
                        Ok(final_status) => {
                            Ok((Some(request_id), final_status.status, final_status.message))
                        }
                        Err(err) => Ok(submitted_status_unknown(
                            request_id,
                            format!(
                                "PLU submission returned a request ID, but status polling failed: {err}. The PLU may have been accepted; do not retry blindly."
                            ),
                        )),
                    };
                }
                Ok((
                    Some(request_id),
                    ProcessingStatus::SubmittedStatusUnknown,
                    Some(message.unwrap_or_else(|| {
                        "PLU submission returned a request ID, but no status endpoint is configured or returned by the API. The PLU may have been accepted; do not retry blindly."
                            .to_string()
                    })),
                ))
            }
            SubmissionInterpretation::Unknown {
                request_id,
                message,
            } => Ok((
                request_id,
                ProcessingStatus::SubmittedStatusUnknown,
                Some(message),
            )),
        }
    }

    #[allow(dead_code)]
    pub async fn poll_request_status(
        &self,
        token: &AccessToken,
        request_id: &str,
        logger: &mut AuditLogger,
    ) -> Result<DigiwebStatusResponse, AppError> {
        self.poll_request_status_internal(token, request_id, logger, None)
            .await
    }

    async fn poll_request_status_internal(
        &self,
        token: &AccessToken,
        request_id: &str,
        logger: &mut AuditLogger,
        progress_label: Option<&str>,
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
        self.poll_url(token, &url, request_id, logger, progress_label)
            .await
    }

    #[allow(dead_code)]
    pub async fn poll_location(
        &self,
        token: &AccessToken,
        location: &str,
        logger: &mut AuditLogger,
    ) -> Result<DigiwebStatusResponse, AppError> {
        self.poll_location_internal(token, location, logger, None)
            .await
    }

    async fn poll_location_internal(
        &self,
        token: &AccessToken,
        location: &str,
        logger: &mut AuditLogger,
        progress_label: Option<&str>,
    ) -> Result<DigiwebStatusResponse, AppError> {
        let url = self.resolve_location(location)?;
        self.poll_url(token, &url, location, logger, progress_label)
            .await
    }

    async fn poll_url(
        &self,
        token: &AccessToken,
        url: &str,
        status_reference: &str,
        logger: &mut AuditLogger,
        progress_label: Option<&str>,
    ) -> Result<DigiwebStatusResponse, AppError> {
        let deadline =
            Instant::now() + Duration::from_secs(self.config.timeouts.poll_timeout_seconds);
        loop {
            let response = self
                .http
                .get(url)
                .header("Authorization", token.bearer_value())
                .send()
                .await
                .map_err(|err| AppError::Network(err.to_string()))?;
            let captured = capture_response(
                "Asynchronous request-status response",
                "GET",
                url,
                true,
                false,
                response,
                logger,
            )
            .await?;
            if let Some(label) = progress_label {
                logger.line(format!("{label} Polling request {status_reference}"))?;
                logger.line(format!("{label} HTTP {}", format_status(captured.status)))?;
            }
            if !captured.status.is_success() {
                logger.kv(
                    "Detailed non-success status response body",
                    &sanitize_response_body(&captured.body),
                )?;
                return Err(http_error(
                    &format!("status request {status_reference}"),
                    &captured,
                ));
            }
            let status_response = match interpret_status_response(&captured) {
                Ok(status_response) => status_response,
                Err(err) => {
                    logger.kv(
                        "Detailed undecodable status response body",
                        &sanitize_response_body(&captured.body),
                    )?;
                    return Err(err);
                }
            };
            if let Some(label) = progress_label {
                let status_for_log = status_text_for_log(&captured)
                    .unwrap_or_else(|| status_response.status.as_str().to_string());
                logger.line(format!("{label} DIGIweb status: {}", status_for_log))?;
            }
            if status_response.status == ProcessingStatus::Fail {
                logger.kv(
                    "Detailed failed status response body",
                    &sanitize_response_body(&captured.body),
                )?;
            }
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

    fn has_configured_status_path(&self) -> bool {
        !self
            .config
            .digiweb
            .request_status_path_template
            .trim()
            .is_empty()
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

async fn capture_response(
    label: &str,
    method: &str,
    url: &str,
    bearer_auth_attached: bool,
    log_body_detail: bool,
    response: reqwest::Response,
    logger: &mut AuditLogger,
) -> Result<CapturedHttpResponse, AppError> {
    let status = response.status();
    let headers = response.headers().clone();
    let content_type = header_value(&headers, "content-type");
    let location = headers
        .get(LOCATION)
        .and_then(|value| value.to_str().ok())
        .map(ToOwned::to_owned);
    let request_id_header = request_id_header(&headers);
    let body_bytes = response
        .bytes()
        .await
        .map_err(|err| AppError::Http(format!("response body read failed: {err}")))?;
    let body_empty = body_bytes.is_empty();
    let body = String::from_utf8_lossy(&body_bytes).to_string();
    let captured = CapturedHttpResponse {
        status,
        content_type,
        location,
        request_id_header,
        body,
        body_empty,
    };
    log_captured_response(
        label,
        method,
        url,
        bearer_auth_attached,
        log_body_detail,
        &captured,
        logger,
    )?;
    Ok(captured)
}

fn log_captured_response(
    label: &str,
    method: &str,
    url: &str,
    bearer_auth_attached: bool,
    log_body_detail: bool,
    response: &CapturedHttpResponse,
    logger: &mut AuditLogger,
) -> Result<(), AppError> {
    logger.line(format!("{label}:"))?;
    logger.kv("HTTP method", method)?;
    logger.kv("Request URL/path", &sanitized_url_path(url))?;
    logger.kv(
        "Bearer authentication attached",
        if bearer_auth_attached { "yes" } else { "no" },
    )?;
    logger.kv("HTTP status", &format_status(response.status))?;
    logger.kv(
        "Content-Type",
        response.content_type.as_deref().unwrap_or("<none>"),
    )?;
    logger.kv("Location", response.location.as_deref().unwrap_or("<none>"))?;
    logger.kv(
        "Request ID header",
        response.request_id_header.as_deref().unwrap_or("<none>"),
    )?;
    logger.kv(
        "Response body empty",
        if response.body_empty { "yes" } else { "no" },
    )?;
    if log_body_detail {
        logger.kv(
            "Sanitized raw response body",
            &sanitize_response_body(&response.body),
        )?;
    } else {
        logger.kv(
            "Sanitized raw response body",
            "<omitted for routine polling>",
        )?;
    }
    Ok(())
}

fn interpret_plu_submission(
    response: &CapturedHttpResponse,
) -> Result<SubmissionInterpretation, AppError> {
    if response.status == StatusCode::NO_CONTENT {
        return Ok(SubmissionInterpretation::Final {
            request_id: response.request_id_header.clone(),
            status: ProcessingStatus::Success,
            message: None,
        });
    }

    if response.body_empty {
        return if response.status == StatusCode::ACCEPTED {
            if let Some(request_id) = response.request_id_header.clone() {
                Ok(SubmissionInterpretation::Async {
                    request_id,
                    status_path: None,
                    message: None,
                })
            } else {
                Ok(SubmissionInterpretation::Unknown {
                    request_id: None,
                    message: "PLU submission returned 202 Accepted with an empty body and no request ID. The PLU may have been accepted; do not retry blindly.".to_string(),
                })
            }
        } else {
            Ok(SubmissionInterpretation::Final {
                request_id: response.request_id_header.clone(),
                status: ProcessingStatus::Success,
                message: None,
            })
        };
    }

    let value = json_value(response)?;
    let submission = parse_plu_submission(&value);
    let request_id = submission
        .request_id()
        .or_else(|| response.request_id_header.clone());
    let status_path = submission.status_path();
    let message = submission.message.clone();

    if let Some(status) = submission.processing_status() {
        return match status {
            ProcessingStatus::Success | ProcessingStatus::Fail => {
                Ok(SubmissionInterpretation::Final {
                    request_id,
                    status,
                    message,
                })
            }
            ProcessingStatus::Processing => {
                if let Some(request_id) = request_id {
                    Ok(SubmissionInterpretation::Async {
                        request_id,
                        status_path,
                        message,
                    })
                } else {
                    Ok(SubmissionInterpretation::Unknown {
                        request_id: None,
                        message: "PLU submission returned PROCESSING without a request ID. The PLU may have been accepted; do not retry blindly.".to_string(),
                    })
                }
            }
            ProcessingStatus::SubmittedStatusUnknown | ProcessingStatus::UnknownOrTimeout => {
                Ok(SubmissionInterpretation::Unknown {
                    request_id,
                    message: message.unwrap_or_else(|| {
                        "PLU submission returned an unknown final status. The PLU may have been accepted; do not retry blindly.".to_string()
                    }),
                })
            }
        };
    }

    if response.status == StatusCode::ACCEPTED {
        if let Some(request_id) = request_id {
            return Ok(SubmissionInterpretation::Async {
                request_id,
                status_path,
                message,
            });
        }
    }

    if matches!(response.status, StatusCode::OK | StatusCode::CREATED) {
        return Ok(SubmissionInterpretation::Unknown {
            request_id,
            message: "PLU submission returned successful HTTP status with JSON that does not match the expected DIGIweb status shape. The PLU may have been accepted; do not retry blindly.".to_string(),
        });
    }

    Ok(SubmissionInterpretation::Unknown {
        request_id,
        message: "PLU submission response could not be classified. The PLU may have been accepted; do not retry blindly.".to_string(),
    })
}

fn interpret_status_response(
    response: &CapturedHttpResponse,
) -> Result<DigiwebStatusResponse, AppError> {
    if response.status == StatusCode::NO_CONTENT || response.body_empty {
        return Ok(DigiwebStatusResponse {
            id: response.request_id_header.clone(),
            status: ProcessingStatus::Success,
            method: None,
            message: None,
        });
    }

    let value = json_value(response)?;
    let status_response = parse_async_status(&value);
    let status = status_response.processing_status().ok_or_else(|| {
        AppError::Http(
            "status response JSON did not contain a recognizable status field".to_string(),
        )
    })?;
    Ok(DigiwebStatusResponse {
        id: status_response
            .id
            .or_else(|| response.request_id_header.clone()),
        status,
        method: status_response.method,
        message: status_response
            .message
            .as_deref()
            .map(concise_backend_message),
    })
}

impl PluSubmissionResponse {
    fn request_id(&self) -> Option<String> {
        first_text([
            self.id.as_deref(),
            self.request_id.as_deref(),
            self.request_id_camel.as_deref(),
        ])
    }

    fn status_path(&self) -> Option<String> {
        first_text([self.status_url.as_deref(), self.status_url_camel.as_deref()])
    }

    fn processing_status(&self) -> Option<ProcessingStatus> {
        self.status
            .as_deref()
            .or(self.state.as_deref())
            .and_then(parse_processing_status)
    }
}

impl AsyncRequestStatusResponse {
    fn processing_status(&self) -> Option<ProcessingStatus> {
        self.status
            .as_deref()
            .or(self.state.as_deref())
            .and_then(parse_processing_status)
    }
}

fn parse_plu_submission(value: &Value) -> PluSubmissionResponse {
    PluSubmissionResponse {
        id: json_text(value, &["id"]),
        request_id: json_text(value, &["request_id", "request-id", "requestID"]),
        request_id_camel: json_text(value, &["requestId"]),
        status: json_text(value, &["status", "result"]),
        state: json_text(value, &["state", "processStatus", "processingStatus"]),
        message: json_text(value, &["message", "error", "detail", "title"]),
        status_url: json_text(value, &["status_url", "status-url", "statusPath"]),
        status_url_camel: json_text(value, &["statusUrl"]),
    }
}

fn parse_async_status(value: &Value) -> AsyncRequestStatusResponse {
    AsyncRequestStatusResponse {
        id: json_text(value, &["id", "request_id", "requestId"]),
        status: json_text(value, &["status", "result"]),
        state: json_text(value, &["state", "processStatus", "processingStatus"]),
        method: json_text(value, &["method"]),
        message: json_text(value, &["message", "error", "detail", "title"]),
    }
}

fn status_text_for_log(response: &CapturedHttpResponse) -> Option<String> {
    if response.body_empty || is_html_response(response) {
        return None;
    }
    let value = serde_json::from_str::<Value>(&response.body).ok()?;
    let status = parse_async_status(&value)
        .status
        .or_else(|| parse_async_status(&value).state)?;
    Some(status.trim().to_string()).filter(|value| !value.is_empty())
}

fn concise_backend_message(message: &str) -> String {
    let first_line = message.lines().next().unwrap_or(message).trim();
    if let Some((_prefix, detail)) = first_line.split_once("java.lang.Exception:") {
        let detail = detail.trim();
        if !detail.is_empty() {
            return detail.to_string();
        }
    }
    first_line.to_string()
}

fn json_value(response: &CapturedHttpResponse) -> Result<Value, AppError> {
    if is_html_response(response) {
        return Err(AppError::Http(format!(
            "status endpoint returned HTML instead of JSON; likely routing/status-endpoint error; content-type={}; body preview={}",
            response.content_type.as_deref().unwrap_or("<none>"),
            sanitize_response_body(&response.body)
        )));
    }
    serde_json::from_str(&response.body).map_err(|err| {
        AppError::Http(format!(
            "response body was not valid JSON for HTTP {}: {}; captured body was logged before deserialization",
            response.status, err
        ))
    })
}

fn submitted_status_unknown(
    request_id: String,
    message: String,
) -> (Option<String>, ProcessingStatus, Option<String>) {
    (
        Some(request_id),
        ProcessingStatus::SubmittedStatusUnknown,
        Some(message),
    )
}

fn is_html_response(response: &CapturedHttpResponse) -> bool {
    response
        .content_type
        .as_deref()
        .map(|value| value.to_ascii_lowercase().contains("text/html"))
        .unwrap_or(false)
        || response
            .body
            .trim_start()
            .to_ascii_lowercase()
            .starts_with("<!doctype html")
        || response
            .body
            .trim_start()
            .to_ascii_lowercase()
            .starts_with("<html")
}

fn json_text(value: &Value, keys: &[&str]) -> Option<String> {
    for key in keys {
        if let Some(text) = value.get(key).and_then(Value::as_str) {
            let trimmed = text.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
        if let Some(number) = value.get(key).and_then(Value::as_i64) {
            return Some(number.to_string());
        }
        if let Some(number) = value.get(key).and_then(Value::as_u64) {
            return Some(number.to_string());
        }
    }
    None
}

fn first_text<const N: usize>(values: [Option<&str>; N]) -> Option<String> {
    values
        .into_iter()
        .flatten()
        .map(str::trim)
        .find(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn parse_processing_status(value: &str) -> Option<ProcessingStatus> {
    Some(match value.trim().to_ascii_uppercase().as_str() {
        "SUCCESS" | "SUCCEEDED" | "OK" => ProcessingStatus::Success,
        "FAIL" | "FAILED" | "ERROR" => ProcessingStatus::Fail,
        "PROCESSING" | "PENDING" | "RUNNING" | "ACCEPTED" | "TODO" => ProcessingStatus::Processing,
        "SUBMITTED_STATUS_UNKNOWN" => ProcessingStatus::SubmittedStatusUnknown,
        "UNKNOWN_OR_TIMEOUT" => ProcessingStatus::UnknownOrTimeout,
        _ => return None,
    })
}

fn http_error(stage: &str, response: &CapturedHttpResponse) -> AppError {
    let body = if response.body_empty {
        "<empty>".to_string()
    } else {
        sanitize_response_body(&response.body)
    };
    AppError::Http(format!(
        "{stage} returned HTTP {}; content-type={}; body={}",
        response.status,
        response.content_type.as_deref().unwrap_or("<none>"),
        body
    ))
}

fn header_value(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(ToOwned::to_owned)
}

fn request_id_header(headers: &HeaderMap) -> Option<String> {
    [
        "x-request-id",
        "x-requestid",
        "request-id",
        "requestid",
        "x-correlation-id",
        "x-ms-request-id",
    ]
    .iter()
    .find_map(|name| header_value(headers, name))
}

fn format_status(status: StatusCode) -> String {
    match status.canonical_reason() {
        Some(reason) => format!("{} {}", status.as_u16(), reason),
        None => status.as_u16().to_string(),
    }
}

fn sanitized_url_path(url: &str) -> String {
    let Ok(parsed) = Url::parse(url) else {
        return url.to_string();
    };
    let mut path = parsed.path().to_string();
    if parsed.query().is_some() {
        let query = parsed
            .query_pairs()
            .map(|(key, _value)| format!("{key}=<redacted>"))
            .collect::<Vec<_>>()
            .join("&");
        path.push('?');
        path.push_str(&query);
    }
    path
}

fn sanitize_response_body(body: &str) -> String {
    if body.trim().is_empty() {
        return "<empty>".to_string();
    }
    if let Ok(mut value) = serde_json::from_str::<Value>(body) {
        redact_json_value(&mut value);
        return serde_json::to_string(&value).unwrap_or_else(|_| "<unprintable json>".to_string());
    }
    let lower = body.to_ascii_lowercase();
    if lower.contains("authorization:")
        || lower.contains("bearer ")
        || lower.contains("access_token")
        || lower.contains("client_secret")
        || lower.contains("password")
    {
        return "<redacted body contains sensitive text>".to_string();
    }
    let max_logged_body_chars = if lower.contains("<html") || lower.contains("<!doctype html") {
        512
    } else {
        16_384
    };
    if body.chars().count() > max_logged_body_chars {
        let mut truncated = body.chars().take(max_logged_body_chars).collect::<String>();
        truncated.push_str("...<truncated>");
        truncated
    } else {
        body.to_string()
    }
}

fn redact_json_value(value: &mut Value) {
    match value {
        Value::Object(map) => {
            for (key, value) in map {
                if is_sensitive_key(key) {
                    *value = Value::String("<redacted>".to_string());
                } else {
                    redact_json_value(value);
                }
            }
        }
        Value::Array(values) => {
            for value in values {
                redact_json_value(value);
            }
        }
        _ => {}
    }
}

fn is_sensitive_key(key: &str) -> bool {
    let lower = key.to_ascii_lowercase();
    lower.contains("token")
        || lower.contains("secret")
        || lower.contains("password")
        || lower.contains("authorization")
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;
    use crate::config::{
        AppConfig, DigiwebConfig, ImportConfig, MappingConfig, TimeoutConfig, VerificationConfig,
    };
    use crate::models::plu::{Plu, PriceMode};

    #[test]
    fn api_success_response_parses() {
        let response = parse_async_status(
            &serde_json::from_str(r#"{"id":"abc","status":"SUCCESS","message":"done"}"#)
                .expect("json"),
        );

        assert_eq!(response.id.as_deref(), Some("abc"));
        assert_eq!(
            response.processing_status(),
            Some(ProcessingStatus::Success)
        );
    }

    #[test]
    fn api_failure_response_parses() {
        let response = parse_async_status(
            &serde_json::from_str(r#"{"id":"abc","status":"FAIL","message":"bad PLU"}"#)
                .expect("json"),
        );

        assert_eq!(response.processing_status(), Some(ProcessingStatus::Fail));
        assert_eq!(response.message.as_deref(), Some("bad PLU"));
    }

    #[tokio::test]
    async fn successful_empty_204_response_marks_success() {
        let server = TestServer::start(vec![raw_response(204, "No Content", &[], "")]).await;
        let client = DigiwebClient::new(test_config(&server.base_url, "", 1, 5)).expect("client");
        let mut logger = test_logger();

        let (_request_id, status, _message) = client
            .upsert_plu(&test_token(), &test_payload(), &mut logger)
            .await
            .expect("upsert");

        assert_eq!(status, ProcessingStatus::Success);
    }

    #[tokio::test]
    async fn successful_empty_200_response_marks_success() {
        let server = TestServer::start(vec![raw_response(200, "OK", &[], "")]).await;
        let client = DigiwebClient::new(test_config(&server.base_url, "", 1, 5)).expect("client");
        let mut logger = test_logger();

        let (_request_id, status, _message) = client
            .upsert_plu(&test_token(), &test_payload(), &mut logger)
            .await
            .expect("upsert");

        assert_eq!(status, ProcessingStatus::Success);
    }

    #[tokio::test]
    async fn accepted_202_with_json_request_id_uses_configured_status_path() {
        let server = TestServer::start(vec![
            raw_response(
                202,
                "Accepted",
                &[("Content-Type", "application/json")],
                r#"{"id":"abc"}"#,
            ),
            raw_response(
                200,
                "OK",
                &[("Content-Type", "application/json")],
                r#"{"status":"SUCCESS","message":"done"}"#,
            ),
        ])
        .await;
        let client =
            DigiwebClient::new(test_config(&server.base_url, "/status/{request_id}", 1, 5))
                .expect("client");
        let mut logger = test_logger();

        let (request_id, status, message) = client
            .upsert_plu(&test_token(), &test_payload(), &mut logger)
            .await
            .expect("upsert");

        assert_eq!(request_id.as_deref(), Some("abc"));
        assert_eq!(status, ProcessingStatus::Success);
        assert_eq!(message.as_deref(), Some("done"));
    }

    #[tokio::test]
    async fn accepted_202_with_request_id_header_uses_configured_status_path() {
        let server = TestServer::start(vec![
            raw_response(202, "Accepted", &[("X-Request-ID", "hdr-123")], ""),
            raw_response(
                200,
                "OK",
                &[("Content-Type", "application/json")],
                r#"{"status":"SUCCESS"}"#,
            ),
        ])
        .await;
        let client =
            DigiwebClient::new(test_config(&server.base_url, "/status/{request_id}", 1, 5))
                .expect("client");
        let mut logger = test_logger();

        let (request_id, status, _message) = client
            .upsert_plu(&test_token(), &test_payload(), &mut logger)
            .await
            .expect("upsert");

        assert_eq!(request_id.as_deref(), Some("hdr-123"));
        assert_eq!(status, ProcessingStatus::Success);
    }

    #[tokio::test]
    async fn old_vb_successful_response_shape_todo_is_polled() {
        let server = TestServer::start(vec![
            raw_response(
                200,
                "OK",
                &[("Content-Type", "application/json")],
                r#"{"id":"019f85f5","status":"TODO","path":"write","method":"WRITE","type":"Plu"}"#,
            ),
            raw_response(
                200,
                "OK",
                &[("Content-Type", "application/json")],
                r#"{"id":"019f85f5","status":"SUCCESS","message":null}"#,
            ),
        ])
        .await;
        let client = DigiwebClient::new(test_config(
            &server.base_url,
            "/api/thirdpartylinker/api/v1/requests/{request_id}",
            1,
            5,
        ))
        .expect("client");
        let mut logger = test_logger();

        let (request_id, status, _message) = client
            .upsert_plu(&test_token(), &test_payload(), &mut logger)
            .await
            .expect("upsert");

        assert_eq!(request_id.as_deref(), Some("019f85f5"));
        assert_eq!(status, ProcessingStatus::Success);
        assert_eq!(server.handled_requests().await, 2);
    }

    #[tokio::test]
    async fn created_201_todo_prefers_body_id_and_configured_vb_status_path_over_location() {
        let server = TestServer::start(vec![
            raw_response(
                201,
                "Created",
                &[
                    ("Content-Type", "application/json"),
                    ("Location", "/v1/requests/019f8655"),
                ],
                r#"{"id":"019f8655","status":"TODO","type":"Plu","method":"WRITE"}"#,
            ),
            raw_response(
                200,
                "OK",
                &[("Content-Type", "application/json")],
                r#"{"id":"019f8655","status":"SUCCESS","method":"WRITE"}"#,
            ),
        ])
        .await;
        let client = DigiwebClient::new(test_config(
            &server.base_url,
            "/api/thirdpartylinker/api/v1/requests/{request_id}",
            1,
            5,
        ))
        .expect("client");
        let mut logger = test_logger();

        let (request_id, status, _message) = client
            .upsert_plu(&test_token(), &test_payload(), &mut logger)
            .await
            .expect("upsert");

        let request_lines = server.request_lines().await;
        assert_eq!(request_id.as_deref(), Some("019f8655"));
        assert_eq!(status, ProcessingStatus::Success);
        assert_eq!(request_lines.len(), 2);
        assert!(request_lines[0].starts_with("POST /api/v1/third-party/plus/write "));
        assert!(
            request_lines[1].starts_with("GET /api/thirdpartylinker/api/v1/requests/019f8655 ")
        );
    }

    #[tokio::test]
    async fn post_and_status_get_receive_same_bearer_token() {
        let server = TestServer::start(vec![
            raw_response(
                201,
                "Created",
                &[("Content-Type", "application/json")],
                r#"{"id":"auth-check","status":"TODO"}"#,
            ),
            raw_response(
                200,
                "OK",
                &[("Content-Type", "application/json")],
                r#"{"id":"auth-check","status":"SUCCESS"}"#,
            ),
        ])
        .await;
        let client =
            DigiwebClient::new(test_config(&server.base_url, "/status/{request_id}", 1, 5))
                .expect("client");
        let mut logger = test_logger();

        let (_request_id, status, _message) = client
            .upsert_plu(
                &AccessToken::for_tests("same-access-token"),
                &test_payload(),
                &mut logger,
            )
            .await
            .expect("upsert");

        let requests = server.request_texts().await;
        assert_eq!(status, ProcessingStatus::Success);
        assert_eq!(requests.len(), 2);
        assert!(request_has_authorization(
            &requests[0],
            "Bearer same-access-token"
        ));
        assert!(request_has_authorization(
            &requests[1],
            "Bearer same-access-token"
        ));
    }

    #[tokio::test]
    async fn html_from_incorrect_status_route_is_unknown_not_plu_failure() {
        let html = format!(
            "<!doctype html><html><body><app-root></app-root>{}</body></html>",
            "x".repeat(2_000)
        );
        let server = TestServer::start(vec![
            raw_response(
                201,
                "Created",
                &[
                    ("Content-Type", "application/json"),
                    ("Location", "/v1/requests/019f8655"),
                ],
                r#"{"id":"019f8655","status":"TODO","type":"Plu","method":"WRITE"}"#,
            ),
            raw_response(200, "OK", &[("Content-Type", "text/html")], &html),
        ])
        .await;
        let client = DigiwebClient::new(test_config(&server.base_url, "", 1, 5)).expect("client");
        let mut logger = test_logger();

        let (request_id, status, message) = client
            .upsert_plu(&test_token(), &test_payload(), &mut logger)
            .await
            .expect("upsert");

        assert_eq!(request_id.as_deref(), Some("019f8655"));
        assert_eq!(status, ProcessingStatus::SubmittedStatusUnknown);
        assert_ne!(status, ProcessingStatus::Fail);
        assert_eq!(server.handled_requests().await, 2);
        let message = message.unwrap_or_default();
        assert!(message.contains("routing/status-endpoint error"));
        assert!(message.contains("may have been accepted"));
    }

    #[tokio::test]
    async fn unauthorized_status_polling_is_unknown_not_confirmed_plu_failure() {
        let server = TestServer::start(vec![
            raw_response(
                201,
                "Created",
                &[("Content-Type", "application/json")],
                r#"{"id":"abc","status":"TODO"}"#,
            ),
            raw_response(401, "Unauthorized", &[], ""),
        ])
        .await;
        let client =
            DigiwebClient::new(test_config(&server.base_url, "/status/{request_id}", 1, 5))
                .expect("client");
        let mut logger = test_logger();

        let (request_id, status, message) = client
            .upsert_plu(&test_token(), &test_payload(), &mut logger)
            .await
            .expect("upsert");

        assert_eq!(request_id.as_deref(), Some("abc"));
        assert_eq!(status, ProcessingStatus::SubmittedStatusUnknown);
        assert_ne!(status, ProcessingStatus::Fail);
        let message = message.unwrap_or_default();
        assert!(message.contains("HTTP 401 Unauthorized"));
        assert!(message.contains("may have been accepted"));
    }

    #[tokio::test]
    async fn tokens_are_absent_from_response_logs() {
        let server = TestServer::start(vec![raw_response(
            200,
            "OK",
            &[("Content-Type", "application/json")],
            r#"{"status":"SUCCESS"}"#,
        )])
        .await;
        let client = DigiwebClient::new(test_config(&server.base_url, "", 1, 5)).expect("client");
        let (_dir, path, mut logger) = test_logger_with_path();

        let (_request_id, status, _message) = client
            .upsert_plu(
                &AccessToken::for_tests("very-secret-access-token"),
                &test_payload(),
                &mut logger,
            )
            .await
            .expect("upsert");
        logger.flush().expect("flush");
        let contents = std::fs::read_to_string(path).expect("read log");

        assert_eq!(status, ProcessingStatus::Success);
        assert!(contents.contains("Bearer authentication attached: yes"));
        assert!(!contents.contains("very-secret-access-token"));
        assert!(!contents.contains("Authorization"));
        assert!(!contents.contains("Bearer very-secret"));
    }

    #[tokio::test]
    async fn polling_waits_for_todo_and_processing_until_success() {
        let server = TestServer::start(vec![
            raw_response(
                201,
                "Created",
                &[("Content-Type", "application/json")],
                r#"{"id":"abc","status":"TODO"}"#,
            ),
            raw_response(
                200,
                "OK",
                &[("Content-Type", "application/json")],
                r#"{"id":"abc","status":"TODO"}"#,
            ),
            raw_response(
                200,
                "OK",
                &[("Content-Type", "application/json")],
                r#"{"id":"abc","status":"PROCESSING"}"#,
            ),
            raw_response(
                200,
                "OK",
                &[("Content-Type", "application/json")],
                r#"{"id":"abc","status":"SUCCESS"}"#,
            ),
        ])
        .await;
        let client =
            DigiwebClient::new(test_config(&server.base_url, "/status/{request_id}", 1, 5))
                .expect("client");
        let mut logger = test_logger();

        let (_request_id, status, _message) = client
            .upsert_plu(&test_token(), &test_payload(), &mut logger)
            .await
            .expect("upsert");

        assert_eq!(status, ProcessingStatus::Success);
        assert_eq!(server.handled_requests().await, 4);
    }

    #[tokio::test]
    async fn successful_polling_logs_are_concise_without_full_bodies() {
        let server = TestServer::start(vec![
            raw_response(
                201,
                "Created",
                &[("Content-Type", "application/json")],
                r#"{"id":"abc","status":"TODO"}"#,
            ),
            raw_response(
                200,
                "OK",
                &[("Content-Type", "application/json")],
                r#"{"id":"abc","status":"TODO","record":{"large":"payload-body-should-not-repeat"}}"#,
            ),
            raw_response(
                200,
                "OK",
                &[("Content-Type", "application/json")],
                r#"{"id":"abc","status":"PROCESSING","record":{"large":"payload-body-should-not-repeat"}}"#,
            ),
            raw_response(
                200,
                "OK",
                &[("Content-Type", "application/json")],
                r#"{"id":"abc","status":"SUCCESS","record":{"large":"payload-body-should-not-repeat"}}"#,
            ),
        ])
        .await;
        let client =
            DigiwebClient::new(test_config(&server.base_url, "/status/{request_id}", 1, 5))
                .expect("client");
        let (_dir, path, mut logger) = test_logger_with_path();

        let (_request_id, status, _message) = client
            .upsert_plu_with_progress(&test_token(), &test_payload(), &mut logger, "[1/1]")
            .await
            .expect("upsert");
        logger.flush().expect("flush");
        let contents = std::fs::read_to_string(path).expect("read log");

        assert_eq!(status, ProcessingStatus::Success);
        assert!(contents.contains("[1/1] Request submitted: abc"));
        assert!(contents.contains("[1/1] HTTP 200 OK"));
        assert!(contents.contains("[1/1] DIGIweb status: TODO"));
        assert!(contents.contains("[1/1] DIGIweb status: PROCESSING"));
        assert!(contents.contains("[1/1] DIGIweb status: SUCCESS"));
        assert!(contents.contains("Sanitized raw response body: <omitted for routine polling>"));
        assert!(!contents.contains("payload-body-should-not-repeat"));
    }

    #[tokio::test]
    async fn json_response_with_unexpected_shape_is_unknown_not_failure() {
        let server = TestServer::start(vec![raw_response(
            200,
            "OK",
            &[("Content-Type", "application/json")],
            r#"{"unexpected":true}"#,
        )])
        .await;
        let client = DigiwebClient::new(test_config(&server.base_url, "", 1, 5)).expect("client");
        let mut logger = test_logger();

        let (_request_id, status, message) = client
            .upsert_plu(&test_token(), &test_payload(), &mut logger)
            .await
            .expect("upsert");

        assert_eq!(status, ProcessingStatus::SubmittedStatusUnknown);
        assert_ne!(status, ProcessingStatus::Fail);
        assert!(
            message
                .as_deref()
                .unwrap_or_default()
                .contains("may have been accepted")
        );
    }

    #[tokio::test]
    async fn plain_text_error_response_reports_status_and_body() {
        let server = TestServer::start(vec![raw_response(
            400,
            "Bad Request",
            &[("Content-Type", "text/plain")],
            "bad PLU",
        )])
        .await;
        let client = DigiwebClient::new(test_config(&server.base_url, "", 1, 5)).expect("client");
        let mut logger = test_logger();

        let err = client
            .upsert_plu(&test_token(), &test_payload(), &mut logger)
            .await
            .expect_err("error");

        assert!(err.to_string().contains("HTTP 400 Bad Request"));
        assert!(err.to_string().contains("bad PLU"));
    }

    #[tokio::test]
    async fn html_error_response_reports_status_and_body() {
        let server = TestServer::start(vec![raw_response(
            500,
            "Internal Server Error",
            &[("Content-Type", "text/html")],
            "<html>server error</html>",
        )])
        .await;
        let client = DigiwebClient::new(test_config(&server.base_url, "", 1, 5)).expect("client");
        let mut logger = test_logger();

        let err = client
            .upsert_plu(&test_token(), &test_payload(), &mut logger)
            .await
            .expect_err("error");

        assert!(err.to_string().contains("HTTP 500 Internal Server Error"));
        assert!(err.to_string().contains("<html>server error</html>"));
    }

    #[tokio::test]
    async fn empty_status_path_configuration_returns_submitted_status_unknown() {
        let server = TestServer::start(vec![raw_response(
            202,
            "Accepted",
            &[("Content-Type", "application/json")],
            r#"{"id":"abc"}"#,
        )])
        .await;
        let client = DigiwebClient::new(test_config(&server.base_url, "", 1, 5)).expect("client");
        let mut logger = test_logger();

        let (request_id, status, message) = client
            .upsert_plu(&test_token(), &test_payload(), &mut logger)
            .await
            .expect("upsert");

        assert_eq!(request_id.as_deref(), Some("abc"));
        assert_eq!(status, ProcessingStatus::SubmittedStatusUnknown);
        assert!(
            message
                .as_deref()
                .unwrap_or_default()
                .contains("may have been accepted")
        );
    }

    #[tokio::test]
    async fn response_body_is_captured_once_then_deserialized_from_capture() {
        let server = TestServer::start(vec![raw_response(
            200,
            "OK",
            &[("Content-Type", "application/json")],
            r#"{"status":"SUCCESS"}"#,
        )])
        .await;
        let client = DigiwebClient::new(test_config(&server.base_url, "", 1, 5)).expect("client");
        let mut logger = test_logger();

        let (_request_id, status, _message) = client
            .upsert_plu(&test_token(), &test_payload(), &mut logger)
            .await
            .expect("upsert");

        assert_eq!(status, ProcessingStatus::Success);
        assert_eq!(server.handled_requests().await, 1);
    }

    #[tokio::test]
    async fn unknown_final_status_is_not_reported_as_confirmed_failure() {
        let server = TestServer::start(vec![raw_response(
            202,
            "Accepted",
            &[("Content-Type", "application/json")],
            r#"{"id":"abc"}"#,
        )])
        .await;
        let client = DigiwebClient::new(test_config(&server.base_url, "", 1, 5)).expect("client");
        let mut logger = test_logger();

        let (_request_id, status, _message) = client
            .upsert_plu(&test_token(), &test_payload(), &mut logger)
            .await
            .expect("upsert");

        assert_eq!(status, ProcessingStatus::SubmittedStatusUnknown);
        assert_ne!(status, ProcessingStatus::Fail);
    }

    #[tokio::test]
    async fn status_polling_success() {
        let server = TestServer::start(vec![raw_response(
            200,
            "OK",
            &[("Content-Type", "application/json")],
            r#"{"status":"SUCCESS","message":"done"}"#,
        )])
        .await;
        let client =
            DigiwebClient::new(test_config(&server.base_url, "/status/{request_id}", 1, 5))
                .expect("client");
        let mut logger = test_logger();

        let response = client
            .poll_request_status(&test_token(), "abc", &mut logger)
            .await
            .expect("poll");

        assert_eq!(response.status, ProcessingStatus::Success);
        assert_eq!(response.message.as_deref(), Some("done"));
    }

    #[tokio::test]
    async fn status_polling_failure() {
        let server = TestServer::start(vec![raw_response(
            200,
            "OK",
            &[("Content-Type", "application/json")],
            r#"{"status":"FAIL","message":"bad"}"#,
        )])
        .await;
        let client =
            DigiwebClient::new(test_config(&server.base_url, "/status/{request_id}", 1, 5))
                .expect("client");
        let mut logger = test_logger();

        let response = client
            .poll_request_status(&test_token(), "abc", &mut logger)
            .await
            .expect("poll");

        assert_eq!(response.status, ProcessingStatus::Fail);
        assert_eq!(response.message.as_deref(), Some("bad"));
    }

    #[tokio::test]
    async fn status_polling_timeout() {
        let server = TestServer::start(vec![raw_response(
            200,
            "OK",
            &[("Content-Type", "application/json")],
            r#"{"status":"PROCESSING","message":"wait"}"#,
        )])
        .await;
        let client =
            DigiwebClient::new(test_config(&server.base_url, "/status/{request_id}", 1, 0))
                .expect("client");
        let mut logger = test_logger();

        let response = client
            .poll_request_status(&test_token(), "abc", &mut logger)
            .await
            .expect("poll");

        assert_eq!(response.status, ProcessingStatus::UnknownOrTimeout);
    }

    fn test_config(
        base_url: &str,
        status_template: &str,
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
                request_status_path_template: status_template.to_string(),
                plu_barcode_type: String::new(),
                plu_barcode_ref_no: String::new(),
            },
            timeouts: TimeoutConfig {
                request_seconds: 5,
                poll_interval_seconds,
                poll_timeout_seconds,
            },
            verification: VerificationConfig::default(),
            import: ImportConfig::default(),
            mapping: MappingConfig::default(),
        }
    }

    fn test_payload() -> DigiwebPluPayload {
        let config = DigiwebConfig::default();
        DigiwebPluPayload::from_plu(
            &Plu {
                plu_number: 1,
                store_number: 1,
                department_number: Some(1),
                group_number: Some(997),
                source_department: Some("0001".to_string()),
                source_group: Some("997".to_string()),
                group_default_applied: false,
                name: "Apples".to_string(),
                barcode: Some("0200001".to_string()),
                barcode_type: Some("5".to_string()),
                barcode_ref_no: Some("5".to_string()),
                source_barcode: Some("1".to_string()),
                source_barcode_format: Some("05".to_string()),
                source_flag_data: Some("02".to_string()),
                price: rust_decimal::Decimal::new(199, 2),
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
            },
            &config,
        )
        .expect("payload")
    }

    fn test_token() -> AccessToken {
        AccessToken::for_tests("token")
    }

    fn test_logger() -> AuditLogger {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("logs.txt");
        AuditLogger::create(&path).expect("logger")
    }

    fn test_logger_with_path() -> (tempfile::TempDir, std::path::PathBuf, AuditLogger) {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("logs.txt");
        let logger = AuditLogger::create(&path).expect("logger");
        (dir, path, logger)
    }

    #[test]
    fn html_response_body_logging_is_bounded() {
        let body = format!(
            "<html><body><app-root></app-root>{}</body></html>",
            "x".repeat(2_000)
        );

        let sanitized = sanitize_response_body(&body);

        assert!(sanitized.contains("<app-root>"));
        assert!(sanitized.contains("<truncated>"));
        assert!(sanitized.chars().count() < 600);
    }

    #[test]
    fn backend_stack_trace_message_is_summarized_for_record_result() {
        let message = "Error processing shop. Shop nr: 5001: java.lang.Exception: barcodetype_uuid is null\n\tat eu.digi.skyproject.registry.service.PluConsumerService";

        assert_eq!(concise_backend_message(message), "barcodetype_uuid is null");
    }

    fn raw_response(
        status_code: u16,
        reason: &str,
        headers: &[(&str, &str)],
        body: &str,
    ) -> String {
        let mut response = format!("HTTP/1.1 {status_code} {reason}\r\n");
        let has_content_length = headers
            .iter()
            .any(|(name, _)| name.eq_ignore_ascii_case("content-length"));
        for (name, value) in headers {
            response.push_str(&format!("{name}: {value}\r\n"));
        }
        if !has_content_length {
            response.push_str(&format!("Content-Length: {}\r\n", body.len()));
        }
        response.push_str("Connection: close\r\n\r\n");
        response.push_str(body);
        response
    }

    struct TestServer {
        base_url: String,
        handled_requests: std::sync::Arc<tokio::sync::Mutex<usize>>,
        request_lines: std::sync::Arc<tokio::sync::Mutex<Vec<String>>>,
        request_texts: std::sync::Arc<tokio::sync::Mutex<Vec<String>>>,
    }

    impl TestServer {
        async fn start(responses: Vec<String>) -> Self {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            use tokio::net::TcpListener;

            let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
            let addr = listener.local_addr().expect("addr");
            let handled_requests = std::sync::Arc::new(tokio::sync::Mutex::new(0));
            let request_lines = std::sync::Arc::new(tokio::sync::Mutex::new(Vec::new()));
            let request_texts = std::sync::Arc::new(tokio::sync::Mutex::new(Vec::new()));
            let handled_requests_task = handled_requests.clone();
            let request_lines_task = request_lines.clone();
            let request_texts_task = request_texts.clone();
            tokio::spawn(async move {
                for response in responses {
                    let Ok((mut stream, _peer)) = listener.accept().await else {
                        return;
                    };
                    let mut buffer = [0_u8; 4096];
                    let bytes_read = stream.read(&mut buffer).await.unwrap_or_default();
                    *handled_requests_task.lock().await += 1;
                    let request_text = String::from_utf8_lossy(&buffer[..bytes_read]);
                    let request_line = request_text.lines().next().unwrap_or_default().to_string();
                    request_lines_task.lock().await.push(request_line);
                    request_texts_task
                        .lock()
                        .await
                        .push(request_text.to_string());
                    let _ = stream.write_all(response.as_bytes()).await;
                }
            });
            Self {
                base_url: format!("http://{addr}"),
                handled_requests,
                request_lines,
                request_texts,
            }
        }

        async fn handled_requests(&self) -> usize {
            *self.handled_requests.lock().await
        }

        async fn request_lines(&self) -> Vec<String> {
            self.request_lines.lock().await.clone()
        }

        async fn request_texts(&self) -> Vec<String> {
            self.request_texts.lock().await.clone()
        }
    }

    fn request_has_authorization(request: &str, expected_value: &str) -> bool {
        request.lines().any(|line| {
            let Some((name, value)) = line.split_once(':') else {
                return false;
            };
            name.eq_ignore_ascii_case("authorization") && value.trim() == expected_value
        })
    }
}
