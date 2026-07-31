//! The Target-KB link (contreforts/contreforts-workspace#58 D4; #18 point 3): a connector's
//! config gains a link to the knowledge-base instance it targets. One-directional -- a connector
//! names its target KB, a KB never names its connectors -- because that direction is what stays
//! safe once config and KG data are separate stores (#18's "why the guard is the crux" section).
//! D4 only establishes the link; D5 builds the write-time/startup guard that enforces the
//! direction against *other* IRI families (foreign-instance graphs, the reserved product graph).
//! This file only proves the link itself round-trips and that no reverse edge is ever written.
//!
//! `ConfigGraph::{set,get}_connector_target_kb` do not exist yet -- sanctioned compile-error RED.

use contreforts_config::{
    CompanyConfig, ConfigGraph, ConfigStore, ForgejoConnectorConfig, KnowledgeBaseConfig,
};
use contreforts_core::namespaces::{self, CONFIG_GRAPH};
use contreforts_declaration::ConnectorDeclarations;

fn store() -> (tempfile::TempDir, ConfigStore) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config_store");
    let store = ConfigStore::open(&path).expect("store opens at a fresh path");
    (dir, store)
}

/// `(predicate, plain object)` pairs stored for `subject_iri` in `CONFIG_GRAPH`, sorted. Same
/// shape as `tests/config_graph.rs`'s own `stored_triples` helper, duplicated here rather than
/// shared because each test file in this crate is self-contained (matching that file's own
/// pattern).
fn stored_triples(store: &ConfigStore, subject_iri: &str) -> Vec<(String, String)> {
    let sparql =
        format!("SELECT ?p ?o WHERE {{ GRAPH <{CONFIG_GRAPH}> {{ <{subject_iri}> ?p ?o }} }}");
    let mut rows: Vec<(String, String)> = store
        .select(&sparql)
        .expect("wildcard triple query succeeds")
        .into_iter()
        .map(|row| {
            let p = row.iter().find(|(k, _)| k == "p").unwrap().1.clone();
            let o = row.iter().find(|(k, _)| k == "o").unwrap().1.clone();
            (p, o)
        })
        .collect();
    rows.sort();
    rows
}

fn setup_company_kb_and_connector(cg: &ConfigGraph<'_>) {
    cg.add_company(&CompanyConfig {
        slug: "acme".to_string(),
        name: "Acme".to_string(),
    })
    .expect("company registers cleanly");
    cg.set_knowledge_base(
        "acme",
        &KnowledgeBaseConfig {
            label: "support".to_string(),
            kg_instance_label: None,
            graph: None,
            vector_store_label: "primary-vs".to_string(),
        },
    )
    .expect("knowledge base registers cleanly");
    cg.set_forgejo_connector(
        "acme",
        &ForgejoConnectorConfig {
            label: "main".to_string(),
            url: "https://git.example.com".to_string(),
            token: "tok".to_string(),
        },
    )
    .expect("connector registers cleanly");
}

#[test]
fn a_connector_names_its_target_kb_and_the_link_round_trips() {
    let (_dir, store) = store();
    let cg = ConfigGraph::new(&store, ConnectorDeclarations::none());
    setup_company_kb_and_connector(&cg);

    cg.set_connector_target_kb("acme", "forgejo", Some("main"), "support")
        .expect("linking a connector to a KB that already exists must succeed");

    let target = cg
        .get_connector_target_kb("acme", "forgejo", Some("main"))
        .expect("lookup succeeds")
        .expect("the link just set is found");

    assert_eq!(
        target, "support",
        "reading the target-KB link back must return exactly the label that was set"
    );
}

#[test]
fn a_connector_with_no_target_kb_set_reports_none() {
    let (_dir, store) = store();
    let cg = ConfigGraph::new(&store, ConnectorDeclarations::none());
    setup_company_kb_and_connector(&cg);

    let target = cg
        .get_connector_target_kb("acme", "forgejo", Some("main"))
        .expect("lookup succeeds even when nothing was ever linked");

    assert!(
        target.is_none(),
        "a connector that was never linked to a KB must report no target, got {target:?}"
    );
}

/// Direction is legible: the KB's own record must never name the connector that targets it.
/// Asserted directly against the store's triples for the KB's subject IRI, and again with an
/// explicit reverse-pattern query -- not merely "the getter doesn't expose it", which would pass
/// even if a reverse edge existed and was simply unused.
#[test]
fn the_knowledge_base_does_not_name_its_connectors() {
    let (_dir, store) = store();
    let cg = ConfigGraph::new(&store, ConnectorDeclarations::none());
    setup_company_kb_and_connector(&cg);

    cg.set_connector_target_kb("acme", "forgejo", Some("main"), "support")
        .expect("linking succeeds");

    let kb_iri = namespaces::knowledge_base_iri("acme", "support");
    let connector_iri = namespaces::connector_iri("forgejo", "acme", Some("main"));

    let kb_triples = stored_triples(&store, &kb_iri);
    assert!(
        !kb_triples.is_empty(),
        "sanity check: the KB record itself must have some stored triples"
    );
    assert!(
        kb_triples.iter().all(|(_, o)| o != &connector_iri),
        "the KB's own record must never carry the connector's IRI as an object of any of its \
         triples -- found it in {kb_triples:?}"
    );

    // Same claim, checked with a direct reverse-pattern query rather than through the forward
    // triples of the KB alone -- covers a reverse edge stored under a predicate that happens not
    // to have the KB itself as the *only* subject touched (e.g. a separate reverse-index triple).
    let reverse_sparql = format!(
        "SELECT ?p WHERE {{ GRAPH <{CONFIG_GRAPH}> {{ <{kb_iri}> ?p <{connector_iri}> }} }}"
    );
    let reverse_rows = store
        .select(&reverse_sparql)
        .expect("wildcard reverse-edge query succeeds");
    assert!(
        reverse_rows.is_empty(),
        "found a reverse edge from the KB to the connector under predicate(s) {reverse_rows:?} \
         -- the Target-KB link must be one-directional: a connector names its target KB, a KB \
         never names its connectors"
    );
}
