//! Behavioural contract for the ported config-graph engine (contreforts/contreforts-workspace#58,
//! comment 7904, item D3c): `ConnectorDescriptor`, `write_connector`, the generic get/list
//! machinery, the 11 `*ConnectorConfig` structs and their thin per-kind wrappers, running
//! against `contreforts_config::ConfigStore` instead of `contreforts_kg::GraphStore`.
//!
//! `contreforts-kg/src/config_graph.rs`'s own 1,140-line `#[cfg(test)]` module (`:2538` on) is
//! the existing behavioural contract this file restates from the new crate's side. Every test
//! below says, in its own doc comment, whether it is ported verbatim/adapted from that module,
//! ported from the separate integration file `crates/contreforts-kg/tests/config_graph.rs`, or
//! new coverage this port adds.
//!
//! Two of the original inline module's tests do **not** appear here at all:
//! `connector_write_validates_exactly_what_it_writes` and
//! `connector_instance_graph_matches_typed_write` call the private helper
//! `ConfigGraph::connector_instance_graph` directly -- unreachable from an external `tests/`
//! crate. See this crate's PR description / the task report for why that is flagged for a2
//! rather than worked around here.
//!
//! Also not ported: `measured_per_write_validation_cost` (a perf measurement, not a behavioural
//! contract; its own doc comment says the numbers live in a PR description, not in `assert!`).
//!
//! This file does not compile against the current `develop` -- `contreforts_config::ConfigGraph`
//! does not exist yet. That is the sanctioned RED (`crates/contreforts-kg/CONTRIBUTING.md` §3).

use contreforts_config::{
    AgentConfig, ChannelRef, CompanyConfig, ConfigGraph, ConfigStore, ConnectorConfig,
    ForgejoConnectorConfig, KnowledgeBaseConfig, O365ConnectorAuth, O365ConnectorConfig,
    PennylaneConnectorConfig, SmtpConnectorConfig, SmtpTlsMode, SparqlTemplateConfig,
    StalwartConnectorConfig, VectorStoreColumnType, VectorStoreConnectorConfig, VectorStoreKind,
    all_connector_kinds,
};
use contreforts_core::namespaces::{self, CONFIG_GRAPH, CORE_NS, RDF};
use contreforts_declaration::{ConnectorDeclarations, ConnectorValidator};
use oxigraph::model::{GraphName, Literal, NamedNode, Quad, Term};

// ── Fixtures: real declarations, reproduced as plain Turtle ─────────────────────────────────
//
// `contreforts-config` cannot depend on the connector crates (any more than `contreforts-kg`
// could -- see `connector_validation`'s own module docs) -- these are reproduced here as plain
// Turtle, the way any composition root would hand them in. Trimmed of `sh:name`/
// `sh:description`/`sh:order`/`sh:group`/comments, same trimming style
// `crates/contreforts-kg/tests/config_graph.rs`'s own `FORGEJO_DECLARATION_TTL`/
// `VECTOR_STORE_DECLARATION_TTL` use -- `sh:targetClass`, every `sh:path`, and every
// `sh:datatype`/`sh:pattern`/count constraint that write_connector's validated instance is
// actually checked against are kept byte-faithful to the source file cited in each comment.
//
// Per this issue's own fixture warning: every IRI below is a full bracketed `<...>` or a
// `prefix:localname` pair with no raw `/` in any local part -- confirmed parseable by every
// test that constructs a `ConnectorValidator` from it (a parse failure would surface as
// `ConnectorValidator::new(...).unwrap()` panicking, not a silent pass).

/// Verbatim reproduction of `contreforts-connector-forgejo/declaration.ttl:185-224`'s
/// `sh:targetClass`/`sh:path`/`sh:datatype`/`sh:pattern` -- identical in content to
/// `crates/contreforts-kg/tests/config_graph.rs`'s own `FORGEJO_DECLARATION_TTL` const.
const FORGEJO_DECLARATION_TTL: &str = r#"
    @prefix sh:      <http://www.w3.org/ns/shacl#> .
    @prefix xsd:     <http://www.w3.org/2001/XMLSchema#> .
    @prefix forgejo: <https://contreforts.ds-labs.org/ontologies/forgejo#> .

    forgejo:ForgejoConnectorShape a sh:NodeShape ;
        sh:targetClass forgejo:ForgejoConnector ;
        sh:property [
            sh:path forgejo:label ;
            sh:datatype xsd:string ;
            sh:minCount 1 ;
            sh:maxCount 1 ;
        ] ;
        sh:property [
            sh:path forgejo:instanceUrl ;
            sh:datatype xsd:string ;
            sh:minCount 1 ;
            sh:maxCount 1 ;
            sh:pattern "^https?://[^\\s]+$" ;
        ] ;
        sh:property [
            sh:path forgejo:token ;
            sh:datatype xsd:string ;
            sh:minCount 1 ;
            sh:maxCount 1 ;
        ] .
"#;

/// A deliberately incomplete "declaration": targets `forgejo:ForgejoConnector` (so
/// `write_connector` resolves forgejo as case 1, declared) but gives `sh:path` for
/// `instanceUrl`/`token` only -- `label`, which `set_forgejo_connector` always writes first, has
/// none. Ported verbatim from `crates/contreforts-kg/src/config_graph.rs`'s inline test module
/// (`FORGEJO_DECLARATION_MISSING_LABEL_TTL`).
const FORGEJO_DECLARATION_MISSING_LABEL_TTL: &str = r#"
    @prefix sh:      <http://www.w3.org/ns/shacl#> .
    @prefix forgejo: <https://contreforts.ds-labs.org/ontologies/forgejo#> .

    forgejo:ForgejoConnectorShape a sh:NodeShape ;
        sh:targetClass forgejo:ForgejoConnector ;
        sh:property [
            sh:path forgejo:instanceUrl ;
            sh:minCount 1 ;
            sh:maxCount 1 ;
        ] ;
        sh:property [
            sh:path forgejo:token ;
            sh:minCount 1 ;
            sh:maxCount 1 ;
        ] .
"#;

/// Verbatim (content-preserving, comments/labels/order/groups trimmed) reproduction of
/// `contreforts-connector-stalwart/declaration.ttl:109-387`'s `sh:targetClass` and all 16
/// `sh:path`/`sh:datatype`/count constraints, in `set_stalwart_connector`'s own write order.
/// Stalwart is real-world's first declaration with numeric fields (contreforts-kg#26/D16), which
/// is exactly why it doubles here as this port's second real-declaration `write_connector` round
/// trip *and* the struct-round-trip coverage item 5 asks for.
const STALWART_DECLARATION_TTL: &str = r#"
    @prefix sh:       <http://www.w3.org/ns/shacl#> .
    @prefix xsd:      <http://www.w3.org/2001/XMLSchema#> .
    @prefix stalwart: <https://contreforts.ds-labs.org/ontologies/stalwart#> .

    stalwart:StalwartConnectorShape a sh:NodeShape ;
        sh:targetClass stalwart:StalwartConnector ;
        sh:property [ sh:path stalwart:label ; sh:datatype xsd:string ; sh:minCount 1 ; sh:maxCount 1 ] ;
        sh:property [
            sh:path stalwart:jmapBaseUrl ; sh:datatype xsd:string ;
            sh:minCount 1 ; sh:maxCount 1 ; sh:pattern "^https?://[^\\s]+$" ;
        ] ;
        sh:property [ sh:path stalwart:adminUser ; sh:datatype xsd:string ; sh:minCount 1 ; sh:maxCount 1 ] ;
        sh:property [ sh:path stalwart:adminPass ; sh:datatype xsd:string ; sh:minCount 1 ; sh:maxCount 1 ] ;
        sh:property [
            sh:path stalwart:listenPort ; sh:datatype xsd:integer ;
            sh:minCount 1 ; sh:maxCount 1 ; sh:minInclusive 1 ; sh:maxInclusive 65535 ;
        ] ;
        sh:property [ sh:path stalwart:stateDir ; sh:datatype xsd:string ; sh:minCount 1 ; sh:maxCount 1 ] ;
        sh:property [ sh:path stalwart:dbPath ; sh:datatype xsd:string ; sh:minCount 1 ; sh:maxCount 1 ] ;
        sh:property [ sh:path stalwart:smtpLocalHost ; sh:datatype xsd:string ; sh:minCount 1 ; sh:maxCount 1 ] ;
        sh:property [
            sh:path stalwart:smtpLocalPort ; sh:datatype xsd:integer ;
            sh:minCount 1 ; sh:maxCount 1 ; sh:minInclusive 1 ; sh:maxInclusive 65535 ;
        ] ;
        sh:property [ sh:path stalwart:smtpRelayHost ; sh:datatype xsd:string ; sh:minCount 0 ; sh:maxCount 1 ] ;
        sh:property [
            sh:path stalwart:smtpRelayPort ; sh:datatype xsd:integer ;
            sh:minCount 1 ; sh:maxCount 1 ; sh:minInclusive 1 ; sh:maxInclusive 65535 ;
        ] ;
        sh:property [ sh:path stalwart:imipAnchorDomain ; sh:datatype xsd:string ; sh:minCount 1 ; sh:maxCount 1 ] ;
        sh:property [
            sh:path stalwart:ollamaUrl ; sh:datatype xsd:string ;
            sh:minCount 1 ; sh:maxCount 1 ; sh:pattern "^https?://[^\\s]+$" ;
        ] ;
        sh:property [ sh:path stalwart:ollamaModel ; sh:datatype xsd:string ; sh:minCount 1 ; sh:maxCount 1 ] ;
        sh:property [
            sh:path stalwart:ollamaTimeoutSecs ; sh:datatype xsd:integer ;
            sh:minCount 1 ; sh:maxCount 1 ; sh:minInclusive 1 ; sh:maxInclusive 300 ;
        ] ;
        sh:property [ sh:path stalwart:customer ; sh:datatype xsd:string ; sh:minCount 0 ; sh:maxCount 1 ] .
"#;

/// Deliberately synthetic (no real declaration has a numeric field other than stalwart's, which
/// this file exercises separately): the minimum shape needed to drive the `sh:datatype
/// xsd:integer` read path for `vector_store`. Ported verbatim from
/// `crates/contreforts-kg/src/config_graph.rs`'s inline test module
/// (`VECTOR_STORE_INTEGER_DECLARATION_TTL`).
const VECTOR_STORE_INTEGER_DECLARATION_TTL: &str = r#"
    @prefix sh:  <http://www.w3.org/ns/shacl#> .
    @prefix xsd: <http://www.w3.org/2001/XMLSchema#> .
    @prefix vs:  <https://contreforts.ds-labs.org/ontologies/vectorstore#> .

    vs:VectorStoreConnectorShape a sh:NodeShape ;
        sh:targetClass vs:VectorStoreConnector ;
        sh:property [ sh:path vs:label ; sh:datatype xsd:string ; sh:minCount 1 ; sh:maxCount 1 ] ;
        sh:property [ sh:path vs:vectorStoreKind ; sh:datatype xsd:string ; sh:minCount 1 ; sh:maxCount 1 ] ;
        sh:property [ sh:path vs:instanceUrl ] ;
        sh:property [ sh:path vs:tableName ] ;
        sh:property [ sh:path vs:dimension ; sh:datatype xsd:integer ; sh:minCount 1 ; sh:maxCount 1 ] ;
        sh:property [ sh:path vs:columnType ; sh:datatype xsd:string ; sh:minCount 1 ; sh:maxCount 1 ] ;
        sh:property [ sh:path vs:adminUrl ; sh:datatype xsd:string ; sh:maxCount 1 ] .
"#;

// ── Test scaffolding ─────────────────────────────────────────────────────────

fn store() -> (tempfile::TempDir, ConfigStore) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config_store");
    let store = ConfigStore::open(&path).expect("store opens at a fresh path");
    (dir, store)
}

fn setup() -> (tempfile::TempDir, ConfigStore, &'static str) {
    let (dir, store) = store();
    let cg = ConfigGraph::new(&store, ConnectorDeclarations::none());
    cg.add_company(&CompanyConfig {
        slug: "acme".to_string(),
        name: "Acme".to_string(),
    })
    .unwrap();
    (dir, store, "acme")
}

/// `(predicate, plain object)` pairs stored for `subject_iri` in `CONFIG_GRAPH`, sorted --
/// through `ConfigStore::select`, one of the three ported primitives, the same way
/// `crates/contreforts-kg/tests/config_graph.rs`'s own `triples_for` reads through
/// `QueryEngine::select` rather than store internals.
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

/// `(predicate, datatype IRI)` pairs actually stored for `subject` in `graph` -- reads through
/// `ConfigStore::inner()` directly (the second ported primitive), since `select`'s simplified
/// `(String, String)` rows collapse `"1536"^^xsd:integer` down to the same value string a plain
/// `"1536"` literal would produce, and datatype divergence is exactly what this needs to catch.
fn stored_datatypes(
    store: &ConfigStore,
    subject: &NamedNode,
    graph: &NamedNode,
) -> Vec<(String, String)> {
    let quads: Vec<_> = store
        .inner()
        .quads_for_pattern(Some(subject.into()), None, None, Some(graph.into()))
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    let mut v: Vec<(String, String)> = quads
        .into_iter()
        .filter_map(|q| match q.object {
            Term::Literal(l) => Some((
                q.predicate.as_str().to_string(),
                l.datatype().as_str().to_string(),
            )),
            _ => None,
        })
        .collect();
    v.sort();
    v
}

// ── Company / KnowledgeBase / Agent / SparqlTemplate CRUD ───────────────────────────────────
//
// Foundational, generic-named-graph CRUD every connector helper below builds on
// (`require_company` gates every `write_connector` call). Ported/adapted from
// `crates/contreforts-kg/src/config_graph.rs`'s inline test module.

/// Adapted from the inline module's `add_and_get_company_round_trip` +
/// `add_company_is_idempotent_and_overwrites_name` (contreforts-kg's own integration test file),
/// combined into one round trip.
#[test]
fn company_add_get_list_round_trip_and_is_idempotent() {
    let (_dir, store) = store();
    let cg = ConfigGraph::new(&store, ConnectorDeclarations::none());

    cg.add_company(&CompanyConfig {
        slug: "acme".into(),
        name: "Acme Corp".into(),
    })
    .unwrap();
    let fetched = cg.get_company("acme").unwrap().expect("company present");
    assert_eq!(fetched.slug, "acme");
    assert_eq!(fetched.name, "Acme Corp");

    // Idempotent: calling add_company again with the same slug overwrites the name rather than
    // duplicating the company or erroring.
    cg.add_company(&CompanyConfig {
        slug: "acme".into(),
        name: "Acme Corp (renamed)".into(),
    })
    .unwrap();
    let fetched = cg.get_company("acme").unwrap().unwrap();
    assert_eq!(fetched.name, "Acme Corp (renamed)");

    let list = cg.list_companies().unwrap();
    assert_eq!(
        list.len(),
        1,
        "one add + one overwrite must still be one company: {list:?}"
    );
}

/// Ported near-verbatim from the inline module's `knowledge_base_roundtrip`.
#[test]
fn knowledge_base_round_trip() {
    let (_dir, store, slug) = setup();
    let cg = ConfigGraph::new(&store, ConnectorDeclarations::none());
    let kb = KnowledgeBaseConfig {
        label: "docs".into(),
        graph: Some("http://example.org/code-graph".into()),
        vector_store_label: "primary".into(),
    };
    cg.set_knowledge_base(slug, &kb).unwrap();

    let got = cg.get_knowledge_base(slug, "docs").unwrap().unwrap();
    assert_eq!(got.label, "docs");
    assert_eq!(got.graph.as_deref(), Some("http://example.org/code-graph"));
    assert_eq!(got.vector_store_label, "primary");

    assert_eq!(cg.list_knowledge_bases(slug).unwrap().len(), 1);

    cg.remove_knowledge_base(slug, "docs").unwrap();
    assert!(cg.get_knowledge_base(slug, "docs").unwrap().is_none());
}

/// Ported near-verbatim from the inline module's `agent_roundtrip`.
#[test]
fn agent_round_trip() {
    let (_dir, store, slug) = setup();
    let cg = ConfigGraph::new(&store, ConnectorDeclarations::none());
    let agent = AgentConfig {
        label: "helper".into(),
        display_name: Some("Helper Bot".into()),
        knowledge_base_label: "docs".into(),
        channels: vec![
            ChannelRef {
                kind: "matrix".into(),
                label: "main".into(),
            },
            ChannelRef {
                kind: "smtp".into(),
                label: "noreply".into(),
            },
        ],
    };
    cg.set_agent(slug, &agent).unwrap();

    let got = cg.get_agent(slug, "helper").unwrap().unwrap();
    assert_eq!(got.label, "helper");
    assert_eq!(got.display_name.as_deref(), Some("Helper Bot"));
    assert_eq!(got.knowledge_base_label, "docs");
    assert_eq!(got.channels.len(), 2);
    assert!(
        got.channels
            .iter()
            .any(|c| c.kind == "matrix" && c.label == "main")
    );
    assert!(
        got.channels
            .iter()
            .any(|c| c.kind == "smtp" && c.label == "noreply")
    );

    let list = cg.list_agents(slug).unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].channels.len(), 2);

    cg.remove_agent(slug, "helper").unwrap();
    assert!(cg.get_agent(slug, "helper").unwrap().is_none());
}

/// Ported near-verbatim from the inline module's `sparql_template_roundtrip`.
#[test]
fn sparql_template_round_trip_and_defaults() {
    let (_dir, store, slug) = setup();
    let cg = ConfigGraph::new(&store, ConnectorDeclarations::none());

    cg.ensure_default_templates(slug).unwrap();
    let list = cg.list_sparql_templates(slug).unwrap();
    assert!(list.len() >= 3);
    assert!(list.iter().any(|t| t.label == "find_by_type"));

    let custom = SparqlTemplateConfig {
        label: "custom_filter".into(),
        description: "Custom filter description".into(),
        pattern: "SELECT ?s WHERE { ?s <p> <o> }".into(),
    };
    cg.set_sparql_template(slug, &custom).unwrap();
    let got = cg
        .get_sparql_template(slug, "custom_filter")
        .unwrap()
        .unwrap();
    assert_eq!(got.description, "Custom filter description");

    cg.remove_sparql_template(slug, "custom_filter").unwrap();
    assert!(
        cg.get_sparql_template(slug, "custom_filter")
            .unwrap()
            .is_none()
    );
}

// ── `write_connector` round trip through the generic engine, real declarations ──────────────
//
// Two different connector kinds, each validated against its own *real* declaration (trimmed of
// comments/metadata but content-faithful, per this file's fixture section) rather than a
// synthetic `core:`-namespaced stand-in -- per this issue's fixture warning, a defect earlier in
// this epic was invisible to synthetic fixtures and only surfaced against real ones.

/// Adapted from the inline module's `connector_write_succeeds_when_it_conforms`.
#[test]
fn forgejo_connector_round_trips_through_the_generic_engine_via_its_real_declaration() {
    let (_dir, store, slug) = setup();
    let validator = ConnectorValidator::new(FORGEJO_DECLARATION_TTL, &all_connector_kinds())
        .expect("the real (trimmed) forgejo declaration is valid SHACL");
    let cg = ConfigGraph::with_validator(&store, validator.declarations(), &validator);

    cg.set_forgejo_connector(
        slug,
        &ForgejoConnectorConfig {
            label: "main".into(),
            url: "https://git.example.com".into(),
            token: "tok-abc".into(),
        },
    )
    .unwrap();

    let got = cg
        .get_forgejo_connector(slug, "main")
        .unwrap()
        .expect("connector present");
    assert_eq!(got.label, "main");
    assert_eq!(got.url, "https://git.example.com");
    assert_eq!(got.token, "tok-abc");
    assert_eq!(validator.validated_write_count(), 1);
    assert_eq!(validator.unvalidated_write_count(), 0);

    // The stored class and every field predicate came from the real declaration, not CORE_NS --
    // the actual namespace migration (contreforts-kg#21), not merely "the write didn't error".
    const FORGEJO_NS: &str = "https://contreforts.ds-labs.org/ontologies/forgejo#";
    let conn_iri = namespaces::connector_iri("forgejo", slug, Some("main"));
    let mut expected = vec![
        (
            format!("{RDF}type"),
            format!("{FORGEJO_NS}ForgejoConnector"),
        ),
        (format!("{FORGEJO_NS}label"), "main".to_string()),
        (
            format!("{FORGEJO_NS}instanceUrl"),
            "https://git.example.com".to_string(),
        ),
        (format!("{FORGEJO_NS}token"), "tok-abc".to_string()),
    ];
    expected.sort();
    assert_eq!(stored_triples(&store, &conn_iri), expected);
}

/// The second real-declaration kind (item 2's "at least two different connector kinds"), and
/// simultaneously this port's `StalwartConnectorConfig` struct round trip (item 5): all 16
/// fields, including its four numeric ones, survive write→read, and the numeric fields are
/// actually stored as `xsd:integer` (not a plain/`xsd:string` literal a lexical-only comparison
/// could not distinguish). New test -- no equivalent exists in the inline module (which only
/// exercises forgejo against a real declaration).
#[test]
fn stalwart_connector_round_trips_through_the_generic_engine_via_its_real_declaration() {
    let (_dir, store, slug) = setup();
    let validator = ConnectorValidator::new(STALWART_DECLARATION_TTL, &all_connector_kinds())
        .expect("the real (trimmed) stalwart declaration is valid SHACL");
    let cg = ConfigGraph::with_validator(&store, validator.declarations(), &validator);

    let config = StalwartConnectorConfig {
        label: "default".into(),
        jmap_base_url: "https://mail.example.com".into(),
        admin_user: "admin".into(),
        admin_pass: "s3cret".into(),
        listen_port: 8443,
        state_dir: "/state".into(),
        db_path: "/data/summaries.db".into(),
        smtp_local_host: "127.0.0.1".into(),
        smtp_local_port: 25,
        smtp_relay_host: Some("relay.example.com".into()),
        smtp_relay_port: 587,
        imip_anchor_domain: "mail.example.com".into(),
        ollama_url: "http://localhost:11434".into(),
        ollama_model: "gemma3:4b".into(),
        ollama_timeout_secs: 30,
        customer: Some("acme-eu".into()),
    };
    cg.set_stalwart_connector(slug, &config).unwrap();

    let got = cg
        .get_stalwart_connector(slug, "default")
        .unwrap()
        .expect("connector present");
    assert_eq!(got.label, "default");
    assert_eq!(got.jmap_base_url, "https://mail.example.com");
    assert_eq!(got.admin_user, "admin");
    assert_eq!(got.admin_pass, "s3cret");
    assert_eq!(got.listen_port, 8443);
    assert_eq!(got.state_dir, "/state");
    assert_eq!(got.db_path, "/data/summaries.db");
    assert_eq!(got.smtp_local_host, "127.0.0.1");
    assert_eq!(got.smtp_local_port, 25);
    assert_eq!(got.smtp_relay_host.as_deref(), Some("relay.example.com"));
    assert_eq!(got.smtp_relay_port, 587);
    assert_eq!(got.imip_anchor_domain, "mail.example.com");
    assert_eq!(got.ollama_url, "http://localhost:11434");
    assert_eq!(got.ollama_model, "gemma3:4b");
    assert_eq!(got.ollama_timeout_secs, 30);
    assert_eq!(got.customer.as_deref(), Some("acme-eu"));
    assert_eq!(validator.validated_write_count(), 1);
    assert_eq!(validator.unvalidated_write_count(), 0);

    const STALWART_NS: &str = "https://contreforts.ds-labs.org/ontologies/stalwart#";
    const XSD_INTEGER: &str = "http://www.w3.org/2001/XMLSchema#integer";
    let conn_iri = namespaces::connector_iri("stalwart", slug, Some("default"));
    let conn_node = NamedNode::new(&conn_iri).unwrap();
    let graph_node = NamedNode::new(CONFIG_GRAPH).unwrap();
    let datatypes = stored_datatypes(&store, &conn_node, &graph_node);
    for numeric_field in [
        "listenPort",
        "smtpLocalPort",
        "smtpRelayPort",
        "ollamaTimeoutSecs",
    ] {
        assert!(
            datatypes.contains(&(
                format!("{STALWART_NS}{numeric_field}"),
                XSD_INTEGER.to_string()
            )),
            "{numeric_field} must be stored as xsd:integer, not a plain/string literal: {datatypes:?}"
        );
    }
}

/// New: `write_connector`'s "`None` values are skipped entirely" guarantee (its own doc comment,
/// `config_graph.rs:852-854`), pinned for stalwart's two optional fields -- an absent
/// `smtp_relay_host`/`customer` must write no triple at all, not an empty-string literal.
#[test]
fn stalwart_optional_fields_absent_write_no_triple_at_all() {
    let (_dir, store, slug) = setup();
    let validator =
        ConnectorValidator::new(STALWART_DECLARATION_TTL, &all_connector_kinds()).unwrap();
    let cg = ConfigGraph::with_validator(&store, validator.declarations(), &validator);

    cg.set_stalwart_connector(
        slug,
        &StalwartConnectorConfig {
            label: "noopt".into(),
            jmap_base_url: "https://mail.example.com".into(),
            admin_user: "admin".into(),
            admin_pass: "pw".into(),
            listen_port: 8080,
            state_dir: "/state".into(),
            db_path: "/data/db".into(),
            smtp_local_host: "127.0.0.1".into(),
            smtp_local_port: 25,
            smtp_relay_host: None,
            smtp_relay_port: 25,
            imip_anchor_domain: "mail.example.com".into(),
            ollama_url: "http://localhost:11434".into(),
            ollama_model: "llama3".into(),
            ollama_timeout_secs: 30,
            customer: None,
        },
    )
    .unwrap();

    let got = cg.get_stalwart_connector(slug, "noopt").unwrap().unwrap();
    assert_eq!(got.smtp_relay_host, None);
    assert_eq!(got.customer, None);

    const STALWART_NS: &str = "https://contreforts.ds-labs.org/ontologies/stalwart#";
    let conn_iri = namespaces::connector_iri("stalwart", slug, Some("noopt"));
    let triples = stored_triples(&store, &conn_iri);
    assert!(
        triples
            .iter()
            .all(|(p, _)| p != &format!("{STALWART_NS}smtpRelayHost")),
        "an absent optional field must write no triple at all: {triples:?}"
    );
    assert!(
        triples
            .iter()
            .all(|(p, _)| p != &format!("{STALWART_NS}customer"))
    );
}

// ── The declared/undeclared split -- `config_graph.rs`'s own case 1 / case 2 ────────────────

/// New (states the split directly, in one `ConfigGraph`, rather than trusting it to two
/// separately-run tests): forgejo (case 1: declared, in a validator built with declarations
/// covering it) and pennylane (case 2: no shape anywhere in those same declarations) resolved by
/// the very same `ConfigGraph`. No validator wired -- proving (contreforts-kg#23) that namespace
/// resolution reads `declarations`, not `validator`, so it applies identically whether or not
/// SHACL write-validation is switched on.
#[test]
fn declared_and_undeclared_connectors_coexist_under_the_same_declarations() {
    let (_dir, store, slug) = setup();
    let validator =
        ConnectorValidator::new(FORGEJO_DECLARATION_TTL, &all_connector_kinds()).unwrap();
    let cg = ConfigGraph::new(&store, validator.declarations());

    cg.set_forgejo_connector(
        slug,
        &ForgejoConnectorConfig {
            label: "main".into(),
            url: "https://git.example.com".into(),
            token: "tok".into(),
        },
    )
    .unwrap();
    cg.set_pennylane_connector(
        slug,
        &PennylaneConnectorConfig {
            token: "pl-tok".into(),
            base_url: None,
        },
    )
    .unwrap();

    const FORGEJO_NS: &str = "https://contreforts.ds-labs.org/ontologies/forgejo#";
    let forgejo_iri = namespaces::connector_iri("forgejo", slug, Some("main"));
    let forgejo_triples = stored_triples(&store, &forgejo_iri);
    assert!(
        forgejo_triples
            .iter()
            .any(|(p, _)| p.starts_with(FORGEJO_NS)),
        "the declared kind (case 1) must land under its own namespace: {forgejo_triples:?}"
    );

    let pennylane_iri = namespaces::connector_iri("pennylane", slug, None);
    let pennylane_triples = stored_triples(&store, &pennylane_iri);
    assert!(
        !pennylane_triples.is_empty(),
        "the write must have landed somewhere: {pennylane_triples:?}"
    );
    assert!(
        pennylane_triples
            .iter()
            .all(|(p, _)| p.starts_with(CORE_NS) || p == &format!("{RDF}type")),
        "the undeclared kind (case 2) must stay entirely under CORE_NS, even with a validator \
         wired that declares other kinds: {pennylane_triples:?}"
    );

    // Read back through the same declared-but-unvalidated ConfigGraph, matching the inline
    // module's `declared_namespace_without_validation_writes_and_reads_back`.
    assert_eq!(
        cg.get_forgejo_connector(slug, "main").unwrap().unwrap().url,
        "https://git.example.com"
    );
    assert!(cg.get_pennylane_connector(slug).unwrap().is_some());
}

// ── Coverage gap closed by a2 (contreforts/contreforts-workspace#58, D3c coverage accounting) ──
//
// The two tests below port two more of the inline module's `#[cfg(test)]` tests that a1's port
// did not carry forward and did not list as inline-kept or deliberately dropped:
// `connector_write_allowed_and_counted_when_kind_undeclared` and
// `connector_namespace_independent_of_validation_policy`. Both exercise real behaviour of the
// ported engine (the validator's case-2 counting *through* `ConfigGraph::with_validator`, and
// the namespace/validation independence contreforts-kg#23 exists to guarantee) that is not
// otherwise covered by any test in this file. A third dropped test,
// `unvalidated_kinds_named_at_construction_before_any_write`, is *not* re-added here: it never
// touched `ConfigGraph` at all (it only calls `ConnectorValidator::new` and inspects
// `unvalidated_kinds`/`Display`), and its behaviour already has a direct descendant --
// `unvalidated_kinds_and_write_counts_track_the_case_2_policy` in
// `crates/contreforts-core/declaration/tests/connector_validation.rs`, ported there during D3a,
// before this port ever started.

/// Ported/adapted from the inline module's `connector_write_allowed_and_counted_when_kind_undeclared`.
/// Unlike `declared_and_undeclared_connectors_coexist_under_the_same_declarations` above (which
/// uses `ConfigGraph::new`, no validator at all), this wires a real `ConnectorValidator` in and
/// proves the case-2 "allow and count" policy actually fires *through* `write_connector`, not
/// just when `ConnectorValidator::validate` is called directly on a hand-built instance.
#[test]
fn connector_write_allowed_and_counted_when_kind_undeclared() {
    let (_dir, store, slug) = setup();
    // The shapes cover forgejo only -- pennylane (like nine other kinds) has no shape at all, so
    // this exercises the case-2 "allow and count" policy, not case 1.
    let validator =
        ConnectorValidator::new(FORGEJO_DECLARATION_TTL, &all_connector_kinds()).unwrap();
    let cg = ConfigGraph::with_validator(&store, validator.declarations(), &validator);

    cg.set_pennylane_connector(
        slug,
        &PennylaneConnectorConfig {
            token: "t".into(),
            base_url: None,
        },
    )
    .unwrap();

    // The write went through despite there being no declaration to check it against -- and,
    // critically, that fact was counted, not silently dropped.
    assert!(cg.get_pennylane_connector(slug).unwrap().is_some());
    assert_eq!(validator.unvalidated_write_count(), 1);
    assert_eq!(validator.validated_write_count(), 0);
    assert!(validator.unvalidated_kinds().contains(&"pennylane"));
    assert!(!validator.unvalidated_kinds().contains(&"forgejo"));

    // Case 2 (contreforts-kg#21): pennylane's class and fields stayed `CORE_NS`, exactly as
    // before this issue -- the honest "not migrated yet" state, not a fallback that happens to
    // look the same.
    let conn_iri = namespaces::connector_iri("pennylane", slug, None);
    let triples = stored_triples(&store, &conn_iri);
    assert!(
        triples
            .iter()
            .all(|(p, _)| p.starts_with(CORE_NS) || *p == format!("{RDF}type")),
        "undeclared kind must stay entirely under CORE_NS: {triples:?}"
    );
    assert!(triples.contains(&(format!("{RDF}type"), format!("{CORE_NS}PennylaneConnector"))));
}

/// Ported/adapted from the inline module's `connector_namespace_independent_of_validation_policy`
/// -- the property contreforts-kg#23 exists to establish, stated directly: two `ConfigGraph`s
/// built from the *same* declarations -- one with `ConnectorValidator::validate` wired in, one
/// without -- resolve a forgejo write to the exact same namespace, byte for byte.
#[test]
fn connector_namespace_independent_of_validation_policy() {
    let validator =
        ConnectorValidator::new(FORGEJO_DECLARATION_TTL, &all_connector_kinds()).unwrap();

    let (_dir_validated, store_validated) = store();
    let cg_setup = ConfigGraph::new(&store_validated, ConnectorDeclarations::none());
    cg_setup
        .add_company(&CompanyConfig {
            slug: "acme".to_string(),
            name: "Acme".to_string(),
        })
        .unwrap();
    let cg_validated =
        ConfigGraph::with_validator(&store_validated, validator.declarations(), &validator);
    cg_validated
        .set_forgejo_connector(
            "acme",
            &ForgejoConnectorConfig {
                label: "main".into(),
                url: "https://git.example.com".into(),
                token: "tok".into(),
            },
        )
        .unwrap();

    let (_dir_unvalidated, store_unvalidated) = store();
    let cg_setup2 = ConfigGraph::new(&store_unvalidated, ConnectorDeclarations::none());
    cg_setup2
        .add_company(&CompanyConfig {
            slug: "acme".to_string(),
            name: "Acme".to_string(),
        })
        .unwrap();
    // Same declarations, no `ConnectorValidator` wired at all -- write validation cannot run,
    // but namespace resolution must not notice.
    let cg_unvalidated = ConfigGraph::new(&store_unvalidated, validator.declarations());
    cg_unvalidated
        .set_forgejo_connector(
            "acme",
            &ForgejoConnectorConfig {
                label: "main".into(),
                url: "https://git.example.com".into(),
                token: "tok".into(),
            },
        )
        .unwrap();

    let conn_iri = namespaces::connector_iri("forgejo", "acme", Some("main"));
    let validated_triples = stored_triples(&store_validated, &conn_iri);
    let unvalidated_triples = stored_triples(&store_unvalidated, &conn_iri);
    assert_eq!(
        validated_triples, unvalidated_triples,
        "the same declarations must produce the same namespace regardless of whether write \
         validation is enabled"
    );

    // Not merely "the two sides agree with each other" -- pin that they actually landed under
    // `forgejo:`, not `core:`, on the unvalidated side too.
    const FORGEJO_NS: &str = "https://contreforts.ds-labs.org/ontologies/forgejo#";
    assert!(
        unvalidated_triples.contains(&(
            format!("{RDF}type"),
            format!("{FORGEJO_NS}ForgejoConnector")
        )),
        "declared namespace must apply even without a wired validator: {unvalidated_triples:?}"
    );
}

// ── The three error cases ────────────────────────────────────────────────────────────────────
//
// Each asserted purely through `.to_string()` -- deliberately not `matches!`-ing on any specific
// error enum variant/path. `contreforts-config`'s own error type is an open design question this
// port surfaces rather than decides (comment 7904); these tests only require that *some* error
// comes back, implementing `Display`, and that its message names what went wrong.

/// Ported/adapted from the inline module's `connector_write_rejected_on_shacl_violation` -- the
/// "ConnectorValidation" error case, raised from a real SHACL check.
#[test]
fn connector_write_rejected_on_a_real_shacl_violation_names_kind_and_field() {
    let (_dir, store, slug) = setup();
    let validator =
        ConnectorValidator::new(FORGEJO_DECLARATION_TTL, &all_connector_kinds()).unwrap();
    let cg = ConfigGraph::with_validator(&store, validator.declarations(), &validator);

    // No http(s):// scheme -- violates the real declaration's sh:pattern on instanceUrl.
    let err = cg
        .set_forgejo_connector(
            slug,
            &ForgejoConnectorConfig {
                label: "main".into(),
                url: "not-a-url".into(),
                token: "tok".into(),
            },
        )
        .unwrap_err();

    let message = err.to_string();
    assert!(
        message.contains("forgejo"),
        "error should name the connector kind: {message}"
    );
    assert!(
        message.contains("instanceUrl"),
        "error should name the violating field: {message}"
    );
    assert!(
        cg.get_forgejo_connector(slug, "main").unwrap().is_none(),
        "a rejected write must not land"
    );
}

/// Ported/adapted from the inline module's `connector_declaration_never_mixes_namespaces` -- the
/// *other* trigger of the "ConnectorValidation" error class: not a SHACL library rejection, but
/// `ConnectorNamespace::field_iri`'s own refusal when a declared class omits one field's
/// `sh:path`, rather than silently falling back to `core:` for just that field.
#[test]
fn declaration_missing_one_fields_sh_path_refuses_the_write_instead_of_mixing_namespaces() {
    let (_dir, store, slug) = setup();
    let validator = ConnectorValidator::new(
        FORGEJO_DECLARATION_MISSING_LABEL_TTL,
        &all_connector_kinds(),
    )
    .unwrap();
    let cg = ConfigGraph::with_validator(&store, validator.declarations(), &validator);

    let err = cg
        .set_forgejo_connector(
            slug,
            &ForgejoConnectorConfig {
                label: "main".into(),
                url: "https://git.example.com".into(),
                token: "tok".into(),
            },
        )
        .unwrap_err();

    let message = err.to_string();
    assert!(
        message.contains("label"),
        "error should name the field with no declared IRI: {message}"
    );
    assert!(
        message.contains("mix") && message.contains("core:"),
        "error should explain the refusal is about mixing namespaces: {message}"
    );

    let conn_iri = namespaces::connector_iri("forgejo", slug, Some("main"));
    assert!(
        stored_triples(&store, &conn_iri).is_empty(),
        "a rejected write must leave no triples behind, mixed or otherwise"
    );
}

/// New: the "InvalidIri" error case, raised from `remove_connector`'s own kind lookup.
#[test]
fn removing_an_unknown_connector_type_is_a_named_error() {
    let (_dir, store, slug) = setup();
    let cg = ConfigGraph::new(&store, ConnectorDeclarations::none());

    let err = cg
        .remove_connector(slug, "not-a-real-kind", None)
        .unwrap_err();
    let message = err.to_string();
    assert!(
        message.contains("not-a-real-kind"),
        "error should name the invalid connector type: {message}"
    );
}

/// New: a second trigger of the same "InvalidIri" error class, from `require_company`'s guard --
/// every `set_*_connector` call goes through it before writing anything.
#[test]
fn writing_a_connector_for_a_nonexistent_company_is_a_named_error() {
    let (_dir, store) = store();
    let cg = ConfigGraph::new(&store, ConnectorDeclarations::none());

    let err = cg
        .set_forgejo_connector(
            "no-such-company",
            &ForgejoConnectorConfig {
                label: "main".into(),
                url: "https://git.example.com".into(),
                token: "tok".into(),
            },
        )
        .unwrap_err();

    let message = err.to_string();
    assert!(
        message.contains("no-such-company"),
        "error should name the missing company: {message}"
    );
}

/// Ported/adapted from the inline module's
/// `declared_kind_read_errors_on_mismatched_literal_instead_of_defaulting` -- the
/// "DeclaredFieldMismatch" error case. Bypasses `set_vector_store_connector` (which would itself
/// reject a non-numeric dimension at write time via SHACL) by hand-inserting through
/// `ConfigStore::inner()` directly, simulating data already corrupted on disk.
#[test]
fn declared_kind_read_errors_on_a_mismatched_literal_instead_of_silently_defaulting() {
    let (_dir, store, slug) = setup();
    let validator =
        ConnectorValidator::new(VECTOR_STORE_INTEGER_DECLARATION_TTL, &all_connector_kinds())
            .unwrap();
    let cg = ConfigGraph::with_validator(&store, validator.declarations(), &validator);

    const VS_NS: &str = "https://contreforts.ds-labs.org/ontologies/vectorstore#";
    let conn_iri = namespaces::connector_iri("vector_store", slug, Some("broken"));
    let conn_node = NamedNode::new(&conn_iri).unwrap();
    let graph_node = NamedNode::new(CONFIG_GRAPH).unwrap();
    let company_node = NamedNode::new(namespaces::company_iri(slug)).unwrap();

    for (pred, obj) in [
        (
            format!("{RDF}type"),
            Term::NamedNode(NamedNode::new(format!("{VS_NS}VectorStoreConnector")).unwrap()),
        ),
        (
            format!("{VS_NS}label"),
            Term::Literal(Literal::new_simple_literal("broken")),
        ),
        (
            format!("{VS_NS}vectorStoreKind"),
            Term::Literal(Literal::new_simple_literal("pgvector")),
        ),
        (
            format!("{VS_NS}dimension"),
            Term::Literal(Literal::new_simple_literal("not-a-number")),
        ),
    ] {
        store
            .inner()
            .insert(&Quad::new(
                conn_node.clone(),
                NamedNode::new(&pred).unwrap(),
                obj,
                GraphName::NamedNode(graph_node.clone()),
            ))
            .unwrap();
    }
    store
        .inner()
        .insert(&Quad::new(
            company_node,
            NamedNode::new(format!("{CORE_NS}hasConnector")).unwrap(),
            Term::NamedNode(conn_node),
            GraphName::NamedNode(graph_node),
        ))
        .unwrap();

    let err = cg
        .get_vector_store_connector(slug, "broken")
        .expect_err("a malformed dimension on a declared kind must error, not default to 0");
    let message = err.to_string();
    assert!(
        message.contains("dimension"),
        "error should name the malformed field: {message}"
    );
}

/// Companion/contrast to the test above, ported/adapted from the inline module's
/// `undeclared_kind_read_still_defaults_on_mismatched_literal`: the same malformed value on an
/// *undeclared* kind (stalwart has no shape in `VECTOR_STORE_INTEGER_DECLARATION_TTL`) keeps
/// today's exact behaviour -- silently default, not error.
#[test]
fn undeclared_kind_read_still_defaults_on_a_mismatched_literal() {
    let (_dir, store, slug) = setup();
    let validator =
        ConnectorValidator::new(VECTOR_STORE_INTEGER_DECLARATION_TTL, &all_connector_kinds())
            .unwrap();
    let cg = ConfigGraph::with_validator(&store, validator.declarations(), &validator);

    let conn_iri = namespaces::connector_iri("stalwart", slug, Some("broken"));
    let conn_node = NamedNode::new(&conn_iri).unwrap();
    let graph_node = NamedNode::new(CONFIG_GRAPH).unwrap();
    let company_node = NamedNode::new(namespaces::company_iri(slug)).unwrap();

    let fields: &[(&str, &str)] = &[
        (&format!("{CORE_NS}jmapBaseUrl"), "https://mail.example.com"),
        (&format!("{CORE_NS}adminUser"), "admin"),
        (&format!("{CORE_NS}adminPass"), "adminpw"),
        (&format!("{CORE_NS}listenPort"), "not-a-number"), // malformed
        (&format!("{CORE_NS}stateDir"), "/var/lib/stalwart"),
        (&format!("{CORE_NS}dbPath"), "/var/lib/stalwart/db.sqlite"),
        (&format!("{CORE_NS}smtpLocalHost"), "127.0.0.1"),
        (&format!("{CORE_NS}smtpLocalPort"), "25"),
        (&format!("{CORE_NS}smtpRelayPort"), "587"),
        (&format!("{CORE_NS}imipAnchorDomain"), "mail.example.com"),
        (&format!("{CORE_NS}ollamaUrl"), "http://localhost:11434"),
        (&format!("{CORE_NS}ollamaModel"), "llama3"),
        (&format!("{CORE_NS}ollamaTimeoutSecs"), "30"),
    ];
    store
        .inner()
        .insert(&Quad::new(
            conn_node.clone(),
            NamedNode::new(format!("{RDF}type")).unwrap(),
            Term::NamedNode(NamedNode::new(format!("{CORE_NS}StalwartConnector")).unwrap()),
            GraphName::NamedNode(graph_node.clone()),
        ))
        .unwrap();
    for (pred, val) in fields {
        store
            .inner()
            .insert(&Quad::new(
                conn_node.clone(),
                NamedNode::new(*pred).unwrap(),
                Term::Literal(Literal::new_simple_literal(*val)),
                GraphName::NamedNode(graph_node.clone()),
            ))
            .unwrap();
    }
    store
        .inner()
        .insert(&Quad::new(
            company_node,
            NamedNode::new(format!("{CORE_NS}hasConnector")).unwrap(),
            Term::NamedNode(conn_node),
            GraphName::NamedNode(graph_node),
        ))
        .unwrap();

    let got = cg
        .get_stalwart_connector(slug, "broken")
        .expect("undeclared kind: malformed value must default, not error")
        .expect("connector present");
    assert_eq!(
        got.listen_port, 8080,
        "malformed listenPort on an undeclared kind must silently default, unchanged"
    );
}

// ── The 11 structs' field round trips -- non-trivial shapes ─────────────────────────────────
//
// Stalwart's is covered above (real-declaration round trip doubles as its struct test).
// `ConnectorDeclarations::none()` (case 2, CORE_NS) is used below since none of O365/SMTP/
// VectorStore has a real declaration in this workspace today -- these tests are about the
// struct's own shape (the auth enum, the TLS enum, the geometry validation), independent of
// namespace resolution, which is already covered separately above.

/// New: `O365ConnectorAuth::ClientCredentials` round trip, plus the optional
/// `user_principal`/`customer` fields present.
#[test]
fn o365_client_credentials_auth_round_trips() {
    let (_dir, store, slug) = setup();
    let cg = ConfigGraph::new(&store, ConnectorDeclarations::none());

    cg.set_o365_connector(
        slug,
        &O365ConnectorConfig {
            label: "main".into(),
            auth: O365ConnectorAuth::ClientCredentials {
                tenant_id: "tenant-1".into(),
                client_id: "client-1".into(),
                client_secret: "secret-1".into(),
            },
            user_principal: Some("svc@contoso.com".into()),
            customer: Some("acme".into()),
        },
    )
    .unwrap();

    let got = cg
        .get_o365_connector(slug, "main")
        .unwrap()
        .expect("connector present");
    assert_eq!(got.label, "main");
    match got.auth {
        O365ConnectorAuth::ClientCredentials {
            tenant_id,
            client_id,
            client_secret,
        } => {
            assert_eq!(tenant_id, "tenant-1");
            assert_eq!(client_id, "client-1");
            assert_eq!(client_secret, "secret-1");
        }
        other => panic!("expected ClientCredentials, got {other:?}"),
    }
    assert_eq!(got.user_principal.as_deref(), Some("svc@contoso.com"));
    assert_eq!(got.customer.as_deref(), Some("acme"));
}

/// New: `O365ConnectorAuth::Delegated` round trip, both with and without the optional
/// `refresh_token`.
#[test]
fn o365_delegated_auth_round_trips_with_and_without_refresh_token() {
    let (_dir, store, slug) = setup();
    let cg = ConfigGraph::new(&store, ConnectorDeclarations::none());

    cg.set_o365_connector(
        slug,
        &O365ConnectorConfig {
            label: "delegated".into(),
            auth: O365ConnectorAuth::Delegated {
                access_token: "at-1".into(),
                refresh_token: Some("rt-1".into()),
            },
            user_principal: None,
            customer: None,
        },
    )
    .unwrap();
    let got = cg.get_o365_connector(slug, "delegated").unwrap().unwrap();
    match got.auth {
        O365ConnectorAuth::Delegated {
            access_token,
            refresh_token,
        } => {
            assert_eq!(access_token, "at-1");
            assert_eq!(refresh_token.as_deref(), Some("rt-1"));
        }
        other => panic!("expected Delegated, got {other:?}"),
    }
    assert_eq!(got.user_principal, None);
    assert_eq!(got.customer, None);

    cg.set_o365_connector(
        slug,
        &O365ConnectorConfig {
            label: "delegated-no-refresh".into(),
            auth: O365ConnectorAuth::Delegated {
                access_token: "at-2".into(),
                refresh_token: None,
            },
            user_principal: None,
            customer: None,
        },
    )
    .unwrap();
    let got2 = cg
        .get_o365_connector(slug, "delegated-no-refresh")
        .unwrap()
        .unwrap();
    match got2.auth {
        O365ConnectorAuth::Delegated { refresh_token, .. } => assert_eq!(refresh_token, None),
        other => panic!("expected Delegated, got {other:?}"),
    }
}

/// New: every `SmtpTlsMode` variant round trips, alongside the rest of the struct's fields.
#[test]
fn smtp_connector_round_trips_each_tls_mode() {
    let (_dir, store, slug) = setup();
    let cg = ConfigGraph::new(&store, ConnectorDeclarations::none());

    for (label, mode) in [
        ("plain", SmtpTlsMode::None),
        ("starttls", SmtpTlsMode::Starttls),
        ("tls", SmtpTlsMode::Tls),
    ] {
        cg.set_smtp_connector(
            slug,
            &SmtpConnectorConfig {
                label: label.into(),
                host: "smtp.example.com".into(),
                port: 587,
                username: Some("user".into()),
                password: Some("pass".into()),
                from_address: "agent@example.com".into(),
                tls: mode.clone(),
            },
        )
        .unwrap();

        let got = cg
            .get_smtp_connector(slug, label)
            .unwrap()
            .expect("connector present");
        assert_eq!(got.tls, mode, "TLS mode must round-trip for label {label}");
        assert_eq!(got.host, "smtp.example.com");
        assert_eq!(got.port, 587);
        assert_eq!(got.from_address, "agent@example.com");
        assert_eq!(got.username.as_deref(), Some("user"));
        assert_eq!(got.password.as_deref(), Some("pass"));
    }
}

/// New: the optional `username`/`password` fields round-trip to `None` when absent, rather than
/// an empty string.
#[test]
fn smtp_connector_optional_credentials_absent_round_trip_to_none() {
    let (_dir, store, slug) = setup();
    let cg = ConfigGraph::new(&store, ConnectorDeclarations::none());

    cg.set_smtp_connector(
        slug,
        &SmtpConnectorConfig {
            label: "noauth".into(),
            host: "relay.example.com".into(),
            port: 25,
            username: None,
            password: None,
            from_address: "noreply@example.com".into(),
            tls: SmtpTlsMode::None,
        },
    )
    .unwrap();

    let got = cg.get_smtp_connector(slug, "noauth").unwrap().unwrap();
    assert_eq!(got.username, None);
    assert_eq!(got.password, None);
}

/// Ported near-verbatim from the inline module's `vector_store_roundtrip`.
#[test]
fn vector_store_connector_round_trips_and_appears_in_the_unified_connector_list() {
    let (_dir, store, slug) = setup();
    let cg = ConfigGraph::new(&store, ConnectorDeclarations::none());

    let vs = VectorStoreConnectorConfig {
        label: "primary".into(),
        kind: VectorStoreKind::Pgvector,
        url: Some("postgres://localhost/test".into()),
        table: Some("chunks".into()),
        dimension: 384,
        column_type: VectorStoreColumnType::Vector,
        admin_url: None,
    };
    cg.set_vector_store_connector(slug, &vs).unwrap();

    let got = cg
        .get_vector_store_connector(slug, "primary")
        .unwrap()
        .unwrap();
    assert_eq!(got.label, "primary");
    assert_eq!(got.kind, VectorStoreKind::Pgvector);
    assert_eq!(got.url.as_deref(), Some("postgres://localhost/test"));
    assert_eq!(got.table.as_deref(), Some("chunks"));
    assert_eq!(got.dimension, 384);
    assert_eq!(got.column_type, VectorStoreColumnType::Vector);

    assert_eq!(cg.list_vector_store_connectors(slug).unwrap().len(), 1);

    let all = cg.list_connectors(slug).unwrap();
    assert!(
        all.iter()
            .any(|c| matches!(c, ConnectorConfig::VectorStore(_)))
    );

    cg.remove_connector(slug, "vector_store", Some("primary"))
        .unwrap();
    assert!(
        cg.get_vector_store_connector(slug, "primary")
            .unwrap()
            .is_none()
    );
}

/// Ported/adapted from `crates/contreforts-kg/tests/config_graph.rs`'s
/// `an_unindexable_geometry_is_refused_at_write_time` (first half): the geometry guard's
/// dimension-must-be-nonzero branch, which that integration test does not separately cover.
#[test]
fn vector_store_geometry_rejects_zero_dimension() {
    let (_dir, store, slug) = setup();
    let cg = ConfigGraph::new(&store, ConnectorDeclarations::none());

    let err = cg
        .set_vector_store_connector(
            slug,
            &VectorStoreConnectorConfig {
                label: "zero-dim".into(),
                kind: VectorStoreKind::Pgvector,
                url: None,
                table: None,
                dimension: 0,
                column_type: VectorStoreColumnType::Vector,
                admin_url: None,
            },
        )
        .unwrap_err();

    let msg = err.to_string();
    assert!(
        msg.contains("zero-dim"),
        "error should name the connector: {msg}"
    );
    assert!(
        msg.contains("dimension"),
        "error should name the offending field: {msg}"
    );
    assert!(
        cg.get_vector_store_connector(slug, "zero-dim")
            .unwrap()
            .is_none(),
        "a refused write must not land"
    );
}

/// Ported/adapted from `crates/contreforts-kg/tests/config_graph.rs`'s
/// `an_unindexable_geometry_is_refused_at_write_time`.
#[test]
fn vector_store_geometry_rejects_a_dimension_beyond_the_column_types_hnsw_limit() {
    let (_dir, store, slug) = setup();
    let cg = ConfigGraph::new(&store, ConnectorDeclarations::none());

    // vector tops out at 2000 (measured, pgvector 0.8.5 / PostgreSQL 16).
    let err = cg
        .set_vector_store_connector(
            slug,
            &VectorStoreConnectorConfig {
                label: "too-wide".into(),
                kind: VectorStoreKind::Pgvector,
                url: None,
                table: None,
                dimension: 3072,
                column_type: VectorStoreColumnType::Vector,
                admin_url: None,
            },
        )
        .unwrap_err();

    let msg = err.to_string();
    for expected in ["too-wide", "vector", "3072", "2000"] {
        assert!(
            msg.contains(expected),
            "the refusal should name {expected:?}: {msg}"
        );
    }
    assert!(
        cg.get_vector_store_connector(slug, "too-wide")
            .unwrap()
            .is_none()
    );

    // The same dimension under halfvec (tops out at 4000) is fine -- the whole point of
    // carrying the column type alongside the dimension.
    cg.set_vector_store_connector(
        slug,
        &VectorStoreConnectorConfig {
            label: "fine".into(),
            kind: VectorStoreKind::Pgvector,
            url: None,
            table: None,
            dimension: 3072,
            column_type: VectorStoreColumnType::Halfvec,
            admin_url: None,
        },
    )
    .expect("halfvec(3072) is within the measured 4000-dim limit");
}

/// Ported/adapted from `crates/contreforts-kg/tests/config_graph.rs`'s
/// `an_in_memory_store_is_not_held_to_a_pgvector_geometry`.
#[test]
fn vector_store_geometry_is_not_checked_for_an_in_memory_store() {
    let (_dir, store, slug) = setup();
    let cg = ConfigGraph::new(&store, ConnectorDeclarations::none());

    cg.set_vector_store_connector(
        slug,
        &VectorStoreConnectorConfig {
            label: "mem".into(),
            kind: VectorStoreKind::InMemory,
            url: None,
            table: None,
            dimension: 999_999,
            column_type: VectorStoreColumnType::Vector,
            admin_url: None,
        },
    )
    .expect(
        "in-memory stores have no pgvector column to index, so geometry is not their constraint",
    );
}

/// Ported/adapted from `crates/contreforts-kg/tests/config_graph.rs`'s `admin_url_is_write_only`.
#[test]
fn vector_store_admin_url_is_write_only_never_returned_by_get_list_or_serialization() {
    let (_dir, store, slug) = setup();
    let cg = ConfigGraph::new(&store, ConnectorDeclarations::none());

    const ADMIN: &str = "postgres://ddl_role:s3cret@db.internal:5432/kb";
    cg.set_vector_store_connector(
        slug,
        &VectorStoreConnectorConfig {
            label: "primary".into(),
            kind: VectorStoreKind::Pgvector,
            url: Some("postgres://ro_role:pw@db.internal:5432/kb".into()),
            table: Some("kb_chunks".into()),
            dimension: 1024,
            column_type: VectorStoreColumnType::Vector,
            admin_url: Some(ADMIN.into()),
        },
    )
    .unwrap();

    let got = cg
        .get_vector_store_connector(slug, "primary")
        .unwrap()
        .expect("connector present");
    assert_eq!(
        got.admin_url, None,
        "get_vector_store_connector must not return the DDL credential"
    );

    for item in cg.list_vector_store_connectors(slug).unwrap() {
        assert_eq!(item.admin_url, None, "list must not return it either");
    }

    let json = serde_json::to_string(&got).expect("serializes");
    assert!(
        !json.contains("s3cret") && !json.contains("adminUrl") && !json.contains("admin_url"),
        "the DDL credential must not survive serialization: {json}"
    );

    assert_eq!(
        cg.get_vector_store_admin_url(slug, "primary").unwrap(),
        Some(ADMIN.to_string()),
        "the explicit reader is the one path that returns it"
    );

    cg.set_vector_store_connector(
        slug,
        &VectorStoreConnectorConfig {
            label: "no-admin".into(),
            kind: VectorStoreKind::Pgvector,
            url: None,
            table: None,
            dimension: 1024,
            column_type: VectorStoreColumnType::Vector,
            admin_url: None,
        },
    )
    .unwrap();
    assert_eq!(
        cg.get_vector_store_admin_url(slug, "no-admin").unwrap(),
        None,
        "no admin URL configured is a reportable state, not an error"
    );
}
