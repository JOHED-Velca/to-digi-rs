use std::net::IpAddr;
use std::path::{Path, PathBuf};

use secrecy::{ExposeSecret, SecretString};

use crate::confirm::write_confirmed_config;
use crate::error::AppError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomerConfigUpdateResult {
    pub host: Option<String>,
    pub client_secret_configured: bool,
    pub backup: Option<PathBuf>,
    pub fields_updated: Vec<&'static str>,
}

pub fn apply_customer_config_update(
    path: &Path,
    ip: Option<IpAddr>,
    client_secret: Option<&SecretString>,
) -> Result<CustomerConfigUpdateResult, AppError> {
    let current_text = std::fs::read_to_string(path)
        .map_err(|err| AppError::Config(format!("failed to read config.toml: {err}")))?;
    let (updated_text, fields_updated) =
        apply_customer_config_update_to_text(&current_text, ip, client_secret)?;
    let backup = if updated_text == current_text {
        None
    } else {
        Some(write_confirmed_config(path, &updated_text)?)
    };
    Ok(CustomerConfigUpdateResult {
        host: ip.map(|value| value.to_string()),
        client_secret_configured: client_secret.is_some(),
        backup,
        fields_updated,
    })
}

pub fn apply_customer_config_update_to_text(
    config_text: &str,
    ip: Option<IpAddr>,
    client_secret: Option<&SecretString>,
) -> Result<(String, Vec<&'static str>), AppError> {
    let parsed = config_text
        .parse::<toml::Value>()
        .map_err(|err| AppError::Config(format!("config.toml is invalid: {err}")))?;
    let mut updated = config_text.to_string();
    let mut fields_updated = Vec::new();
    if let Some(ip) = ip {
        let old_base = digiweb_string(&parsed, "base_url")?.unwrap_or("https://CHANGE_ME");
        let old_base_host = parse_url_host(old_base).ok().flatten();
        let new_base = replace_absolute_url_host("digiweb.base_url", old_base, ip)?;
        updated = set_digiweb_string(&updated, "base_url", &new_base);
        fields_updated.push("digiweb.base_url");

        if let Some(token_url) = digiweb_string(&parsed, "token_url")? {
            if is_absolute_url(token_url) {
                if should_update_absolute_endpoint(token_url, old_base_host.as_deref())? {
                    let new_token_url =
                        replace_absolute_url_host("digiweb.token_url", token_url, ip)?;
                    updated = set_digiweb_string(&updated, "token_url", &new_token_url);
                    fields_updated.push("digiweb.token_url");
                }
            }
        }
    }
    if let Some(secret) = client_secret {
        let value = secret.expose_secret();
        if value.trim().is_empty() {
            return Err(AppError::Config(
                "DIGIweb client secret must not be empty".to_string(),
            ));
        }
        updated = set_digiweb_string(&updated, "client_secret", value);
        fields_updated.push("digiweb.client_secret");
    }
    toml::from_str::<crate::config::AppConfig>(&updated)
        .map_err(|err| AppError::Config(format!("updated config.toml is invalid: {err}")))?;
    Ok((updated, fields_updated))
}

fn digiweb_string<'a>(parsed: &'a toml::Value, key: &str) -> Result<Option<&'a str>, AppError> {
    let Some(digiweb) = parsed.get("digiweb") else {
        return Ok(None);
    };
    let Some(table) = digiweb.as_table() else {
        return Err(AppError::Config(
            "[digiweb] must be a TOML table".to_string(),
        ));
    };
    let Some(value) = table.get(key) else {
        return Ok(None);
    };
    value.as_str().map(Some).ok_or_else(|| {
        AppError::Config(format!(
            "digiweb.{key} must be a string before it can be updated"
        ))
    })
}

fn is_absolute_url(value: &str) -> bool {
    value.starts_with("http://") || value.starts_with("https://")
}

fn parse_url_host(value: &str) -> Result<Option<String>, AppError> {
    if let Ok(url) = reqwest::Url::parse(value) {
        return Ok(url.host_str().map(ToOwned::to_owned));
    }
    if placeholder_url_parts(value).is_some() {
        return Ok(None);
    }
    Err(AppError::Config(format!("malformed existing URL: {value}")))
}

fn should_update_absolute_endpoint(
    value: &str,
    old_base_host: Option<&str>,
) -> Result<bool, AppError> {
    if placeholder_url_parts(value).is_some() {
        return Ok(true);
    }
    let url = reqwest::Url::parse(value)
        .map_err(|err| AppError::Config(format!("invalid digiweb.token_url: {err}")))?;
    let Some(host) = url.host_str() else {
        return Err(AppError::Config(
            "digiweb.token_url must contain a host when absolute".to_string(),
        ));
    };
    Ok(
        old_base_host.is_some_and(|base_host| host.eq_ignore_ascii_case(base_host))
            || host.parse::<IpAddr>().is_ok(),
    )
}

fn replace_absolute_url_host(field: &str, value: &str, ip: IpAddr) -> Result<String, AppError> {
    if let Ok(mut url) = reqwest::Url::parse(value) {
        url.set_host(Some(&ip.to_string()))
            .map_err(|_| AppError::Config(format!("{field} host could not be replaced")))?;
        let mut rendered = url.to_string();
        if url.path() == "/"
            && url.query().is_none()
            && url.fragment().is_none()
            && !value.ends_with('/')
        {
            rendered.pop();
        }
        return Ok(rendered);
    }
    if let Some(parts) = placeholder_url_parts(value) {
        return Ok(format!(
            "{}://{}{}{}",
            parts.scheme,
            host_for_url(ip),
            parts.port,
            parts.suffix
        ));
    }
    Err(AppError::Config(format!(
        "{field} is not a valid absolute URL"
    )))
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PlaceholderUrlParts<'a> {
    scheme: &'a str,
    port: String,
    suffix: &'a str,
}

fn placeholder_url_parts(value: &str) -> Option<PlaceholderUrlParts<'_>> {
    let (scheme, rest) = value.split_once("://")?;
    if !matches!(scheme, "http" | "https") {
        return None;
    }
    let authority_end = rest
        .find(|ch| matches!(ch, '/' | '?' | '#'))
        .unwrap_or(rest.len());
    let authority = &rest[..authority_end];
    if !is_placeholder(authority) {
        return None;
    }
    let suffix = &rest[authority_end..];
    let port = authority
        .rsplit_once(':')
        .and_then(|(_, port)| {
            if port.chars().all(|ch| ch.is_ascii_digit()) {
                Some(format!(":{port}"))
            } else {
                None
            }
        })
        .unwrap_or_default();
    Some(PlaceholderUrlParts {
        scheme,
        port,
        suffix,
    })
}

fn is_placeholder(value: &str) -> bool {
    let upper = value.to_ascii_uppercase();
    upper.contains("CHANGE_ME") || upper.contains("REPLACE_WITH")
}

fn host_for_url(ip: IpAddr) -> String {
    match ip {
        IpAddr::V4(value) => value.to_string(),
        IpAddr::V6(value) => format!("[{value}]"),
    }
}

fn set_digiweb_string(config_text: &str, key: &str, value: &str) -> String {
    let rendered = format!("{key} = {}", toml_string(value));
    let lines = config_text.lines().collect::<Vec<_>>();
    let section_start = lines.iter().position(|line| line.trim() == "[digiweb]");
    let mut output = Vec::<String>::new();
    match section_start {
        Some(start) => {
            output.extend(lines[..=start].iter().map(|line| (*line).to_string()));
            let end = lines[start + 1..]
                .iter()
                .position(|line| is_section_header(line))
                .map(|offset| start + 1 + offset)
                .unwrap_or(lines.len());
            let mut replaced = false;
            for line in &lines[start + 1..end] {
                if is_key_line(line, key) {
                    let indent_len = line.len() - line.trim_start().len();
                    output.push(format!("{}{}", &line[..indent_len], rendered));
                    replaced = true;
                } else {
                    output.push((*line).to_string());
                }
            }
            if !replaced {
                output.push(rendered);
            }
            output.extend(lines[end..].iter().map(|line| (*line).to_string()));
        }
        None => {
            output.extend(lines.iter().map(|line| (*line).to_string()));
            if output.last().is_some_and(|line| !line.trim().is_empty()) {
                output.push(String::new());
            }
            output.push("[digiweb]".to_string());
            output.push(rendered);
        }
    }
    let mut text = output.join("\n");
    text.push('\n');
    text
}

fn toml_string(value: &str) -> String {
    toml::Value::String(value.to_string()).to_string()
}

fn is_key_line(line: &str, key: &str) -> bool {
    let trimmed = line.trim_start();
    let Some(rest) = trimmed.strip_prefix(key) else {
        return false;
    };
    rest.trim_start().starts_with('=')
}

fn is_section_header(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.starts_with('[') && trimmed.ends_with(']')
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{AppConfig, load_client_secret};
    use std::fs;
    use tempfile::tempdir;

    fn ip() -> IpAddr {
        "192.168.0.150".parse().expect("ip")
    }

    fn secret(value: &str) -> SecretString {
        SecretString::new(value.to_string())
    }

    #[test]
    fn valid_ipv4_updates_base_host_and_preserves_scheme_port_path_and_query() {
        let text = r#"[digiweb]
base_url = "https://192.168.0.100:8443/app?x=1"
token_url = "/auth/realms/skypro/protocol/openid-connect/token"
client_secret = "keep"
"#;

        let (updated, fields) =
            apply_customer_config_update_to_text(text, Some(ip()), None).expect("update");

        assert!(updated.contains("base_url = \"https://192.168.0.150:8443/app?x=1\""));
        assert!(
            updated.contains("token_url = \"/auth/realms/skypro/protocol/openid-connect/token\"")
        );
        assert_eq!(fields, vec!["digiweb.base_url"]);
    }

    #[test]
    fn absolute_token_url_on_customer_host_is_updated_but_relative_endpoint_stays_relative() {
        let text = r#"[digiweb]
base_url = "https://192.168.0.100"
token_url = "https://192.168.0.100/auth/realms/skypro/protocol/openid-connect/token?client=digi"
plu_upsert_path = "/api/v1/third-party/plus/write"
"#;

        let (updated, fields) =
            apply_customer_config_update_to_text(text, Some(ip()), None).expect("update");

        assert!(updated.contains("base_url = \"https://192.168.0.150\""));
        assert!(updated.contains(
            "token_url = \"https://192.168.0.150/auth/realms/skypro/protocol/openid-connect/token?client=digi\""
        ));
        assert!(updated.contains("plu_upsert_path = \"/api/v1/third-party/plus/write\""));
        assert_eq!(fields, vec!["digiweb.base_url", "digiweb.token_url"]);
    }

    #[test]
    fn unrelated_absolute_token_url_is_preserved() {
        let text = r#"[digiweb]
base_url = "https://192.168.0.100"
token_url = "https://identity.example/token"
store_number = 7
"#;

        let (updated, fields) =
            apply_customer_config_update_to_text(text, Some(ip()), None).expect("update");

        assert!(updated.contains("base_url = \"https://192.168.0.150\""));
        assert!(updated.contains("token_url = \"https://identity.example/token\""));
        assert!(updated.contains("store_number = 7"));
        assert_eq!(fields, vec!["digiweb.base_url"]);
    }

    #[test]
    fn invalid_existing_url_fails_without_returning_modified_text() {
        let text = "[digiweb]\nbase_url = \"not a url\"\nclient_secret = \"keep\"\n";

        let err = apply_customer_config_update_to_text(text, Some(ip()), None).expect_err("fail");

        assert!(err.to_string().contains("digiweb.base_url"));
    }

    #[test]
    fn secret_update_writes_value_and_debug_redacts_it() {
        let text = "[digiweb]\nbase_url = \"https://192.168.0.100\"\n";
        let (updated, fields) =
            apply_customer_config_update_to_text(text, None, Some(&secret("top-secret")))
                .expect("update");
        let config: AppConfig = toml::from_str(&updated).expect("config");

        assert!(updated.contains("client_secret = \"top-secret\""));
        assert_eq!(fields, vec!["digiweb.client_secret"]);
        assert!(!format!("{config:?}").contains("top-secret"));
        assert!(format!("{config:?}").contains("<redacted>"));
    }

    #[test]
    fn combined_update_is_loaded_by_app_config_after_write_and_backup_is_created() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        fs::write(
            &path,
            "[digiweb]\nbase_url = \"https://192.168.0.100\"\nclient_secret = \"old\"\n\n[verification]\nconfirmed_departments = [2]\nconfirmed_groups = [\"2:997\"]\nconfirmed_label_formats = [1]\n\n[profiles]\ndefault = \"bigway\"\n",
        )
        .expect("write");
        let source_path = dir.path().join("plu.mdb");
        fs::write(&source_path, "mdb").expect("source");

        let result = apply_customer_config_update(&path, Some(ip()), Some(&secret("new-secret")))
            .expect("update");
        let config = AppConfig::load(&path).expect("load");
        let source = fs::read_to_string(source_path).expect("source unchanged");

        assert!(result.backup.expect("backup").is_file());
        assert_eq!(config.digiweb.base_url, "https://192.168.0.150");
        assert_eq!(config.verification.confirmed_departments, vec![2]);
        assert_eq!(config.profiles.default, "bigway");
        assert_eq!(
            load_client_secret(&config).expect("secret").expose_secret(),
            "new-secret"
        );
        assert_eq!(source, "mdb");
    }

    #[test]
    fn failed_update_leaves_original_config_intact_without_backup() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        let original = "[digiweb]\nbase_url = \"not a url\"\nclient_secret = \"old\"\n";
        fs::write(&path, original).expect("write");

        let err = apply_customer_config_update(&path, Some(ip()), None).expect_err("fail");
        let backups = fs::read_dir(dir.path())
            .expect("read dir")
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().ends_with(".bak"))
            .count();

        assert!(err.to_string().contains("digiweb.base_url"));
        assert_eq!(fs::read_to_string(&path).expect("config"), original);
        assert_eq!(backups, 0);
    }

    #[test]
    fn empty_secret_is_rejected_before_config_change() {
        let text = "[digiweb]\nbase_url = \"https://192.168.0.100\"\nclient_secret = \"old\"\n";

        let err = apply_customer_config_update_to_text(text, None, Some(&secret("   ")))
            .expect_err("fail");

        assert!(err.to_string().contains("must not be empty"));
    }
}
