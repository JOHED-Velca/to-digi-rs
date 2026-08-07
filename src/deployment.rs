use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use crate::cli::ProfileSelection;
use crate::config::AppConfig;
use crate::error::AppError;
use crate::logging::AuditLogger;
use crate::sanitization::load_profile_from_safe_path;
use crate::source::{FIXED_SOURCE_FILE, VerifiedSourceFile};

const IMAGE_REPOSITORY: &str = match option_env!("TO_DIGI_RS_IMAGE_REPOSITORY") {
    Some(value) => value,
    None => "ghcr.io/johed-velca/to-digi-rs",
};
const IMAGE_DIGEST: Option<&str> = option_env!("TO_DIGI_RS_IMAGE_DIGEST");

const TO_DIGI_TEMPLATE: &str = include_str!("../deploy/to-digi");
const IMPORT_SH_TEMPLATE: &str = include_str!("../deploy/import.sh");
const RUN_SH_TEMPLATE: &str = include_str!("../deploy/run.sh");
const COMPOSE_TEMPLATE: &str = include_str!("../deploy/compose.yaml");
const CONFIG_EXAMPLE_TEMPLATE: &str = include_str!("../deploy/config.example.toml");
const PROFILE_EXAMPLE_TEMPLATE: &str = include_str!("../profiles/example.toml");
const PROFILE_STARSKY_TEMPLATE: &str = include_str!("../profiles/starsky.toml");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AssetKind {
    Generated,
    CustomerConfig,
}

struct Asset {
    path: &'static str,
    contents: &'static str,
    mode: u32,
    kind: AssetKind,
}

pub fn default_image_reference() -> String {
    match IMAGE_DIGEST {
        Some(digest) if !digest.trim().is_empty() => {
            format!("{IMAGE_REPOSITORY}:{}@{digest}", env!("CARGO_PKG_VERSION"))
        }
        _ => format!("{IMAGE_REPOSITORY}:{}", env!("CARGO_PKG_VERSION")),
    }
}

pub fn run_init(refresh_generated_files: bool, logger: &mut AuditLogger) -> Result<i32, AppError> {
    println!("Initializing to-digi-rs deployment...");
    println!("Deployment directory: {}", current_dir_display()?);
    logger.line("DEPLOYMENT INITIALIZATION")?;
    logger.kv("Deployment directory", &current_dir_display()?)?;
    logger.kv("Default image", &default_image_reference())?;

    let assets = deployment_assets();
    for asset in assets {
        install_asset(&asset, refresh_generated_files, logger)?;
    }
    ensure_directory(Path::new("output"), 0o755)?;

    println!("Created or verified deployment files.");
    println!("Primary launcher: ./to-digi");
    println!("Next steps: edit config.toml, place plu.mdb here, then run ./to-digi doctor");
    logger.line("Initialization complete.")?;
    logger.flush()?;
    Ok(0)
}

pub fn run_doctor(
    config: &AppConfig,
    pull: bool,
    inside_container: bool,
    profile: Option<&ProfileSelection>,
    logger: &mut AuditLogger,
) -> Result<i32, AppError> {
    println!("Running to-digi-rs doctor...");
    println!("Application version: {}", env!("CARGO_PKG_VERSION"));
    println!("Selected image: {}", selected_image_from_env());
    println!("Outputs will be written to logs.txt");
    logger.line("DEPLOYMENT DOCTOR")?;
    logger.kv("Application version", env!("CARGO_PKG_VERSION"))?;
    logger.kv("Selected image", &selected_image_from_env())?;
    logger.kv(
        "Inside container",
        if inside_container { "yes" } else { "no" },
    )?;
    logger.kv("Docker pull requested", if pull { "yes" } else { "no" })?;

    let mut failures = Vec::new();
    check_writable_dir(Path::new("."), "Deployment directory", &mut failures);
    check_file_readable(Path::new("config.toml"), "config.toml", &mut failures);
    match VerifiedSourceFile::verify(Path::new(FIXED_SOURCE_FILE)) {
        Ok(_) => record_check("plu.mdb", true),
        Err(err) => {
            record_check("plu.mdb", false);
            failures.push(err.to_string());
        }
    }
    ensure_directory(Path::new("output"), 0o755)?;
    check_writable_dir(Path::new("output"), "output directory", &mut failures);

    if let Err(err) = config.validate_startup() {
        failures.push(err.to_string());
    } else {
        record_check("Configuration parses", true);
        println!("Target: {}", config.digiweb.base_url);
        println!("Store number: {}", config.digiweb.store_number);
        println!(
            "TLS certificate validation: {}",
            if config.digiweb.allow_invalid_certificates {
                "disabled"
            } else {
                "enabled"
            }
        );
        logger.kv("Target base URL", &config.digiweb.base_url)?;
        logger.kv("Store number", &config.digiweb.store_number.to_string())?;
        logger.kv(
            "TLS certificate validation disabled",
            if config.digiweb.allow_invalid_certificates {
                "yes"
            } else {
                "no"
            },
        )?;
        match config.token_url() {
            Ok(url) => logger.kv("Resolved token URL", &sanitize_url_for_display(&url))?,
            Err(err) => failures.push(err.to_string()),
        }
    }

    let profile_label = match profile {
        Some(selection) => selection.display(),
        None => "<none>".to_string(),
    };
    println!("Selected profile: {profile_label}");
    logger.kv("Selected profile", &profile_label)?;
    if let Some(selection) = profile {
        if let Err(err) = check_profile(selection) {
            failures.push(err.to_string());
        } else {
            record_check("Profile available", true);
        }
    }

    if !inside_container {
        check_command_available("docker", &mut failures);
        check_command_status(["info"], "Docker daemon reachable", &mut failures);
        let image = selected_image_from_env();
        if pull {
            println!("Pull requested: {image}");
            check_command_status(["pull", &image], "Docker image pull", &mut failures);
        } else {
            check_command_status(
                ["image", "inspect", &image],
                "Docker image available",
                &mut failures,
            );
        }
    }

    for failure in &failures {
        logger.error(failure)?;
    }
    logger.flush()?;
    if failures.is_empty() {
        println!("Result: PASS");
        Ok(0)
    } else {
        println!("Result: FAIL");
        for failure in &failures {
            println!("Reason: {failure}");
        }
        Ok(2)
    }
}

fn deployment_assets() -> Vec<Asset> {
    vec![
        Asset {
            path: "to-digi",
            contents: TO_DIGI_TEMPLATE,
            mode: 0o755,
            kind: AssetKind::Generated,
        },
        Asset {
            path: "import.sh",
            contents: IMPORT_SH_TEMPLATE,
            mode: 0o755,
            kind: AssetKind::Generated,
        },
        Asset {
            path: "run.sh",
            contents: RUN_SH_TEMPLATE,
            mode: 0o755,
            kind: AssetKind::Generated,
        },
        Asset {
            path: "compose.yaml",
            contents: COMPOSE_TEMPLATE,
            mode: 0o644,
            kind: AssetKind::Generated,
        },
        Asset {
            path: "config.example.toml",
            contents: CONFIG_EXAMPLE_TEMPLATE,
            mode: 0o644,
            kind: AssetKind::Generated,
        },
        Asset {
            path: "config.toml",
            contents: CONFIG_EXAMPLE_TEMPLATE,
            mode: 0o600,
            kind: AssetKind::CustomerConfig,
        },
        Asset {
            path: "profiles/example.toml",
            contents: PROFILE_EXAMPLE_TEMPLATE,
            mode: 0o644,
            kind: AssetKind::Generated,
        },
        Asset {
            path: "profiles/starsky.toml",
            contents: PROFILE_STARSKY_TEMPLATE,
            mode: 0o644,
            kind: AssetKind::Generated,
        },
    ]
}

fn install_asset(
    asset: &Asset,
    refresh_generated_files: bool,
    logger: &mut AuditLogger,
) -> Result<(), AppError> {
    let path = Path::new(asset.path);
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            ensure_directory(parent, 0o755)?;
        }
    }
    if path.exists() {
        if fs::symlink_metadata(path)
            .map(|metadata| metadata.file_type().is_symlink())
            .unwrap_or(false)
        {
            println!("Preserved existing symbolic link: {}", asset.path);
            logger.kv("Init preserved symbolic link", asset.path)?;
            return Ok(());
        }
        match asset.kind {
            AssetKind::Generated if refresh_generated_files => {
                if file_contents_match(path, asset.contents)? {
                    println!("Unchanged: {}", asset.path);
                    logger.kv("Init preserved unchanged generated file", asset.path)?;
                } else {
                    backup_existing(path)?;
                    write_asset(path, asset.contents, asset.mode)?;
                    println!("Refreshed: {}", asset.path);
                    logger.kv("Init refreshed generated file", asset.path)?;
                }
            }
            _ => {
                println!("Preserved existing: {}", asset.path);
                logger.kv("Init preserved existing file", asset.path)?;
                set_mode(path, asset.mode)?;
            }
        }
    } else {
        write_asset(path, asset.contents, asset.mode)?;
        println!("Created: {}", asset.path);
        logger.kv("Init created file", asset.path)?;
    }
    Ok(())
}

fn file_contents_match(path: &Path, contents: &str) -> Result<bool, AppError> {
    let existing = fs::read_to_string(path)
        .map_err(|err| AppError::Config(format!("failed to read '{}': {err}", path.display())))?;
    Ok(existing == contents)
}

fn write_asset(path: &Path, contents: &str, mode: u32) -> Result<(), AppError> {
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)
        .map_err(|err| AppError::Config(format!("failed to create '{}': {err}", path.display())))?;
    file.write_all(contents.as_bytes())
        .map_err(|err| AppError::Config(format!("failed to write '{}': {err}", path.display())))?;
    file.sync_all()
        .map_err(|err| AppError::Config(format!("failed to flush '{}': {err}", path.display())))?;
    set_mode(path, mode)
}

fn backup_existing(path: &Path) -> Result<(), AppError> {
    let backup = timestamped_backup_path(path)?;
    fs::copy(path, &backup).map_err(|err| {
        AppError::Config(format!(
            "failed to create backup '{}' for '{}': {err}",
            backup.display(),
            path.display()
        ))
    })?;
    fs::remove_file(path)
        .map_err(|err| AppError::Config(format!("failed to replace '{}': {err}", path.display())))
}

fn timestamped_backup_path(path: &Path) -> Result<PathBuf, AppError> {
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
    let filename = path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| AppError::Config(format!("invalid generated path '{}'", path.display())))?;
    Ok(path.with_file_name(format!("{filename}.{stamp}.bak")))
}

fn ensure_directory(path: &Path, mode: u32) -> Result<(), AppError> {
    fs::create_dir_all(path)
        .map_err(|err| AppError::Config(format!("failed to create '{}': {err}", path.display())))?;
    set_mode(path, mode)
}

fn set_mode(path: &Path, mode: u32) -> Result<(), AppError> {
    #[cfg(unix)]
    {
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).map_err(|err| {
            AppError::Config(format!(
                "failed to set permissions on '{}': {err}",
                path.display()
            ))
        })?;
    }
    #[cfg(not(unix))]
    {
        let _ = (path, mode);
    }
    Ok(())
}

fn current_dir_display() -> Result<String, AppError> {
    std::env::current_dir()
        .map(|path| path.display().to_string())
        .map_err(|err| AppError::Config(format!("failed to resolve current directory: {err}")))
}

fn selected_image_from_env() -> String {
    std::env::var("TO_DIGI_RS_IMAGE")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(default_image_reference)
}

fn check_writable_dir(path: &Path, label: &str, failures: &mut Vec<String>) {
    if path.is_dir() {
        let probe = path.join(".to-digi-rs-write-test");
        match fs::write(&probe, b"ok").and_then(|_| fs::remove_file(&probe)) {
            Ok(_) => record_check(label, true),
            Err(err) => {
                record_check(label, false);
                failures.push(format!("{label} is not writable: {err}"));
            }
        }
    } else {
        record_check(label, false);
        failures.push(format!("{label} is missing or is not a directory"));
    }
}

fn check_file_readable(path: &Path, label: &str, failures: &mut Vec<String>) {
    match fs::File::open(path) {
        Ok(_) => record_check(label, true),
        Err(err) => {
            record_check(label, false);
            failures.push(format!("{label} is not readable: {err}"));
        }
    }
}

fn check_profile(selection: &ProfileSelection) -> Result<(), AppError> {
    match selection {
        ProfileSelection::BuiltIn(name) if name == "starsky" => Ok(()),
        ProfileSelection::BuiltIn(name) => Err(AppError::Config(format!(
            "unknown built-in sanitization profile '{name}'"
        ))),
        ProfileSelection::External(path) => load_profile_from_safe_path(path).map(|_| ()),
    }
}

fn check_command_available(command: &str, failures: &mut Vec<String>) {
    let found = Command::new(command).arg("--version").output().is_ok();
    record_check(&format!("{command} CLI available"), found);
    if !found {
        failures.push(format!("{command} CLI is not available"));
    }
}

fn check_command_status<const N: usize>(args: [&str; N], label: &str, failures: &mut Vec<String>) {
    let status = Command::new("docker").args(args).status();
    match status {
        Ok(status) if status.success() => record_check(label, true),
        Ok(status) => {
            record_check(label, false);
            failures.push(format!("{label} failed with exit code {:?}", status.code()));
        }
        Err(err) => {
            record_check(label, false);
            failures.push(format!("{label} could not run: {err}"));
        }
    }
}

fn record_check(label: &str, ok: bool) {
    println!("[{}] {label}", if ok { "OK" } else { "FAIL" });
}

fn sanitize_url_for_display(url: &str) -> String {
    match reqwest::Url::parse(url) {
        Ok(mut parsed) => {
            if parsed.password().is_some() {
                let _ = parsed.set_password(Some("<redacted>"));
            }
            if !parsed.username().is_empty() {
                let _ = parsed.set_username("<redacted>");
            }
            parsed.to_string()
        }
        Err(_) => "<invalid url>".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn default_image_uses_current_version() {
        assert_eq!(
            default_image_reference(),
            "ghcr.io/johed-velca/to-digi-rs:0.9.0"
        );
    }

    #[test]
    fn embedded_starsky_profile_matches_external_profile() {
        assert_eq!(
            PROFILE_STARSKY_TEMPLATE,
            include_str!("../profiles/starsky.toml")
        );
    }

    #[test]
    fn init_creates_expected_files_and_preserves_customer_files() {
        let dir = tempdir().expect("tempdir");
        let previous = std::env::current_dir().expect("cwd");
        std::env::set_current_dir(dir.path()).expect("cd");
        fs::write("plu.mdb", b"customer").expect("plu");

        let mut logger = AuditLogger::create(Path::new("logs.txt")).expect("logger");
        run_init(false, &mut logger).expect("init");

        assert!(Path::new("to-digi").is_file());
        assert!(Path::new("import.sh").is_file());
        assert!(Path::new("compose.yaml").is_file());
        assert!(Path::new("config.example.toml").is_file());
        assert!(Path::new("config.toml").is_file());
        assert!(Path::new("profiles/example.toml").is_file());
        assert!(Path::new("profiles/starsky.toml").is_file());
        assert!(Path::new("output").is_dir());
        assert_eq!(fs::read("plu.mdb").expect("plu"), b"customer");

        let config = fs::read_to_string("config.toml").expect("config");
        fs::write("config.toml", "customer-config").expect("custom");
        run_init(false, &mut logger).expect("repeat");
        assert_eq!(
            fs::read_to_string("config.toml").expect("config"),
            "customer-config"
        );
        assert!(config.contains("CHANGE_ME"));

        std::env::set_current_dir(previous).expect("restore");
    }

    #[test]
    fn generated_shell_assets_use_lf_line_endings() {
        for contents in [TO_DIGI_TEMPLATE, IMPORT_SH_TEMPLATE, RUN_SH_TEMPLATE] {
            assert!(!contents.contains("\r\n"));
        }
    }
}
