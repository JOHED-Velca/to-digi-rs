use std::env;
use std::fs;
use std::path::Path;

use secrecy::SecretString;
use serde::Deserialize;

use crate::error::AppError;

const DEFAULT_BASE_URL: &str = "https://192.168.0.150";
const DEFAULT_CLIENT_ID: &str = "digi";
const DEFAULT_TOKEN_PATH: &str = "/auth/realms/skypro/protocol/openid-connect/token";
const DEFAULT_REQUEST_STATUS_PATH_TEMPLATE: &str =
    "/api/thirdpartylinker/api/v1/requests/{request_id}";

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    pub digiweb: DigiwebConfig,
    pub timeouts: TimeoutConfig,
    pub import: ImportConfig,
    pub mapping: MappingConfig,
    pub profiles: ProfileConfig,
    pub verification: VerificationConfig,
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
    pub poll_interval_millis: u64,
    pub poll_timeout_seconds: u64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ImportConfig {
    pub continue_after_record_failure: bool,
    pub send_only_first_plu: bool,
    pub dry_run_inspect_only: bool,
    pub write_payload_preview: bool,
    pub max_in_flight: usize,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct MappingConfig {
    pub main_plu_table: String,
    pub ingredient_table: String,
    pub nutrition_table: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ProfileConfig {
    pub default: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct VerificationConfig {
    pub confirmed_departments: Vec<u32>,
    pub confirmed_groups: Vec<String>,
    pub confirmed_label_formats: Vec<u32>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            digiweb: DigiwebConfig::default(),
            timeouts: TimeoutConfig::default(),
            import: ImportConfig::default(),
            mapping: MappingConfig::default(),
            profiles: ProfileConfig::default(),
            verification: VerificationConfig::default(),
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
            token_url: DEFAULT_TOKEN_PATH.to_string(),
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
            poll_interval_millis: 500,
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
            max_in_flight: 16,
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

impl Default for ProfileConfig {
    fn default() -> Self {
        Self {
            default: String::new(),
        }
    }
}

impl AppConfig {
    pub fn load(path: &Path) -> Result<Self, AppError> {
        let mut config = if !path.exists() {
            Self::default()
        } else {
            let contents =
                fs::read_to_string(path).map_err(|err| AppError::Config(err.to_string()))?;
            toml::from_str(&contents).map_err(|err| AppError::Config(err.to_string()))?
        };
        config.apply_environment_overrides()?;
        Ok(config)
    }

    pub fn validate_startup(&self) -> Result<(), AppError> {
        let base_url = self.digiweb.base_url.trim();
        if base_url.is_empty() || is_placeholder(base_url) {
            return Err(AppError::Config(
                "digiweb.base_url must be set to the customer DIGIweb host or URL".to_string(),
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
        if self.timeouts.poll_interval_millis == 0 {
            return Err(AppError::Config(
                "timeouts.poll_interval_millis must be greater than zero".to_string(),
            ));
        }
        if !(1..=64).contains(&self.import.max_in_flight) {
            return Err(AppError::Config(
                "import.max_in_flight must be in range 1..64".to_string(),
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

    pub fn token_url(&self) -> Result<String, AppError> {
        let raw = self.digiweb.token_url.trim();
        if raw.is_empty() || is_placeholder(raw) {
            return resolve_relative_url(&self.digiweb.base_url, DEFAULT_TOKEN_PATH);
        }
        if raw.starts_with("http://") || raw.starts_with("https://") {
            reqwest::Url::parse(raw)
                .map_err(|err| AppError::Config(format!("invalid digiweb.token_url: {err}")))?;
            Ok(raw.to_string())
        } else {
            required_configured_path("digiweb.token_url", raw)?;
            resolve_relative_url(&self.digiweb.base_url, raw)
        }
    }

    pub fn plu_upsert_path(&self) -> Result<&str, AppError> {
        required_configured_path("digiweb.plu_upsert_path", &self.digiweb.plu_upsert_path)
    }

    pub fn deprecated_command_selector_flags_present(&self) -> bool {
        self.import.send_only_first_plu || self.import.dry_run_inspect_only
    }

    fn apply_environment_overrides(&mut self) -> Result<(), AppError> {
        if let Some(value) = env_nonempty("TO_DIGI_RS_BASE_URL") {
            self.digiweb.base_url = value;
        }
        if let Some(value) = env_nonempty("TO_DIGI_RS_STORE_NUMBER") {
            self.digiweb.store_number = value.parse::<u32>().map_err(|err| {
                AppError::Config(format!("invalid TO_DIGI_RS_STORE_NUMBER: {err}"))
            })?;
        }
        if let Some(value) = env_nonempty("TO_DIGI_RS_ALLOW_INVALID_CERTIFICATES") {
            self.digiweb.allow_invalid_certificates =
                parse_bool_env("TO_DIGI_RS_ALLOW_INVALID_CERTIFICATES", &value)?;
        }
        if let Some(value) = env_nonempty("TO_DIGI_RS_DEFAULT_PROFILE") {
            self.profiles.default = value;
        }
        Ok(())
    }
}

fn env_nonempty(name: &str) -> Option<String> {
    env::var(name).ok().filter(|value| !value.trim().is_empty())
}

fn parse_bool_env(name: &str, value: &str) -> Result<bool, AppError> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Ok(true),
        "0" | "false" | "no" | "off" => Ok(false),
        _ => Err(AppError::Config(format!(
            "{name} must be true/false, yes/no, on/off, or 1/0"
        ))),
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
    resolve_client_secret(
        config,
        env::var("TO_DIGI_RS_CLIENT_SECRET").ok(),
        env::var("DIGIWEB_CLIENT_SECRET").ok(),
        env::var("TO_DIGI_RS_CLIENT_SECRET_FILE").ok(),
    )
}

fn resolve_client_secret(
    config: &AppConfig,
    primary_env_secret: Option<String>,
    legacy_env_secret: Option<String>,
    env_secret_file: Option<String>,
) -> Result<SecretString, AppError> {
    if let Some(value) = primary_env_secret.filter(|value| !value.is_empty()) {
        return Ok(SecretString::new(value));
    }
    if let Some(path) = env_secret_file.filter(|value| !value.trim().is_empty()) {
        let secret = fs::read_to_string(path.trim())
            .map_err(|err| AppError::Config(format!("failed to read client secret file: {err}")))?;
        let secret = secret.trim_end_matches(['\r', '\n']).to_string();
        if !secret.is_empty() {
            return Ok(SecretString::new(secret));
        }
    }
    if let Some(value) = legacy_env_secret.filter(|value| !value.is_empty()) {
        return Ok(SecretString::new(value));
    }
    let configured = config.digiweb.client_secret.trim();
    if !configured.is_empty() && !is_placeholder(configured) {
        return Ok(SecretString::new(configured.to_string()));
    }

    Err(AppError::MissingEnv("TO_DIGI_RS_CLIENT_SECRET"))
}

pub fn client_secret_log_message(config: &AppConfig, env_secret_present: bool) -> &'static str {
    if env_secret_present || env::var("TO_DIGI_RS_CLIENT_SECRET_FILE").is_ok() {
        "loaded from environment (redacted)"
    } else if config.digiweb.client_secret.trim().is_empty() {
        "not configured"
    } else {
        "loaded from config.toml (redacted)"
    }
}

fn resolve_relative_url(base_url: &str, path: &str) -> Result<String, AppError> {
    let base = reqwest::Url::parse(base_url)
        .map_err(|err| AppError::Config(format!("invalid digiweb.base_url: {err}")))?;
    let joined = base
        .join(path.trim_start_matches('/'))
        .map_err(|err| AppError::Config(format!("invalid DIGIweb URL path '{path}': {err}")))?;
    Ok(joined.to_string())
}

fn is_placeholder(value: &str) -> bool {
    let upper = value.to_ascii_uppercase();
    upper.contains("CHANGE_ME") || upper.contains("REPLACE_WITH")
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
    fn default_config_derives_token_endpoint_from_base_url() {
        let config = AppConfig::default();
        assert_eq!(
            config.token_url().expect("token url"),
            "https://192.168.0.150/auth/realms/skypro/protocol/openid-connect/token"
        );
    }

    #[test]
    fn client_secret_can_come_from_config_when_env_is_absent() {
        let mut config = AppConfig::default();
        config.digiweb.client_secret = "hard-coded-test-password".to_string();

        let secret = resolve_client_secret(&config, None, None, None).expect("secret");

        assert_eq!(
            secrecy::ExposeSecret::expose_secret(&secret),
            "hard-coded-test-password"
        );
    }

    #[test]
    fn relative_token_url_resolves_against_base_url() {
        let mut config = AppConfig::default();
        config.digiweb.base_url = "https://192.168.24.122".to_string();
        config.digiweb.token_url = "/auth/realms/skypro/protocol/openid-connect/token".to_string();

        assert_eq!(
            config.token_url().expect("token url"),
            "https://192.168.24.122/auth/realms/skypro/protocol/openid-connect/token"
        );
    }

    #[test]
    fn absolute_token_url_remains_compatible() {
        let mut config = AppConfig::default();
        config.digiweb.token_url = "https://identity.example/token".to_string();

        assert_eq!(
            config.token_url().expect("token url"),
            "https://identity.example/token"
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
    fn verification_confirmations_default_empty() {
        let config = AppConfig::default();

        assert!(config.verification.confirmed_departments.is_empty());
        assert!(config.verification.confirmed_groups.is_empty());
        assert!(config.verification.confirmed_label_formats.is_empty());
    }

    #[test]
    fn bounded_import_defaults_are_visible() {
        let config = AppConfig::default();

        assert_eq!(config.import.max_in_flight, 16);
        assert_eq!(config.timeouts.poll_interval_millis, 500);
    }

    #[test]
    fn max_in_flight_must_stay_in_safe_range() {
        let mut config = AppConfig::default();
        config.import.max_in_flight = 0;
        assert!(config.validate_startup().is_err());

        config.import.max_in_flight = 65;
        assert!(config.validate_startup().is_err());

        config.import.max_in_flight = 1;
        assert!(config.validate_startup().is_ok());

        config.import.max_in_flight = 64;
        assert!(config.validate_startup().is_ok());
    }

    #[test]
    fn poll_interval_millis_must_be_nonzero() {
        let mut config = AppConfig::default();
        config.timeouts.poll_interval_millis = 0;

        assert!(config.validate_startup().is_err());
    }

    #[test]
    fn verification_confirmations_parse_from_toml() {
        let config: AppConfig = toml::from_str(
            r#"
            [verification]
            confirmed_departments = [2]
            confirmed_groups = ["2:997", "2:998"]
            confirmed_label_formats = [1, 2, 3, 4, 6, 8, 21]
            "#,
        )
        .expect("config");

        assert_eq!(config.verification.confirmed_departments, vec![2]);
        assert_eq!(
            config.verification.confirmed_groups,
            vec!["2:997".to_string(), "2:998".to_string()]
        );
        assert_eq!(
            config.verification.confirmed_label_formats,
            vec![1, 2, 3, 4, 6, 8, 21]
        );
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

        let secret = resolve_client_secret(
            &config,
            Some("env-password".to_string()),
            Some("legacy-password".to_string()),
            None,
        )
        .expect("secret");

        assert_eq!(
            secrecy::ExposeSecret::expose_secret(&secret),
            "env-password"
        );
    }
}
