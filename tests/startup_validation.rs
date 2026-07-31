//! D5's second enforcement point (#18 Q3, #19 O2 answered identically): a write-time-only guard
//! is bypassable through the raw SPARQL update route, which is unrestricted today
//! (`contreforts-config-api/src/routes/graph.rs`). This file proves the startup validation pass
//! catches what the guarded write path in `tests/kb_graph_prefix_guard.rs` and
//! `tests/kb_reference_guard.rs` would have rejected, when the store's contents are corrupted by
//! writing directly through `ConfigStore::inner()` -- exactly the shape of write that route
//! performs, and exactly what D5's own design work names as the reason a write-time-only guard
//! is not enough.
//!
//! Per the task's own trap warning ("a guard that examines nothing still reports success"):
//! every rejection test here is paired with a clean-store control proving the same pass reports
//! *no* violations when there genuinely are none -- a pass that always failed would trivially
//! "catch" the corrupted cases too.
//!
//! `ConfigGraph::validate_startup` does not exist yet, and neither does
//! `KnowledgeBaseConfig::kg_instance_label` -- this file does not compile against `develop`.
//! Sanctioned compile-error RED, same as this crate's other new D5/D6 files.

use contreforts_config::{
    AgentConfig, CompanyConfig, ConfigGraph, ConfigStore, ForgejoConnectorConfig, KgInstanceConfig,
    KnowledgeBaseConfig,
};
use contreforts_core::namespaces::{CONFIG_GRAPH, CORE_NS};
use contreforts_declaration::ConnectorDeclarations;
use oxigraph::model::{GraphName, Literal, NamedNode, Term};

fn store() -> (tempfile::TempDir, ConfigStore) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config_store");
    let store = ConfigStore::open(&path).expect("store opens at a fresh path");
    (dir, store)
}

const PRIMARY_PREFIX: &str = "https://contreforts.ds-labs.org/data/instance/startup-a1/";
const OTHER_PREFIX: &str = "https://contreforts.ds-labs.org/data/instance/startup-b2/";

fn setup(cg: &ConfigGraph<'_>) {
    cg.set_kg_instance(&KgInstanceConfig {
        label: "primary".to_string(),
        iri_prefix: PRIMARY_PREFIX.to_string(),
        // D8 part 1 (contreforts-workspace#58): datadir is a new, required field, unrelated to
        // startup validation's own guard, distinct per instance for the same reason given in
        // tests/kb_graph_prefix_guard.rs.
        datadir: Some("/var/lib/contreforts/kg-instances/startup-primary".to_string()),
    })
    .expect("registering the primary instance succeeds");
    cg.set_kg_instance(&KgInstanceConfig {
        label: "other".to_string(),
        iri_prefix: OTHER_PREFIX.to_string(),
        datadir: Some("/var/lib/contreforts/kg-instances/startup-other".to_string()),
    })
    .expect("registering the other instance succeeds");
    cg.add_company(&CompanyConfig {
        slug: "acme".to_string(),
        name: "Acme".to_string(),
    })
    .expect("company registers cleanly");
}

/// A store with nothing wrong in it must report no violations -- proves the pass actually
/// examines the store's real contents and reaches a real "everything is fine" conclusion, not
/// merely one it happens never to contradict.
#[test]
fn validate_startup_reports_no_violations_for_a_clean_store() {
    let (_dir, store) = store();
    let cg = ConfigGraph::new(&store, ConnectorDeclarations::none());
    setup(&cg);

    cg.set_knowledge_base(
        "acme",
        &KnowledgeBaseConfig {
            label: "support".to_string(),
            kg_instance_label: Some("primary".to_string()),
            graph: Some(format!("{PRIMARY_PREFIX}entity/1")),
            vector_store_label: "vs".to_string(),
        },
    )
    .expect("registering a clean KB succeeds");
    cg.set_forgejo_connector(
        "acme",
        &ForgejoConnectorConfig {
            label: "main".to_string(),
            url: "https://git.example.com".to_string(),
            token: "tok".to_string(),
        },
    )
    .expect("connector registers cleanly");
    cg.set_connector_target_kb("acme", "forgejo", Some("main"), "support")
        .expect("a legitimate label-based link succeeds");

    let result = cg.validate_startup();
    assert!(
        result.is_ok(),
        "a store with no invariant violations must validate cleanly at startup, got: {result:?}"
    );
}

/// Corrupts a KB's stored `graphIri` literal directly through `ConfigStore::inner()` -- bypassing
/// `set_knowledge_base`'s own write-time guard entirely, exactly as the unrestricted raw SPARQL
/// update route would -- so the KB ends up claiming instance `primary` while its graph actually
/// falls under `other`'s prefix. `validate_startup` must catch this even though it never went
/// through the guarded write path, and must name the KB, the corrupted graph IRI, and the
/// instance whose prefix it violates.
#[test]
fn validate_startup_catches_a_kb_graph_corrupted_via_the_raw_store() {
    let (_dir, store) = store();
    let cg = ConfigGraph::new(&store, ConnectorDeclarations::none());
    setup(&cg);

    cg.set_knowledge_base(
        "acme",
        &KnowledgeBaseConfig {
            label: "support".to_string(),
            kg_instance_label: Some("primary".to_string()),
            graph: Some(format!("{PRIMARY_PREFIX}entity/1")),
            vector_store_label: "vs".to_string(),
        },
    )
    .expect("registering the (initially clean) KB succeeds");

    // Bypass: overwrite the KB's `graphIri` literal directly, exactly as the unrestricted raw
    // SPARQL update route could, with no involvement of `set_knowledge_base`'s own guard.
    let kb_iri = contreforts_core::namespaces::knowledge_base_iri("acme", "support");
    let kb_node = NamedNode::new(&kb_iri).expect("valid IRI");
    let graph_pred = NamedNode::new(format!("{CORE_NS}graphIri")).expect("valid IRI");
    let config_graph = NamedNode::new(CONFIG_GRAPH).expect("valid IRI");
    let corrupted_graph = format!("{OTHER_PREFIX}entity/1");

    store
        .remove_quad(
            &kb_node,
            &graph_pred,
            &Term::Literal(Literal::new_simple_literal(format!(
                "{PRIMARY_PREFIX}entity/1"
            ))),
            &config_graph,
        )
        .expect("removing the original graphIri literal succeeds");
    store
        .inner()
        .insert(&oxigraph::model::Quad::new(
            kb_node.clone(),
            graph_pred.clone(),
            Term::Literal(Literal::new_simple_literal(&corrupted_graph)),
            GraphName::NamedNode(config_graph.clone()),
        ))
        .expect("inserting the corrupted graphIri literal directly succeeds");

    // Sanity: the corruption really is invisible to the guarded read path used elsewhere --
    // `get_knowledge_base` has no reason to itself re-validate on every read.
    let got = cg
        .get_knowledge_base("acme", "support")
        .expect("lookup succeeds")
        .expect("the KB is still there");
    assert_eq!(got.graph.as_deref(), Some(corrupted_graph.as_str()));

    let violations = cg.validate_startup().expect_err(
        "a KB whose graph was corrupted to fall under a foreign instance's prefix \
                     must be reported at startup, even though it never went through the \
                     guarded write path",
    );

    let combined = violations.join("\n");
    assert!(
        combined.contains("support"),
        "the report must name the offending KB, got: {combined:?}"
    );
    assert!(
        combined.contains(&corrupted_graph),
        "the report must name the offending graph IRI, got: {combined:?}"
    );
    assert!(
        combined.contains("primary"),
        "the report must name the instance whose prefix was violated, got: {combined:?}"
    );
}

/// The second invariant's bypass: corrupts a connector's Target-KB link directly through
/// `ConfigStore::inner()` so its stored value becomes a KB's graph IRI rather than a label --
/// the exact violation `tests/kb_reference_guard.rs` proves is rejected when reached through
/// `set_connector_target_kb` itself. `validate_startup` must catch this too, since the raw
/// SPARQL route can write this predicate exactly as easily as any other.
#[test]
fn validate_startup_catches_a_target_kb_link_corrupted_to_a_graph_iri_via_the_raw_store() {
    let (_dir, store) = store();
    let cg = ConfigGraph::new(&store, ConnectorDeclarations::none());
    setup(&cg);

    let kb_graph = format!("{PRIMARY_PREFIX}entity/1");
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
    cg.set_forgejo_connector(
        "acme",
        &ForgejoConnectorConfig {
            label: "main".to_string(),
            url: "https://git.example.com".to_string(),
            token: "tok".to_string(),
        },
    )
    .expect("connector registers cleanly");
    cg.set_connector_target_kb("acme", "forgejo", Some("main"), "support")
        .expect("the legitimate, label-based link succeeds");

    // Bypass: overwrite the stored target-KB literal to the KB's graph IRI directly, exactly as
    // the unrestricted raw SPARQL update route could -- `set_connector_target_kb` itself would
    // have rejected this (see `tests/kb_reference_guard.rs`).
    let conn_iri = contreforts_core::namespaces::connector_iri("forgejo", "acme", Some("main"));
    let conn_node = NamedNode::new(&conn_iri).expect("valid IRI");
    let target_pred = NamedNode::new(format!("{CORE_NS}targetKnowledgeBase")).expect("valid IRI");
    let config_graph = NamedNode::new(CONFIG_GRAPH).expect("valid IRI");

    store
        .remove_quad(
            &conn_node,
            &target_pred,
            &Term::Literal(Literal::new_simple_literal("support")),
            &config_graph,
        )
        .expect("removing the original label literal succeeds");
    store
        .inner()
        .insert(&oxigraph::model::Quad::new(
            conn_node,
            target_pred,
            Term::Literal(Literal::new_simple_literal(&kb_graph)),
            GraphName::NamedNode(config_graph),
        ))
        .expect("inserting the corrupted graph-IRI literal directly succeeds");

    let violations = cg.validate_startup().expect_err(
        "a Target-KB link corrupted to hold a KB's graph IRI instead of its label must be \
         reported at startup, even though it never went through the guarded write path",
    );

    let combined = violations.join("\n");
    assert!(
        combined.contains(&kb_graph),
        "the report must name the offending graph IRI found outside its own KB's definition, \
         got: {combined:?}"
    );
}

/// Review addendum: `Agent` is not a connector kind, so it is not in `ALL_CONNECTOR_DESCRIPTORS`
/// and the second invariant's original scan never examined `Agent`-typed subjects at all --
/// `ConfigGraph::set_agent`'s `knowledge_base_label` names a KB exactly the way the Target-KB link
/// does, and the same raw-store corruption reproduced here for it must be caught the same way.
#[test]
fn validate_startup_catches_an_agent_knowledge_base_label_corrupted_to_a_graph_iri_via_the_raw_store()
 {
    let (_dir, store) = store();
    let cg = ConfigGraph::new(&store, ConnectorDeclarations::none());
    setup(&cg);

    let kb_graph = format!("{PRIMARY_PREFIX}entity/1");
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
    cg.set_agent(
        "acme",
        &AgentConfig {
            label: "bot".to_string(),
            display_name: None,
            knowledge_base_label: "support".to_string(),
            channels: vec![],
        },
    )
    .expect("the legitimate, label-based agent registration succeeds");

    // Bypass: overwrite the stored `usesKnowledgeBase` literal to the KB's graph IRI directly,
    // exactly as the unrestricted raw SPARQL update route could -- `set_agent` itself would have
    // rejected this (see `tests/kb_reference_guard.rs`).
    let agent_iri = contreforts_core::namespaces::agent_iri("acme", "bot");
    let agent_node = NamedNode::new(&agent_iri).expect("valid IRI");
    let uses_kb_pred = NamedNode::new(format!("{CORE_NS}usesKnowledgeBase")).expect("valid IRI");
    let config_graph = NamedNode::new(CONFIG_GRAPH).expect("valid IRI");

    store
        .remove_quad(
            &agent_node,
            &uses_kb_pred,
            &Term::Literal(Literal::new_simple_literal("support")),
            &config_graph,
        )
        .expect("removing the original label literal succeeds");
    store
        .inner()
        .insert(&oxigraph::model::Quad::new(
            agent_node,
            uses_kb_pred,
            Term::Literal(Literal::new_simple_literal(&kb_graph)),
            GraphName::NamedNode(config_graph),
        ))
        .expect("inserting the corrupted graph-IRI literal directly succeeds");

    let violations = cg.validate_startup().expect_err(
        "an Agent's knowledge_base_label corrupted to hold a KB's graph IRI instead of its \
         label must be reported at startup, even though Agent is not a connector kind",
    );

    let combined = violations.join("\n");
    assert!(
        combined.contains(&kb_graph),
        "the report must name the offending graph IRI found outside its own KB's definition, \
         got: {combined:?}"
    );
}
