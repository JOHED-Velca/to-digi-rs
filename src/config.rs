use std::env;
use std::fs;
use std::path::Path;

use secrecy::SecretString;
use serde::Deserialize;

use crate::error::AppError;

const DEFAULT_BASE_URL: &str = "https://192.168.0.150";
const DEFAULT_CLIENT_ID: &str = "digi";

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    pub digiweb: DigiwebConfig,
    pub timeouts: TimeoutConfig,
    pub import: ImportConfig,
    pub mapping: MappingConfig,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct DigiwebConfig {
    pub base_url: String,
    pub client_id: String,
    pub token_url: String,
    pub store_number: u32,
    pub allow_invalid_certificates: bool,
    pub plu_upsert_path: String,
    pub request_status_path_template: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct TimeoutConfig {
    pub request_seconds: u64,
    pub poll_interval_seconds: u64,
    pub poll_timeout_seconds: u64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ImportConfig {
    pub continue_after_record_failure: bool,
    pub send_only_first_plu: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct MappingConfig {
    pub main_plu_table: String,
    pub ingredient_table: String,
    pub nutrition_table: String,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            digiweb: DigiwebConfig::default(),
            timeouts: TimeoutConfig::default(),
            import: ImportConfig::default(),
            mapping: MappingConfig::default(),
        }
    }
}

impl Default for DigiwebConfig {
    fn default() -> Self {
        Self {
            base_url: DEFAULT_BASE_URL.to_string(),
            client_id: DEFAULT_CLIENT_ID.to_string(),
            token_url: String::new(),
            store_number: 1,
            allow_invalid_certificates: false,
            plu_upsert_path: String::new(),
            request_status_path_template: String::new(),
        }
    }
}

impl Default for TimeoutConfig {
    fn default() -> Self {
        Self {
            request_seconds: 30,
            poll_interval_seconds: 2,
            poll_timeout_seconds: 120,
        }
    }
}

impl Default for ImportConfig {
    fn default() -> Self {
        Self {
            continue_after_record_failure: true,
            send_only_first_plu: false,
        }
    }
}

impl Default for MappingConfig {
    fn default() -> Self {
        Self {
            main_plu_table: "Pludata".to_string(),
            ingredient_table: "PluIng".to_string(),
            nutrition_table: "PluNut".to_string(),
        }
    }
}

impl AppConfig {
    pub fn load(path: &Path) -> Result<Self, AppError> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let contents = fs::read_to_string(path).map_err(|err| AppError::Config(err.to_string()))?;
        toml::from_str(&contents).map_err(|err| AppError::Config(err.to_string()))
    }

    pub fn validate_startup(&self) -> Result<(), AppError> {
        if self.digiweb.base_url.trim().is_empty() {
            return Err(AppError::Config(
                "digiweb.base_url must not be empty".to_string(),
            ));
        }
        if self.digiweb.client_id.trim().is_empty() {
            return Err(AppError::Config(
                "digiweb.client_id must not be empty".to_string(),
            ));
        }
        if self.digiweb.store_number == 0 {
            return Err(AppError::Config(
                "digiweb.store_number must be greater than zero".to_string(),
            ));
        }
        if self.timeouts.request_seconds == 0 {
            return Err(AppError::Config(
                "timeouts.request_seconds must be greater than zero".to_string(),
            ));
        }
        if self.timeouts.poll_interval_seconds == 0 || self.timeouts.poll_timeout_seconds == 0 {
            return Err(AppError::Config(
                "poll interval and timeout must be greater than zero".to_string(),
            ));
        }
        Ok(())
    }

    pub fn token_url(&self) -> Result<&str, AppError> {
        required_configured_url("digiweb.token_url", &self.digiweb.token_url)
    }

    pub fn plu_upsert_path(&self) -> Result<&str, AppError> {
        required_configured_path("digiweb.plu_upsert_path", &self.digiweb.plu_upsert_path)
    }
}

pub fn load_client_secret() -> Result<SecretString, AppError> {
    let value = env::var("DIGIWEB_CLIENT_SECRET")
        .map_err(|_| AppError::MissingEnv("DIGIWEB_CLIENT_SECRET"))?;
    if value.is_empty() {
        return Err(AppError::MissingEnv("DIGIWEB_CLIENT_SECRET"));
    }
    Ok(SecretString::new(value))
}

fn required_configured_url<'a>(name: &str, value: &'a str) -> Result<&'a str, AppError> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.contains("REPLACE_WITH") {
        return Err(AppError::Config(format!(
            "{name} must be set to the confirmed DIGIweb endpoint before contacting DIGIweb"
        )));
    }
    Ok(trimmed)
}

fn required_configured_path<'a>(name: &str, value: &'a str) -> Result<&'a str, AppError> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.contains("REPLACE_WITH") {
        return Err(AppError::Config(format!(
            "{name} must be set to the confirmed DIGIweb PLU endpoint before contacting DIGIweb"
        )));
    }
    if !trimmed.starts_with('/') {
        return Err(AppError::Config(format!("{name} must start with '/'")));
    }
    Ok(trimmed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_keeps_token_endpoint_unconfirmed() {
        let config = AppConfig::default();
        assert!(config.token_url().is_err());
    }
}
