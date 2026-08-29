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
//!
//! # Three classes of graph, named separately
//!
//! contreforts-workspace#18's "new requirement 1" (the config store must be able to import
//! ontologies) is explicit that the store's graph classes must be *named separately*, because they
//! have three different write policies -- collapsing any two of them makes one policy silently
//! govern data it was never argued for:
//!
//! 1. [`contreforts_core::namespaces::CONFIG_GRAPH`] -- hand-entered configuration. Written one
//!    triple at a time through [`ConfigGraph`]'s typed engine, never replaced wholesale
//!    ([`ConfigStoreError::DestructiveReplaceRefused`]). Not re-derivable by anything.
//! 2. [`PRODUCT_GRAPH`] -- build-derived declarations. Write-rejected at runtime
//!    ([`ConfigStoreError::ReservedGraphWrite`]) and rebuilt from the binary's own compiled-in
//!    Turtle at every startup ([`ConfigStore::reload_product_graph`]).
//! 3. One named graph per imported ontology, under [`IMPORTED_ONTOLOGY_GRAPH_PREFIX`] --
//!    operator-supplied vocabulary. Replaced only through [`ConfigGraph::import_ontology`], and
//!    deliberately **never** rebuilt at startup: no binary carries a copy to rebuild it from, so
//!    the mechanism that makes class 2 safe would simply destroy class 3.
//!
//! See [`IMPORTED_ONTOLOGY_GRAPH_PREFIX`]'s own doc comment for the full table and the reasoning
//! behind the third row.

pub mod config_graph;
pub mod error;
pub mod migration;
mod persistence;

pub use config_graph::{
    AgentConfig, CaldavConnectorAuth, CaldavConnectorConfig, ChannelRef,
    CisoAssistantConnectorConfig, CompanyConfig, ConfigGraph, ConnectorConfig,
    ErpNextConnectorConfig, ForgejoConnectorConfig, GitlabConnectorConfig, ImportedOntologyConfig,
    KgInstanceConfig, KnowledgeBaseConfig, MatrixConnectorConfig, O365ConnectorAuth,
    O365ConnectorConfig, OntologyFormat, PennylaneConnectorConfig, SmtpConnectorConfig,
    SmtpTlsMode, SparqlTemplateConfig, StalwartConnectorConfig, TextMirrorConnectorConfig,
    VectorStoreColumnType, VectorStoreConnectorConfig, VectorStoreKind, VisioConnectorConfig,
    all_connector_kinds, imported_ontology_graph_iri,
};
pub use migration::{MigrationOutcome, verify_config_graph_copy};
#[cfg(feature = "legacy-combined-store-migration")]
pub use migration::{migrate_config_graph_if_needed, migrate_rocksdb_datadir_if_needed};
// Deliberately *not* re-exported as a bare `Result` at this crate's root: this file's own
// `ConfigStoreError`-returning functions below already spell `Result<T, ConfigStoreError>` with
// two type parameters, and bringing `error::Result<T>` (one type parameter) into scope here
// would shadow `std::result::Result` for the rest of this file, breaking every one of them. The
// ported engine (`config_graph` module) imports `crate::error::Result` itself instead.
pub use error::ConfigGraphError;

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use contreforts_core::namespaces::{CONFIG_GRAPH, CORE_NS};
use oxigraph::io::{RdfFormat, RdfParseError, RdfParser};
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

/// Prefix under which every **imported ontology** gets its own named graph
/// (contreforts-workspace#18, "New requirement 1 -- config must be able to import ontologies",
/// added to that issue in the 2026-07-26 design session; #19 D2 is where the requirement
/// originates, as an *alignment target* for extension terms).
///
/// This is the **third** kind of graph living in the config store, and the issue is explicit that
/// the three must be named separately because they have three different write policies:
///
/// | graph | origin | write policy |
/// |---|---|---|
/// | [`contreforts_core::namespaces::CONFIG_GRAPH`] | hand-entered by an operator | read-write through the typed engine; never replaced wholesale |
/// | [`PRODUCT_GRAPH`] | build-derived, compiled into the binary | reserved: write-rejected, and rebuilt from scratch at every startup |
/// | `{IMPORTED_ONTOLOGY_GRAPH_PREFIX}{label}` | operator-supplied vocabulary | replaced only through [`config_graph::ConfigGraph::import_ontology`]; **never** rebuilt at startup |
///
/// The third row is the one this constant exists for, and its "never rebuilt at startup" is the
/// whole point: an imported ontology is operator-supplied data, not a build artifact. Reloading it
/// at startup -- the mechanism that makes [`PRODUCT_GRAPH`] safe -- would silently destroy it,
/// because no binary carries a copy to reload it *from*. It is durable in exactly the sense this
/// issue exists to create: it survives a KG drop-and-re-sync, because it does not live in the KG
/// store at all.
///
/// One graph per ontology, not one shared graph, so that re-importing or removing one vocabulary
/// cannot disturb another -- `import_ontology` clears its target graph before loading, and a
/// shared graph would make that clear destroy every other import.
///
/// The trailing slash is load-bearing: the prefix must be a *proper* prefix of every ontology
/// graph IRI so that `starts_with` is a sound membership test, the same way an instance's
/// `iri_prefix` is (contreforts-workspace#18: "the per-instance IRI prefix is what makes the guard
/// decidable"). `tests/imported_ontology.rs` pins that this prefix can never collide with
/// [`PRODUCT_GRAPH`] or [`contreforts_core::namespaces::CONFIG_GRAPH`].
pub const IMPORTED_ONTOLOGY_GRAPH_PREFIX: &str =
    "https://contreforts.ds-labs.org/data/graph/ontology/";

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
    /// [`ConfigStore::open`]'s datadir could not be created or is unusable (for example, a
    /// regular file occupies where a directory component is required). No longer an oxigraph
    /// `StorageError` -- this crate's store is in-memory (see `persistence` module doc), so
    /// `open` only ever touches the filesystem itself, never oxigraph's own on-disk backend.
    #[error("cannot open config store at {path}: {source}")]
    Open {
        path: PathBuf,
        source: std::io::Error,
    },

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

    /// [`ConfigStore::replace_named_graph`] / [`ConfigStore::clear_named_graph`] refused to
    /// operate on [`contreforts_core::namespaces::CONFIG_GRAPH`]. Both methods *replace* a graph's
    /// whole contents; pointed at the config graph that is a one-call wipe of every company,
    /// connector, agent, KB and KG-instance record an operator ever hand-entered -- the single
    /// most expensive thing this store holds, and the one thing in it that is not re-derivable
    /// (contreforts-workspace#18's whole premise). Ordinary, triple-at-a-time config writes go
    /// through [`ConfigStore::insert_quad`] and are unaffected.
    #[error(
        "refusing to replace the configuration graph <{graph}> wholesale: it holds \
         hand-entered, non-regenerable configuration and is only ever written one triple at a \
         time through the typed config engine"
    )]
    DestructiveReplaceRefused { graph: String },

    /// [`ConfigStore::replace_named_graph`] was handed a payload that parsed cleanly but yielded
    /// zero triples. Refused *before* the target graph is cleared, so an empty (or
    /// wrong-format-but-still-parseable, e.g. an HTML 404 body fed to the N-Triples parser, which
    /// yields no triples rather than an error) payload cannot silently destroy a previous import
    /// and report success. "Absence presenting as success" is this epic's recurring defect; here
    /// it would be silent data loss.
    #[error(
        "refusing to replace <{graph}>: the payload parsed successfully but contains zero \
         triples -- an empty replacement would destroy the graph's current contents and \
         report success"
    )]
    EmptyGraphPayload { graph: String },

    /// [`ConfigStore::replace_named_graph`] could not parse its payload. Distinct from
    /// [`Self::ProductGraphLoad`], which can only ever originate from
    /// [`ConfigStore::reload_product_graph`]'s own compiled-in Turtle: this one is always an
    /// *operator-supplied* file, so it is a client error (HTTP 400 in `contreforts-config-api`),
    /// not a server fault.
    #[error("failed to parse the supplied RDF: {0}")]
    RdfParse(#[from] RdfParseError),

    /// A named graph could not be serialized for on-disk persistence (`persistence` module).
    /// Distinct from [`Self::Storage`] (an oxigraph store operation failing) and
    /// [`Self::RdfParse`] (an operator-supplied payload failing to parse) -- this is this
    /// crate's own dump-to-disk step, on the write side.
    #[error("failed to serialize named graph <{graph}> for on-disk persistence: {reason}")]
    PersistSerialize { graph: String, reason: String },

    /// Writing (or reading back) a persisted graph file failed at the filesystem level -- disk
    /// full, permissions, or similar. Every mutating [`ConfigStore`] method persists
    /// synchronously before returning `Ok`, so this surfaces at exactly the call that would
    /// otherwise have reported success over a write that never reached disk.
    #[error("failed to persist named graph <{graph}> to {path}: {source}")]
    PersistWrite {
        graph: String,
        path: PathBuf,
        source: std::io::Error,
    },

    /// [`ConfigStore::open`] found a persisted graph file that fails to parse, and none of its
    /// last `attempted` backup generations parse either. Refused rather than silently starting
    /// that graph empty -- this crate's recurring rule (contreforts-workspace#18) applied to
    /// storage corruption instead of an unusable path.
    #[error(
        "config data at {path} is corrupt, and none of its last {attempted} backup(s) could \
         be recovered either -- refusing to start with this graph silently emptied, since the \
         data may still be recoverable by hand; last parse error: {reason}"
    )]
    CorruptGraphFile {
        path: PathBuf,
        attempted: usize,
        reason: String,
    },
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
    /// Open or create a config store rooted at `path`: an in-memory Oxigraph `Store` (this crate
    /// ships no RocksDB backend -- see the `persistence` module doc) loaded from whatever
    /// `path` already holds.
    ///
    /// Returns [`ConfigStoreError::Open`] — never panics, never silently
    /// falls back to a temp or current directory — when `path` cannot be
    /// used (for example, a regular file occupies where a directory
    /// component is required).
    ///
    /// Loads [`CONFIG_GRAPH`] from `path`'s persisted file, then every imported ontology
    /// **registered in [`CONFIG_GRAPH`]** (not by scanning `path`'s `ontologies/` directory --
    /// the definition record is what makes an import enumerable, per
    /// [`IMPORTED_ONTOLOGY_GRAPH_PREFIX`]'s own doc comment, so a stray file with no record is
    /// deliberately left unloaded rather than resurrected). [`PRODUCT_GRAPH`] is never loaded
    /// here: it holds no persisted file at all, and starts empty until a caller reloads it from
    /// the binary's own compiled-in Turtle ([`Self::reload_product_graph`]).
    pub fn open(path: impl AsRef<Path>) -> Result<Self, ConfigStoreError> {
        let path = path.as_ref();
        std::fs::create_dir_all(path).map_err(|source| ConfigStoreError::Open {
            path: path.to_path_buf(),
            source,
        })?;

        let store = Store::new()?;
        let this = Self {
            store: Arc::new(store),
            path: Some(path.to_path_buf()),
        };

        let config_graph_node = NamedNode::new(CONFIG_GRAPH).expect("CONFIG_GRAPH is a valid IRI");
        let config_graph_path = path.join(
            persistence::graph_relpath(CONFIG_GRAPH).expect("CONFIG_GRAPH always maps to a file"),
        );
        persistence::load_graph_with_recovery(&this.store, &config_graph_path, &config_graph_node)?;

        for graph_iri in this.registered_ontology_graph_iris()? {
            let Some(rel) = persistence::graph_relpath(&graph_iri) else {
                continue;
            };
            let node = NamedNode::new(&graph_iri).map_err(|source| ConfigStoreError::CorruptGraphFile {
                path: path.join(&rel),
                attempted: 0,
                reason: format!("CONFIG_GRAPH registers an invalid ontology graph IRI {graph_iri:?}: {source}"),
            })?;
            persistence::load_graph_with_recovery(&this.store, &path.join(&rel), &node)?;
        }

        Ok(this)
    }

    /// Every graph IRI a `CONFIG_GRAPH`-resident `ImportedOntology` record names, read with the
    /// same `?graph` shape [`config_graph::ConfigGraph::list_imported_ontologies`] queries --
    /// duplicated rather than shared because that method lives one layer up (on `ConfigGraph`,
    /// which wraps a `&ConfigStore` that does not exist yet at the point [`Self::open`] needs
    /// this).
    fn registered_ontology_graph_iris(&self) -> Result<Vec<String>, ConfigStoreError> {
        let sparql = format!(
            "SELECT ?graph WHERE {{ \
             GRAPH <{CONFIG_GRAPH}> {{ ?ont a <{CORE_NS}ImportedOntology> ; <{CORE_NS}graphIri> ?graph }} \
             }}"
        );
        Ok(self
            .select(&sparql)?
            .into_iter()
            .filter_map(|row| row.into_iter().find(|(name, _)| name == "graph").map(|(_, v)| v))
            .collect())
    }

    /// Borrow the underlying Oxigraph store.
    pub fn inner(&self) -> &Store {
        &self.store
    }

    /// Serialize `graph`'s current contents to its on-disk file (see the `persistence` module
    /// doc), or does nothing for a graph this crate never persists ([`PRODUCT_GRAPH`] -- rebuilt
    /// from compiled-in Turtle at every startup, so a copy on disk would only ever be stale).
    ///
    /// Called at the end of every operation that mutates a named graph's contents -- in this
    /// file and in [`config_graph`] alike -- so a caller that gets `Ok(())` back already has the
    /// write durable on disk; there is no separate flush step to forget.
    pub(crate) fn persist_graph(&self, graph: &NamedNode) -> Result<(), ConfigStoreError> {
        let Some(dir) = &self.path else {
            return Ok(());
        };
        let Some(rel) = persistence::graph_relpath(graph.as_str()) else {
            return Ok(());
        };

        let mut buf = Vec::new();
        self.store
            .dump_graph_to_writer(graph, RdfFormat::Turtle, &mut buf)
            .map_err(|source| ConfigStoreError::PersistSerialize {
                graph: graph.as_str().to_string(),
                reason: source.to_string(),
            })?;

        let file_path = dir.join(&rel);
        persistence::atomic_write_with_backups(&file_path, &buf).map_err(|source| {
            ConfigStoreError::PersistWrite {
                graph: graph.as_str().to_string(),
                path: file_path,
                source,
            }
        })
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
        self.persist_graph(graph)
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
        self.persist_graph(graph)
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

    /// Replace `graph`'s entire contents with `data`, parsed as `format`. Returns the number of
    /// triples the graph holds afterwards.
    ///
    /// The write path for an **imported ontology** (contreforts-workspace#18, new requirement 1).
    /// Refuses [`PRODUCT_GRAPH`] (reserved; [`Self::reload_product_graph`] is its only sanctioned
    /// writer) and [`contreforts_core::namespaces::CONFIG_GRAPH`] (see
    /// [`ConfigStoreError::DestructiveReplaceRefused`]).
    ///
    /// **Parse first, write second -- deliberately, and at the cost of one in-memory copy of the
    /// payload.** `Store::load_from_slice` streams straight into the store, so a payload that goes
    /// malformed halfway through leaves the graph holding whatever prefix parsed, with the
    /// previous import already cleared. That is acceptable for [`Self::reload_product_graph`],
    /// whose source is compiled-in and re-loadable on the next restart; it is not acceptable here,
    /// where the previous contents are operator-supplied and gone for good. Staging into a
    /// `Vec<Quad>` buys the property that a rejected import changes nothing at all --
    /// `tests/imported_ontology.rs`'s `a_malformed_payload_leaves_the_previous_import_intact` is
    /// what holds this in place.
    ///
    /// The count returned is read back **from the store**, not taken from `quads.len()`. A payload
    /// that repeats a triple parses to more quads than the graph ends up holding, and returning
    /// the parsed count would make every later "does the graph still hold what we imported?" check
    /// fail forever on such a file.
    pub fn replace_named_graph(
        &self,
        graph: &NamedNode,
        format: RdfFormat,
        data: &[u8],
    ) -> Result<usize, ConfigStoreError> {
        self.reject_non_replaceable_graph(graph)?;

        // `without_named_graphs` is the load-bearing half of this parser configuration: a
        // quad-bearing payload (TriG, N-Quads) would otherwise be free to place triples in graphs
        // the caller never named -- including, given the right file, the very reserved graph the
        // guard above just refused as a target. Refusing such a file outright is the only answer
        // that keeps "the guard checks one IRI" sound.
        let parser = RdfParser::from_format(format)
            .with_default_graph(GraphName::NamedNode(graph.clone()))
            .without_named_graphs();

        // `SliceQuadParser`'s item error type is `RdfSyntaxError`, not `RdfParseError`, so this
        // cannot lean on `?`'s single implicit conversion -- the widening to `RdfParseError` (the
        // type `ConfigStoreError::RdfParse` carries, chosen to match `oxigraph::io`'s own public
        // parse error rather than its syntax-only inner half) has to be spelled out.
        let mut quads: Vec<Quad> = Vec::new();
        for parsed in parser.for_slice(data) {
            quads.push(parsed.map_err(RdfParseError::Syntax)?);
        }

        if quads.is_empty() {
            return Err(ConfigStoreError::EmptyGraphPayload {
                graph: graph.as_str().to_string(),
            });
        }

        self.store.clear_graph(graph)?;
        self.store.extend(quads)?;
        self.persist_graph(graph)?;
        self.named_graph_len(graph)
    }

    /// Empty `graph`, returning how many triples were removed. Same two refusals as
    /// [`Self::replace_named_graph`], for the same reasons.
    ///
    /// The count is returned rather than discarded so a caller can tell "removed an ontology that
    /// held 4,102 triples" from "removed a registry record pointing at nothing" -- the second is a
    /// real, reportable condition (see `ConfigGraph::validate_startup`'s invariant 3), not a
    /// successful delete.
    pub fn clear_named_graph(&self, graph: &NamedNode) -> Result<usize, ConfigStoreError> {
        self.reject_non_replaceable_graph(graph)?;
        let removed = self.named_graph_len(graph)?;
        self.store.clear_graph(graph)?;
        self.persist_graph(graph)?;
        Ok(removed)
    }

    /// How many triples `graph` currently holds. Counted from the store on every call, never
    /// cached and never persisted: a stored copy of a derived fact is a fact that can lie, and
    /// this one would start lying the moment anything wrote the graph outside
    /// [`Self::replace_named_graph`] -- including the raw SPARQL update route
    /// (`contreforts-config-api/src/routes/graph.rs`), which is allowed to.
    pub fn named_graph_len(&self, graph: &NamedNode) -> Result<usize, ConfigStoreError> {
        let mut count = 0usize;
        for quad in self
            .store
            .quads_for_pattern(None, None, None, Some(graph.into()))
        {
            quad?;
            count += 1;
        }
        Ok(count)
    }

    /// The two graphs neither [`Self::replace_named_graph`] nor [`Self::clear_named_graph`] may
    /// ever target, each with its own named error rather than one shared "not allowed": the
    /// reserved product graph is refused because it is build-derived and rebuilt at startup, the
    /// config graph because it is hand-entered and irreplaceable. Collapsing them would tell an
    /// operator which call failed but not which invariant they hit.
    fn reject_non_replaceable_graph(&self, graph: &NamedNode) -> Result<(), ConfigStoreError> {
        if graph.as_str() == PRODUCT_GRAPH {
            return Err(ConfigStoreError::ReservedGraphWrite {
                graph: PRODUCT_GRAPH.to_string(),
            });
        }
        if graph.as_str() == CONFIG_GRAPH {
            return Err(ConfigStoreError::DestructiveReplaceRefused {
                graph: CONFIG_GRAPH.to_string(),
            });
        }
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
