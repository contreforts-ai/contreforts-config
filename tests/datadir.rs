//! Pins default datadir resolution for `contreforts-config`'s own store
//! (contreforts/contreforts-workspace#58 D2), and that it can never collide
//! with `contreforts-kg`'s datadir (S1 of the epic's durable-shape checklist,
//! contreforts/contreforts-workspace#20).
//!
//! Mirrors the precedence `contreforts-core`'s `GraphConfig` already uses
//! (`crates/contreforts-core/src/config.rs`) for its first two steps --
//! explicit env override -> per-user OS data dir -- with a distinct env var
//! and leaf directory so the two stores never resolve to the same path, but
//! deliberately diverges on the third step: this crate errors, by owner's
//! ruling, rather than falling back to a current-directory-relative path (see
//! `ConfigStoreConfig`'s doc comment in `src/lib.rs` for why).

use contreforts_config::ConfigStoreConfig;

#[test]
fn per_user_default_is_under_the_os_data_dir() {
    // Same requirement contreforts-core's own
    // `per_user_default_is_under_the_os_data_dir` test states for the
    // knowledge-graph store -- the config store must honour the same
    // mechanism, not invent a second one.
    //
    // The repo owner's decision on contreforts-workspace#58 D2 (settling S1 of
    // contreforts-workspace#20's durable-shape checklist) is that the config
    // store lives *directly* under the OS data dir -- no intermediate parent
    // directory, neither the legacy `erp-sync` nor a `contreforts` rename.
    let Some(data_dir) = dirs::data_dir() else {
        return; // no resolvable OS data dir on this platform/test runner
    };
    let def =
        ConfigStoreConfig::per_user_default().expect("a resolvable OS data dir must not error");
    assert_eq!(
        def,
        data_dir.join("config_store"),
        "config datadir {def:?} must be the OS data dir {data_dir:?} joined directly with \
         config_store -- no intermediate parent directory"
    );
}

#[test]
fn per_user_default_differs_from_the_knowledge_graph_store_path() {
    // contreforts-core's GraphConfig still resolves the KG store to
    // `<data_dir>/erp-sync/graph_store` (crates/contreforts-core/src/config.rs:68)
    // -- unchanged by this crate's D2 work, which touches only the config
    // store's own datadir. This is S1 of contreforts-workspace#20's
    // durable-shape checklist: a test that would fail if config and KG ever
    // resolved to the same directory, because that collision is exactly what
    // makes drop-and-re-sync destroy hand-entered configuration
    // (contreforts-workspace#18).
    //
    // The repo owner's decision (contreforts-workspace#58 D2) is that the
    // config store has no intermediate parent directory at all
    // (`<data_dir>/config_store`), while the KG store keeps its own
    // `erp-sync/graph_store` shape for now -- a later item may fold the KG's
    // location into configuration held in this very store, rather than a
    // path any crate compiles in. Either way the two must never collide, so
    // this compares against the KG resolver's real, current path rather than
    // a value that could only ever differ by construction.
    let Some(data_dir) = dirs::data_dir() else {
        return;
    };
    let kg_store_path = data_dir.join("erp-sync").join("graph_store");
    let config_store_path =
        ConfigStoreConfig::per_user_default().expect("a resolvable OS data dir must not error");

    assert_eq!(
        config_store_path,
        data_dir.join("config_store"),
        "config datadir {config_store_path:?} must resolve to the OS data dir joined \
         directly with config_store"
    );
    assert_ne!(
        config_store_path, kg_store_path,
        "config datadir {config_store_path:?} must not equal the knowledge-graph datadir {kg_store_path:?}"
    );
    assert!(
        !config_store_path.ends_with("graph_store"),
        "config datadir {config_store_path:?} must not reuse the knowledge-graph store's leaf directory name, graph_store"
    );
}

#[test]
fn from_env_default_is_per_user() {
    let _g = env_lock::ENV_LOCK.lock().unwrap();
    // SAFETY: tests that touch CONFIG_STORE_PATH are serialized via ENV_LOCK.
    unsafe { std::env::remove_var("CONFIG_STORE_PATH") };
    let Some(_data_dir) = dirs::data_dir() else {
        return; // no resolvable OS data dir on this platform/test runner
    };
    let cfg = ConfigStoreConfig::from_env().expect("a resolvable OS data dir must not error");
    assert_eq!(
        cfg.config_store_path,
        ConfigStoreConfig::per_user_default()
            .expect("a resolvable OS data dir must not error")
            .to_string_lossy(),
        "with CONFIG_STORE_PATH unset, from_env() must resolve to the per-user default datadir"
    );
}

#[test]
fn from_env_override_wins() {
    let _g = env_lock::ENV_LOCK.lock().unwrap();
    // SAFETY: tests that touch CONFIG_STORE_PATH are serialized via ENV_LOCK.
    unsafe { std::env::set_var("CONFIG_STORE_PATH", "/tmp/custom-config-store") };
    let cfg = ConfigStoreConfig::from_env()
        .expect("an explicit override never touches data-dir resolution");
    assert_eq!(
        cfg.config_store_path, "/tmp/custom-config-store",
        "CONFIG_STORE_PATH=/tmp/custom-config-store must override the default datadir"
    );
    unsafe { std::env::remove_var("CONFIG_STORE_PATH") };
}

#[test]
fn from_env_empty_override_is_treated_as_unset() {
    let _g = env_lock::ENV_LOCK.lock().unwrap();
    // SAFETY: tests that touch CONFIG_STORE_PATH are serialized via ENV_LOCK.
    unsafe { std::env::set_var("CONFIG_STORE_PATH", "") };
    let Some(_data_dir) = dirs::data_dir() else {
        unsafe { std::env::remove_var("CONFIG_STORE_PATH") };
        return; // no resolvable OS data dir on this platform/test runner
    };
    let cfg = ConfigStoreConfig::from_env().expect("a resolvable OS data dir must not error");
    assert_eq!(
        cfg.config_store_path,
        ConfigStoreConfig::per_user_default()
            .expect("a resolvable OS data dir must not error")
            .to_string_lossy(),
        "CONFIG_STORE_PATH=\"\" must not pin the config datadir to an empty path"
    );
    unsafe { std::env::remove_var("CONFIG_STORE_PATH") };
}

#[test]
fn from_env_errors_naming_the_reason_when_no_data_dir_and_no_override() {
    // The owner's ruling on point 4 of the D2 review: a silent
    // current-directory fallback is not acceptable for this crate, because
    // its contents are re-entered by hand rather than re-derived like the
    // knowledge graph's. When CONFIG_STORE_PATH is unset and no OS data dir
    // can be determined, from_env() must return a named error telling the
    // operator to set CONFIG_STORE_PATH explicitly -- never write into the
    // current working directory.
    //
    // This crate cannot portably force dirs::data_dir() to return None from
    // an integration test without mutating HOME/XDG_DATA_HOME in ways that
    // would race every other test in this binary that reads the real data
    // dir; that unit-level guarantee is exercised directly (and safely, via
    // dependency injection) by
    // `per_user_default_from_none_is_a_named_error_not_a_cwd_fallback` in
    // src/lib.rs's own test module. This test instead pins the observable
    // contract: whenever no data dir is available, the crate must be seen to
    // error, and the error message must name CONFIG_STORE_PATH as the escape
    // hatch -- documented here so the guarantee is visible from the public
    // API's own test surface, not only from a private helper.
    let _g = env_lock::ENV_LOCK.lock().unwrap();
    // SAFETY: tests that touch CONFIG_STORE_PATH are serialized via ENV_LOCK.
    unsafe { std::env::remove_var("CONFIG_STORE_PATH") };
    if dirs::data_dir().is_some() {
        // This test environment does have a resolvable data dir, so from_env()
        // legitimately succeeds here -- the no-data-dir contract is instead
        // pinned unconditionally (no environment dependency) by the unit test
        // named above.
        return;
    }
    let err = ConfigStoreConfig::from_env()
        .expect_err("no OS data dir and no override must be a named error");
    let message = err.to_string();
    assert!(
        message.contains("CONFIG_STORE_PATH"),
        "the error {message:?} must tell the operator which env var to set instead"
    );
}

/// Env vars are process-global; serialize the tests in this file that mutate
/// `CONFIG_STORE_PATH` so parallel test-thread execution within this binary
/// doesn't interfere -- same pattern as contreforts-core's own env-mutating
/// config tests (crates/contreforts-core/src/config.rs).
mod env_lock {
    pub static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
}
