pub mod lock;
pub mod model;
pub mod planner;
pub mod store;
pub mod validator;

use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;

use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::error::AppError;

pub use lock::ManifestLock;
pub use model::SourceIdentity;
pub use planner::build_resume_plan;
pub use store::{atomic_write_manifest, load_manifest};
pub use validator::validate_resume_compatibility;

pub const DEFAULT_MANIFEST_PATH: &str = "import-results.json";
pub const MANIFEST_SCHEMA_VERSION: u32 = 1;

pub fn sha256_file(path: &Path) -> Result<String, AppError> {
    let file = File::open(path).map_err(|err| AppError::InvalidSourceFile {
        path: path.to_path_buf(),
        message: err.to_string(),
    })?;
    let mut reader = BufReader::new(file);
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = reader
            .read(&mut buffer)
            .map_err(|err| AppError::InvalidSourceFile {
                path: path.to_path_buf(),
                message: err.to_string(),
            })?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(hex::encode(hasher.finalize()))
}

pub fn sha256_json<T: Serialize>(value: &T) -> Result<String, AppError> {
    let bytes = serde_json::to_vec(value)
        .map_err(|err| AppError::Internal(format!("canonical JSON serialization failed: {err}")))?;
    Ok(hex::encode(Sha256::digest(&bytes)))
}

pub fn sanitize_manifest_error(value: impl AsRef<str>) -> String {
    let value = value.as_ref();
    let sanitized = value
        .replace("Authorization", "<redacted-header>")
        .replace("authorization", "<redacted-header>");
    let mut pieces = Vec::new();
    let mut redact_next = false;
    for piece in sanitized.split_whitespace() {
        if redact_next {
            pieces.push("<redacted-token>");
            redact_next = false;
            continue;
        }
        pieces.push(piece);
        if piece.eq_ignore_ascii_case("Bearer") {
            redact_next = true;
        }
    }
    let mut sanitized = pieces.join(" ");
    for marker in ["access_token", "client_secret", "password", "secret"] {
        sanitized = sanitized.replace(marker, "<redacted>");
    }
    const MAX: usize = 500;
    if sanitized.chars().count() > MAX {
        let mut truncated = sanitized.chars().take(MAX).collect::<String>();
        truncated.push_str("...<truncated>");
        truncated
    } else {
        sanitized
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_error_sanitizer_removes_secret_markers() {
        let sanitized = sanitize_manifest_error(
            "Authorization: Bearer abc access_token client_secret password secret",
        );

        assert!(!sanitized.contains("abc"));
        assert!(!sanitized.contains("Authorization"));
        assert!(!sanitized.contains("access_token"));
        assert!(!sanitized.contains("client_secret"));
    }
}
