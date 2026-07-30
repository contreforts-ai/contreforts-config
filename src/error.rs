//! The ported config-graph engine's own error type (contreforts/contreforts-workspace#58,
//! comment 7904, D3c; ruling 1 in the same issue's follow-up).
//!
//! Three variants correspond to what the engine itself deliberately raises as a *domain*
//! outcome -- `InvalidIri`, `ConnectorValidation`, `DeclaredFieldMismatch` -- the same three
//! cases `contreforts_kg::config_graph.rs` raises today via `GraphError::InvalidIri` /
//! `::ConnectorValidation` / `::DeclaredFieldMismatch`. `contreforts-kg` maps each one, one to
//! one, onto its own `GraphError`'s identically-named variant (see that crate's
//! `src/config_graph.rs` `impl From<ConfigGraphError> for GraphError`) -- **not** collapsed into
//! a catch-all, because `contreforts-config-api/src/error.rs` matches on `GraphError::InvalidIri`
//! / `SparqlSyntax` / `ConnectorValidation` to choose an HTTP status; flattening the mapping would
//! silently turn some of those into the wrong status code with every test still green.
//!
//! Two more, [`KgInstanceLabelConflict`](ConfigGraphError::KgInstanceLabelConflict) and
//! [`KgInstancePrefixConflict`](ConfigGraphError::KgInstancePrefixConflict), were added for D4
//! (contreforts/contreforts-workspace#58, ruling 1 on that issue's four flagged ambiguities):
//! `ConfigGraph::set_kg_instance` enforces uniqueness on both a KG instance's label and its
//! assigned IRI prefix, each rejected by its own named variant naming which constraint was
//! violated and which existing instance it collided with, rather than one generic
//! "already exists" message. Both have an identity-preserving `GraphError` counterpart of the
//! same name: `contreforts-kg`'s shim maps them one-to-one, the same rule as the three domain
//! cases above, rather than folding them into `GraphError::Adapter` -- even though no route
//! exercises `set_kg_instance` through that shim today, an identical-looking fold of
//! `SparqlSyntax` into `Adapter` one chain ago turned a 400 into a 500 on a path no test
//! asserted on, which is exactly the failure shape "nothing calls it yet" invites if repeated
//! here. `contreforts-config-api/src/error.rs`'s `ApiError::status` does not yet have a
//! dedicated arm for either variant (its `GraphError` catch-all still answers 500 for both) --
//! that remaining gap is D8's to close when it wires an HTTP route to KG instance CRUD.
//!
//! [`Store`](ConfigGraphError::Store) is the fourth variant, and it is deliberately *not* one of
//! "the three cases the engine raises": it carries an underlying [`ConfigStoreError`] (a SPARQL
//! parse/evaluation failure from [`crate::ConfigStore::select`], or a storage failure from
//! [`crate::ConfigStore::remove_quad`] or a direct `ConfigStore::inner()` write) that the engine
//! only *propagates*, via `?`, rather than raises as a semantic outcome of its own logic. Keeping
//! it separate is what lets the three domain cases above stay a clean, exhaustive one-to-one
//! mapping; `contreforts-kg`'s conversion maps this one onto `GraphError::Adapter` instead, which
//! -- like every other `GraphError` variant besides the three named ones -- already falls through
//! to a generic 500 in `contreforts-config-api`'s status mapping, so this does not change any
//! observed HTTP status.
use crate::ConfigStoreError;

/// The ported config-graph engine's own error type. See the module docs above for why this is
/// four variants, not the three that get an identity-preserving mapping into `GraphError`.
#[derive(Debug, thiserror::Error)]
pub enum ConfigGraphError {
    /// An IRI the engine tried to mint or resolve was not a valid absolute IRI, or a required
    /// entity (e.g. a company) was not found -- mirrors `GraphError::InvalidIri` exactly.
    #[error("Invalid IRI: {0}")]
    InvalidIri(String),

    /// A connector write violated its declared SHACL shape, or a declared class's field has no
    /// declared IRI to write under (refusing to mix its namespace with `core:`) -- mirrors
    /// `GraphError::ConnectorValidation` exactly.
    #[error("{0}")]
    ConnectorValidation(String),

    /// A *declared* connector kind's stored literal for a field does not parse as that field's
    /// declared `sh:datatype` -- mirrors `GraphError::DeclaredFieldMismatch` exactly.
    #[error("{0}")]
    DeclaredFieldMismatch(String),

    /// An underlying store/query failure, propagated rather than raised -- see the module docs.
    #[error("config store error: {0}")]
    Store(#[from] ConfigStoreError),

    /// `ConfigGraph::set_kg_instance` refused to register a KG instance because its **label**
    /// is already registered to a different instance (a different assigned prefix). Two
    /// instances sharing a label would make resolving "the instance named X" ambiguous
    /// (contreforts-workspace#18 Q5) -- contreforts-workspace#58 D4, ruling 1.
    #[error(
        "KG instance label '{label}' is already registered (existing assigned prefix \
         '{existing_prefix}') -- instance labels must be unique"
    )]
    KgInstanceLabelConflict {
        label: String,
        existing_prefix: String,
    },

    /// `ConfigGraph::set_kg_instance` refused to register a KG instance because its **IRI
    /// prefix** is already assigned to a different instance (a different label). Two instances
    /// sharing a prefix would silently merge their entity data into one IRI space -- invisible
    /// until the data is already wrong -- contreforts-workspace#58 D4, ruling 1.
    #[error(
        "KG instance IRI prefix '{prefix}' is already assigned to instance '{existing_label}' \
         -- instance prefixes must be unique"
    )]
    KgInstancePrefixConflict {
        prefix: String,
        existing_label: String,
    },
}

impl From<oxigraph::store::StorageError> for ConfigGraphError {
    fn from(e: oxigraph::store::StorageError) -> Self {
        ConfigGraphError::Store(ConfigStoreError::from(e))
    }
}

/// This crate's own `Result` alias, mirroring `contreforts_kg::Result`.
pub type Result<T> = std::result::Result<T, ConfigGraphError>;
