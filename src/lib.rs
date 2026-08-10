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
//! D3c (contreforts/contreforts-workspace#58, comment 7904) populates this crate from the
//! generalised config-graph engine (`config_graph` module below): the 11 `*ConnectorConfig`
//! structs, the `ConnectorDescriptor` table and `write_connector`, and their thin per-kind
//! wrappers, ported from `contreforts-kg/src/config_graph.rs` to run against [`ConfigStore`]
//! instead of `contreforts_kg::GraphStore`. This crate depends on `contreforts-core` (shared
//! vocabulary, D3b) and `contreforts-declaration` (connector-instance SHACL validation, D3a) --
//! it still does not, and must not, depend on `contreforts-kg`.

pub mod config_graph;
pub mod error;
pub mod migration;

pub use config_graph::{
    AgentConfig, CaldavConnectorAuth, CaldavConnectorConfig, ChannelRef,
    CisoAssistantConnectorConfig, CompanyConfig, ConfigGraph, ConnectorConfig,
    ErpNextConnectorConfig, ForgejoConnectorConfig, GitlabConnectorConfig, KgInstanceConfig,
    KnowledgeBaseConfig, MatrixConnectorConfig, O365ConnectorAuth, O365ConnectorConfig,
    PennylaneConnectorConfig, SmtpConnectorConfig, SmtpTlsMode, SparqlTemplateConfig,
    StalwartConnectorConfig, VectorStoreColumnType, VectorStoreConnectorConfig, VectorStoreKind,
    VisioConnectorConfig, all_connector_kinds,
};
pub use migration::{MigrationOutcome, migrate_config_graph_if_needed, verify_config_graph_copy};
// Deliberately *not* re-exported as a bare `Result` at this crate's root: this file's own
// `ConfigStoreError`-returning functions below already spell `Result<T, ConfigStoreError>` with
// two type parameters, and bringing `error::Result<T>` (one type parameter) into scope here
// would shadow `std::result::Result` for the rest of this file, breaking every one of them. The
// ported engine (`config_graph` module) imports `crate::error::Result` itself instead.
pub use error::ConfigGraphError;

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use oxigraph::io::{RdfFormat, RdfParser};
use oxigraph::model::{GraphName, NamedNode, Quad, Term};
use oxigraph::sparql::{QueryResults, QuerySolution, SparqlEvaluator};
use oxigraph::store::{LoaderError, StorageError, Store};

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

/// Reserved named graph holding the build-derived product declarations
/// (`contreforts-config-api/product`'s `PRODUCT_GRAPH_TTL`), loaded as real, queryable data
/// rather than staying only a Rust `&'static str` consumed for SHACL validation
/// (contreforts-workspace#58 D6; #19 O2, answered identically to #18 Q3).
///
/// Two enforcement points, one design: [`ConfigStore::insert_quad`] rejects any write targeting
/// this graph at the moment it is attempted, and [`ConfigStore::reload_product_graph`] is called
/// at every process startup to rebuild this graph from the binary's own compiled-in data --
/// which is what makes any edit that slips in another way (the unrestricted raw SPARQL `update`
/// route this crate does not yet constrain, tracked as D7) transient rather than permanent,
/// rather than relying on write-time rejection alone.
///
/// Kept in this crate rather than promoted to `contreforts_core::namespaces` (ruling 4 on
/// contreforts-workspace#58's D5/D6 follow-up): nothing outside `contreforts-config` needs this
/// IRI while D7 (constraining the raw SPARQL route against it) and D8 (consumer rewiring) are out
/// of scope.
///
/// Distinct from [`contreforts_core::namespaces::CONFIG_GRAPH`] -- loading this graph must never
/// mix build-derived declarations into hand-entered configuration data.
pub const PRODUCT_GRAPH: &str = "https://contreforts.ds-labs.org/data/graph/product";

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

    /// A `SELECT` handed to [`ConfigStore::select`] was not valid SPARQL -- distinct from a
    /// query that parses and simply matches nothing (`Ok(vec![])`), per that method's own doc
    /// comment.
    #[error("SPARQL syntax error: {0}")]
    SparqlSyntax(#[from] oxigraph::sparql::SparqlSyntaxError),

    /// A `SELECT` handed to [`ConfigStore::select`] parsed but failed during evaluation.
    #[error("SPARQL evaluation error: {0}")]
    SparqlEvaluation(#[from] oxigraph::sparql::QueryEvaluationError),

    /// The underlying Oxigraph store failed a write (e.g. [`ConfigStore::remove_quad`], or a
    /// direct `ConfigStore::inner()` insert/remove) -- not related to `open`'s own [`Self::Open`]
    /// path-resolution failure above.
    #[error("store error: {0}")]
    Storage(#[from] StorageError),

    /// [`ConfigStore::insert_quad`] refused to write into [`PRODUCT_GRAPH`] -- it is reserved for
    /// build-derived product declarations, reloaded from scratch at every startup
    /// (contreforts-workspace#58 D6; #19 O2). Naming the graph in the message is what lets
    /// `tests/reserved_product_graph.rs`'s rejection tests assert the guard is specific to this
    /// one graph rather than one that rejects every write outright.
    #[error(
        "cannot write into the reserved product graph <{graph}>: it holds build-derived \
         declarations, is reloaded from scratch at every startup, and is not writable at runtime"
    )]
    ReservedGraphWrite { graph: String },

    /// [`ConfigStore::reload_product_graph`] was handed Turtle that failed to parse, or the
    /// underlying store failed the load -- distinct from [`Self::Storage`] because this always
    /// originates from that one call, never from an ordinary quad write.
    #[error("failed to load the reserved product graph: {0}")]
    ProductGraphLoad(#[from] LoaderError),

    /// [`crate::verify_config_graph_copy`] found the config store's `CONFIG_GRAPH` copy
    /// incomplete relative to the combined store's own `CONFIG_GRAPH` (contreforts-workspace#58,
    /// D8 part 2a). Returned as a named error, never a warning and never a lower-count "success"
    /// -- an incomplete copy of hand-entered, non-regenerable data must refuse to proceed.
    /// Names both the source's total triple count and how many were actually found in the
    /// config store, so an operator can see exactly how incomplete the copy was.
    #[error(
        "config graph copy verification failed: expected {expected} triple(s) from the \
         combined store's CONFIG_GRAPH, found only {found} in the config store -- the copy is \
         incomplete, so migration has not completed; the combined store's own CONFIG_GRAPH is \
         left untouched, so a retry (or manual investigation) can start from a known-good source"
    )]
    ConfigGraphCopyIncomplete { expected: usize, found: usize },
}

/// Configuration's own persistent Oxigraph store, wrapping an `Arc<Store>` so
/// it can be cloned cheaply and shared across the process, independent of
/// `contreforts-kg`'s knowledge-graph store.
#[derive(Clone)]
pub struct ConfigStore {
    store: Arc<Store>,
    /// The filesystem path this store was opened at. Used only for diagnostic logging
    /// (`migration::migrate_config_graph_if_needed` naming both the source and destination
    /// paths on the one path that genuinely needs to be loud) -- nothing in this crate resolves
    /// behaviour from it. Kept `Option` (always `Some` since contreforts-workspace#58 D8 part
    /// 2b deleted this struct's only other constructor, `from_arc`, which had no path of its
    /// own) rather than narrowed to a bare `PathBuf`: that would be a public API change outside
    /// this chain's remit, and every caller already handles the `None` case (`unwrap_or_else`),
    /// so there is nothing broken by leaving it as-is.
    path: Option<PathBuf>,
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
            path: Some(path.to_path_buf()),
        })
    }

    /// Borrow the underlying Oxigraph store.
    pub fn inner(&self) -> &Store {
        &self.store
    }

    /// The filesystem path this store was opened at.
    ///
    /// contreforts-workspace#58 D8 part 2b: this struct used to have a second constructor,
    /// `from_arc`, that wrapped an already-open `Arc<Store>` with no path of its own -- a bridge
    /// for `contreforts-kg`'s `config_graph` re-export shim, back when one physical Oxigraph
    /// store held both a user's config graph and knowledge graph. That shim (and its only
    /// caller of `from_arc`, `contreforts-config-api/src/routes/graph.rs`'s `list_kg_instances`
    /// helper) is deleted now that every `ConfigGraph` consumer resolves its instance from this
    /// crate directly -- `from_arc` is deleted with it, per comment 7791's ruling 2 ("if it
    /// outlives D8, that is a defect, not a design"). [`Self::open`] is this struct's only
    /// constructor now, so this always returns `Some` in practice -- still `Option<&Path>`
    /// rather than narrowed to `&Path`, since that would be an unrelated public API change.
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// Execute a SPARQL `SELECT` query, returning solutions as `(variable name, value)` rows.
    ///
    /// Modeled on `crates/contreforts-kg/src/query.rs`'s `QueryEngine::select` -- the one of its
    /// two methods the ported config-graph engine actually calls (7 sites; `ask` is never used,
    /// contreforts-workspace#58 comment 7904). A query that parses and legitimately matches
    /// nothing returns `Ok(vec![])`, never an error -- `fetch_connector`'s "no such connector yet"
    /// path depends on being able to tell that apart from a malformed query, which is `Err`.
    pub fn select(&self, sparql: &str) -> Result<Vec<Vec<(String, String)>>, ConfigStoreError> {
        let prepared = SparqlEvaluator::new().parse_query(sparql)?;
        match prepared.on_store(&self.store).execute()? {
            QueryResults::Solutions(solutions) => {
                let mut rows = Vec::new();
                for solution in solutions {
                    let solution: QuerySolution = solution?;
                    let mut row = Vec::new();
                    for (var, term) in solution.iter() {
                        let val = match term {
                            Term::Literal(l) => l.value().to_string(),
                            // Bare IRI, not `<...>`-wrapped Turtle-serialization syntax --
                            // matching `QueryEngine::select`'s own behaviour (contreforts-kg#30).
                            Term::NamedNode(n) => n.as_str().to_string(),
                            _ => term.to_string(),
                        };
                        row.push((var.as_str().to_string(), val));
                    }
                    rows.push(row);
                }
                Ok(rows)
            }
            _ => Ok(Vec::new()),
        }
    }

    /// Remove exactly one quad (subject, predicate, object, named graph) -- not the whole
    /// subject, and not a sibling predicate on the same subject.
    ///
    /// Modeled on `crates/contreforts-kg/src/store.rs`'s `GraphStore::remove_quad`, the one of
    /// its several helpers the ported engine actually calls (4 sites; every other write already
    /// goes through [`Self::inner`] directly, contreforts-workspace#58 comment 7904). Removing an
    /// absent quad is a no-op, not an error.
    pub fn remove_quad(
        &self,
        subject: &NamedNode,
        predicate: &NamedNode,
        object: &Term,
        graph: &NamedNode,
    ) -> Result<(), ConfigStoreError> {
        self.store.remove(&Quad::new(
            subject.clone(),
            predicate.clone(),
            object.clone(),
            GraphName::NamedNode(graph.clone()),
        ))?;
        Ok(())
    }

    /// Write one quad, refusing to target [`PRODUCT_GRAPH`] (contreforts-workspace#58 D6; #19
    /// O2). This is the write-time half of that invariant; [`Self::reload_product_graph`], called
    /// at every startup, is what makes any edit that reaches the reserved graph another way (the
    /// raw SPARQL `update` route this crate does not yet constrain -- D7) transient rather than
    /// permanent. A write targeting any other, ordinary named graph succeeds exactly as a direct
    /// `inner().insert(...)` would.
    pub fn insert_quad(
        &self,
        subject: &NamedNode,
        predicate: &NamedNode,
        object: &Term,
        graph: &NamedNode,
    ) -> Result<(), ConfigStoreError> {
        if graph.as_str() == PRODUCT_GRAPH {
            return Err(ConfigStoreError::ReservedGraphWrite {
                graph: PRODUCT_GRAPH.to_string(),
            });
        }
        self.store.insert(&Quad::new(
            subject.clone(),
            predicate.clone(),
            object.clone(),
            GraphName::NamedNode(graph.clone()),
        ))?;
        Ok(())
    }

    /// Rebuild [`PRODUCT_GRAPH`] from `ttl`, replacing whatever it held before
    /// (contreforts-workspace#58 D6; #19 O2). Meant to be called once at every process startup,
    /// from the binary's own compiled-in product declarations: because this always clears the
    /// graph first rather than merging into it, any edit that slipped past
    /// [`Self::insert_quad`]'s guard another way does not survive the next restart -- the reload
    /// is what turns "rejected at write time" into "gone even when it wasn't."
    pub fn reload_product_graph(&self, ttl: &str) -> Result<(), ConfigStoreError> {
        let graph = NamedNode::new(PRODUCT_GRAPH).expect("PRODUCT_GRAPH is a valid IRI");
        self.store.clear_graph(&graph)?;
        let parser = RdfParser::from_format(RdfFormat::Turtle).with_default_graph(graph);
        self.store.load_from_slice(parser, ttl)?;
        Ok(())
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
