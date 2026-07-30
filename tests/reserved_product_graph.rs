//! D6 (contreforts/contreforts-workspace#58; #19 D5/O2, answered identically to #18 Q3): the
//! product graph (`PRODUCT_GRAPH_TTL` in `contreforts-config-api/product`) is loaded into the
//! config store as a **reserved named graph**, reloaded at every startup, and write-rejected at
//! runtime. Today it exists only as a Rust `&'static str` consumed for SHACL validation -- never
//! as queryable data (comment 7969 / the D-table's own inventory, point 6).
//!
//! This crate must not depend on any connector crate, or on `contreforts-product` (which pulls
//! connector crates in transitively through its optional features) -- `tests/config_graph.rs`
//! already establishes this precedent by reproducing a trimmed, byte-faithful excerpt of the real
//! `contreforts-connector-forgejo/declaration.ttl` as a plain Turtle constant rather than adding
//! that dependency. This file follows the same pattern: `PRODUCT_FIXTURE_TTL` below is a trimmed,
//! byte-faithful reproduction of that same declaration file's `sh:NodeShape` section, standing in
//! for "whatever `contreforts-product` assembles" -- the mechanism under test (load Turtle into a
//! reserved graph; reload; guard writes) does not depend on which connectors happen to be
//! compiled in, and a synthetic-but-real fixture keeps these tests deterministic regardless.
//!
//! Every IRI below is a full bracketed `<...>` or a `prefix:localname` pair with no raw `/` in any
//! local part -- confirmed parseable, see the RED-verification note in the task report.
//!
//! `contreforts_config::PRODUCT_GRAPH`, `ConfigStore::reload_product_graph` and
//! `ConfigStore::insert_quad` do not exist yet -- this file does not compile against `develop`.
//! Sanctioned compile-error RED, same as this crate's other new D5/D6 files.

use contreforts_config::{ConfigStore, PRODUCT_GRAPH};
use oxigraph::model::{GraphName, Literal, NamedNode, Quad, Term};

fn store() -> (tempfile::TempDir, ConfigStore) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config_store");
    let store = ConfigStore::open(&path).expect("store opens at a fresh path");
    (dir, store)
}

/// Trimmed, byte-faithful reproduction of `contreforts-connector-forgejo/declaration.ttl`'s
/// `sh:targetClass`/`sh:property` section -- same trimming convention as
/// `tests/config_graph.rs`'s own `FORGEJO_DECLARATION_TTL`.
const PRODUCT_FIXTURE_TTL: &str = r#"
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
        ] .
"#;

/// A second, distinct fixture (different subject) used to prove reload *replaces* rather than
/// accumulates.
const PRODUCT_FIXTURE_TTL_V2: &str = r#"
    @prefix sh:      <http://www.w3.org/ns/shacl#> .
    @prefix gitlab:  <https://contreforts.ds-labs.org/ontologies/gitlab#> .

    gitlab:GitlabConnectorShape a sh:NodeShape ;
        sh:targetClass gitlab:GitlabConnector .
"#;

fn shacl_node_shape_iri() -> String {
    "http://www.w3.org/ns/shacl#NodeShape".to_string()
}

// ── Item 5: present as queryable data, not merely a Rust string ────────────────────────────────

/// Loading the reserved graph must make its content reachable by ordinary SPARQL -- not merely
/// "the load call returned Ok". A guard or a UI that only trusted the return value, never the
/// actual store contents, would pass even if `reload_product_graph` silently no-op'd.
#[test]
fn reload_product_graph_makes_its_content_reachable_by_sparql() {
    let (_dir, store) = store();
    store
        .reload_product_graph(PRODUCT_FIXTURE_TTL)
        .expect("loading well-formed Turtle into the reserved graph succeeds");

    let node_shape = shacl_node_shape_iri();
    let sparql = format!(
        "SELECT ?s WHERE {{ GRAPH <{PRODUCT_GRAPH}> {{ \
           ?s a <{node_shape}> \
         }} }}"
    );
    let rows = store
        .select(&sparql)
        .expect("a well-formed SELECT against the reserved graph must not error");

    assert_eq!(
        rows.len(),
        1,
        "the loaded declaration's sh:NodeShape triple must be reachable by SPARQL as real data, \
         not merely exist as a Rust &str -- got {rows:?}"
    );
    assert_eq!(
        rows[0][0].1, "https://contreforts.ds-labs.org/ontologies/forgejo#ForgejoConnectorShape",
        "the specific subject the fixture declares must be the one found, got {rows:?}"
    );
}

/// The reserved graph's IRI must be distinct from the ordinary config graph -- otherwise loading
/// it would silently mix build-derived declarations into hand-entered configuration data.
#[test]
fn the_reserved_graph_is_distinct_from_the_ordinary_config_graph() {
    assert_ne!(
        PRODUCT_GRAPH,
        contreforts_core::namespaces::CONFIG_GRAPH,
        "the reserved product graph must be a different named graph from cfdata:graph/config"
    );
}

// ── Item 6, part 1: a write into the reserved graph is rejected ────────────────────────────────

/// The guarded write primitive must refuse to write into the reserved graph, naming it, while a
/// write into an ordinary graph through the same primitive still succeeds -- proving the guard is
/// specific to the reserved graph rather than one that rejects every write outright.
#[test]
fn a_direct_write_into_the_reserved_graph_through_the_guarded_primitive_is_rejected() {
    let (_dir, store) = store();
    let subject = NamedNode::new("https://contreforts.test/subject/1").unwrap();
    let predicate = NamedNode::new("https://contreforts.test/predicate/name").unwrap();
    let object = Term::Literal(Literal::new_simple_literal("smuggled"));
    let reserved_graph = NamedNode::new(PRODUCT_GRAPH).expect("PRODUCT_GRAPH is a valid IRI");

    let err = store
        .insert_quad(&subject, &predicate, &object, &reserved_graph)
        .expect_err(
            "writing into the reserved product graph through the guarded primitive must be \
             rejected",
        );
    let message = err.to_string();
    assert!(
        message.contains(PRODUCT_GRAPH),
        "the error must name the reserved graph being protected, got: {message:?}"
    );

    let sparql = format!(
        "SELECT ?o WHERE {{ GRAPH <{PRODUCT_GRAPH}> {{ \
           <https://contreforts.test/subject/1> <https://contreforts.test/predicate/name> ?o \
         }} }}"
    );
    let rows = store.select(&sparql).expect("select succeeds");
    assert!(
        rows.is_empty(),
        "the rejected write must not have actually landed in the reserved graph, got {rows:?}"
    );
}

/// The control: the exact same primitive, targeting an ordinary (non-reserved) graph, must still
/// succeed -- without this, `insert_quad` rejecting everything would still pass the test above.
#[test]
fn the_same_guarded_primitive_still_writes_an_ordinary_graph() {
    let (_dir, store) = store();
    let subject = NamedNode::new("https://contreforts.test/subject/1").unwrap();
    let predicate = NamedNode::new("https://contreforts.test/predicate/name").unwrap();
    let object = Term::Literal(Literal::new_simple_literal("ordinary"));
    let ordinary_graph = NamedNode::new("https://contreforts.test/graph/not-reserved").unwrap();

    store
        .insert_quad(&subject, &predicate, &object, &ordinary_graph)
        .expect(
            "writing into an ordinary, non-reserved graph through the same primitive must succeed",
        );

    let rows = store
        .select(&format!(
            "SELECT ?o WHERE {{ GRAPH <https://contreforts.test/graph/not-reserved> {{ \
               <https://contreforts.test/subject/1> <https://contreforts.test/predicate/name> ?o \
             }} }}"
        ))
        .expect("select succeeds");
    assert_eq!(
        rows.len(),
        1,
        "the accepted write must actually be present: {rows:?}"
    );
}

// ── Item 6, part 2: an edit that slips through another way is gone after reload ────────────────

/// The `#19 O2` guarantee, proven directly: something that bypasses the guard entirely -- exactly
/// as the unrestricted raw SPARQL update route would -- must not survive the next startup's
/// reload, even though it is trivially present immediately after being written. This is the half
/// of O2 that "write-time rejection" alone cannot provide.
#[test]
fn an_edit_that_bypasses_the_guard_is_gone_after_the_next_reload() {
    let (_dir, store) = store();
    store
        .reload_product_graph(PRODUCT_FIXTURE_TTL)
        .expect("first load (the initial 'startup') succeeds");

    // Bypass: write directly through `ConfigStore::inner()`, exactly as the unrestricted raw
    // SPARQL update route could -- `insert_quad` above would have rejected this.
    let smuggled_subject = NamedNode::new("https://contreforts.test/smuggled/1").unwrap();
    let smuggled_predicate = NamedNode::new("https://contreforts.test/predicate/smuggled").unwrap();
    let reserved_graph = NamedNode::new(PRODUCT_GRAPH).expect("PRODUCT_GRAPH is a valid IRI");
    store
        .inner()
        .insert(&Quad::new(
            smuggled_subject.clone(),
            smuggled_predicate.clone(),
            Term::Literal(Literal::new_simple_literal("smuggled")),
            GraphName::NamedNode(reserved_graph.clone()),
        ))
        .expect(
            "a direct insert via inner() bypasses the guard entirely, exactly as the raw \
                 SPARQL route would",
        );

    // Sanity: the bypass really landed -- otherwise this test would trivially "pass" no matter
    // what the next reload did.
    let smuggled_query = format!(
        "SELECT ?o WHERE {{ GRAPH <{PRODUCT_GRAPH}> {{ \
           <https://contreforts.test/smuggled/1> <https://contreforts.test/predicate/smuggled> ?o \
         }} }}"
    );
    let before = store.select(&smuggled_query).expect("select succeeds");
    assert_eq!(
        before.len(),
        1,
        "sanity check: the bypass write must have actually landed before the next reload, got \
         {before:?}"
    );

    // The next startup: reload with the exact same fixture content.
    store
        .reload_product_graph(PRODUCT_FIXTURE_TTL)
        .expect("the next 'startup' reload succeeds");

    let after = store.select(&smuggled_query).expect("select succeeds");
    assert!(
        after.is_empty(),
        "an edit that slipped through outside the guarded write path must be gone after the \
         next startup reload -- #19 O2's guarantee is that this is transient by construction, \
         not merely rejected when caught. Got {after:?}"
    );

    // The legitimate fixture content must still be there -- reload must not have simply wiped
    // the graph empty; it must have rebuilt it from the given Turtle.
    let node_shape = shacl_node_shape_iri();
    let legitimate = store
        .select(&format!(
            "SELECT ?s WHERE {{ GRAPH <{PRODUCT_GRAPH}> {{ ?s a <{node_shape}> }} }}"
        ))
        .expect("select succeeds");
    assert_eq!(
        legitimate.len(),
        1,
        "the reload must have rebuilt the reserved graph's legitimate content, not merely \
         emptied it, got {legitimate:?}"
    );
}

/// Reload is a genuine replace, not an accumulation: reloading with different content entirely
/// must leave no trace of the previous content behind, independent of any bypass scenario.
#[test]
fn reload_replaces_previous_content_rather_than_accumulating_it() {
    let (_dir, store) = store();
    store
        .reload_product_graph(PRODUCT_FIXTURE_TTL)
        .expect("first load succeeds");
    store
        .reload_product_graph(PRODUCT_FIXTURE_TTL_V2)
        .expect("second load, with entirely different content, succeeds");

    let node_shape = shacl_node_shape_iri();
    let rows = store
        .select(&format!(
            "SELECT ?s WHERE {{ GRAPH <{PRODUCT_GRAPH}> {{ ?s a <{node_shape}> }} }}"
        ))
        .expect("select succeeds");

    assert_eq!(
        rows.len(),
        1,
        "after reloading with V2's content, exactly V2's one subject must remain -- V1's subject \
         must be gone, not accumulated alongside it: got {rows:?}"
    );
    assert_eq!(
        rows[0][0].1, "https://contreforts.ds-labs.org/ontologies/gitlab#GitlabConnectorShape",
        "the surviving subject must be V2's, not V1's: got {rows:?}"
    );
}
