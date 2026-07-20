pub mod mapping;
pub mod mdb_tools;
pub mod schema;

use std::collections::BTreeMap;
use std::fs::File;
use std::path::{Path, PathBuf};

use crate::error::AppError;

pub const FIXED_SOURCE_FILE: &str = "plu.mdb";

#[derive(Debug)]
pub struct VerifiedSourceFile {
    path: PathBuf,
    #[allow(dead_code)]
    handle: File,
}

impl VerifiedSourceFile {
    pub fn verify(path: &Path) -> Result<Self, AppError> {
        if path != Path::new(FIXED_SOURCE_FILE) {
            return Err(AppError::InvalidSourceFile {
                path: path.to_path_buf(),
                message: "application may only open ./plu.mdb".to_string(),
            });
        }
        if path.file_name().and_then(|name| name.to_str()) != Some(FIXED_SOURCE_FILE) {
            return Err(AppError::InvalidSourceFile {
                path: path.to_path_buf(),
                message: "filename must be exactly plu.mdb".to_string(),
            });
        }
        let metadata =
            std::fs::symlink_metadata(path).map_err(|err| AppError::InvalidSourceFile {
                path: path.to_path_buf(),
                message: err.to_string(),
            })?;
        if metadata.file_type().is_symlink() {
            return Err(AppError::InvalidSourceFile {
                path: path.to_path_buf(),
                message: "symbolic links are not allowed".to_string(),
            });
        }
        if !metadata.file_type().is_file() {
            return Err(AppError::InvalidSourceFile {
                path: path.to_path_buf(),
                message: "source path is not a regular file".to_string(),
            });
        }
        let handle = File::open(path).map_err(|err| AppError::InvalidSourceFile {
            path: path.to_path_buf(),
            message: format!("file is not readable: {err}"),
        })?;
        Ok(Self {
            path: path.to_path_buf(),
            handle,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[derive(Debug, Clone)]
pub struct SourceRow {
    pub table: String,
    pub values: BTreeMap<String, String>,
}

impl SourceRow {
    pub fn get(&self, name: &str) -> Option<&str> {
        self.values.get(name).map(String::as_str)
    }
}

#[derive(Debug, Default)]
pub struct SourceDataset {
    pub plu_rows: Vec<SourceRow>,
    pub ingredient_rows: Vec<SourceRow>,
    pub nutrition_rows: Vec<SourceRow>,
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::{Mutex, OnceLock};

    use tempfile::tempdir;

    use super::*;

    static CURRENT_DIR_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

    fn cwd_lock() -> std::sync::MutexGuard<'static, ()> {
        CURRENT_DIR_LOCK
            .get_or_init(|| Mutex::new(()))
            .lock()
            .expect("cwd lock")
    }

    #[test]
    fn accepts_exact_plu_path() {
        let _guard = cwd_lock();
        let dir = tempdir().expect("tempdir");
        let original = std::env::current_dir().expect("cwd");
        std::env::set_current_dir(dir.path()).expect("chdir");
        fs::write(FIXED_SOURCE_FILE, b"not a real mdb").expect("write");

        let result = VerifiedSourceFile::verify(Path::new(FIXED_SOURCE_FILE));

        std::env::set_current_dir(original).expect("restore");
        assert!(result.is_ok());
    }

    #[test]
    fn rejects_alternate_filename() {
        let result = VerifiedSourceFile::verify(Path::new("other.mdb"));
        assert!(result.is_err());
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symbolic_link() {
        use std::os::unix::fs::symlink;

        let _guard = cwd_lock();
        let dir = tempdir().expect("tempdir");
        let original = std::env::current_dir().expect("cwd");
        std::env::set_current_dir(dir.path()).expect("chdir");
        fs::write("actual.mdb", b"not a real mdb").expect("write");
        symlink("actual.mdb", FIXED_SOURCE_FILE).expect("symlink");

        let result = VerifiedSourceFile::verify(Path::new(FIXED_SOURCE_FILE));

        std::env::set_current_dir(original).expect("restore");
        assert!(result.is_err());
    }
}
