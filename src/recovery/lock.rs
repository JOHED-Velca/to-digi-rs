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
}
