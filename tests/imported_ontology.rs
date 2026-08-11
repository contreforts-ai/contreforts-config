//! contreforts-workspace#18, **new requirement 1**: "the config store must be able to import
//! ontologies" -- the one item of #18's own stated scope that phase D (contreforts-workspace#58)
//! never filed as a sub-task and never built.
//!
//! An imported ontology is the **third** class of graph in the config store, alongside the
//! hand-entered `CONFIG_GRAPH` and the reserved, build-derived `PRODUCT_GRAPH`, and #18 is
//! explicit that the three must be named separately because their write policies differ. What
//! makes this class its own thing, and what this file pins:
//!
//! - it is **durable**: it survives a KG drop-and-re-sync, because it does not live in the KG
//!   store at all, and unlike `PRODUCT_GRAPH` it is never rebuilt at startup (nothing carries a
//!   copy to rebuild it *from*);
//! - it is **operator-supplied and not re-derivable**, so a failed import must destroy nothing;
//! - it must never sit inside a KG instance's *disposable* IRI space, checked both at write time
//!   and again at startup, because the two orderings are not the same check.
//!
//! Per this repo's standing rule about a guard that examines nothing still reporting success,
//! every rejection test below is paired with a control proving the *same* shape of operation is
//! accepted when it does not violate the rule -- a guard that refused everything, or a validator
//! that always failed, would still pass the rejection tests alone. The four controls are marked
//! `[CONTROL]`.
//!
//! Fixtures are hand-written Turtle constants rather than a dependency on any connector or product
//! crate, following `tests/reserved_product_graph.rs`'s own convention and for the same reason:
//! the mechanism under test does not depend on which vocabularies happen to be compiled in.

use contreforts_config::{
    CompanyConfig, ConfigGraph, ConfigGraphError, ConfigStore, ConfigStoreError,
    IMPORTED_ONTOLOGY_GRAPH_PREFIX, ImportedOntologyConfig, KgInstanceConfig, KnowledgeBaseConfig,
    OntologyFormat, PRODUCT_GRAPH, imported_ontology_graph_iri,
};
use contreforts_core::namespaces::{CONFIG_GRAPH, CORE_NS, DATA_NS, RDF};
use contreforts_declaration::ConnectorDeclarations;
use oxigraph::io::RdfFormat;
use oxigraph::model::{GraphName, Literal, NamedNode, Quad, Term};

fn store() -> (tempfile::TempDir, ConfigStore) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config_store");
    let store = ConfigStore::open(&path).expect("store opens at a fresh path");
    (dir, store)
}

/// A realistic instance prefix -- narrow, under `/data/instance/`, the shape
/// `tests/kb_graph_prefix_guard.rs` already uses. Does **not** swallow the imported-ontology
/// prefix, which is the whole point of the controls below.
const REALISTIC_INSTANCE_PREFIX: &str =
    "https://contreforts.ds-labs.org/data/instance/primary-a1b2/";

/// A prefix broad enough to swallow `IMPORTED_ONTOLOGY_GRAPH_PREFIX`
/// (`.../data/graph/ontology/...`). Nothing forbids an operator assigning this today --
/// `set_kg_instance` enforces uniqueness, not narrowness -- which is exactly why the collision
/// guard has to exist.
const SWALLOWING_INSTANCE_PREFIX: &str = "https://contreforts.ds-labs.org/data/graph/";

const CONCEPT_A: &str = "https://example.org/vocab#conceptA";
const CONCEPT_B: &str = "https://example.org/vocab#conceptB";

/// Three triples on one subject.
const VOCAB_A_TTL: &str = r#"
    @prefix ex:   <https://example.org/vocab#> .
    @prefix skos: <http://www.w3.org/2004/02/skos/core#> .

    ex:conceptA a skos:Concept ;
        skos:prefLabel "Concept A" ;
        skos:notation "A" .
"#;

/// One triple, on a *different* subject -- so "did the re-import replace or accumulate?" is
/// answerable by both a count and a subject lookup, not by a count alone.
const VOCAB_B_TTL: &str = r#"
    @prefix ex:   <https://example.org/vocab#> .
    @prefix skos: <http://www.w3.org/2004/02/skos/core#> .

    ex:conceptB a skos:Concept .
"#;

/// Two well-formed triples followed by a syntax error, so the streaming parser reaches the failure
/// only *after* it has produced usable quads. A payload that failed on its very first byte would
/// not distinguish "parse first, write second" from "clear, then stream in".
const VOCAB_MALFORMED_TAIL_TTL: &str = r#"
    @prefix ex:   <https://example.org/vocab#> .
    @prefix skos: <http://www.w3.org/2004/02/skos/core#> .

    ex:conceptC a skos:Concept .
    ex:conceptD a skos:Concept .
    ex:conceptE a skos:Concept this is not turtle
"#;

/// Trimmed Turtle standing in for whatever `contreforts-product` assembles, same convention as
/// `tests/reserved_product_graph.rs`'s own fixture.
const PRODUCT_FIXTURE_TTL: &str = r#"
    @prefix sh:      <http://www.w3.org/ns/shacl#> .
    @prefix forgejo: <https://contreforts.ds-labs.org/ontologies/forgejo#> .

    forgejo:ForgejoConnectorShape a sh:NodeShape ;
        sh:targetClass forgejo:ForgejoConnector .
"#;

fn graph_node(iri: &str) -> NamedNode {
    NamedNode::new(iri).expect("fixture IRI is valid")
}

/// Every `(?s, ?p, ?o)` in `graph`, as returned by `ConfigStore::select`.
fn triples_in(store: &ConfigStore, graph: &str) -> Vec<Vec<(String, String)>> {
    store
        .select(&format!(
            "SELECT ?s ?p ?o WHERE {{ GRAPH <{graph}> {{ ?s ?p ?o }} }}"
        ))
        .expect("the probe query is valid SPARQL")
}

fn subjects_in(store: &ConfigStore, graph: &str) -> Vec<String> {
    triples_in(store, graph)
        .into_iter()
        .filter_map(|row| {
            row.into_iter()
                .find(|(var, _)| var == "s")
                .map(|(_, val)| val)
        })
        .collect()
}

fn register_instance(cg: &ConfigGraph<'_>, label: &str, prefix: &str) {
    cg.set_kg_instance(&KgInstanceConfig {
        label: label.to_string(),
        iri_prefix: prefix.to_string(),
        datadir: Some(format!(
            "/var/lib/contreforts/kg-instances/ontology-{label}"
        )),
    })
    .expect("registering an instance succeeds");
}

fn import_vocab_a(cg: &ConfigGraph<'_>, label: &str) -> ImportedOntologyConfig {
    cg.import_ontology(
        label,
        Some("https://example.org/vocab.ttl"),
        OntologyFormat::Turtle,
        VOCAB_A_TTL.as_bytes(),
    )
    .expect("importing well-formed Turtle succeeds")
}

// ── Item 1: an import is data in a graph, not a row in a registry ──────────────────────────────

/// The load-bearing meaning of "import": the vocabulary must be **reachable by ordinary SPARQL**
/// afterwards. Without this, `import_ontology` could mean "wrote a registry record" forever --
/// `get_imported_ontology` would return `Some`, nothing would error, and no test would notice that
/// the ontology itself was never stored.
#[test]
fn an_imported_ontology_is_reachable_by_sparql_afterwards() {
    let (_dir, store) = store();
    let cg = ConfigGraph::new(&store, ConnectorDeclarations::none());

    let record = import_vocab_a(&cg, "acme-vocab");
    assert_eq!(
        record.graph,
        imported_ontology_graph_iri("acme-vocab"),
        "the record must name the graph the label mints, not some other graph"
    );
    assert_eq!(record.triple_count, 3, "VOCAB_A_TTL holds three triples");

    let subjects = subjects_in(&store, &record.graph);
    assert_eq!(
        subjects.len(),
        3,
        "all three triples must be queryable in the ontology's own graph, got {subjects:?}"
    );
    assert!(
        subjects.iter().all(|s| s == CONCEPT_A),
        "every triple in VOCAB_A_TTL is on ex:conceptA, got {subjects:?}"
    );
}

/// Re-importing a label must **replace**, not accumulate. Asserted on both the count and the
/// disappearance of the old subject: a count alone would still pass if the clear ran but the
/// previous subject somehow survived under a different predicate.
#[test]
fn reimporting_the_same_label_replaces_rather_than_accumulates() {
    let (_dir, store) = store();
    let cg = ConfigGraph::new(&store, ConnectorDeclarations::none());

    import_vocab_a(&cg, "acme-vocab");
    let record = cg
        .import_ontology(
            "acme-vocab",
            None,
            OntologyFormat::Turtle,
            VOCAB_B_TTL.as_bytes(),
        )
        .expect("re-importing under the same label succeeds");

    assert_eq!(
        record.triple_count, 1,
        "the second import replaces the first, so only VOCAB_B_TTL's one triple remains"
    );
    let subjects = subjects_in(&store, &record.graph);
    assert_eq!(
        subjects,
        vec![CONCEPT_B.to_string()],
        "conceptA must be gone, not merged alongside conceptB"
    );
}

// ── Item 2: a refused import destroys nothing ──────────────────────────────────────────────────

/// The single most important test in this file: it distinguishes "the import failed" from "the
/// import failed **and destroyed what was there**".
///
/// A `clear_graph` + `load_from_slice` implementation (the shape `reload_product_graph`
/// legitimately uses, because its source is compiled in and re-loadable) still returns `Err` here,
/// so an error-only assertion would pass while the operator's previous, non-re-derivable
/// vocabulary was already half-overwritten. Only the assertion on the surviving contents catches
/// that.
#[test]
fn a_malformed_payload_leaves_the_previous_import_intact() {
    let (_dir, store) = store();
    let cg = ConfigGraph::new(&store, ConnectorDeclarations::none());

    import_vocab_a(&cg, "acme-vocab");

    let err = cg
        .import_ontology(
            "acme-vocab",
            None,
            OntologyFormat::Turtle,
            VOCAB_MALFORMED_TAIL_TTL.as_bytes(),
        )
        .expect_err("a payload that stops being Turtle partway through must be refused");
    assert!(
        matches!(err, ConfigGraphError::Store(ConfigStoreError::RdfParse(_))),
        "expected ConfigStoreError::RdfParse -- an operator-supplied file that will not parse is a \
         client error, not a server fault; got: {err:?}"
    );

    let record = cg
        .get_imported_ontology("acme-vocab")
        .expect("reading the record back succeeds")
        .expect("the previous import is still registered");
    assert_eq!(
        record.triple_count, 3,
        "the refused import must not have cleared, truncated or partially overwritten the graph"
    );
    let subjects = subjects_in(&store, &record.graph);
    assert!(
        subjects.iter().all(|s| s == CONCEPT_A) && subjects.len() == 3,
        "the original vocabulary must be byte-for-byte still there, got {subjects:?}"
    );
}

/// A payload that parses cleanly and yields **zero** triples is refused before the target graph is
/// cleared. This is the wrong-format-but-parseable case (an HTML error body fed to the N-Triples
/// parser yields no triples rather than an error) and it is the one where "absence presenting as
/// success" becomes silent data loss: without the emptiness check the clear runs, the extend adds
/// nothing, and `import_ontology` reports a successful import of a wiped graph.
#[test]
fn an_empty_payload_is_refused_and_changes_nothing() {
    let (_dir, store) = store();
    let cg = ConfigGraph::new(&store, ConnectorDeclarations::none());

    import_vocab_a(&cg, "acme-vocab");

    let err = cg
        .import_ontology(
            "acme-vocab",
            None,
            OntologyFormat::Turtle,
            b"# only a comment, no triples at all\n",
        )
        .expect_err("a payload with no triples in it must be refused, not silently applied");
    assert!(
        matches!(
            err,
            ConfigGraphError::Store(ConfigStoreError::EmptyGraphPayload { .. })
        ),
        "expected ConfigStoreError::EmptyGraphPayload, got: {err:?}"
    );

    let record = cg
        .get_imported_ontology("acme-vocab")
        .expect("reading the record back succeeds")
        .expect("the previous import is still registered");
    assert_eq!(
        record.triple_count, 3,
        "an empty replacement must leave the previous import untouched"
    );
}

// ── Item 3: the collision guard, and its control ───────────────────────────────────────────────

/// An imported ontology is durable configuration; a KG instance's IRI space is disposable under
/// drop-and-re-sync. An ontology minted inside one would be destroyed by the routine operation
/// this whole issue exists to make safe, so the import is refused at write time.
#[test]
fn import_is_refused_when_its_graph_falls_under_a_registered_instance_prefix() {
    let (_dir, store) = store();
    let cg = ConfigGraph::new(&store, ConnectorDeclarations::none());
    register_instance(&cg, "greedy", SWALLOWING_INSTANCE_PREFIX);

    let err = cg
        .import_ontology(
            "acme-vocab",
            None,
            OntologyFormat::Turtle,
            VOCAB_A_TTL.as_bytes(),
        )
        .expect_err("an ontology graph inside an instance's disposable space must be refused");
    assert!(
        matches!(
            err,
            ConfigGraphError::ImportedOntologyGraphCollidesWithInstance { .. }
        ),
        "expected ConfigGraphError::ImportedOntologyGraphCollidesWithInstance, got: {err:?}"
    );

    let message = err.to_string();
    assert!(
        message.contains("acme-vocab") && message.contains("greedy"),
        "the message must name both the ontology and the instance it collided with, so an \
         operator knows which prefix to narrow; got: {message}"
    );
    assert!(
        cg.get_imported_ontology("acme-vocab")
            .expect("reading back succeeds")
            .is_none(),
        "a refused import must leave no registry record behind"
    );
}

/// [CONTROL] The same import, against a realistically narrow instance prefix, must **succeed** and
/// be queryable. Without this, a `reject_ontology_graph_collision` that refused every import at all
/// times would pass the rejection test above and look green.
#[test]
fn import_succeeds_when_the_registered_instance_prefix_does_not_swallow_it() {
    let (_dir, store) = store();
    let cg = ConfigGraph::new(&store, ConnectorDeclarations::none());
    register_instance(&cg, "primary", REALISTIC_INSTANCE_PREFIX);

    let record = import_vocab_a(&cg, "acme-vocab");
    assert_eq!(record.triple_count, 3);
    assert_eq!(
        subjects_in(&store, &record.graph).len(),
        3,
        "a legitimate import must land in its graph exactly as it does with no instance registered"
    );
}

// ── Item 4: the two graphs `replace_named_graph` may never target ──────────────────────────────

/// `replace_named_graph` is a **second** write path into the same store, next to `insert_quad`.
/// D6's reserved-graph guard lives on `insert_quad`; adding a write path without extending the
/// guard is exactly how a protection becomes decorative, so this pins the new path against the
/// reserved product graph directly -- and asserts the graph's contents survived, not merely that
/// the call returned `Err`.
#[test]
fn the_reserved_product_graph_cannot_be_replaced_through_replace_named_graph() {
    let (_dir, store) = store();
    store
        .reload_product_graph(PRODUCT_FIXTURE_TTL)
        .expect("loading the compiled-in product declarations succeeds");
    let product = graph_node(PRODUCT_GRAPH);
    let before = store
        .named_graph_len(&product)
        .expect("counting the reserved graph succeeds");
    assert!(before > 0, "the fixture must have loaded something");

    let err = store
        .replace_named_graph(&product, RdfFormat::Turtle, VOCAB_A_TTL.as_bytes())
        .expect_err("the reserved product graph is not replaceable at runtime");
    assert!(
        matches!(err, ConfigStoreError::ReservedGraphWrite { .. }),
        "expected ConfigStoreError::ReservedGraphWrite, got: {err:?}"
    );
    assert_eq!(
        store
            .named_graph_len(&product)
            .expect("counting the reserved graph succeeds"),
        before,
        "the refused replace must not have cleared the reserved graph"
    );
}

/// The config graph holds every hand-entered, non-regenerable record in the system. One
/// `replace_named_graph` call pointed at it would erase all of them and report a successful
/// import, so it is refused with its own named error. The assertion on `list_companies` -- not
/// merely on the error -- is what catches a guard that returned `Err` after already clearing.
#[test]
fn the_config_graph_cannot_be_replaced_wholesale() {
    let (_dir, store) = store();
    let cg = ConfigGraph::new(&store, ConnectorDeclarations::none());
    cg.add_company(&CompanyConfig {
        slug: "acme".to_string(),
        name: "Acme".to_string(),
    })
    .expect("company registers cleanly");

    let config = graph_node(CONFIG_GRAPH);
    let err = store
        .replace_named_graph(&config, RdfFormat::Turtle, VOCAB_A_TTL.as_bytes())
        .expect_err("the configuration graph is never replaced wholesale");
    assert!(
        matches!(err, ConfigStoreError::DestructiveReplaceRefused { .. }),
        "expected ConfigStoreError::DestructiveReplaceRefused -- a distinct error from the \
         reserved product graph's, so an operator knows which invariant they hit; got: {err:?}"
    );

    assert!(
        cg.list_companies()
            .expect("listing companies succeeds")
            .iter()
            .any(|c| c.slug == "acme"),
        "the refused replace must not have wiped hand-entered configuration"
    );
}

/// The collision guard checks exactly **one** IRI, so soundness rests on the payload landing in
/// exactly that IRI. A dataset payload naming graphs of its own would break that, so the parser is
/// configured to refuse one outright rather than scatter triples across graphs nobody checked.
///
/// Driven through `RdfFormat::NQuads` deliberately: `OntologyFormat` only offers triple-only
/// syntaxes, so an N-Quads line handed to the *N-Triples* parser would be rejected by that
/// grammar regardless of the parser's named-graph setting -- a test that could not fail if the
/// setting were removed. `replace_named_graph` takes an arbitrary `RdfFormat`, so this exercises
/// the setting itself.
#[test]
fn a_quad_payload_cannot_place_triples_outside_the_target_graph() {
    let (_dir, store) = store();
    let target = graph_node(&imported_ontology_graph_iri("acme-vocab"));
    let sneaky = "https://example.org/sneaky-graph";

    let err = store
        .replace_named_graph(
            &target,
            RdfFormat::NQuads,
            format!("<{CONCEPT_A}> <{RDF}type> <{CONCEPT_B}> <{sneaky}> .\n").as_bytes(),
        )
        .expect_err("a payload that names its own graph must be refused outright");
    assert!(
        matches!(err, ConfigStoreError::RdfParse(_)),
        "expected ConfigStoreError::RdfParse (the parser refuses named graphs), got: {err:?}"
    );

    assert_eq!(
        store
            .named_graph_len(&graph_node(sneaky))
            .expect("counting succeeds"),
        0,
        "not one triple may land in a graph the collision guard never checked"
    );
    assert_eq!(
        store.named_graph_len(&target).expect("counting succeeds"),
        0,
        "nor in the target, since the whole payload was refused"
    );
}

// ── Item 5: the triple count is a live reading, not a stored figure ────────────────────────────

/// The raw SPARQL update route is deliberately still allowed to write ontology graphs, so a
/// persisted `tripleCount` would start disagreeing with reality the moment it did. Here the extra
/// triple is inserted straight through `inner()`, exactly as that route would, and the reported
/// count must follow the graph rather than the record.
#[test]
fn triple_count_is_counted_live_not_read_from_a_stored_copy() {
    let (_dir, store) = store();
    let cg = ConfigGraph::new(&store, ConnectorDeclarations::none());
    let record = import_vocab_a(&cg, "acme-vocab");

    store
        .inner()
        .insert(&Quad::new(
            graph_node("https://example.org/vocab#conceptZ"),
            graph_node(&format!("{RDF}type")),
            graph_node("http://www.w3.org/2004/02/skos/core#Concept"),
            GraphName::NamedNode(graph_node(&record.graph)),
        ))
        .expect("a direct insert into an ontology graph is allowed");

    let reread = cg
        .get_imported_ontology("acme-vocab")
        .expect("reading the record back succeeds")
        .expect("the import is registered");
    assert_eq!(
        reread.triple_count, 4,
        "the count must be read from the store, not from a figure frozen at import time"
    );
}

// ── Item 6: removal is a real removal, and is scoped to one ontology ───────────────────────────

/// Removing an import must drop **both** halves. Deleting only the record would leave the triples
/// in the store forever with nothing left that names them: invisible to every listing, unreachable
/// by any later delete, and still answering alignment queries.
#[test]
fn removing_an_imported_ontology_clears_both_its_graph_and_its_record() {
    let (_dir, store) = store();
    let cg = ConfigGraph::new(&store, ConnectorDeclarations::none());
    let record = import_vocab_a(&cg, "acme-vocab");

    let removed = cg
        .remove_imported_ontology("acme-vocab")
        .expect("removing a registered import succeeds");
    assert_eq!(
        removed, 3,
        "the reported count is what was actually dropped"
    );

    assert!(
        cg.get_imported_ontology("acme-vocab")
            .expect("reading back succeeds")
            .is_none(),
        "the definition record must be gone"
    );
    assert_eq!(
        store
            .named_graph_len(&graph_node(&record.graph))
            .expect("counting succeeds"),
        0,
        "the ontology's triples must be gone too, not orphaned in a graph nothing names"
    );
}

/// #18 Q4's "wipe is not delete" restated for this record type: the caller must be able to tell
/// "there was nothing there" from "it is gone now". A silent `Ok(0)` is indistinguishable from a
/// real removal and would let an operator believe a vocabulary was gone while it is still loaded.
#[test]
fn removing_an_unregistered_ontology_is_a_named_error_not_a_silent_ok() {
    let (_dir, store) = store();
    let cg = ConfigGraph::new(&store, ConnectorDeclarations::none());

    let err = cg
        .remove_imported_ontology("never-imported")
        .expect_err("removing a label that names no import must be a named error");
    assert!(
        matches!(err, ConfigGraphError::ImportedOntologyUnregistered { .. }),
        "expected ConfigGraphError::ImportedOntologyUnregistered, got: {err:?}"
    );
}

/// [CONTROL] One graph per ontology, not one shared graph. The previous test passes just as well
/// against an implementation that clears the whole `IMPORTED_ONTOLOGY_GRAPH_PREFIX` space, or that
/// puts every import in one graph; this is the test that forces the isolation.
#[test]
fn removing_one_ontology_leaves_the_other_untouched() {
    let (_dir, store) = store();
    let cg = ConfigGraph::new(&store, ConnectorDeclarations::none());

    let a = import_vocab_a(&cg, "vocab-a");
    let b = cg
        .import_ontology(
            "vocab-b",
            None,
            OntologyFormat::Turtle,
            VOCAB_B_TTL.as_bytes(),
        )
        .expect("importing a second, distinct vocabulary succeeds");
    assert_ne!(
        a.graph, b.graph,
        "two labels must mint two graphs, or nothing below is isolated"
    );

    cg.remove_imported_ontology("vocab-a")
        .expect("removing the first import succeeds");

    let survivors = cg
        .list_imported_ontologies()
        .expect("listing imports succeeds");
    assert_eq!(
        survivors.len(),
        1,
        "exactly one import must remain, got {survivors:?}"
    );
    assert_eq!(survivors[0].label, "vocab-b");
    assert_eq!(
        survivors[0].triple_count, 1,
        "the surviving import must keep all of its triples"
    );
}

// ── Item 7: startup re-validation (invariant 3) ────────────────────────────────────────────────

/// Invariant 3(c). A record naming an empty graph is a phantom import: listed in the UI, reporting
/// a vocabulary that is not there. The graph is emptied here through `inner()`, behind the typed
/// engine's back, exactly as the still-unconstrained raw SPARQL update route could.
#[test]
fn startup_validation_reports_a_record_whose_graph_is_empty() {
    let (_dir, store) = store();
    let cg = ConfigGraph::new(&store, ConnectorDeclarations::none());
    let record = import_vocab_a(&cg, "acme-vocab");

    store
        .inner()
        .clear_graph(&graph_node(&record.graph))
        .expect("clearing a graph directly succeeds");

    let violations = cg
        .validate_startup()
        .expect_err("a registered import whose graph is empty must be reported");
    assert!(
        violations
            .iter()
            .any(|v| v.contains("acme-vocab") && v.contains("absent")),
        "expected a violation naming the ontology and saying it is absent, got: {violations:?}"
    );
}

/// Invariant 3(b), in the ordering **no write-time guard can see**: the ontology was legitimate
/// when it landed (zero instances registered), and an instance registered afterwards with a broad
/// prefix swallowed it. This is precisely why #18 Q3 answered "write-time *and* startup" rather
/// than either alone -- a test that only drove `import_ontology`'s own guard would leave this
/// undetected.
#[test]
fn startup_validation_reports_an_ontology_swallowed_by_an_instance_registered_afterwards() {
    let (_dir, store) = store();
    let cg = ConfigGraph::new(&store, ConnectorDeclarations::none());

    import_vocab_a(&cg, "acme-vocab");
    register_instance(&cg, "greedy", SWALLOWING_INSTANCE_PREFIX);

    let violations = cg
        .validate_startup()
        .expect_err("an ontology now inside an instance's disposable space must be reported");
    assert!(
        violations
            .iter()
            .any(|v| v.contains("acme-vocab") && v.contains("greedy")),
        "expected a violation naming both the ontology and the instance that now swallows it, \
         got: {violations:?}"
    );
}

/// Invariant 3(a). An import's graph is *minted*, never chosen, so a record naming a graph outside
/// the reserved prefix cannot have been written by `import_ontology` -- it arrived some other way,
/// and the prefix-based collision reasoning does not hold for it. Hand-written through `inner()`,
/// since no typed path can produce it.
#[test]
fn startup_validation_reports_a_record_naming_a_graph_outside_the_import_prefix() {
    let (_dir, store) = store();
    let cg = ConfigGraph::new(&store, ConnectorDeclarations::none());

    let subject = graph_node(&format!("{DATA_NS}imported-ontology/handwritten"));
    let config = graph_node(CONFIG_GRAPH);
    let quads = [
        Quad::new(
            subject.clone(),
            graph_node(&format!("{RDF}type")),
            graph_node(&format!("{CORE_NS}ImportedOntology")),
            GraphName::NamedNode(config.clone()),
        ),
        Quad::new(
            subject.clone(),
            graph_node(&format!("{CORE_NS}label")),
            Term::Literal(Literal::new_simple_literal("handwritten")),
            GraphName::NamedNode(config.clone()),
        ),
        Quad::new(
            subject,
            graph_node(&format!("{CORE_NS}graphIri")),
            Term::Literal(Literal::new_simple_literal("https://example.org/elsewhere")),
            GraphName::NamedNode(config),
        ),
    ];
    for quad in &quads {
        store
            .inner()
            .insert(quad)
            .expect("a direct write into the config graph succeeds");
    }

    let violations = cg
        .validate_startup()
        .expect_err("a record naming a graph outside the reserved import prefix must be reported");
    assert!(
        violations
            .iter()
            .any(|v| v.contains("handwritten") && v.contains(IMPORTED_ONTOLOGY_GRAPH_PREFIX)),
        "expected a violation naming the record and the prefix it is outside of, got: \
         {violations:?}"
    );
}

/// [CONTROL] A store holding a registered instance, a company, a knowledge base and one healthy
/// import must validate clean. Tests 3(a)/(b)/(c) above all pass against a validator that pushes a
/// violation unconditionally; this is the only one that does not.
#[test]
fn startup_validation_passes_on_a_healthy_import() {
    let (_dir, store) = store();
    let cg = ConfigGraph::new(&store, ConnectorDeclarations::none());

    register_instance(&cg, "primary", REALISTIC_INSTANCE_PREFIX);
    cg.add_company(&CompanyConfig {
        slug: "acme".to_string(),
        name: "Acme".to_string(),
    })
    .expect("company registers cleanly");
    cg.set_knowledge_base(
        "acme",
        &KnowledgeBaseConfig {
            label: "support".to_string(),
            kg_instance_label: Some("primary".to_string()),
            graph: Some(format!("{REALISTIC_INSTANCE_PREFIX}graph/support")),
            vector_store_label: "main".to_string(),
        },
    )
    .expect("a KB inside its own instance's prefix is accepted");
    import_vocab_a(&cg, "acme-vocab");

    assert_eq!(
        cg.validate_startup(),
        Ok(()),
        "a healthy store must validate clean -- otherwise the three reporting tests above prove \
         nothing about invariant 3 specifically"
    );
}

// ── Item 8: durability, the claim the whole issue rests on ─────────────────────────────────────

/// #18's durability claim asserted as behaviour rather than left as prose: an imported ontology
/// survives the deletion of the knowledge base and the KG instance it coexisted with, because it
/// does not live in either of their spaces. Any delete path that cleared graphs by prefix would
/// take it with them.
#[test]
fn an_imported_ontology_survives_removing_a_kg_instance_and_a_knowledge_base() {
    let (_dir, store) = store();
    let cg = ConfigGraph::new(&store, ConnectorDeclarations::none());

    register_instance(&cg, "primary", REALISTIC_INSTANCE_PREFIX);
    cg.add_company(&CompanyConfig {
        slug: "acme".to_string(),
        name: "Acme".to_string(),
    })
    .expect("company registers cleanly");
    cg.set_knowledge_base(
        "acme",
        &KnowledgeBaseConfig {
            label: "support".to_string(),
            kg_instance_label: Some("primary".to_string()),
            graph: Some(format!("{REALISTIC_INSTANCE_PREFIX}graph/support")),
            vector_store_label: "main".to_string(),
        },
    )
    .expect("a KB inside its own instance's prefix is accepted");
    let record = import_vocab_a(&cg, "acme-vocab");

    cg.remove_knowledge_base("acme", "support")
        .expect("removing an unreferenced KB succeeds");
    cg.remove_kg_instance("primary")
        .expect("removing an instance no KB belongs to succeeds");

    let survivors = cg
        .list_imported_ontologies()
        .expect("listing imports succeeds");
    assert_eq!(
        survivors,
        vec![ImportedOntologyConfig {
            label: "acme-vocab".to_string(),
            graph: record.graph.clone(),
            source_uri: Some("https://example.org/vocab.ttl".to_string()),
            triple_count: 3,
        }],
        "the imported vocabulary must survive both deletions, contents and provenance intact"
    );
    assert_eq!(
        subjects_in(&store, &record.graph).len(),
        3,
        "and its triples must still be queryable"
    );
}

// ── Item 9: the prefix itself ──────────────────────────────────────────────────────────────────

/// The prefix's own properties, on which every `starts_with` test above rests. The trailing slash
/// is load-bearing: without it `.../graph/ontologyproduct` becomes reachable and the prefix stops
/// being a *proper* prefix, and pointing the constant at `.../data/graph/` instead would make
/// `PRODUCT_GRAPH` and `CONFIG_GRAPH` themselves fall inside the import space.
#[test]
fn the_import_prefix_can_never_collide_with_the_reserved_or_config_graphs() {
    assert!(
        IMPORTED_ONTOLOGY_GRAPH_PREFIX.ends_with('/'),
        "a prefix that is not a path prefix makes every starts_with test in this file unsound"
    );

    for reserved in [PRODUCT_GRAPH, CONFIG_GRAPH] {
        assert!(
            !reserved.starts_with(IMPORTED_ONTOLOGY_GRAPH_PREFIX),
            "'{reserved}' must not fall inside the imported-ontology space"
        );
        assert!(
            !IMPORTED_ONTOLOGY_GRAPH_PREFIX.starts_with(reserved),
            "the imported-ontology space must not fall inside '{reserved}'"
        );
    }

    // The two labels that would collide if the prefix were `.../data/graph/` rather than
    // `.../data/graph/ontology/`.
    for label in ["product", "config"] {
        let minted = imported_ontology_graph_iri(label);
        assert_ne!(minted, PRODUCT_GRAPH);
        assert_ne!(minted, CONFIG_GRAPH);
        assert!(
            minted.starts_with(IMPORTED_ONTOLOGY_GRAPH_PREFIX),
            "'{minted}' must stay inside the space the guard checks"
        );
    }
}
