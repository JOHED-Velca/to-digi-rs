use std::env;
use std::fs;
use std::path::Path;

use secrecy::SecretString;
use serde::Deserialize;

use crate::error::AppError;

const DEFAULT_BASE_URL: &str = "https://192.168.0.150";
const DEFAULT_CLIENT_ID: &str = "digi";
const DEFAULT_REQUEST_STATUS_PATH_TEMPLATE: &str =
    "/api/thirdpartylinker/api/v1/requests/{request_id}";

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
    pub client_secret: String,
    pub log_credentials_for_testing: bool,
    pub token_url: String,
    pub store_number: u32,
    pub allow_invalid_certificates: bool,
    pub plu_upsert_path: String,
    pub request_status_path_template: String,
    pub plu_barcode_type: String,
    pub plu_barcode_ref_no: String,
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
    pub dry_run_inspect_only: bool,
    pub write_payload_preview: bool,
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
            client_secret: String::new(),
            log_credentials_for_testing: false,
            token_url: String::new(),
            store_number: 1,
            allow_invalid_certificates: false,
            plu_upsert_path: "/api/v1/third-party/plus/write".to_string(),
            request_status_path_template: DEFAULT_REQUEST_STATUS_PATH_TEMPLATE.to_string(),
            plu_barcode_type: String::new(),
            plu_barcode_ref_no: String::new(),
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
            continue_after_record_failure: false,
            send_only_first_plu: false,
            dry_run_inspect_only: false,
            write_payload_preview: true,
        }
    }
}

impl Default for MappingConfig {
    fn default() -> Self {
        Self {
            main_plu_table: "Pludata".to_string(),
            ingredient_table: "PluIng".to_string(),
            nutrition_table: String::new(),
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
        if self.digiweb.store_number > 999_999 {
            return Err(AppError::Config(
                "digiweb.store_number must be in DIGIweb range 1..999999".to_string(),
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
        validate_optional_numeric_override(
            "digiweb.plu_barcode_type",
            &self.digiweb.plu_barcode_type,
        )?;
        validate_optional_numeric_override(
            "digiweb.plu_barcode_ref_no",
            &self.digiweb.plu_barcode_ref_no,
        )?;
        Ok(())
    }

    pub fn token_url(&self) -> Result<&str, AppError> {
        required_configured_url("digiweb.token_url", &self.digiweb.token_url)
    }

    pub fn plu_upsert_path(&self) -> Result<&str, AppError> {
        required_configured_path("digiweb.plu_upsert_path", &self.digiweb.plu_upsert_path)
    }
}

fn validate_optional_numeric_override(name: &str, value: &str) -> Result<(), AppError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Ok(());
    }
    if trimmed.chars().all(|ch| ch.is_ascii_digit()) {
        Ok(())
    } else {
        Err(AppError::Config(format!(
            "{name} must be empty or contain only digits"
        )))
    }
}

pub fn load_client_secret(config: &AppConfig) -> Result<SecretString, AppError> {
    resolve_client_secret(config, env::var("DIGIWEB_CLIENT_SECRET").ok())
}

fn resolve_client_secret(
    config: &AppConfig,
    env_secret: Option<String>,
) -> Result<SecretString, AppError> {
    if let Some(value) = env_secret.filter(|value| !value.is_empty()) {
        return Ok(SecretString::new(value));
    }
    let configured = config.digiweb.client_secret.trim();
    if !configured.is_empty() && !configured.contains("REPLACE_WITH") {
        return Ok(SecretString::new(configured.to_string()));
    }

    Err(AppError::MissingEnv("DIGIWEB_CLIENT_SECRET"))
}

pub fn client_secret_log_message(config: &AppConfig, env_secret_present: bool) -> &'static str {
    if env_secret_present {
        "loaded from DIGIWEB_CLIENT_SECRET (redacted)"
    } else if config.digiweb.client_secret.trim().is_empty() {
        "not configured"
    } else {
        "loaded from config.toml (redacted)"
    }
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

    #[test]
    fn client_secret_can_come_from_config_when_env_is_absent() {
        let mut config = AppConfig::default();
        config.digiweb.client_secret = "hard-coded-test-password".to_string();

        let secret = resolve_client_secret(&config, None).expect("secret");

        assert_eq!(
            secrecy::ExposeSecret::expose_secret(&secret),
            "hard-coded-test-password"
        );
    }

    #[test]
    fn default_mapping_does_not_require_plunut() {
        let config = AppConfig::default();
        assert_eq!(config.mapping.main_plu_table, "Pludata");
        assert_eq!(config.mapping.ingredient_table, "PluIng");
        assert!(config.mapping.nutrition_table.is_empty());
    }

    #[test]
    fn default_request_status_path_matches_working_vb_contract() {
        let config = AppConfig::default();

        assert_eq!(
            config.digiweb.request_status_path_template,
            "/api/thirdpartylinker/api/v1/requests/{request_id}"
        );
    }

    #[test]
    fn secret_log_message_does_not_include_secret_value() {
        let mut config = AppConfig::default();
        config.digiweb.client_secret = "super-secret-password".to_string();

        let message = client_secret_log_message(&config, false);

        assert!(!message.contains("super-secret-password"));
        assert_eq!(message, "loaded from config.toml (redacted)");
    }

    #[test]
    fn env_secret_takes_precedence_over_config_secret() {
        let mut config = AppConfig::default();
        config.digiweb.client_secret = "config-password".to_string();

        let secret =
            resolve_client_secret(&config, Some("env-password".to_string())).expect("secret");

        assert_eq!(
            secrecy::ExposeSecret::expose_secret(&secret),
            "env-password"
        );
    }
}
