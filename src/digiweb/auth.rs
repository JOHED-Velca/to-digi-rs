use reqwest::StatusCode;
use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;

use crate::config::AppConfig;
use crate::error::AppError;

pub struct AccessToken {
    token: SecretString,
}

impl AccessToken {
    pub fn bearer_value(&self) -> String {
        format!("Bearer {}", self.token.expose_secret())
    }
}

#[derive(Debug, Deserialize)]
pub struct TokenResponse {
    pub access_token: String,
    #[allow(dead_code)]
    pub token_type: Option<String>,
    #[allow(dead_code)]
    pub expires_in: Option<u64>,
}

pub async fn authenticate(
    http: &reqwest::Client,
    config: &AppConfig,
    client_secret: &SecretString,
) -> Result<AccessToken, AppError> {
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
    Ok(AccessToken {
        token: SecretString::new(token_response.access_token),
    })
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
