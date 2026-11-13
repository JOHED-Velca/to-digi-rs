use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock};

static CURRENT_DIR_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

pub(crate) struct CurrentDirGuard {
    _lock: MutexGuard<'static, ()>,
    original: PathBuf,
}

impl CurrentDirGuard {
    pub(crate) fn enter(path: &Path) -> Self {
        let lock = CURRENT_DIR_LOCK
            .get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let original = std::env::current_dir().expect("current test directory");
        std::env::set_current_dir(path).expect("change test directory");
        Self {
            _lock: lock,
            original,
        }
    }
}

impl Drop for CurrentDirGuard {
    fn drop(&mut self) {
        std::env::set_current_dir(&self.original).expect("restore test directory");
    }
}
