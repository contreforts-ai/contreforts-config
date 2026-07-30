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
/// distinctness from it. (The HTTP-reachable KG itself is out of scope here —
/// tracked as its own epic in contreforts/contreforts-kg#33.)
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
/// (`crates/contreforts-core/src/config.rs`) for the first two steps, with a
/// distinct env var and leaf directory so the two stores never resolve to the
/// same path — but **deliberately diverges on the third step**:
/// 1. `CONFIG_STORE_PATH` env var — explicit override, empty is treated as
///    unset.
/// 2. The per-user OS data directory —
///    `<data_dir>/config_store`, with no intermediate parent directory (see
///    `CONFIG_STORE_DIR_NAME`'s doc comment).
/// 3. **No further fallback.** If no OS data directory can be determined,
///    resolution fails with [`ConfigStoreError::NoDataDir`] rather than
///    silently writing configuration into the current working directory.
///
/// `contreforts-core::GraphConfig` keeps its own `./graph_store`
/// current-directory fallback (`crates/contreforts-core/src/config.rs:49`) —
/// that is intentional and is not a discrepancy to "fix" by making the two
/// consistent. The knowledge-graph store's contents are re-derived from
/// connectors on every sync, so a CWD-dependent location that is occasionally
/// wrong costs a re-sync; the config store's contents are entered by hand and
/// are not re-derivable, so the same silent fallback would risk configuration
/// written somewhere nobody looks — the most expensive form of
/// contreforts-workspace#18's recurring defect. The knowledge-graph store's
/// own CWD-fallback question, and whether it should keep it, is tracked
/// separately in contreforts/contreforts-kg#33.
#[derive(Debug, Clone)]
pub struct ConfigStoreConfig {
    pub config_store_path: String,
}

impl ConfigStoreConfig {
    /// Resolve the store path following the documented precedence
    /// (`CONFIG_STORE_PATH` override → per-user OS data dir → named error).
    ///
    /// Returns [`ConfigStoreError::NoDataDir`] when `CONFIG_STORE_PATH` is
    /// unset (or empty) and no per-user OS data directory can be determined —
    /// never a silent fallback to the current working directory.
    pub fn from_env() -> Result<Self, ConfigStoreError> {
        Ok(Self {
            config_store_path: resolve_store_path(std::env::var_os(CONFIG_STORE_PATH_ENV))?,
        })
    }

    /// The per-user default store directory, ignoring any `CONFIG_STORE_PATH`
    /// override: `<os-data-dir>/config_store` (no intermediate parent
    /// directory).
    ///
    /// Returns [`ConfigStoreError::NoDataDir`] when no OS data directory can
    /// be determined — see this struct's doc comment for why that is an
    /// error here rather than a current-directory fallback.
    pub fn per_user_default() -> Result<PathBuf, ConfigStoreError> {
        per_user_default_from(dirs::data_dir())
    }
}

/// Apply the resolution precedence given the (possibly unset/empty) override.
/// Split out from [`ConfigStoreConfig::from_env`] so it can be unit-tested
/// without mutating the process environment.
fn resolve_store_path(override_var: Option<OsString>) -> Result<String, ConfigStoreError> {
    if let Some(val) = override_var
        && !val.is_empty()
    {
        return Ok(val.to_string_lossy().into_owned());
    }
    Ok(ConfigStoreConfig::per_user_default()?
        .to_string_lossy()
        .into_owned())
}

/// Apply the OS-data-dir resolution given an injected (possibly absent) data
/// directory. Split out from [`ConfigStoreConfig::per_user_default`] so the
/// no-data-dir error path can be unit-tested without touching real process
/// environment (`HOME` / `XDG_DATA_HOME`), mirroring why [`resolve_store_path`]
/// takes its override as a parameter.
fn per_user_default_from(data_dir: Option<PathBuf>) -> Result<PathBuf, ConfigStoreError> {
    data_dir
        .map(|d| d.join(CONFIG_STORE_DIR_NAME))
        .ok_or(ConfigStoreError::NoDataDir)
}

/// The error a config store can fail to open, or a datadir fail to resolve, with.
///
/// An unusable path must produce this named error — never a panic, never a
/// silent fallback to a temp directory or the current directory.
/// Configuration written somewhere nobody looks is the most expensive form of
/// contreforts-workspace#18's recurring defect.
#[derive(thiserror::Error, Debug)]
pub enum ConfigStoreError {
    #[error("cannot open config store at {path}: {source}")]
    Open { path: PathBuf, source: StorageError },

    #[error(
        "cannot determine a per-user OS data directory to place the config store in; \
         set CONFIG_STORE_PATH explicitly to an absolute path"
    )]
    NoDataDir,
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
        let got = resolve_store_path(Some(OsString::from("/tmp/custom-config-store")))
            .expect("an explicit override never touches data-dir resolution");
        assert_eq!(got, "/tmp/custom-config-store");
    }

    #[test]
    fn resolve_treats_empty_override_as_unset() {
        let got = resolve_store_path(Some(OsString::new()))
            .expect("data dir resolves in this test environment");
        assert_eq!(
            got,
            ConfigStoreConfig::per_user_default()
                .expect("data dir resolves in this test environment")
                .to_string_lossy()
        );
    }

    #[test]
    fn resolve_falls_back_to_per_user_default() {
        let got = resolve_store_path(None).expect("data dir resolves in this test environment");
        assert_eq!(
            got,
            ConfigStoreConfig::per_user_default()
                .expect("data dir resolves in this test environment")
                .to_string_lossy()
        );
    }

    #[test]
    fn from_env_is_serialized_smoke_test() {
        // Full env-var behaviour (default/override/empty) is pinned by
        // tests/datadir.rs; this just exercises from_env() under the lock
        // without asserting anything datadir.rs already covers twice.
        let _g = ENV_LOCK.lock().unwrap();
        // SAFETY: serialized via ENV_LOCK.
        unsafe { std::env::remove_var(CONFIG_STORE_PATH_ENV) };
        let cfg =
            ConfigStoreConfig::from_env().expect("data dir resolves in this test environment");
        assert_eq!(
            cfg.config_store_path,
            ConfigStoreConfig::per_user_default()
                .expect("data dir resolves in this test environment")
                .to_string_lossy()
        );
    }

    #[test]
    fn per_user_default_from_none_is_a_named_error_not_a_cwd_fallback() {
        // The owner's ruling: when no OS data directory can be resolved, this
        // must be a named error -- never a silent `./config_store` write into
        // the current working directory. Injected directly (no HOME/
        // XDG_DATA_HOME manipulation) so this doesn't need ENV_LOCK or risk
        // interfering with concurrently running tests that read the real
        // data dir.
        let err = per_user_default_from(None)
            .expect_err("no data dir must be a named error, not a resolved path");
        assert!(matches!(err, ConfigStoreError::NoDataDir));
        let message = err.to_string();
        assert!(
            message.contains("CONFIG_STORE_PATH"),
            "the error {message:?} must tell the operator which env var to set instead"
        );
    }

    #[test]
    fn per_user_default_from_some_joins_the_config_store_leaf() {
        let got = per_user_default_from(Some(PathBuf::from("/home/someone/.local/share")))
            .expect("a present data dir always resolves");
        assert_eq!(
            got,
            PathBuf::from("/home/someone/.local/share/config_store")
        );
    }
}
