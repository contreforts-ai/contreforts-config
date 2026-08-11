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
//! unique. All three used to fall through `contreforts-config-api/src/error.rs`'s catch-all to a
//! 500; D8 part 2c (comment 8127 item 2) gave each of the three its own `ApiError::status` arm
//! answering 409 -- a uniqueness conflict is a client error -- even though no HTTP route reaches
//! `ConfigGraph::set_kg_instance` yet (a2's ruling on the same comment: wiring one is out of
//! scope for an error-semantics sweep), matching the same "a status arm exists before any route
//! reaches it" precedent this module already followed for `SparqlSyntax`.
//!
//! D5/D6/D8-discovery's guard rejections (contreforts-workspace#58, comment 7969; #18 Q3 / #19
//! O2) **used to** all reuse [`InvalidIri`](ConfigGraphError::InvalidIri) -- reused because it
//! already mapped to HTTP 400, not because the rejection *is* an invalid IRI. D8 part 2c
//! (comment 8127, "carried forward rather than fixed") does the coordinated, two-repo sweep that
//! was deferred until both `contreforts-config` and `contreforts-config-api` could change
//! together: seven variants below --
//! [`KbInstanceUnregistered`](ConfigGraphError::KbInstanceUnregistered),
//! [`KgInstanceAmbiguous`](ConfigGraphError::KgInstanceAmbiguous),
//! [`KbGraphPrefixViolation`](ConfigGraphError::KbGraphPrefixViolation),
//! [`KbGraphReferencedElsewhere`](ConfigGraphError::KbGraphReferencedElsewhere) (D5/D6's own four
//! guard rejections), plus discovery's own
//! [`KgInstanceDiscoveryUnregistered`](ConfigGraphError::KgInstanceDiscoveryUnregistered),
//! [`KgInstanceDiscoveryNoneRegistered`](ConfigGraphError::KgInstanceDiscoveryNoneRegistered) and
//! [`KgInstanceDiscoveryAmbiguous`](ConfigGraphError::KgInstanceDiscoveryAmbiguous) (D8 part 1),
//! renamed together rather than four-of-seven, per the ruling on comment 8127: leaving discovery
//! named `InvalidIri` "because no route reaches it yet" is exactly the justification that made
//! the original reuse look safe in D5, and this sweep is the one chance to close it before it
//! calcifies as `contreforts-kg`'s shim once did. `contreforts-config-api/src/error.rs`'s
//! `ApiError::status` gained a matching arm for every one of the seven in the same sweep -- see
//! that file -- so no live 400 silently became a 500.
//!
//! `contreforts-kg::config_graph`'s re-export shim (the thing that made a single-repo rename
//! unsafe -- it mapped `ConfigGraphError` onto its own `GraphError` one variant at a time,
//! "deliberately exhaustive, never a wildcard catch-all") was deleted in D8 part 2b, before this
//! sweep. There is no longer any second mapping to keep in sync outside `contreforts-config-api`,
//! which is why this is a two-repo change now rather than the three-repo one the previous note
//! here anticipated.
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
//!
//! Five more, added for D9 (contreforts-workspace#58; #18 Q4, "wipe is not delete"): deleting a
//! *definition* record (a `KnowledgeBaseConfig` or a `KgInstanceConfig`) is the rare, explicit,
//! referentially-guarded half of the wipe/delete split, as distinct from wiping an instance's
//! *data* (`contreforts_kg::GraphStore::wipe`, which never touches this store at all).
//! [`KnowledgeBaseUnregistered`](ConfigGraphError::KnowledgeBaseUnregistered) and
//! [`KgInstanceUnregisteredForDelete`](ConfigGraphError::KgInstanceUnregisteredForDelete) reject
//! an unknown label as a named error rather than a silent no-op;
//! [`KbDeleteBlockedByConnector`](ConfigGraphError::KbDeleteBlockedByConnector) and
//! [`KbDeleteBlockedByAgent`](ConfigGraphError::KbDeleteBlockedByAgent) refuse to delete a KB
//! still referenced by a connector (any of the eleven kinds) or an `AgentConfig` respectively --
//! `Agent` is not a connector kind, the exact shape D5's own guard originally missed, so it is
//! its own case rather than folded into the connector one; and
//! [`KgInstanceDeleteBlockedByKb`](ConfigGraphError::KgInstanceDeleteBlockedByKb) refuses to
//! delete an instance a KB still belongs to, the one legitimate config -> instance reference.
//! None of these five reuse `InvalidIri` -- ruling 3 on contreforts-workspace#58's D9 follow-up
//! is explicit that a reused variant is exactly what D8 part 2c's sweep just finished undoing.
//! `contreforts-config-api/src/error.rs` maps the two `*Unregistered*`/`*UnregisteredForDelete`
//! variants to 404 (there is nothing to delete) and the three `*BlockedBy*` variants to 409,
//! matching the `KgInstance*Conflict` trio's own precedent that a referential conflict is a
//! client error, not a server fault.
//!
//! Two more, added for contreforts-workspace#18's **new requirement 1** (the config store must be
//! able to import ontologies -- the one item of #18's stated scope that phase D never filed as a
//! sub-task): [`ImportedOntologyGraphCollidesWithInstance`](ConfigGraphError::ImportedOntologyGraphCollidesWithInstance)
//! and [`ImportedOntologyUnregistered`](ConfigGraphError::ImportedOntologyUnregistered). Neither
//! reuses [`InvalidIri`](ConfigGraphError::InvalidIri) -- reusing it is precisely what D8 part 2c's
//! sweep just finished undoing, and "no route reaches it yet" is the same justification that made
//! the original reuse look safe. `contreforts-config-api/src/error.rs`'s `ApiError::status` must
//! answer **400** for the collision (an instance prefix chosen broadly enough to swallow the
//! imported-ontology prefix is the caller's mistake, exactly like
//! [`KbGraphPrefixViolation`](ConfigGraphError::KbGraphPrefixViolation)) and **404** for the
//! unregistered case (nothing to remove, exactly like
//! [`KnowledgeBaseUnregistered`](ConfigGraphError::KnowledgeBaseUnregistered) /
//! [`KgInstanceUnregisteredForDelete`](ConfigGraphError::KgInstanceUnregisteredForDelete)).
//! Without those arms both fall through that file's `Self::Graph(_)` catch-all to a 500 -- the
//! defect this module doc already records as having happened once on this phase. The same applies
//! to the three [`ConfigStoreError`] variants `ConfigGraph::import_ontology` can surface through
//! [`Store`](ConfigGraphError::Store) -- `RdfParse`, `EmptyGraphPayload` and
//! `DestructiveReplaceRefused` -- all three properties of what the caller supplied, so all three
//! 400.
use crate::ConfigStoreError;

/// The ported config-graph engine's own error type. See the module docs above for the three
/// domain cases, the four uniqueness-conflict/guard-rejection families layered on top since, and
/// why [`Store`](Self::Store) stays a separate variant rather than folding into either.
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

    /// D5's guard: `ConfigGraph::set_knowledge_base` refused a `KnowledgeBaseConfig` naming a
    /// `kg_instance_label` that is not a registered `KgInstanceConfig` -- the guard has nothing
    /// to check a dangling reference against. D8 part 2c: renamed off `InvalidIri` (comment
    /// 8127); `contreforts-config-api/src/error.rs` maps this to 400, same as before the rename.
    #[error(
        "knowledge base '{kb_label}' names KG instance '{instance_label}', which is not \
         registered -- register it first with `ConfigGraph::set_kg_instance`"
    )]
    KbInstanceUnregistered {
        kb_label: String,
        instance_label: String,
    },

    /// D5's guard: `ConfigGraph::set_knowledge_base` was given `kg_instance_label: None` while
    /// more than one `KgInstanceConfig` is registered -- resolution is explicit, never a silent
    /// pick: with exactly one registered instance `None` resolves to it, but with several,
    /// guessing would reintroduce the exact "absence presenting as success" failure this epic
    /// keeps paying for. D8 part 2c: renamed off `InvalidIri` (comment 8127).
    #[error(
        "knowledge base '{kb_label}' does not name a KG instance, and {instance_count} are \
         registered -- with more than one, resolution is ambiguous; name one explicitly via \
         `kg_instance_label`"
    )]
    KgInstanceAmbiguous {
        kb_label: String,
        instance_count: usize,
    },

    /// D5's guard: `ConfigGraph::set_knowledge_base` refused a `KnowledgeBaseConfig` whose
    /// `graph` does not fall under its own claimed instance's registered IRI prefix (#18 Q3,
    /// comment 7969): "its graph IRI does not fall under its own instance's assigned prefix" is
    /// what "points into another instance's data" becomes once a KB names its instance. Names
    /// the KB, the offending graph IRI, and the instance whose prefix it violated (the KB's own
    /// *claimed* instance -- not whichever instance the graph happens to match instead, if any).
    /// D8 part 2c: renamed off `InvalidIri` (comment 8127).
    #[error(
        "knowledge base '{kb_label}' claims KG instance '{instance_label}', but its graph \
         '{graph}' does not fall under that instance's registered IRI prefix"
    )]
    KbGraphPrefixViolation {
        kb_label: String,
        graph: String,
        instance_label: String,
    },

    /// D5's guard: a config write (a connector field, the Target-KB link, or an Agent's
    /// `knowledge_base_label`) tried to store, verbatim, a value equal to a registered KB's own
    /// `graph` (#18 Q3, comment 7969): "exactly one config record may name a KB graph IRI -- the
    /// KB's own `KnowledgeBaseConfig.graph` -- and no other config record may name one at all."
    /// Raised wherever it is reached: `ConfigGraph::write_connector`'s generic engine (all eleven
    /// connector kinds' fields), `ConfigGraph::set_connector_target_kb` directly, and
    /// `ConfigGraph::set_agent` (`Agent` is not a connector kind, so it was missed by D5's
    /// original write path entirely; see `set_agent`'s own doc comment). D8 part 2c: renamed off
    /// `InvalidIri` (comment 8127).
    #[error(
        "'{value}' is a registered knowledge base's own graph IRI -- only that KB's own \
         `KnowledgeBaseConfig.graph` may hold this value; no other config record may \
         reference it"
    )]
    KbGraphReferencedElsewhere { value: String },

    /// D8 part 1's `ConfigGraph::discover_kg_instance` was given `Some(label)` naming no
    /// registered instance (#18 Q5). Unlike [`Self::KbInstanceUnregistered`]'s equivalent case,
    /// there is no KB in view to name -- a consumer resolving an instance to open a store simply
    /// has nothing to open. D8 part 2c: renamed off `InvalidIri` alongside the D5/D6 guard
    /// rejections above, per a2's ruling on comment 8127 item 1 -- discovery's own three variants
    /// were flagged by a1 as "no route reaches them today", which is exactly the justification
    /// that made the original `InvalidIri` reuse look safe; closed now rather than left for a
    /// later sweep that might never come.
    #[error(
        "no KG instance is registered under label '{label}' -- register it first with \
         `ConfigGraph::set_kg_instance`"
    )]
    KgInstanceDiscoveryUnregistered { label: String },

    /// D8 part 1's `ConfigGraph::discover_kg_instance` was given `None` with **zero** instances
    /// registered. Ruling 1 on contreforts-workspace#58 D8, part 1: unlike
    /// `KnowledgeBaseConfig::kg_instance_label`'s own `None` case (which tolerates zero as "no
    /// association yet", because it is only recording a link), discovery is asking for an
    /// instance *to use* -- none existing is a real failure, not an empty success that would
    /// leave a caller with no store to open and no error explaining why. D8 part 2c: renamed off
    /// `InvalidIri`, same as `KgInstanceDiscoveryUnregistered` above.
    #[error(
        "no KG instance is registered at all -- register one first with \
         `ConfigGraph::set_kg_instance` before discovering one to open"
    )]
    KgInstanceDiscoveryNoneRegistered,

    /// D8 part 1's `ConfigGraph::discover_kg_instance` was given `None` while more than one
    /// instance is registered -- resolution is explicit, never a silent pick, mirroring
    /// [`Self::KgInstanceAmbiguous`]'s reasoning restated for a consumer with no KB in view. D8
    /// part 2c: renamed off `InvalidIri`, same as the other two discovery variants above.
    #[error(
        "no KG instance label was given, and {instance_count} are registered -- with more \
         than one, resolution is ambiguous; name one explicitly"
    )]
    KgInstanceDiscoveryAmbiguous { instance_count: usize },

    /// D9 (contreforts-workspace#58; #18 Q4, "wipe is not delete"): `ConfigGraph::remove_knowledge_base`
    /// was asked to delete a `(company_slug, label)` pair naming no registered
    /// `KnowledgeBaseConfig`. A silent `Ok(())` here would be indistinguishable from a real
    /// deletion -- exactly the "absence presenting as success" failure this epic keeps paying
    /// for -- so an unknown label is a named error instead.
    #[error(
        "knowledge base '{label}' is not registered for company '{company_slug}' -- nothing to \
         delete"
    )]
    KnowledgeBaseUnregistered { company_slug: String, label: String },

    /// D9: `ConfigGraph::remove_knowledge_base` refused because a connector -- of any of the
    /// eleven kinds, singleton or label-scoped -- still targets this KB via
    /// `ConfigGraph::set_connector_target_kb`. Deleting the KB out from under a connector that
    /// still names it would leave that connector silently pointing at nothing, the next sync
    /// failing with no clue why. `connector_label_suffix` is empty for a singleton connector
    /// kind (e.g. `erpnext`), which has no label of its own to name; for a label-scoped kind it
    /// is `" (label '<label>')"`, built by
    /// [`ConfigGraphError::kb_delete_blocked_by_connector`].
    #[error(
        "knowledge base '{kb_label}' cannot be deleted: connector '{connector_kind}'\
         {connector_label_suffix} still targets it -- retarget or remove that connector first"
    )]
    KbDeleteBlockedByConnector {
        kb_label: String,
        connector_kind: String,
        connector_label_suffix: String,
    },

    /// D9: `ConfigGraph::remove_knowledge_base` refused because an `AgentConfig` still names
    /// this KB via `knowledge_base_label` -- deleting it would leave that agent with no
    /// knowledge base to answer from. The same "record types that can reference a KB" scope as
    /// [`Self::KbDeleteBlockedByConnector`], but `Agent` is not a connector kind (the exact shape
    /// D5's own guard originally missed, see `tests/kb_reference_guard.rs`'s review addendum),
    /// so it is checked as its own case rather than folded into the connector one.
    #[error(
        "knowledge base '{kb_label}' cannot be deleted: agent '{agent_label}' still uses it -- \
         retarget or remove that agent first"
    )]
    KbDeleteBlockedByAgent {
        kb_label: String,
        agent_label: String,
    },

    /// D9: `ConfigGraph::remove_kg_instance` was asked to delete a `label` naming no registered
    /// `KgInstanceConfig`. Mirrors [`Self::KnowledgeBaseUnregistered`]'s reasoning: a silent
    /// `Ok(())` here is indistinguishable from a real deletion.
    #[error("KG instance '{label}' is not registered -- nothing to delete")]
    KgInstanceUnregisteredForDelete { label: String },

    /// D9: `ConfigGraph::remove_kg_instance` refused because a `KnowledgeBaseConfig` still
    /// belongs to this instance via `kg_instance_label` -- the one legitimate config -> instance
    /// reference (#18: "config may name a KB in the KB's definition record and nowhere else").
    /// Deleting the instance out from under that KB would leave D5's own graph-prefix guard with
    /// nothing to check the KB's graph against. Protecting this one link transitively protects
    /// any connector or agent that in turn targets that KB, without this guard needing to know
    /// anything about connectors or agents -- that reference chain is
    /// [`Self::KbDeleteBlockedByConnector`] / [`Self::KbDeleteBlockedByAgent`]'s own concern, one
    /// level down.
    #[error(
        "KG instance '{label}' cannot be deleted: knowledge base '{kb_label}' (company \
         '{company_slug}') still belongs to it -- reassign or remove that knowledge base first"
    )]
    KgInstanceDeleteBlockedByKb {
        label: String,
        kb_label: String,
        company_slug: String,
    },

    /// contreforts-workspace#18 (new requirement 1): `ConfigGraph::import_ontology` refused
    /// because the graph it would mint for `label` falls under a registered `KgInstanceConfig`'s
    /// `iri_prefix`.
    ///
    /// This is the one-directional rule (#18 point 4) applied to the new graph class. An imported
    /// ontology is durable configuration; a KG instance's IRI space is explicitly disposable under
    /// drop-and-re-sync. An ontology sitting inside it would be destroyed by the routine operation
    /// this whole issue exists to make safe -- and destroyed *silently*, since nothing else in the
    /// system would notice a vocabulary had gone missing until an alignment quietly stopped
    /// resolving.
    ///
    /// Decidable by prefix, exactly as #18 argued the guard would be. Reachable only when an
    /// operator assigns an instance a prefix broad enough to swallow
    /// [`crate::IMPORTED_ONTOLOGY_GRAPH_PREFIX`]; `ConfigGraph::validate_startup` re-checks it,
    /// which is what covers the opposite ordering (ontology imported first, swallowing instance
    /// registered afterwards) that no write-time check on `import_ontology` can see.
    #[error(
        "imported ontology '{label}' would live in graph '{graph}', which falls under \
         registered KG instance '{instance_label}' (IRI prefix '{instance_prefix}') -- an \
         imported ontology is durable configuration and must not sit inside a KG instance's \
         disposable data space"
    )]
    ImportedOntologyGraphCollidesWithInstance {
        label: String,
        graph: String,
        instance_label: String,
        instance_prefix: String,
    },

    /// contreforts-workspace#18 (new requirement 1): `ConfigGraph::remove_imported_ontology` was
    /// asked to remove a label naming no registered import. Mirrors
    /// [`Self::KnowledgeBaseUnregistered`] / [`Self::KgInstanceUnregisteredForDelete`]: a silent
    /// `Ok(())` here is indistinguishable from a real removal, and would let an operator believe a
    /// vocabulary was gone while it is still loaded and still being aligned against.
    #[error("imported ontology '{label}' is not registered -- nothing to remove")]
    ImportedOntologyUnregistered { label: String },
}

/// D5/D6/D8-discovery's guard rejections, each raised as one of the seven named variants above
/// rather than a reused `InvalidIri` (D8 part 2c, comment 8127) -- see this module's top doc
/// comment for why. Free functions (not methods) so both `ConfigGraph`'s write path and its
/// `validate_startup` can build the identical message shape without duplicating the wording.
impl ConfigGraphError {
    /// See [`Self::KbInstanceUnregistered`].
    pub(crate) fn kb_instance_unregistered(kb_label: &str, instance_label: &str) -> Self {
        Self::KbInstanceUnregistered {
            kb_label: kb_label.to_string(),
            instance_label: instance_label.to_string(),
        }
    }

    /// See [`Self::KgInstanceAmbiguous`].
    pub(crate) fn kg_instance_ambiguous(kb_label: &str, instance_count: usize) -> Self {
        Self::KgInstanceAmbiguous {
            kb_label: kb_label.to_string(),
            instance_count,
        }
    }

    /// See [`Self::KbGraphPrefixViolation`].
    pub(crate) fn kb_graph_prefix_violation(
        kb_label: &str,
        graph: &str,
        instance_label: &str,
    ) -> Self {
        Self::KbGraphPrefixViolation {
            kb_label: kb_label.to_string(),
            graph: graph.to_string(),
            instance_label: instance_label.to_string(),
        }
    }

    /// See [`Self::KbGraphReferencedElsewhere`].
    pub(crate) fn kb_graph_referenced_elsewhere(value: &str) -> Self {
        Self::KbGraphReferencedElsewhere {
            value: value.to_string(),
        }
    }

    /// See [`Self::KgInstanceDiscoveryUnregistered`].
    pub(crate) fn kg_instance_discovery_unregistered(label: &str) -> Self {
        Self::KgInstanceDiscoveryUnregistered {
            label: label.to_string(),
        }
    }

    /// See [`Self::KgInstanceDiscoveryNoneRegistered`].
    pub(crate) fn kg_instance_discovery_none_registered() -> Self {
        Self::KgInstanceDiscoveryNoneRegistered
    }

    /// See [`Self::KgInstanceDiscoveryAmbiguous`].
    pub(crate) fn kg_instance_discovery_ambiguous(instance_count: usize) -> Self {
        Self::KgInstanceDiscoveryAmbiguous { instance_count }
    }

    /// See [`Self::KnowledgeBaseUnregistered`].
    pub(crate) fn knowledge_base_unregistered(company_slug: &str, label: &str) -> Self {
        Self::KnowledgeBaseUnregistered {
            company_slug: company_slug.to_string(),
            label: label.to_string(),
        }
    }

    /// See [`Self::KbDeleteBlockedByConnector`]. `connector_label` is `None` for a singleton
    /// connector kind, which has no label of its own to name.
    pub(crate) fn kb_delete_blocked_by_connector(
        kb_label: &str,
        connector_kind: &str,
        connector_label: Option<&str>,
    ) -> Self {
        Self::KbDeleteBlockedByConnector {
            kb_label: kb_label.to_string(),
            connector_kind: connector_kind.to_string(),
            connector_label_suffix: connector_label
                .map(|label| format!(" (label '{label}')"))
                .unwrap_or_default(),
        }
    }

    /// See [`Self::KbDeleteBlockedByAgent`].
    pub(crate) fn kb_delete_blocked_by_agent(kb_label: &str, agent_label: &str) -> Self {
        Self::KbDeleteBlockedByAgent {
            kb_label: kb_label.to_string(),
            agent_label: agent_label.to_string(),
        }
    }

    /// See [`Self::KgInstanceUnregisteredForDelete`].
    pub(crate) fn kg_instance_unregistered_for_delete(label: &str) -> Self {
        Self::KgInstanceUnregisteredForDelete {
            label: label.to_string(),
        }
    }

    /// See [`Self::KgInstanceDeleteBlockedByKb`].
    pub(crate) fn kg_instance_delete_blocked_by_kb(
        label: &str,
        kb_label: &str,
        company_slug: &str,
    ) -> Self {
        Self::KgInstanceDeleteBlockedByKb {
            label: label.to_string(),
            kb_label: kb_label.to_string(),
            company_slug: company_slug.to_string(),
        }
    }

    /// See [`Self::ImportedOntologyGraphCollidesWithInstance`].
    pub(crate) fn imported_ontology_graph_collides_with_instance(
        label: &str,
        graph: &str,
        instance_label: &str,
        instance_prefix: &str,
    ) -> Self {
        Self::ImportedOntologyGraphCollidesWithInstance {
            label: label.to_string(),
            graph: graph.to_string(),
            instance_label: instance_label.to_string(),
            instance_prefix: instance_prefix.to_string(),
        }
    }

    /// See [`Self::ImportedOntologyUnregistered`].
    pub(crate) fn imported_ontology_unregistered(label: &str) -> Self {
        Self::ImportedOntologyUnregistered {
            label: label.to_string(),
        }
    }
}

impl From<oxigraph::store::StorageError> for ConfigGraphError {
    fn from(e: oxigraph::store::StorageError) -> Self {
        ConfigGraphError::Store(ConfigStoreError::from(e))
    }
}

/// This crate's own `Result` alias, mirroring `contreforts_kg::Result`.
pub type Result<T> = std::result::Result<T, ConfigGraphError>;
