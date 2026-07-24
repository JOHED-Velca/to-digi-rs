use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::error::AppError;
use crate::recovery::model::ImportManifest;
use crate::recovery::validator::validate_manifest;

pub fn load_manifest(path: &Path) -> Result<ImportManifest, AppError> {
    reject_manifest_path(path)?;
    let contents = fs::read_to_string(path).map_err(|err| {
        AppError::Config(format!(
            "failed to read import manifest '{}': {err}",
            path.display()
        ))
    })?;
    let manifest = serde_json::from_str::<ImportManifest>(&contents).map_err(|err| {
        AppError::Config(format!(
            "import-results.json is invalid or internally inconsistent: {err}. The manifest was not modified and no API requests were attempted."
        ))
    })?;
    validate_manifest(&manifest)?;
    Ok(manifest)
}

pub fn atomic_write_manifest(path: &Path, manifest: &ImportManifest) -> Result<(), AppError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| {
            AppError::Logging(format!(
                "failed to create manifest directory '{}': {err}",
                parent.display()
            ))
        })?;
    }
    let json = serde_json::to_string_pretty(manifest)
        .map_err(|err| AppError::Internal(format!("manifest serialization failed: {err}")))?;
    serde_json::from_str::<ImportManifest>(&json)
        .map_err(|err| AppError::Internal(format!("manifest self-validation failed: {err}")))?;

    let temp_path = temp_path_for(path);
    let mut temp_file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&temp_path)
        .map_err(|err| {
            AppError::Logging(format!(
                "failed to create temporary manifest '{}': {err}",
                temp_path.display()
            ))
        })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = temp_file.set_permissions(fs::Permissions::from_mode(0o600));
    }
    temp_file.write_all(json.as_bytes()).map_err(|err| {
        AppError::Logging(format!(
            "failed to write temporary manifest '{}': {err}",
            temp_path.display()
        ))
    })?;
    temp_file.sync_all().map_err(|err| {
        AppError::Logging(format!(
            "failed to flush temporary manifest '{}': {err}",
            temp_path.display()
        ))
    })?;
    drop(temp_file);

    let temp_contents = fs::read_to_string(&temp_path).map_err(|err| {
        AppError::Logging(format!(
            "failed to validate temporary manifest '{}': {err}",
            temp_path.display()
        ))
    })?;
    serde_json::from_str::<ImportManifest>(&temp_contents).map_err(|err| {
        AppError::Logging(format!(
            "temporary manifest '{}' is not valid JSON: {err}",
            temp_path.display()
        ))
    })?;

    if path.exists() {
        let backup_path = backup_path_for(path);
        fs::copy(path, &backup_path).map_err(|err| {
            AppError::Logging(format!(
                "failed to create manifest backup '{}': {err}",
                backup_path.display()
            ))
        })?;
        set_restrictive_permissions(&backup_path)?;
    }
    fs::rename(&temp_path, path).map_err(|err| {
        AppError::Logging(format!(
            "failed to replace manifest '{}': {err}",
            path.display()
        ))
    })?;
    set_restrictive_permissions(path)?;
    if let Some(parent) = path.parent() {
        if let Ok(dir) = File::open(parent) {
            let _ = dir.sync_all();
        }
    }
    Ok(())
}

#[allow(dead_code)]
pub fn snapshot_manifest(source: &Path, destination: &Path) -> Result<(), AppError> {
    reject_manifest_path(source)?;
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent).map_err(|err| {
            AppError::Logging(format!(
                "failed to create snapshot directory '{}': {err}",
                parent.display()
            ))
        })?;
    }
    fs::copy(source, destination).map_err(|err| {
        AppError::Logging(format!(
            "failed to write manifest snapshot '{}': {err}",
            destination.display()
        ))
    })?;
    set_restrictive_permissions(destination)
}

fn reject_manifest_path(path: &Path) -> Result<(), AppError> {
    let metadata = fs::symlink_metadata(path).map_err(|err| {
        AppError::Config(format!(
            "failed to inspect import manifest '{}': {err}",
            path.display()
        ))
    })?;
    if metadata.file_type().is_symlink() {
        return Err(AppError::Config(format!(
            "symbolic-link import manifests are not allowed: {}",
            path.display()
        )));
    }
    if !metadata.is_file() {
        return Err(AppError::Config(format!(
            "import manifest must be a regular file: {}",
            path.display()
        )));
    }
    Ok(())
}

fn temp_path_for(path: &Path) -> PathBuf {
    path.with_extension("json.tmp")
}

fn backup_path_for(path: &Path) -> PathBuf {
    let mut value = path.as_os_str().to_os_string();
    value.push(".bak");
    PathBuf::from(value)
}

fn set_restrictive_permissions(_path: &Path) -> Result<(), AppError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(_path, fs::Permissions::from_mode(0o600)).map_err(|err| {
            AppError::Logging(format!(
                "failed to set manifest permissions '{}': {err}",
                _path.display()
            ))
        })?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::recovery::model::{
        ImportManifest, ManifestOptions, PluManifestRecord, SourceIdentity, TargetIdentity,
    };

    use super::*;

    fn manifest() -> ImportManifest {
        ImportManifest::new(
            SourceIdentity {
                filename: "plu.mdb".to_string(),
                size_bytes: 1,
                sha256: "a".repeat(64),
            },
            TargetIdentity {
                base_url: "https://example".to_string(),
                store_number: 1,
                client_id: "digi".to_string(),
            },
            ManifestOptions {
                limit: Some(1),
                continue_on_error: false,
                test_alias_used: true,
            },
            1,
            vec![PluManifestRecord::new(
                1,
                Some(1),
                Some(997),
                1,
                "b".repeat(64),
            )],
        )
    }

    #[test]
    fn atomic_write_creates_parseable_manifest_and_backup() {
        let temp = tempfile::tempdir().expect("temp");
        let path = temp.path().join("import-results.json");
        let mut manifest = manifest();

        atomic_write_manifest(&path, &manifest).expect("first write");
        manifest.records[0].status = crate::recovery::model::RecordStatus::Success;
        manifest.recalculate_summary();
        atomic_write_manifest(&path, &manifest).expect("second write");

        let loaded = load_manifest(&path).expect("load");
        assert_eq!(loaded.summary.success, 1);
        assert!(temp.path().join("import-results.json.bak").exists());
    }

    #[test]
    fn corrupt_existing_manifest_is_not_repaired_silently() {
        let temp = tempfile::tempdir().expect("temp");
        let path = temp.path().join("import-results.json");
        fs::write(&path, "{not-json").expect("write");

        let err = load_manifest(&path).expect_err("corrupt");

        assert!(err.to_string().contains("invalid"));
        assert_eq!(fs::read_to_string(&path).expect("read"), "{not-json");
    }
}
