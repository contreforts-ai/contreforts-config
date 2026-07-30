//! `contreforts-config` — configuration's own Oxigraph store and datadir,
//! separate from `contreforts-kg`'s knowledge-graph store.
//!
//! Skeleton only (contreforts/contreforts-workspace#58 D2): the failing tests
//! under `tests/` pin the intended shape (its own datadir resolution, its own
//! `ConfigStore::open`, a named error for an unusable path, and isolation
//! between instances) before any of it is implemented. See
//! contreforts/contreforts-workspace#18 for the design this crate exists to
//! satisfy.
//!
//! Nothing here is populated from the generalised configuration layer yet —
//! that is contreforts-workspace#58 D3 onward. This crate deliberately does
//! not depend on `contreforts-kg` or `contreforts-core`, and does not move or
//! copy `config_graph.rs`.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use oxigraph::store::{StorageError, Store};

/// The config store's location under the per-user OS data directory.
///
/// This is **S1** of contreforts-workspace#20's durable-shape checklist: the
/// one place in this crate that spells where the store lives relative to the
/// OS data dir (`<os-data-dir>/{CONFIG_STORE_DIR_NAME}`). Changing this value
/// after any configuration has actually been written under the old name is a
/// **hand-re-entry migration** for every user who already ran with it — the
/// new path resolves to an empty store, and whatever was entered under the
/// old name is not automatically moved.
///
/// Per the repo owner's decision on contreforts-workspace#58 D2, the config
/// store lives *directly* under the OS data dir — there is no intermediate
/// parent directory. Both candidates considered (the legacy `erp-sync`, and
/// `contreforts` as its Erp*-vocabulary-retired replacement) were rejected in
/// favour of no parent segment at all. This is one whole relative path, not
/// a parent segment composed with a separate leaf name.
///
/// This decision also settles where the knowledge-graph store's location is
/// heading: it becomes configuration *held in* this store, rather than a
/// path any crate compiles in, since a future KG reached purely over HTTP
/// has no local datadir at all. Accordingly, `contreforts-config` holds no
/// knowledge of any KG path beyond what `tests/datadir.rs` needs to assert
/// distinctness from it.
const CONFIG_STORE_DIR_NAME: &str = "config_store";

/// Env var used to override the config store's datadir, mirroring
/// `contreforts-core::GraphConfig`'s `GRAPH_STORE_PATH` but with its own name
/// so the two stores' overrides never collide.
const CONFIG_STORE_PATH_ENV: &str = "CONFIG_STORE_PATH";

/// Configuration for `contreforts-config`'s own store (system-independent).
///
/// # Resolution precedence
///
/// Mirrors `contreforts-core::GraphConfig::from_env`'s precedence
/// (`crates/contreforts-core/src/config.rs`), with a distinct env var and
/// leaf directory so the two stores never resolve to the same path:
/// 1. `CONFIG_STORE_PATH` env var — explicit override, empty is treated as
///    unset.
/// 2. The per-user OS data directory —
///    `<data_dir>/config_store`, with no intermediate parent directory (see
///    `CONFIG_STORE_DIR_NAME`'s doc comment).
/// 3. `./config_store` relative to the cwd — last-resort fallback when no OS
///    data directory can be determined.
#[derive(Debug, Clone)]
pub struct ConfigStoreConfig {
    pub config_store_path: String,
}

impl ConfigStoreConfig {
    /// Resolve the store path following the documented precedence
    /// (`CONFIG_STORE_PATH` override → per-user OS data dir → `./config_store`).
    pub fn from_env() -> Self {
        Self {
            config_store_path: resolve_store_path(std::env::var_os(CONFIG_STORE_PATH_ENV)),
        }
    }

    /// The per-user default store directory, ignoring any `CONFIG_STORE_PATH`
    /// override: `<os-data-dir>/config_store` (no intermediate parent
    /// directory), or `./config_store` when no OS data directory can be
    /// determined.
    pub fn per_user_default() -> PathBuf {
        per_user_store_dir().unwrap_or_else(|| PathBuf::from("./config_store"))
    }
}

/// Apply the resolution precedence given the (possibly unset/empty) override.
/// Split out from [`ConfigStoreConfig::from_env`] so it can be unit-tested
/// without mutating the process environment.
fn resolve_store_path(override_var: Option<OsString>) -> String {
    if let Some(val) = override_var
        && !val.is_empty()
    {
        return val.to_string_lossy().into_owned();
    }
    ConfigStoreConfig::per_user_default()
        .to_string_lossy()
        .into_owned()
}

fn per_user_store_dir() -> Option<PathBuf> {
    dirs::data_dir().map(|d| d.join(CONFIG_STORE_DIR_NAME))
}

/// The error a config store can fail to open with.
///
/// An unusable path must produce this named error — never a panic, never a
/// silent fallback to a temp directory or the current directory.
/// Configuration written somewhere nobody looks is the most expensive form of
/// contreforts-workspace#18's recurring defect.
#[derive(thiserror::Error, Debug)]
pub enum ConfigStoreError {
    #[error("cannot open config store at {path}: {source}")]
    Open { path: PathBuf, source: StorageError },
}

/// Configuration's own persistent Oxigraph store, wrapping an `Arc<Store>` so
/// it can be cloned cheaply and shared across the process, independent of
/// `contreforts-kg`'s knowledge-graph store.
#[derive(Clone)]
pub struct ConfigStore {
    store: Arc<Store>,
}

impl ConfigStore {
    /// Open or create a persistent config store at `path`.
    ///
    /// Returns [`ConfigStoreError::Open`] — never panics, never silently
    /// falls back to a temp or current directory — when `path` cannot be
    /// used (for example, a regular file occupies where a directory
    /// component is required).
    pub fn open(path: impl AsRef<Path>) -> Result<Self, ConfigStoreError> {
        let path = path.as_ref();
        let store = Store::open(path).map_err(|source| ConfigStoreError::Open {
            path: path.to_path_buf(),
            source,
        })?;
        Ok(Self {
            store: Arc::new(store),
        })
    }

    /// Borrow the underlying Oxigraph store.
    pub fn inner(&self) -> &Store {
        &self.store
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Env vars are process-global; serialize the tests in this module that
    /// mutate them, same pattern as `contreforts-core::GraphConfig`'s tests.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn resolve_prefers_explicit_override() {
        let got = resolve_store_path(Some(OsString::from("/tmp/custom-config-store")));
        assert_eq!(got, "/tmp/custom-config-store");
    }

    #[test]
    fn resolve_treats_empty_override_as_unset() {
        let got = resolve_store_path(Some(OsString::new()));
        assert_eq!(got, ConfigStoreConfig::per_user_default().to_string_lossy());
    }

    #[test]
    fn resolve_falls_back_to_per_user_default() {
        let got = resolve_store_path(None);
        assert_eq!(got, ConfigStoreConfig::per_user_default().to_string_lossy());
    }

    #[test]
    fn from_env_is_serialized_smoke_test() {
        // Full env-var behaviour (default/override/empty) is pinned by
        // tests/datadir.rs; this just exercises from_env() under the lock
        // without asserting anything datadir.rs already covers twice.
        let _g = ENV_LOCK.lock().unwrap();
        // SAFETY: serialized via ENV_LOCK.
        unsafe { std::env::remove_var(CONFIG_STORE_PATH_ENV) };
        let cfg = ConfigStoreConfig::from_env();
        assert_eq!(
            cfg.config_store_path,
            ConfigStoreConfig::per_user_default().to_string_lossy()
        );
    }
}
