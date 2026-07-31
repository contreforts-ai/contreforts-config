//! One-time migration of the config graph out of the combined store (contreforts-workspace#58,
//! D8 part 2a). See `tests/migration.rs` for the behaviour this module is pinned against, and
//! that file's own extensive doc comments for the full rationale -- summarised here only where
//! it bears on an implementation choice.
//!
//! # Why the already-populated check is scoped to `CONFIG_GRAPH`, not whole-store emptiness
//!
//! D6's reserved-product-graph reload ([`crate::ConfigStore::reload_product_graph`]) runs at
//! every process startup and writes into this same physical store, under
//! [`crate::PRODUCT_GRAPH`] -- a *different* named graph from `CONFIG_GRAPH`. Ordering between
//! that reload and this migration is part 2b's to wire; a naive "is the whole store non-empty?"
//! check would see the product-graph reload's own triples (if it ran first) and conclude
//! migration had already happened -- forever, silently, on every future startup, without ever
//! copying the real, hand-entered configuration this feature exists to preserve. Scoping the
//! check to `CONFIG_GRAPH` content specifically makes this correct regardless of which runs
//! first: startup ordering becomes belt-and-braces, never load-bearing.
//!
//! # Why "both stores hold a config graph" is not itself logged as a warning
//!
//! This migration copies and deliberately does not delete -- the source is the only rollback
//! that exists for data nothing can regenerate. That means after the *first* successful
//! migration, both the combined store and the config store permanently hold a config graph:
//! that is the normal steady state, not an anomaly. A warning that fires on this state would
//! fire on every subsequent startup forever, and a warning everyone learns to ignore is worse
//! than silence -- it would devalue the loud, one-time logging the real migration path below
//! genuinely needs. So [`MigrationOutcome::ConfigStoreAlreadyPopulated`] is a plain no-op: no
//! `tracing` event at all on this path. Do not "fix" this into a warning.
use std::path::Path;

use oxigraph::model::{NamedNode, Quad};
use oxigraph::store::Store;
use tracing::info;

use contreforts_core::namespaces::CONFIG_GRAPH;

use crate::{ConfigStore, ConfigStoreError};

/// What [`migrate_config_graph_if_needed`] did, named rather than a bare `bool` so a caller (and
/// this crate's own tests) can tell "migrated N triples" apart from the two distinct reasons
/// nothing happened -- nothing here is silent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MigrationOutcome {
    /// A real, one-time copy happened: `triples_copied` `CONFIG_GRAPH` triples were copied from
    /// the combined store into the config store, and independently verified present there
    /// afterwards.
    Migrated { triples_copied: usize },
    /// The config store already held `CONFIG_GRAPH` content before this call -- nothing was
    /// copied. See this module's top doc comment for why this is the normal steady state, not
    /// logged as a warning.
    ConfigStoreAlreadyPopulated,
    /// There was nothing to migrate: no store exists at `combined_store_path` at all, or one
    /// exists but holds no `CONFIG_GRAPH` content.
    NothingToMigrate,
}

/// Migrate `CONFIG_GRAPH` out of the combined store at `combined_store_path`, into
/// `config_store`, exactly once.
///
/// Meant to be called at every process startup (part 2b's wiring), before any consumer touches
/// `config_store` -- but correct regardless of what else has already run against `config_store`
/// first, because the already-populated check below is scoped to `CONFIG_GRAPH` content
/// specifically (see this module's top doc comment). Startup ordering relative to D6's
/// product-graph reload is therefore belt-and-braces, never load-bearing.
///
/// # What this does, in order
///
/// 1. If `config_store` already holds any `CONFIG_GRAPH` triple, this is a no-op --
///    [`MigrationOutcome::ConfigStoreAlreadyPopulated`], nothing logged (see top doc comment).
/// 2. If nothing exists at `combined_store_path`, this is a no-op, and does **not** create
///    anything there (unlike `Store::open`, which would silently create a store at any path
///    handed to it) -- [`MigrationOutcome::NothingToMigrate`].
/// 3. Otherwise the combined store is opened just long enough to read its `CONFIG_GRAPH` quads,
///    then the handle is dropped immediately. Copy, never move: nothing is ever written back to
///    the combined store, which stays the only rollback for data nothing can regenerate.
///    Dropping the handle here (rather than holding it) is also what makes step 5's reopen of
///    the same path legal -- Oxigraph's on-disk backend cannot hold two live handles on one path
///    in one process.
/// 4. If the combined store holds no `CONFIG_GRAPH` triples, this is a no-op --
///    [`MigrationOutcome::NothingToMigrate`].
/// 5. Otherwise the quads read in step 3 are written into `config_store`, the combined store is
///    reopened fresh, and [`verify_config_graph_copy`] checks the copy against what is actually
///    on disk -- not trusting the copy step's own correctness. An incomplete copy is a named
///    error ([`ConfigStoreError::ConfigGraphCopyIncomplete`]), never a warning, never a partial
///    success. Only once verification passes is this logged at `info`, naming both paths and the
///    triple count -- the one path that genuinely needs to be loud -- and
///    [`MigrationOutcome::Migrated`] returned.
///
/// # Errors
///
/// - [`ConfigStoreError::Open`] if a store exists at `combined_store_path` but cannot be opened.
/// - [`ConfigStoreError::Storage`] if reading or writing quads fails.
/// - [`ConfigStoreError::ConfigGraphCopyIncomplete`] if verification finds the copy incomplete.
pub fn migrate_config_graph_if_needed(
    combined_store_path: impl AsRef<Path>,
    config_store: &ConfigStore,
) -> Result<MigrationOutcome, ConfigStoreError> {
    let combined_store_path = combined_store_path.as_ref();

    if config_graph_is_populated(config_store.inner())? {
        return Ok(MigrationOutcome::ConfigStoreAlreadyPopulated);
    }

    if !combined_store_path.exists() {
        return Ok(MigrationOutcome::NothingToMigrate);
    }

    let source_quads = {
        let source = open_store(combined_store_path)?;
        let quads = config_graph_quads(&source)?;
        // Release this handle before this function reopens the same path below for
        // verification -- the on-disk backend cannot hold two live handles on one path in one
        // process.
        drop(source);
        quads
    };

    if source_quads.is_empty() {
        return Ok(MigrationOutcome::NothingToMigrate);
    }

    config_store.inner().extend(source_quads)?;

    let reopened_source = open_store(combined_store_path)?;
    let triples_copied = verify_config_graph_copy(&reopened_source, config_store)?;
    drop(reopened_source);

    info!(
        combined_store_path = %combined_store_path.display(),
        config_store_path = %config_store
            .path()
            .map(|p| p.display().to_string())
            // ConfigStore::open (this struct's only constructor since contreforts-workspace#58
            // D8 part 2b deleted the from_arc bridge) always records a path -- this fallback is
            // defensive, not reachable in practice.
            .unwrap_or_else(|| "<unknown ConfigStore path>".to_string()),
        triples_copied,
        "config-graph migration: copied and verified {triples_copied} triple(s) from the \
         combined store at {} into the config store -- the combined store's own copy is left \
         untouched",
        combined_store_path.display(),
    );

    Ok(MigrationOutcome::Migrated { triples_copied })
}

/// Verify that every `CONFIG_GRAPH` triple in `source` is also present in `config_store`'s own
/// `CONFIG_GRAPH`, returning the number found. Exposed separately from
/// [`migrate_config_graph_if_needed`] (which calls this internally, right after copying) so it
/// can be pinned directly against a deliberately incomplete target, rather than trusting the copy
/// step's own correctness.
///
/// # Errors
///
/// [`ConfigStoreError::ConfigGraphCopyIncomplete`] if any of `source`'s `CONFIG_GRAPH` triples is
/// missing from `config_store` -- naming both the source's total triple count and how many were
/// actually found, so an operator can see exactly how incomplete the copy was, not merely that
/// something was wrong.
pub fn verify_config_graph_copy(
    source: &Store,
    config_store: &ConfigStore,
) -> Result<usize, ConfigStoreError> {
    let expected = config_graph_quads(source)?;
    let expected_total = expected.len();

    let target = config_graph_quads(config_store.inner())?;
    let target: std::collections::HashSet<&Quad> = target.iter().collect();

    let found = expected.iter().filter(|q| target.contains(q)).count();

    if found == expected_total {
        Ok(found)
    } else {
        Err(ConfigStoreError::ConfigGraphCopyIncomplete {
            expected: expected_total,
            found,
        })
    }
}

/// Every quad under `CONFIG_GRAPH` in `store`.
fn config_graph_quads(store: &Store) -> Result<Vec<Quad>, ConfigStoreError> {
    let graph = config_graph_name();
    store
        .quads_for_pattern(None, None, None, Some((&graph).into()))
        .collect::<Result<Vec<_>, _>>()
        .map_err(ConfigStoreError::Storage)
}

/// Whether `store` holds any `CONFIG_GRAPH` triple at all -- deliberately not "is the whole
/// store empty" (see this module's top doc comment for why).
fn config_graph_is_populated(store: &Store) -> Result<bool, ConfigStoreError> {
    let graph = config_graph_name();
    match store
        .quads_for_pattern(None, None, None, Some((&graph).into()))
        .next()
    {
        Some(Ok(_)) => Ok(true),
        Some(Err(e)) => Err(ConfigStoreError::Storage(e)),
        None => Ok(false),
    }
}

fn config_graph_name() -> NamedNode {
    NamedNode::new(CONFIG_GRAPH).expect("CONFIG_GRAPH is a valid IRI")
}

/// Open a store at `path`, mapping a failure onto the same [`ConfigStoreError::Open`] shape
/// [`ConfigStore::open`] itself uses.
fn open_store(path: &Path) -> Result<Store, ConfigStoreError> {
    Store::open(path).map_err(|source| ConfigStoreError::Open {
        path: path.to_path_buf(),
        source,
    })
}
