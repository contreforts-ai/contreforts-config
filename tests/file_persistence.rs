//! Pins the on-disk shape `ConfigStore` now persists to, now that it no longer ships oxigraph's
//! RocksDB backend: an in-memory `Store` mirrored to plain Turtle files (`src/persistence.rs`),
//! one per persisted named graph, written atomically on every mutating call, with a rotating
//! backup tail that a corrupted active file recovers from and self-heals against.
//!
//! These are the tests that fail against the pre-rework, RocksDB-backed `ConfigStore` -- there is
//! no `config_graph.ttl`, no `ontologies/` directory, and no backup tail to speak of against a
//! RocksDB datadir.

use std::path::Path;

use contreforts_config::ConfigStore;
use oxigraph::model::{GraphName, NamedNode, Quad, Term};

const CONFIG_GRAPH: &str = "https://contreforts.ds-labs.org/data/graph/config";
const IMPORTED_ONTOLOGY_GRAPH_PREFIX: &str = "https://contreforts.ds-labs.org/data/graph/ontology/";

fn sample_quad(object: &str) -> Quad {
    Quad::new(
        NamedNode::new("https://contreforts.test/subject").unwrap(),
        NamedNode::new("https://contreforts.test/predicate").unwrap(),
        Term::Literal(object.into()),
        GraphName::NamedNode(NamedNode::new(CONFIG_GRAPH).unwrap()),
    )
}

fn insert(store: &ConfigStore, quad: &Quad) {
    let GraphName::NamedNode(graph) = &quad.graph_name else {
        unreachable!("fixture quads are always in a named graph")
    };
    let subject = match &quad.subject {
        oxigraph::model::NamedOrBlankNode::NamedNode(n) => n,
        _ => unreachable!("fixture subjects are always named nodes"),
    };
    store
        .insert_quad(subject, &quad.predicate, &quad.object, graph)
        .expect("insert succeeds");
}

/// A write to `CONFIG_GRAPH` lands as a human-readable Turtle file at
/// `<datadir>/config_graph.ttl`, containing the written triple -- not an oxigraph RocksDB
/// datadir's SST/manifest files.
#[test]
fn config_graph_writes_persist_to_a_turtle_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = ConfigStore::open(dir.path()).expect("store opens");

    insert(&store, &sample_quad("hello"));

    let file = dir.path().join("config_graph.ttl");
    assert!(file.exists(), "expected {file:?} to exist after a write");
    let contents = std::fs::read_to_string(&file).expect("reading the persisted file");
    assert!(
        contents.contains("hello"),
        "persisted file must contain the written literal, got: {contents:?}"
    );
}

/// An imported ontology graph persists to its own file under `ontologies/`, separate from
/// `config_graph.ttl` -- the "three classes of graph, named separately" split extended to the
/// storage layer.
#[test]
fn imported_ontology_graph_persists_to_its_own_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = ConfigStore::open(dir.path()).expect("store opens");

    let graph_iri = format!("{IMPORTED_ONTOLOGY_GRAPH_PREFIX}my-ontology");
    let graph = NamedNode::new(&graph_iri).unwrap();
    let quad = Quad::new(
        NamedNode::new("https://contreforts.test/concept").unwrap(),
        NamedNode::new("https://contreforts.test/label").unwrap(),
        Term::Literal("Concept".into()),
        GraphName::NamedNode(graph),
    );
    insert(&store, &quad);

    let file = dir.path().join("ontologies").join("my-ontology.ttl");
    assert!(
        file.exists(),
        "expected an imported-ontology write to persist at {file:?}"
    );

    // config_graph.ttl must not have been touched by a write into a different graph.
    assert!(!dir.path().join("config_graph.ttl").exists());
}

/// A reopen at the same path sees data written before -- the file-backed replacement for the old
/// RocksDB datadir's own durability.
#[test]
fn data_survives_close_and_reopen() {
    let dir = tempfile::tempdir().expect("tempdir");
    {
        let store = ConfigStore::open(dir.path()).expect("store opens");
        insert(&store, &sample_quad("persisted"));
    }

    let reopened = ConfigStore::open(dir.path()).expect("store reopens");
    assert!(
        reopened
            .inner()
            .contains(&sample_quad("persisted"))
            .expect("contains check succeeds"),
        "data written before close must still be there after reopen"
    );
}

/// Every write keeps up to 5 prior generations of the graph file (`.bak.1` newest .. `.bak.5`
/// oldest), rotating as further writes happen.
#[test]
fn writes_keep_a_backup_tail_up_to_five_generations() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = ConfigStore::open(dir.path()).expect("store opens");

    for i in 0..7 {
        insert(&store, &sample_quad(&format!("v{i}")));
    }

    let base = dir.path().join("config_graph.ttl");
    assert!(base.exists());
    for generation in 1..=5 {
        let backup = backup_path(&base, generation);
        assert!(
            backup.exists(),
            "expected backup generation {generation} to exist at {backup:?}"
        );
    }
    assert!(
        !backup_path(&base, 6).exists(),
        "must not keep more than 5 backup generations"
    );
}

/// If the active `config_graph.ttl` is corrupted (unparseable), `ConfigStore::open` recovers from
/// the newest backup that still parses, rather than silently starting the graph empty or
/// refusing to start at all.
#[test]
fn a_corrupted_active_file_recovers_from_the_newest_good_backup() {
    let dir = tempfile::tempdir().expect("tempdir");
    {
        let store = ConfigStore::open(dir.path()).expect("store opens");
        insert(&store, &sample_quad("good-version"));
        // A second write rotates "good-version"'s file into `.bak.1` and writes a new active
        // file -- which is then corrupted below, so recovery must fall back to `.bak.1`.
        insert(&store, &sample_quad("about-to-be-corrupted"));
    }

    let file = dir.path().join("config_graph.ttl");
    std::fs::write(&file, b"this is not valid turtle {{{ at all").expect("corrupt the active file");

    let recovered = ConfigStore::open(dir.path()).expect("open must recover from a backup, not error");
    assert!(
        recovered
            .inner()
            .contains(&sample_quad("good-version"))
            .expect("contains check succeeds"),
        "recovery must restore the newest backup's content"
    );

    // Self-healing: the active file must now parse cleanly again (round-trips on a further
    // reopen), without needing another corruption-recovery pass.
    let contents = std::fs::read_to_string(&file).expect("reading the healed file");
    assert!(contents.contains("good-version"));
    let reopened_again = ConfigStore::open(dir.path()).expect("the healed file reopens cleanly");
    assert!(
        reopened_again
            .inner()
            .contains(&sample_quad("good-version"))
            .expect("contains check succeeds")
    );
}

/// A brand-new datadir (no `config_graph.ttl` yet) is not corruption -- `open` must succeed with
/// an empty store, not error.
#[test]
fn a_fresh_datadir_with_no_persisted_file_opens_empty_not_as_an_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = ConfigStore::open(dir.path()).expect("a fresh datadir must open cleanly");
    assert!(
        store
            .select(&format!(
                "SELECT ?s WHERE {{ GRAPH <{CONFIG_GRAPH}> {{ ?s ?p ?o }} }}"
            ))
            .expect("select succeeds")
            .is_empty(),
        "a fresh datadir's config graph must start empty"
    );
}

fn backup_path(path: &Path, generation: usize) -> std::path::PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(format!(".bak.{generation}"));
    std::path::PathBuf::from(name)
}
