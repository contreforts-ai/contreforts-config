//! D8 part 2c, item 1 (contreforts/contreforts-workspace#58, comment 8127, "carried forward
//! rather than fixed"): every one of D5/D6's guard rejections is raised today as
//! `ConfigGraphError::InvalidIri` -- reused because it already mapped to HTTP 400, not because
//! the rejection *is* an invalid IRI. `src/error.rs`'s own module doc explains why this was
//! deferred rather than fixed in D5/D6 or D8 part 1/2b: renaming the variant without *also*
//! updating `contreforts-config-api/src/error.rs`'s status mapping in the same sweep would turn
//! working 400s into 500s. This chain does both halves together (this file pins the rename half;
//! `contreforts-config-api/tests/guard_rejection_status.rs` pins the status half).
//!
//! ## The complete enumeration this file is scoped to
//!
//! Grepping every `ConfigGraphError::InvalidIri` construction site in `src/config_graph.rs`
//! turns up two families, not one:
//!
//! 1. **D5/D6's guard rejections** -- a config write the guard itself refuses because it violates
//!    an invariant the guard exists to enforce. These are what this file renames:
//!    - [`ConfigGraphError::kb_instance_unregistered`] -- a `KnowledgeBaseConfig` names a
//!      `kg_instance_label` that is not a registered `KgInstanceConfig`.
//!    - [`ConfigGraphError::kg_instance_ambiguous`] -- a `KnowledgeBaseConfig` gives no
//!      `kg_instance_label` while more than one instance is registered.
//!    - [`ConfigGraphError::kb_graph_prefix_violation`] -- a KB's own `graph` does not fall under
//!      its claimed instance's registered IRI prefix.
//!    - [`ConfigGraphError::kb_graph_referenced_elsewhere`] -- a config record other than that
//!      KB's own definition stores a registered KB's `graph` IRI verbatim (reached from
//!      `write_connector`, `set_connector_target_kb`, and `set_agent`).
//!
//! 2. **Genuine reuses of "a required entity was not found" or "an IRI could not be minted"**,
//!    which this file deliberately leaves alone -- renaming these would misrepresent them as
//!    something they are not:
//!    - `require_company` ("company '{slug}' not found") -- already HTTP-covered by
//!      `contreforts-config-api/tests/connectors.rs`'s `connector_on_unknown_company_is_a_client_error`.
//!    - `remove_connector`'s "unknown connector type '{type}'" -- a malformed *route parameter*,
//!      not a guard rejection.
//!    - `rename_kg_instance`'s "no KG instance registered under label '{old_label}'" -- the same
//!      "required entity not found" shape as `require_company`, just for a different entity.
//!    - `Self::node`'s wrap of `NamedNode::new` failures -- a literal IRI-syntax failure, the one
//!      case that is actually what the variant's name says.
//!    - The `"missing slug"` / `"missing name"` / `"missing iriPrefix"` / `"missing label"` /
//!      `"missing graphIri"` family in `list_companies`/`get_company`/`get_kg_instance`/
//!      `list_kg_instances`/`all_registered_kb_graphs`, and the o365 "has no access_token stored"
//!      case -- these guard against a SPARQL result row missing a column the query's own `WHERE`
//!      clause requires to be present, i.e. corrupt store contents, not a rejected *input*. They
//!      are practically unreachable through any write path this crate exposes (every write sets
//!      the columns these reads later require), so they get no new test here.
//!
//! **Discovery's three variants are a separate, related family, deliberately not touched here**:
//! `kg_instance_discovery_unregistered`/`_none_registered`/`_ambiguous` (`discover_kg_instance`,
//! D8 part 1) also currently construct `InvalidIri`, but the task scoped renaming to D5/D6's
//! *guard* rejections specifically; discovery's own gap is a missing *test*
//! (`contreforts-config/tests/kg_instance_discovery.rs`), not a misnamed variant, and it has no
//! HTTP route to answer a status code from in the first place (see that file's own addition for
//! why). Left named for a2 to weigh in on: if the same rename logic should extend to discovery's
//! three variants for consistency, that is a broader call than this chain was asked to make.
//!
//! ## What was *not* found: "a write into the reserved product graph" raised as `InvalidIri`
//!
//! The task description's item 1 lists this as a third guard-rejection category alongside the KB
//! ones above. It does not exist as an `InvalidIri` site: D6's actual reserved-graph write guard
//! is `ConfigStoreError::ReservedGraphWrite` (`src/lib.rs`), a distinct variant from day one, not
//! a reuse of `InvalidIri` -- and the raw SPARQL route (`contreforts-config-api/src/routes/graph.rs`,
//! D7) already constructs `ApiError::BadRequest` directly for it, never routing through
//! `ApiError::status`'s `Graph(_)` arms at all. Flagged for a2 rather than silently reinterpreted:
//! either the task description is imprecise here, or there is a fifth call site this sweep missed
//! that raises `InvalidIri` for a reserved-graph write specifically -- not found by this grep.
//! `contreforts-config-api/tests/kg_instance_conflict_status.rs` adds a defensive 400 status pin
//! for `ConfigStoreError::ReservedGraphWrite` regardless (it currently falls through
//! `ApiError::status`'s catch-all to 500, unreachable via HTTP today but a footgun identical in
//! shape to the one D4's own module doc already warns about), which is the closest genuine gap
//! this investigation turned up near that description.
//!
//! ## New variant names this file commits to
//!
//! `ConfigGraphError::KbInstanceUnregistered`, `::KgInstanceAmbiguous`,
//! `::KbGraphPrefixViolation`, `::KbGraphReferencedElsewhere` -- struct variants (exact field
//! names are a2's call; every match below uses `{ .. }` so it does not lock field names, only the
//! variant identity). Chosen as the direct PascalCase of the free functions that already build
//! today's `InvalidIri` value for each case, and matching the existing `KgInstanceLabelConflict`/
//! `KgInstancePrefixConflict`/`KgInstanceDatadirConflict` naming precedent in the same enum.
//!
//! None of these four variants exist yet -- this file does not compile against `develop` at
//! `a7c82bc`. Sanctioned compile-error RED (`crates/contreforts-kg/CONTRIBUTING.md` §3); see the
//! task report for the verbatim `cargo test` failure text.

use contreforts_config::{
    AgentConfig, CompanyConfig, ConfigGraph, ConfigGraphError, ConfigStore, ForgejoConnectorConfig,
    KgInstanceConfig, KnowledgeBaseConfig,
};
use contreforts_declaration::ConnectorDeclarations;

fn store() -> (tempfile::TempDir, ConfigStore) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config_store");
    let store = ConfigStore::open(&path).expect("store opens at a fresh path");
    (dir, store)
}

fn register_instance(cg: &ConfigGraph<'_>, label: &str, iri_prefix: &str) {
    cg.set_kg_instance(&KgInstanceConfig {
        label: label.to_string(),
        iri_prefix: iri_prefix.to_string(),
        datadir: Some(format!("/var/lib/contreforts/kg-instances/{label}")),
    })
    .expect("registering the instance succeeds");
}

/// `resolve_kg_instance_label` naming an unregistered `kg_instance_label` must come back as
/// `KbInstanceUnregistered`, not `InvalidIri` -- proving the rename actually happened, not merely
/// that *some* error was raised (any `InvalidIri`-returning guard would already pass a test that
/// only checked `is_err()`).
#[test]
fn a_kb_naming_an_unregistered_instance_is_kb_instance_unregistered_not_invalid_iri() {
    let (_dir, store) = store();
    let cg = ConfigGraph::new(&store, ConnectorDeclarations::none());
    cg.add_company(&CompanyConfig {
        slug: "acme".to_string(),
        name: "Acme".to_string(),
    })
    .expect("company registers cleanly");

    let err = cg
        .set_knowledge_base(
            "acme",
            &KnowledgeBaseConfig {
                label: "support".to_string(),
                kg_instance_label: Some("nonexistent".to_string()),
                graph: None,
                vector_store_label: "vs".to_string(),
            },
        )
        .expect_err("naming an unregistered instance must be rejected");

    assert!(
        matches!(err, ConfigGraphError::KbInstanceUnregistered { .. }),
        "expected ConfigGraphError::KbInstanceUnregistered, got: {err:?}"
    );
}

/// `resolve_kg_instance_label`'s ambiguous-`None` case must come back as `KgInstanceAmbiguous`,
/// not `InvalidIri`.
#[test]
fn a_kb_with_no_instance_named_while_several_are_registered_is_kg_instance_ambiguous_not_invalid_iri()
 {
    let (_dir, store) = store();
    let cg = ConfigGraph::new(&store, ConnectorDeclarations::none());
    register_instance(
        &cg,
        "alpha",
        "https://contreforts.ds-labs.org/data/instance/evi-alpha/",
    );
    register_instance(
        &cg,
        "beta",
        "https://contreforts.ds-labs.org/data/instance/evi-beta/",
    );
    cg.add_company(&CompanyConfig {
        slug: "acme".to_string(),
        name: "Acme".to_string(),
    })
    .expect("company registers cleanly");

    let err = cg
        .set_knowledge_base(
            "acme",
            &KnowledgeBaseConfig {
                label: "support".to_string(),
                kg_instance_label: None,
                graph: None,
                vector_store_label: "vs".to_string(),
            },
        )
        .expect_err("an unresolved instance with several registered must be rejected");

    assert!(
        matches!(err, ConfigGraphError::KgInstanceAmbiguous { .. }),
        "expected ConfigGraphError::KgInstanceAmbiguous, got: {err:?}"
    );
}

/// The prefix guard's rejection must come back as `KbGraphPrefixViolation`, not `InvalidIri`.
#[test]
fn a_kb_graph_outside_its_instances_prefix_is_kb_graph_prefix_violation_not_invalid_iri() {
    let (_dir, store) = store();
    let cg = ConfigGraph::new(&store, ConnectorDeclarations::none());
    register_instance(
        &cg,
        "primary",
        "https://contreforts.ds-labs.org/data/instance/evi-primary/",
    );
    register_instance(
        &cg,
        "other",
        "https://contreforts.ds-labs.org/data/instance/evi-other/",
    );
    cg.add_company(&CompanyConfig {
        slug: "acme".to_string(),
        name: "Acme".to_string(),
    })
    .expect("company registers cleanly");

    let err = cg
        .set_knowledge_base(
            "acme",
            &KnowledgeBaseConfig {
                label: "support".to_string(),
                kg_instance_label: Some("primary".to_string()),
                graph: Some(
                    "https://contreforts.ds-labs.org/data/instance/evi-other/entity/1".to_string(),
                ),
                vector_store_label: "vs".to_string(),
            },
        )
        .expect_err("a graph under a different instance's prefix must be rejected");

    assert!(
        matches!(err, ConfigGraphError::KbGraphPrefixViolation { .. }),
        "expected ConfigGraphError::KbGraphPrefixViolation, got: {err:?}"
    );
}

/// The KB-reference guard's rejection must come back as `KbGraphReferencedElsewhere`, not
/// `InvalidIri` -- checked through all three call sites (`write_connector`,
/// `set_connector_target_kb`, `set_agent`) so a rename that only touched one of the three would
/// still fail this file.
#[test]
fn a_connector_field_naming_a_registered_kb_graph_is_kb_graph_referenced_elsewhere_not_invalid_iri()
{
    let (_dir, store) = store();
    let cg = ConfigGraph::new(&store, ConnectorDeclarations::none());
    register_instance(
        &cg,
        "primary",
        "https://contreforts.ds-labs.org/data/instance/evi-wc/",
    );
    cg.add_company(&CompanyConfig {
        slug: "acme".to_string(),
        name: "Acme".to_string(),
    })
    .expect("company registers cleanly");
    let kb_graph = "https://contreforts.ds-labs.org/data/instance/evi-wc/entity/1".to_string();
    cg.set_knowledge_base(
        "acme",
        &KnowledgeBaseConfig {
            label: "support".to_string(),
            kg_instance_label: Some("primary".to_string()),
            graph: Some(kb_graph.clone()),
            vector_store_label: "vs".to_string(),
        },
    )
    .expect("registering the KB succeeds");

    let err = cg
        .set_forgejo_connector(
            "acme",
            &ForgejoConnectorConfig {
                label: "main".to_string(),
                url: kb_graph.clone(),
                token: "tok".to_string(),
            },
        )
        .expect_err("a connector field verbatim-equal to a registered KB graph must be rejected");
    assert!(
        matches!(err, ConfigGraphError::KbGraphReferencedElsewhere { .. }),
        "expected ConfigGraphError::KbGraphReferencedElsewhere from write_connector, got: {err:?}"
    );

    cg.set_forgejo_connector(
        "acme",
        &ForgejoConnectorConfig {
            label: "main".to_string(),
            url: "https://git.example.com".to_string(),
            token: "tok".to_string(),
        },
    )
    .expect("a clean connector registers so set_connector_target_kb has something to link");

    let err = cg
        .set_connector_target_kb("acme", "forgejo", Some("main"), &kb_graph)
        .expect_err("a Target-KB link given a graph IRI instead of a label must be rejected");
    assert!(
        matches!(err, ConfigGraphError::KbGraphReferencedElsewhere { .. }),
        "expected ConfigGraphError::KbGraphReferencedElsewhere from set_connector_target_kb, got: \
         {err:?}"
    );

    let err = cg
        .set_agent(
            "acme",
            &AgentConfig {
                label: "bot".to_string(),
                display_name: None,
                knowledge_base_label: kb_graph.clone(),
                channels: vec![],
            },
        )
        .expect_err("an Agent given a graph IRI instead of a label must be rejected");
    assert!(
        matches!(err, ConfigGraphError::KbGraphReferencedElsewhere { .. }),
        "expected ConfigGraphError::KbGraphReferencedElsewhere from set_agent, got: {err:?}"
    );
}
