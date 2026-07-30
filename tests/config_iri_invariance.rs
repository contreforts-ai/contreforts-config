//! The constraint that makes D4 safe (contreforts/contreforts-workspace#58, comment 7936):
//! `DATA_NS` roots two families of IRI, and only the entity/instance-data family (`doc_iri`,
//! `doctype_iri`, `field_iri`, `consolidated_doc_iri`, `company_graph_iri`) may be re-prefixed by
//! instance assignment. Config-graph records -- `company_iri`, `connector_iri`,
//! `knowledge_base_iri`, `agent_iri`, `sparql_template_iri` -- are hand-entered and re-derivable
//! by nothing; re-prefixing them would silently orphan every company, connector and credential in
//! the store, reintroducing exactly the failure D2 rejected when it removed the `./config_store`
//! CWD fallback.
//!
//! This must be pinned by a real test, not left true by accident: it must fail loudly if a future
//! change threads an instance's prefix through the wrong builder. `ConfigGraph::set_kg_instance`
//! does not exist yet, so this file does not compile against current `develop` -- sanctioned
//! compile-error RED, same as this crate's other new D4 test files.

use contreforts_config::{
    AgentConfig, ChannelRef, CompanyConfig, ConfigGraph, ConfigStore, ForgejoConnectorConfig,
    KgInstanceConfig, KnowledgeBaseConfig, SparqlTemplateConfig,
};
use contreforts_core::namespaces::{self, CONFIG_GRAPH, DATA_NS};
use contreforts_declaration::ConnectorDeclarations;

fn store() -> (tempfile::TempDir, ConfigStore) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config_store");
    let store = ConfigStore::open(&path).expect("store opens at a fresh path");
    (dir, store)
}

/// Registers a KG instance with a deliberately alien prefix (sharing no substring with `DATA_NS`,
/// so any leak is unmistakable rather than a subtle diff), writes one record of every
/// config-graph kind 7936's table names, and asserts every one of their IRIs is still rooted at
/// the fixed `DATA_NS` -- both as returned by the namespace builder itself, and as actually
/// present in the store under that exact subject.
#[test]
fn config_graph_iris_are_unaffected_by_instance_assignment() {
    let (_dir, store) = store();
    let cg = ConfigGraph::new(&store, ConnectorDeclarations::none());

    let alien_prefix = "https://an-instance-prefix-must-never-leak-here.example/inst-42/";
    cg.set_kg_instance(&KgInstanceConfig {
        label: "decoy".to_string(),
        iri_prefix: alien_prefix.to_string(),
    })
    .expect("registering an instance succeeds");

    cg.add_company(&CompanyConfig {
        slug: "acme".to_string(),
        name: "Acme".to_string(),
    })
    .expect("company registers cleanly");
    cg.set_forgejo_connector(
        "acme",
        &ForgejoConnectorConfig {
            label: "main".to_string(),
            url: "https://git.example.com".to_string(),
            token: "tok".to_string(),
        },
    )
    .expect("connector registers cleanly");
    cg.set_knowledge_base(
        "acme",
        &KnowledgeBaseConfig {
            label: "support".to_string(),
            graph: None,
            vector_store_label: "vs".to_string(),
        },
    )
    .expect("knowledge base registers cleanly");
    cg.set_agent(
        "acme",
        &AgentConfig {
            label: "assistant".to_string(),
            display_name: None,
            knowledge_base_label: "support".to_string(),
            channels: vec![ChannelRef {
                kind: "matrix".to_string(),
                label: "main".to_string(),
            }],
        },
    )
    .expect("agent registers cleanly");
    cg.set_sparql_template(
        "acme",
        &SparqlTemplateConfig {
            label: "top-customers".to_string(),
            description: "top customers by revenue".to_string(),
            pattern: "SELECT * WHERE { ?s ?p ?o }".to_string(),
        },
    )
    .expect("sparql template registers cleanly");

    let company_iri = namespaces::company_iri("acme");
    let connector_iri = namespaces::connector_iri("forgejo", "acme", Some("main"));
    let kb_iri = namespaces::knowledge_base_iri("acme", "support");
    let agent_iri = namespaces::agent_iri("acme", "assistant");
    let template_iri = namespaces::sparql_template_iri("acme", "top-customers");

    for (name, iri) in [
        ("company_iri", &company_iri),
        ("connector_iri", &connector_iri),
        ("knowledge_base_iri", &kb_iri),
        ("agent_iri", &agent_iri),
        ("sparql_template_iri", &template_iri),
    ] {
        assert!(
            iri.starts_with(DATA_NS),
            "{name} must stay rooted at the fixed DATA_NS regardless of any instance's assigned \
             prefix, got {iri:?}"
        );
        assert!(
            !iri.starts_with(alien_prefix),
            "{name} must never be re-prefixed by an instance's assigned prefix -- config-graph \
             records are hand-entered and not re-derivable, so re-prefixing them would silently \
             orphan every company, connector and credential in the store \
             (contreforts-workspace#58, comment 7936). Got {iri:?}"
        );
    }

    // Same claim, checked against what the write path actually stored -- not just what the free
    // function returns -- so this would also catch a future `ConfigGraph` that resolves an
    // "active instance" and rewrites the subject at write time instead of trusting the namespace
    // builder's own IRI.
    for (name, iri) in [
        ("Company", &company_iri),
        ("connector", &connector_iri),
        ("KnowledgeBase", &kb_iri),
        ("Agent", &agent_iri),
        ("SparqlTemplate", &template_iri),
    ] {
        let sparql = format!("SELECT ?p ?o WHERE {{ GRAPH <{CONFIG_GRAPH}> {{ <{iri}> ?p ?o }} }}");
        let rows = store
            .select(&sparql)
            .expect("wildcard triple query succeeds");
        assert!(
            !rows.is_empty(),
            "expected the {name} record to actually be stored under {iri}, proving this is the \
             real subject the write path used -- found nothing"
        );
    }
}
