use std::time::{Duration, Instant};

use reqwest::StatusCode;
use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;

use crate::config::AppConfig;
use crate::error::AppError;
use crate::logging::AuditLogger;

pub const MAX_AUTHENTICATED_REQUEST_ATTEMPTS: usize = 3;
const TOKEN_REFRESH_SKEW: Duration = Duration::from_secs(30);

pub struct AccessToken {
    token: SecretString,
}

impl AccessToken {
    pub fn bearer_value(&self) -> String {
        format!("Bearer {}", self.token.expose_secret())
    }

    #[cfg(test)]
    pub(crate) fn for_tests(value: &str) -> Self {
        Self {
            token: SecretString::new(value.to_string()),
        }
    }
}

#[derive(Deserialize)]
pub struct TokenResponse {
    pub access_token: String,
    #[allow(dead_code)]
    pub token_type: Option<String>,
    pub expires_in: Option<u64>,
}

pub struct AuthSession {
    current_token: AccessToken,
    expires_at: Option<Instant>,
    refresh_count: u32,
    http: reqwest::Client,
    config: AppConfig,
    client_secret: SecretString,
}

impl AuthSession {
    pub async fn start(
        http: &reqwest::Client,
        config: &AppConfig,
        client_secret: SecretString,
    ) -> Result<Self, AppError> {
        let token = request_token(http, config, &client_secret).await?;
        Ok(Self {
            current_token: token.access_token,
            expires_at: expires_at(token.expires_in),
            refresh_count: 0,
            http: http.clone(),
            config: config.clone(),
            client_secret,
        })
    }

    pub async fn bearer_value_for_request(
        &mut self,
        logger: &mut AuditLogger,
    ) -> Result<String, AppError> {
        if self.token_needs_proactive_refresh() {
            self.refresh(logger, "Access token is near expiration.")
                .await?;
        }
        Ok(self.current_token.bearer_value())
    }

    pub async fn refresh_after_unauthorized(
        &mut self,
        logger: &mut AuditLogger,
        context: &str,
    ) -> Result<(), AppError> {
        logger.line(format!("HTTP 401 received during {context}."))?;
        self.refresh(logger, "Refreshing DIGIweb access token.")
            .await
    }

    pub fn refresh_count(&self) -> u32 {
        self.refresh_count
    }

    async fn refresh(&mut self, logger: &mut AuditLogger, reason: &str) -> Result<(), AppError> {
        logger.line(reason)?;
        let token = request_token(&self.http, &self.config, &self.client_secret).await?;
        self.current_token = token.access_token;
        self.expires_at = expires_at(token.expires_in);
        self.refresh_count = self.refresh_count.saturating_add(1);
        logger.line("Token refresh succeeded.")?;
        logger.kv(
            "Authentication refresh count",
            &self.refresh_count.to_string(),
        )?;
        Ok(())
    }

    fn token_needs_proactive_refresh(&self) -> bool {
        self.expires_at
            .map(|expires_at| Instant::now() + TOKEN_REFRESH_SKEW >= expires_at)
            .unwrap_or(false)
    }
}

pub async fn authenticate(
    http: &reqwest::Client,
    config: &AppConfig,
    client_secret: &SecretString,
) -> Result<AccessToken, AppError> {
    request_token(http, config, client_secret)
        .await
        .map(|token| token.access_token)
}

struct TokenAcquisition {
    access_token: AccessToken,
    expires_in: Option<u64>,
}

async fn request_token(
    http: &reqwest::Client,
    config: &AppConfig,
    client_secret: &SecretString,
) -> Result<TokenAcquisition, AppError> {
    let token_url = config.token_url()?;
    let params = [
        ("grant_type", "client_credentials"),
        ("client_id", config.digiweb.client_id.as_str()),
        ("client_secret", client_secret.expose_secret()),
    ];
    let response = http
        .post(token_url)
        .form(&params)
        .send()
        .await
        .map_err(|err| AppError::Network(err.to_string()))?;
    let status = response.status();
    if status != StatusCode::OK {
        return Err(AppError::Auth(format!("server returned HTTP {status}")));
    }
    let token_response = response
        .json::<TokenResponse>()
        .await
        .map_err(|err| AppError::Auth(format!("invalid token response: {err}")))?;
    if token_response.access_token.is_empty() {
        return Err(AppError::Auth(
            "token response did not include access_token".to_string(),
        ));
    }
    Ok(TokenAcquisition {
        access_token: AccessToken {
            token: SecretString::new(token_response.access_token),
        },
        expires_in: token_response.expires_in,
    })
}

fn expires_at(expires_in: Option<u64>) -> Option<Instant> {
    expires_in.map(|seconds| Instant::now() + Duration::from_secs(seconds))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_response_parses() {
        let response: TokenResponse = serde_json::from_str(
            r#"{"access_token":"abc123","token_type":"Bearer","expires_in":3600}"#,
        )
        .expect("parse token");

        assert_eq!(response.access_token, "abc123");
    }
}
