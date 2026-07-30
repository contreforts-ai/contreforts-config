//! Pins the three store primitives the ported config-graph engine actually needs
//! (contreforts/contreforts-workspace#58, comment 7904, item D3c): `ConfigStore::select`,
//! `ConfigStore::remove_quad`, and `ConfigStore::inner`. Measured against
//! `crates/contreforts-kg/src/config_graph.rs`, the engine calls `QueryEngine::select` 7 times
//! and never calls `ask`; `GraphStore::inner()` once and `GraphStore::remove_quad()` four times,
//! with every other write already going through `inner()`. Modeled on
//! `crates/contreforts-kg/src/query.rs:23` (`select`) and `crates/contreforts-kg/src/store.rs`
//! (`inner`, `remove_quad`).
//!
//! None of `ConfigStore::select`, `::remove_quad` exist yet -- this file does not compile
//! against `develop`. That is the sanctioned RED (`crates/contreforts-kg/CONTRIBUTING.md` §3):
//! a compile error naming the missing method/type is evidence enough.

use contreforts_config::ConfigStore;
use oxigraph::model::{GraphName, Literal, NamedNode, Quad, Term};

const GRAPH: &str = "https://contreforts.test/graph/config";

fn store() -> (tempfile::TempDir, ConfigStore) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config_store");
    let store = ConfigStore::open(&path).expect("store opens at a fresh path");
    (dir, store)
}

fn insert_test_quad(store: &ConfigStore, subject: &str, predicate: &str, object_literal: &str) {
    let quad = Quad::new(
        NamedNode::new(subject).unwrap(),
        NamedNode::new(predicate).unwrap(),
        Term::Literal(Literal::new_simple_literal(object_literal)),
        GraphName::NamedNode(NamedNode::new(GRAPH).unwrap()),
    );
    store
        .inner()
        .insert(&quad)
        .expect("direct insert via inner() succeeds");
}

// ── `select` ─────────────────────────────────────────────────────────────────

#[test]
// The basic contract: a SELECT that matches real data returns exactly those bindings, as
// `(variable name, value)` pairs -- the shape `write_connector`'s read helpers
// (`fetch_connector`, `list_connector_labels`) depend on to build a `BTreeMap` off of.
fn select_returns_bindings_for_a_query_that_matches_real_data() {
    let (_dir, store) = store();
    insert_test_quad(
        &store,
        "https://contreforts.test/subject/1",
        "https://contreforts.test/predicate/name",
        "Acme",
    );

    let sparql = format!(
        "SELECT ?name WHERE {{ GRAPH <{GRAPH}> {{ \
           <https://contreforts.test/subject/1> <https://contreforts.test/predicate/name> ?name \
         }} }}"
    );
    let rows = store
        .select(&sparql)
        .expect("a well-formed SELECT against real data must not error");

    assert_eq!(rows.len(), 1, "exactly one matching row: {rows:?}");
    let (var, value) = &rows[0][0];
    assert_eq!(var, "name");
    assert_eq!(value, "Acme");
}

#[test]
// The hazard this test exists to name: "no results" and "failed" must be distinguishable.
// A syntactically valid SELECT that legitimately matches nothing must return `Ok(vec![])`,
// not an error -- `fetch_connector`'s `rows.first() -> None` path (config_graph.rs:996-997)
// relies on an empty, successful result meaning exactly "this connector does not exist yet".
fn select_returns_an_empty_vec_not_an_error_when_nothing_matches() {
    let (_dir, store) = store();

    let sparql = format!(
        "SELECT ?name WHERE {{ GRAPH <{GRAPH}> {{ \
           <https://contreforts.test/subject/does-not-exist> \
           <https://contreforts.test/predicate/name> ?name \
         }} }}"
    );
    let rows = store
        .select(&sparql)
        .expect("a well-formed SELECT that matches nothing is a success, not a failure");

    assert!(
        rows.is_empty(),
        "no matching data must be an empty Ok(vec![]), not a synthesized row: {rows:?}"
    );
}

#[test]
// The other half of the same hazard, stated as its own test rather than trusted to the
// absence of a panic elsewhere: a query that cannot even be evaluated (malformed SPARQL) must
// be `Err`, never silently collapse to the same `Ok(vec![])` a real "no rows" result produces.
// A `select` that cannot tell these two apart would make every "declared field mismatch" or
// "unknown predicate" bug at the call site look like an ordinary empty result.
fn select_errors_on_malformed_sparql_rather_than_returning_an_empty_vec() {
    let (_dir, store) = store();

    // Missing closing brace: not valid SPARQL under any grammar.
    let malformed = "SELECT ?x WHERE { ?x ?p ?o ";
    let result = store.select(malformed);

    assert!(
        result.is_err(),
        "malformed SPARQL must be a named error, not Ok(vec![]) -- got {result:?}"
    );
}

// ── `remove_quad` ────────────────────────────────────────────────────────────

#[test]
// `remove_quad` must remove exactly the quad named -- not the whole subject
// (`remove_subject_from_named_graph`'s job, not this one) and not a sibling predicate on the
// same subject. `remove_knowledge_base`/`remove_agent`/`remove_sparql_template`/
// `remove_connector` each call this once, right after wiping the linked entity's own triples,
// to drop just the company's `hasX` link -- if this over-removed, it would silently delete
// unrelated links sharing the same subject.
fn remove_quad_removes_exactly_the_named_quad_and_no_sibling() {
    let (_dir, store) = store();
    let subject = NamedNode::new("https://contreforts.test/company/acme").unwrap();
    let predicate = NamedNode::new("https://contreforts.test/predicate/hasConnector").unwrap();
    let graph = NamedNode::new(GRAPH).unwrap();
    let target = Term::NamedNode(NamedNode::new("https://contreforts.test/connector/a").unwrap());
    let sibling = Term::NamedNode(NamedNode::new("https://contreforts.test/connector/b").unwrap());

    store
        .inner()
        .insert(&Quad::new(
            subject.clone(),
            predicate.clone(),
            target.clone(),
            GraphName::NamedNode(graph.clone()),
        ))
        .unwrap();
    store
        .inner()
        .insert(&Quad::new(
            subject.clone(),
            predicate.clone(),
            sibling.clone(),
            GraphName::NamedNode(graph.clone()),
        ))
        .unwrap();

    store
        .remove_quad(&subject, &predicate, &target, &graph)
        .expect("removing a quad that is present succeeds");

    let remaining: Vec<_> = store
        .inner()
        .quads_for_pattern(
            Some((&subject).into()),
            Some((&predicate).into()),
            None,
            Some((&graph).into()),
        )
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(
        remaining.len(),
        1,
        "only the sibling should remain: {remaining:?}"
    );
    assert_eq!(remaining[0].object, sibling);
}

#[test]
// Removing a quad that was never there is a harmless no-op -- callers such as
// `remove_agent`/`remove_knowledge_base` call this unconditionally after
// `remove_subject_from_named_graph`, with no prior existence check.
fn remove_quad_on_an_absent_quad_does_not_error() {
    let (_dir, store) = store();
    let subject = NamedNode::new("https://contreforts.test/company/never-existed").unwrap();
    let predicate = NamedNode::new("https://contreforts.test/predicate/hasConnector").unwrap();
    let object = Term::NamedNode(NamedNode::new("https://contreforts.test/connector/x").unwrap());
    let graph = NamedNode::new(GRAPH).unwrap();

    store
        .remove_quad(&subject, &predicate, &object, &graph)
        .expect("removing an absent quad must not error");
}

// ── `inner` ──────────────────────────────────────────────────────────────────

#[test]
// `inner()` must give real, usable read/write access to the underlying `oxigraph::store::Store`
// -- `add_company`'s idempotent-overwrite path (config_graph.rs:1049-1061) reads
// `quads_for_pattern` and removes stale triples directly through it, with no `ConfigGraph`
// wrapper in between.
fn inner_gives_direct_usable_access_to_the_oxigraph_store() {
    let (_dir, store) = store();
    let subject = NamedNode::new("https://contreforts.test/company/acme").unwrap();
    let predicate = NamedNode::new("https://contreforts.test/predicate/name").unwrap();
    let object = Term::Literal(Literal::new_simple_literal("Acme"));
    let graph = NamedNode::new(GRAPH).unwrap();
    let quad = Quad::new(
        subject.clone(),
        predicate.clone(),
        object.clone(),
        GraphName::NamedNode(graph.clone()),
    );

    store.inner().insert(&quad).expect("insert through inner()");
    assert!(
        store.inner().contains(&quad).unwrap(),
        "a quad inserted through inner() must be visible through inner()"
    );

    // And usable together with `select`: the two primitives must observe the same store.
    let sparql = format!(
        "SELECT ?name WHERE {{ GRAPH <{GRAPH}> {{ \
           <https://contreforts.test/company/acme> <https://contreforts.test/predicate/name> ?name \
         }} }}"
    );
    let rows = store.select(&sparql).expect("select succeeds");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0][0].1, "Acme");
}
