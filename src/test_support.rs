use std::ffi::{OsStr, OsString};
use std::sync::{LazyLock, Mutex, MutexGuard};

static TEST_PROCESS_ENV_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

/// Serializes environment mutations within a test process and restores the
/// original value on drop.
pub(crate) struct EnvVarGuard {
    _lock: MutexGuard<'static, ()>,
    name: &'static str,
    previous_value: Option<OsString>,
}

impl EnvVarGuard {
    pub(crate) fn new(name: &'static str) -> Self {
        let lock = match TEST_PROCESS_ENV_LOCK.lock() {
            Ok(lock) => lock,
            Err(poisoned) => {
                // Test failures should not cascade into unrelated env-var tests.
                let lock = poisoned.into_inner();
                TEST_PROCESS_ENV_LOCK.clear_poison();
                lock
            }
        };
        let previous_value = std::env::var_os(name);
        Self {
            _lock: lock,
            name,
            previous_value,
        }
    }

    pub(crate) fn set(&mut self, value: impl AsRef<OsStr>) {
        unsafe {
            std::env::set_var(self.name, value);
        }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        match self.previous_value.take() {
            Some(value) => unsafe {
                std::env::set_var(self.name, value);
            },
            None => unsafe {
                std::env::remove_var(self.name);
            },
        }
    }
}
