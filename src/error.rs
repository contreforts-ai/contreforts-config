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
//! "already exists" message. A third,
//! [`KgInstanceDatadirConflict`](ConfigGraphError::KgInstanceDatadirConflict), was added by D8
//! part 1 for the same reason, once `KgInstanceConfig` grew a `datadir` field also enforced
//! unique. All three have an identity-preserving `GraphError` counterpart of the same name:
//! `contreforts-kg`'s shim maps them one-to-one, the same rule as the three domain cases above,
//! rather than folding them into `GraphError::Adapter` -- even though no route exercises
//! `set_kg_instance` through that shim today, an identical-looking fold of `SparqlSyntax` into
//! `Adapter` one chain ago turned a 400 into a 500 on a path no test asserted on, which is
//! exactly the failure shape "nothing calls it yet" invites if repeated here.
//! `contreforts-config-api/src/error.rs`'s `ApiError::status` does not yet have a dedicated arm
//! for any of the three (its `GraphError` catch-all still answers 500 for all of them) -- that
//! remaining gap is D8 part 2's to close when it wires an HTTP route to KG instance CRUD.
//!
//! D5/D6 (contreforts-workspace#58, comment 7969; #18 Q3 / #19 O2) deliberately raise **no new
//! variants**: every one of its guard rejections -- a `KnowledgeBaseConfig` naming an
//! unregistered or ambiguous `kg_instance_label`, a KB's `graph` falling outside its claimed
//! instance's prefix, or a config record other than that KB's own `graph` storing its IRI
//! verbatim -- is raised as [`InvalidIri`](ConfigGraphError::InvalidIri), the same variant
//! `require_company` already reuses for "a required entity was not found" beyond literal IRI
//! syntax. Adding a *named* variant per D5/D6 case (matching the D4 pair's own precedent) would
//! break `contreforts-kg::config_graph`'s `impl From<ConfigGraphError> for GraphError` --
//! documented on that `match` as deliberately exhaustive, never a wildcard catch-all -- which
//! lives in a different repo this chain does not touch.
//!
//! **Correction to the original landing note (D5/D6 review, this issue):** the original text here
//! claimed this was deferred because "no route exercises yet" any of these four paths, mirroring
//! the D4 pair's justification. That claim is false for three of the four. Checked concretely,
//! not assumed: `contreforts-config-api`'s `POST /companies/{slug}/knowledge-bases` route calls
//! `ConfigService::set_knowledge_base`, which reaches `ConfigGraph::set_knowledge_base` through
//! `contreforts-kg`'s shim -- live today, so `kb_instance_unregistered`, `kg_instance_ambiguous`
//! and `kb_graph_prefix_violation` are all HTTP-reachable. So are every `POST
//! /companies/{slug}/connectors/*` route, which reach `write_connector` (hence
//! `kb_graph_referenced_elsewhere`) through the same shim. Only `set_connector_target_kb`'s own
//! direct rejection is genuinely unreached (no route calls it, matching the D4 pair exactly).
//!
//! That correction is *why* `InvalidIri` stays the right call here, not merely the cheap one:
//! today, all three of the reachable rejections resolve correctly to **400** via
//! `contreforts-config-api/src/error.rs`'s existing `GraphError::InvalidIri` arm. Minting four
//! dedicated variants (mirrored 1:1 into `GraphError`, per the D4 pair's own precedent) without
//! *also* adding matching arms to that file would silently turn those live 400s into 500s the
//! moment `contreforts-config-api`'s `Self::Graph(_)` catch-all caught them instead -- the exact
//! regression this crate's own rule exists to prevent, this time self-inflicted by "fixing" the
//! variant name. Doing this properly is a **three-repo** change (`contreforts-config`,
//! `contreforts-kg`, `contreforts-config-api`), not the two-repo change the D4 pair's precedent
//! set, because unlike `KgInstanceLabelConflict`/`KgInstancePrefixConflict` these paths are
//! already live. `contreforts-config-api` is deliberately out of this chain's footprint (it is
//! the D8 boundary this shim exists to be removed at), so this is recorded here rather than done
//! unilaterally: **whoever picks up D7/D8 (which touches `contreforts-config-api` regardless)
//! should mint these four variants and their status arms together, in one sweep**, rather than
//! splitting the naming fix from the status-mapping fix it depends on. Revisit this the next
//! time `contreforts-kg`'s shim changes, rather than growing this enum unilaterally in the
//! meantime.
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

    /// `ConfigGraph::set_kg_instance` refused to register a KG instance because its **datadir**
    /// is already assigned to a different instance (a different label). Two instances sharing a
    /// datadir would interleave their writes into one physical Oxigraph store, silently
    /// corrupting both -- contreforts-workspace#58 D8, part 1.
    #[error(
        "KG instance datadir '{datadir}' is already assigned to instance '{existing_label}' -- \
         instance datadirs must be unique"
    )]
    KgInstanceDatadirConflict {
        datadir: String,
        existing_label: String,
    },
}

/// D5/D6's guard rejections (contreforts-workspace#58, comment 7969), each raised as
/// [`ConfigGraphError::InvalidIri`] rather than a dedicated variant -- see this module's top
/// doc comment for why. Free functions (not methods) so both `ConfigGraph`'s write path and its
/// `validate_startup` can build the identical message shape without duplicating the wording.
impl ConfigGraphError {
    /// `ConfigGraph::set_knowledge_base` refused a `KnowledgeBaseConfig` naming a
    /// `kg_instance_label` that is not a registered `KgInstanceConfig` -- D5's guard has nothing
    /// to check a dangling reference against.
    pub(crate) fn kb_instance_unregistered(kb_label: &str, instance_label: &str) -> Self {
        Self::InvalidIri(format!(
            "knowledge base '{kb_label}' names KG instance '{instance_label}', which is not \
             registered -- register it first with `ConfigGraph::set_kg_instance`"
        ))
    }

    /// `ConfigGraph::set_knowledge_base` was given `kg_instance_label: None` while more than one
    /// `KgInstanceConfig` is registered -- resolution is explicit, never a silent pick: with
    /// exactly one registered instance `None` resolves to it, but with several, guessing would
    /// reintroduce the exact "absence presenting as success" failure this epic keeps paying for.
    pub(crate) fn kg_instance_ambiguous(kb_label: &str, instance_count: usize) -> Self {
        Self::InvalidIri(format!(
            "knowledge base '{kb_label}' does not name a KG instance, and {instance_count} are \
             registered -- with more than one, resolution is ambiguous; name one explicitly via \
             `kg_instance_label`"
        ))
    }

    /// `ConfigGraph::set_knowledge_base` refused a `KnowledgeBaseConfig` whose `graph` does not
    /// fall under its own claimed instance's registered IRI prefix -- D5's first invariant (#18
    /// Q3, comment 7969): "its graph IRI does not fall under its own instance's assigned prefix"
    /// is what "points into another instance's data" becomes once a KB names its instance. Names
    /// the KB, the offending graph IRI, and the instance whose prefix it violated (the KB's own
    /// *claimed* instance -- not whichever instance the graph happens to match instead, if any).
    pub(crate) fn kb_graph_prefix_violation(
        kb_label: &str,
        graph: &str,
        instance_label: &str,
    ) -> Self {
        Self::InvalidIri(format!(
            "knowledge base '{kb_label}' claims KG instance '{instance_label}', but its graph \
             '{graph}' does not fall under that instance's registered IRI prefix"
        ))
    }

    /// A config write (a connector field, the Target-KB link, or an Agent's
    /// `knowledge_base_label`) tried to store, verbatim, a value equal to a registered KB's own
    /// `graph` -- D5's second invariant (#18 Q3, comment 7969): "exactly one config record may
    /// name a KB graph IRI -- the KB's own `KnowledgeBaseConfig.graph` -- and no other config
    /// record may name one at all." Raised wherever it is reached: `ConfigGraph::write_connector`'s
    /// generic engine (all eleven connector kinds' fields), `ConfigGraph::set_connector_target_kb`
    /// directly, and `ConfigGraph::set_agent` (added by this review -- `Agent` is not a connector
    /// kind, so it was missed by D5's original write path entirely; see `set_agent`'s own doc
    /// comment).
    pub(crate) fn kb_graph_referenced_elsewhere(value: &str) -> Self {
        Self::InvalidIri(format!(
            "'{value}' is a registered knowledge base's own graph IRI -- only that KB's own \
             `KnowledgeBaseConfig.graph` may hold this value; no other config record may \
             reference it"
        ))
    }

    /// `ConfigGraph::discover_kg_instance` was given `Some(label)` naming no registered instance
    /// (contreforts-workspace#58 D8, part 1; #18 Q5). Unlike
    /// [`Self::kb_instance_unregistered`]'s equivalent case, there is no KB in view to name --
    /// a consumer resolving an instance to open a store simply has nothing to open.
    pub(crate) fn kg_instance_discovery_unregistered(label: &str) -> Self {
        Self::InvalidIri(format!(
            "no KG instance is registered under label '{label}' -- register it first with \
             `ConfigGraph::set_kg_instance`"
        ))
    }

    /// `ConfigGraph::discover_kg_instance` was given `None` with **zero** instances registered.
    /// Ruling 1 on contreforts-workspace#58 D8, part 1: unlike
    /// `KnowledgeBaseConfig::kg_instance_label`'s own `None` case (which tolerates zero as "no
    /// association yet", because it is only recording a link), discovery is asking for an
    /// instance *to use* -- none existing is a real failure, not an empty success that would
    /// leave a caller with no store to open and no error explaining why.
    pub(crate) fn kg_instance_discovery_none_registered() -> Self {
        Self::InvalidIri(
            "no KG instance is registered at all -- register one first with \
             `ConfigGraph::set_kg_instance` before discovering one to open"
                .to_string(),
        )
    }

    /// `ConfigGraph::discover_kg_instance` was given `None` while more than one instance is
    /// registered -- resolution is explicit, never a silent pick, mirroring
    /// [`Self::kg_instance_ambiguous`]'s reasoning restated for a consumer with no KB in view.
    pub(crate) fn kg_instance_discovery_ambiguous(instance_count: usize) -> Self {
        Self::InvalidIri(format!(
            "no KG instance label was given, and {instance_count} are registered -- with more \
             than one, resolution is ambiguous; name one explicitly"
        ))
    }
}

impl From<oxigraph::store::StorageError> for ConfigGraphError {
    fn from(e: oxigraph::store::StorageError) -> Self {
        ConfigGraphError::Store(ConfigStoreError::from(e))
    }
}

/// This crate's own `Result` alias, mirroring `contreforts_kg::Result`.
pub type Result<T> = std::result::Result<T, ConfigGraphError>;
