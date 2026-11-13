use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};

use fs2::FileExt;

use crate::error::AppError;

pub struct ManifestLock {
    file: File,
    #[allow(dead_code)]
    path: PathBuf,
}

impl ManifestLock {
    pub fn acquire(path: &Path) -> Result<Self, AppError> {
        let lock_path = lock_path_for(path);
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)
            .map_err(|err| {
                AppError::Config(format!(
                    "failed to open import manifest lock '{}': {err}",
                    lock_path.display()
                ))
            })?;
        file.try_lock_exclusive().map_err(|err| {
            AppError::Config(format!(
                "This import manifest is already in use by another process. Only one import or resume process may update a manifest at a time. Details: {err}"
            ))
        })?;
        Ok(Self {
            file,
            path: lock_path,
        })
    }

    #[allow(dead_code)]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

fn lock_path_for(path: &Path) -> PathBuf {
    let mut value = path.as_os_str().to_os_string();
    value.push(".lock");
    PathBuf::from(value)
}

impl Drop for ManifestLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    #[test]
    fn second_manifest_lock_fails_clearly() {
        let temp = tempfile::tempdir().expect("temp");
        let path = temp.path().join("import-results.json");
        fs::write(&path, "{}").expect("write");
        let first = ManifestLock::acquire(&path).expect("first lock");

        let second = ManifestLock::acquire(&path);

        assert!(second.is_err());
        drop(first);
        assert!(ManifestLock::acquire(&path).is_ok());
    }

    const HOLDER_ENV: &str = "TO_DIGI_RS_TEST_LOCK_HOLDER_MANIFEST";

    fn wait_for(path: &Path) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while !path.exists() {
            assert!(
                std::time::Instant::now() < deadline,
                "timed out waiting for {}",
                path.display()
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    /// Child half of `manifest_lock_held_by_another_process_fails_clearly`; a no-op in normal runs.
    #[test]
    fn lock_holder_child_process() {
        let Ok(manifest) = std::env::var(HOLDER_ENV) else {
            return;
        };
        let manifest = PathBuf::from(manifest);
        let _lock = ManifestLock::acquire(&manifest).expect("child lock");
        fs::write(manifest.with_extension("ready"), "").expect("ready marker");
        wait_for(&manifest.with_extension("release"));
    }

    #[test]
    fn manifest_lock_held_by_another_process_fails_clearly() {
        let temp = tempfile::tempdir().expect("temp");
        let path = temp.path().join("import-results.json");
        fs::write(&path, r#"{"original":true}"#).expect("write");
        let mut child = std::process::Command::new(std::env::current_exe().expect("test binary"))
            .args([
                "--exact",
                "recovery::lock::tests::lock_holder_child_process",
            ])
            .env(HOLDER_ENV, &path)
            .stdout(std::process::Stdio::null())
            .spawn()
            .expect("spawn lock holder");
        wait_for(&path.with_extension("ready"));

        let result = ManifestLock::acquire(&path);

        fs::write(path.with_extension("release"), "").expect("release marker");
        assert!(child.wait().expect("child exit").success());
        let message = result.err().expect("lock must be refused").to_string();
        assert!(
            message.contains("already in use by another process"),
            "{message}"
        );
        assert_eq!(
            fs::read_to_string(&path).expect("manifest"),
            r#"{"original":true}"#
        );
        assert!(ManifestLock::acquire(&path).is_ok());
    }
}
