//! Migrating a *pre-existing* `ConfigStore` datadir -- one written back when `ConfigStore::open`
//! itself opened a RocksDB-backed `oxigraph::store::Store` at that path, before this crate
//! dropped that backend for per-graph Turtle-file persistence (`src/persistence.rs`'s module
//! doc) -- into the new format, in place.
//!
//! # Why this is a distinct migration from `tests/migration.rs`
//!
//! `migrate_config_graph_if_needed` (pinned by `tests/migration.rs`) copies `CONFIG_GRAPH` out of
//! contreforts-kg's *combined* store -- a different physical store this crate never wrote to
//! itself, and a migration only deployments that ran the pre-D2 combined-store era ever need.
//!
//! This one is the far more common case: *every* real `ConfigStore` deployment that existed
//! before the RocksDB-backend drop has its own RocksDB files sitting at its own datadir path,
//! whether or not it ever also went through the combined-store migration. Without this, the
//! moment a binary upgrades to the Turtle-file-backed `ConfigStore::open`, that data does not
//! error and does not merge -- it simply never loads, because `Store::new()` starts empty and no
//! `config_graph.ttl` exists yet. `ConfigStore::open` succeeds, the store reports as freshly
//! empty, and every company, connector, credential and imported ontology an operator ever
//! hand-entered is silently invisible. This crate's own recurring rule
//! (contreforts-workspace#18): configuration is hand-entered and not re-derivable, so this must
//! never be a silent, undetected data loss.
//!
//! # What this file pins
//!
//! `contreforts_config::migrate_rocksdb_datadir_if_needed(&config_store)` does not
//! exist yet -- this file does not compile against the current tree. That is the sanctioned RED
//! (`crates/contreforts-kg/CONTRIBUTING.md` §3): a compile error naming the missing item is
//! evidence enough.
//!
//! Called once, right after `ConfigStore::open` (so its own Turtle-file load has already
//! happened, and the "already populated" check below sees real content, not a stale read),
//! before any consumer touches the store. It detects a RocksDB datadir at `config_store`'s own
//! path via oxigraph's on-disk backend's `CURRENT` marker file, reads `CONFIG_GRAPH` and every
//! imported-ontology graph out of it (never `PRODUCT_GRAPH`: rebuilt from compiled-in Turtle at
//! every startup regardless), writes them into the new in-memory store, and persists them to
//! their Turtle files -- leaving the RocksDB files themselves untouched, copy never move, exactly
//! like the combined-store migration's own rollback story.
//!
//! Gated behind the `legacy-combined-store-migration` Cargo feature (off by default): opening the
//! fixture "old RocksDB-backed ConfigStore" below needs oxigraph's RocksDB backend, which this
//! crate otherwise no longer links. Run with
//! `cargo test -p contreforts-config --features legacy-combined-store-migration`.
#![cfg(feature = "legacy-combined-store-migration")]

use std::collections::BTreeSet;
use std::path::Path;

use contreforts_config::{
    CompanyConfig, ConfigGraph, ConfigStore, MigrationOutcome, OntologyFormat,
    imported_ontology_graph_iri, migrate_rocksdb_datadir_if_needed,
};
use contreforts_core::namespaces::CONFIG_GRAPH;
use contreforts_declaration::ConnectorDeclarations;
use oxigraph::model::{NamedNode, Quad};
use oxigraph::store::Store;

const ONTOLOGY_LABEL: &str = "widgets";
const ONTOLOGY_TTL: &str = r#"
    @prefix ex: <https://contreforts.ds-labs.org/ontologies/widgets#> .
    ex:Widget a <http://www.w3.org/2000/01/rdf-schema#Class> .
"#;

fn config_store_at(path: &Path) -> ConfigStore {
    ConfigStore::open(path).expect("config store opens at a fresh path")
}

fn all_quads(store: &Store) -> Vec<Quad> {
    store
        .quads_for_pattern(None, None, None, None)
        .collect::<Result<Vec<_>, _>>()
        .expect("reading every quad back from the store succeeds")
}

fn graph_quads(store: &Store, graph_iri: &str) -> BTreeSet<(String, String, String)> {
    let graph = NamedNode::new(graph_iri).expect("graph IRI is valid");
    store
        .quads_for_pattern(None, None, None, Some((&graph).into()))
        .map(|q| {
            let q = q.expect("reading a quad back from the store succeeds");
            (
                q.subject.to_string(),
                q.predicate.to_string(),
                q.object.to_string(),
            )
        })
        .collect()
}

/// Builds a fixture "pre-upgrade" `ConfigStore` datadir at `path`: a genuine RocksDB-backed
/// `oxigraph::store::Store`, carrying a `CONFIG_GRAPH` company record, one imported ontology
/// (both its `ImportedOntology` record in `CONFIG_GRAPH` and its own named graph's triples), and
/// -- so the "never resurrect `PRODUCT_GRAPH`" rule has something real to reject -- a
/// `PRODUCT_GRAPH` triple too, exactly as an old binary's own `reload_product_graph` would have
/// left one sitting in that same physical store.
///
/// The config-shaped triples are generated through `contreforts_config::ConfigGraph` against a
/// throwaway, in-memory-backed scratch `ConfigStore` (so their shape can never drift from what
/// the real engine writes), then every quad is copied into a genuine RocksDB store at `path` --
/// the same fixture-construction trick `tests/migration.rs::build_combined_store` uses, for the
/// same reason: `ConfigStore` itself is no longer RocksDB-backed, so it can no longer double as
/// this fixture's own on-disk format.
fn build_legacy_rocksdb_datadir(path: &Path) -> BTreeSet<(String, String, String)> {
    let scratch_dir = tempfile::tempdir().expect("tempdir for the fixture-generating scratch store");
    let store = config_store_at(&scratch_dir.path().join("scratch"));
    let cg = ConfigGraph::new(&store, ConnectorDeclarations::none());

    cg.add_company(&CompanyConfig {
        slug: "acme".to_string(),
        name: "Acme".to_string(),
    })
    .expect("company registers cleanly");

    cg.import_ontology(
        ONTOLOGY_LABEL,
        Some("https://example.com/widgets.ttl"),
        OntologyFormat::Turtle,
        ONTOLOGY_TTL.as_bytes(),
    )
    .expect("importing the fixture ontology succeeds");

    store
        .reload_product_graph(
            r#"
            @prefix sh: <http://www.w3.org/ns/shacl#> .
            @prefix ex: <https://contreforts.ds-labs.org/ontologies/example#> .
            ex:ExampleShape a sh:NodeShape .
        "#,
        )
        .expect("loading the reserved product graph succeeds");

    let config_graph_triples = graph_quads(store.inner(), CONFIG_GRAPH);
    let fixture_quads = all_quads(store.inner());
    drop(store);

    let legacy = Store::open(path).expect("opening the RocksDB-backed legacy datadir fixture");
    for quad in fixture_quads {
        legacy
            .insert(&quad)
            .expect("copying a fixture quad into the legacy RocksDB store");
    }
    drop(legacy);

    config_graph_triples
}

// ── A full recovery copies CONFIG_GRAPH and the imported ontology, never PRODUCT_GRAPH ────────

#[test]
fn migrates_config_graph_and_ontology_graph_but_never_product_graph() {
    let datadir = tempfile::tempdir().expect("tempdir for the legacy datadir");
    let config_graph_triples = build_legacy_rocksdb_datadir(datadir.path());

    let ontology_graph_iri = imported_ontology_graph_iri(ONTOLOGY_LABEL);

    // `ConfigStore::open` on a RocksDB-only datadir must succeed and simply start empty -- no
    // `config_graph.ttl` exists yet at this point.
    let config_store = config_store_at(datadir.path());
    assert!(
        graph_quads(config_store.inner(), CONFIG_GRAPH).is_empty(),
        "precondition: ConfigStore::open on an un-migrated RocksDB datadir must start empty, \
         not error"
    );

    let outcome = migrate_rocksdb_datadir_if_needed(&config_store)
        .expect("migrating a fresh config store from a legacy RocksDB datadir succeeds");

    let expected_ontology_triples = {
        let legacy = Store::open(datadir.path()).expect("legacy RocksDB store reopens");
        let triples = graph_quads(&legacy, &ontology_graph_iri);
        drop(legacy);
        triples
    };

    match outcome {
        MigrationOutcome::Migrated { triples_copied } => {
            assert_eq!(
                triples_copied,
                config_graph_triples.len() + expected_ontology_triples.len(),
                "reported count must equal CONFIG_GRAPH's triples plus the imported ontology \
                 graph's triples"
            );
        }
        other => panic!("expected MigrationOutcome::Migrated, got {other:?}"),
    }

    assert_eq!(
        graph_quads(config_store.inner(), CONFIG_GRAPH),
        config_graph_triples,
        "every CONFIG_GRAPH triple from the legacy RocksDB store must appear in the new store"
    );
    assert_eq!(
        graph_quads(config_store.inner(), &ontology_graph_iri),
        expected_ontology_triples,
        "the imported ontology's own graph must be migrated too, not just CONFIG_GRAPH"
    );
    assert!(
        !expected_ontology_triples.is_empty(),
        "fixture precondition: the ontology graph must genuinely hold triples"
    );

    assert!(
        graph_quads(config_store.inner(), contreforts_config::PRODUCT_GRAPH).is_empty(),
        "PRODUCT_GRAPH must never be migrated -- it is rebuilt from compiled-in Turtle at every \
         startup, and a stale copy would only ever be wrong"
    );

    // The migrated content must actually be durable on the new Turtle files, not merely
    // in-memory -- a fresh `ConfigStore::open` at the same path, with no further migration call,
    // must see it.
    drop(config_store);
    let reopened = config_store_at(datadir.path());
    assert_eq!(
        graph_quads(reopened.inner(), CONFIG_GRAPH),
        config_graph_triples,
        "the migrated CONFIG_GRAPH content must have been persisted to disk, not just held \
         in-memory for the lifetime of the original ConfigStore handle"
    );
}

// ── The source RocksDB files are left intact ───────────────────────────────────────────────────

#[test]
fn legacy_rocksdb_datadir_is_left_intact_after_migration() {
    let datadir = tempfile::tempdir().expect("tempdir for the legacy datadir");
    let config_graph_triples_before = build_legacy_rocksdb_datadir(datadir.path());

    let config_store = config_store_at(datadir.path());
    migrate_rocksdb_datadir_if_needed(&config_store).expect("migration succeeds");
    drop(config_store);

    let reopened_legacy =
        Store::open(datadir.path()).expect("the legacy RocksDB store must still be openable");
    assert_eq!(
        graph_quads(&reopened_legacy, CONFIG_GRAPH),
        config_graph_triples_before,
        "migration copies, it does not move or delete -- the legacy RocksDB store's own \
         CONFIG_GRAPH must be unchanged"
    );
}

// ── Idempotent ──────────────────────────────────────────────────────────────────────────────────

#[test]
fn re_running_migration_against_an_already_migrated_store_is_a_no_op() {
    let datadir = tempfile::tempdir().expect("tempdir for the legacy datadir");
    build_legacy_rocksdb_datadir(datadir.path());

    let config_store = config_store_at(datadir.path());
    let first =
        migrate_rocksdb_datadir_if_needed(&config_store).expect("first migration succeeds");
    assert!(
        matches!(first, MigrationOutcome::Migrated { .. }),
        "the first call against a fresh config store must actually migrate, got {first:?}"
    );
    let triples_after_first = graph_quads(config_store.inner(), CONFIG_GRAPH);

    let second =
        migrate_rocksdb_datadir_if_needed(&config_store).expect("re-running must never error");
    assert!(
        !matches!(second, MigrationOutcome::Migrated { .. }),
        "a second run against an already-migrated config store must not report a fresh \
         migration, got {second:?}"
    );
    assert_eq!(
        graph_quads(config_store.inner(), CONFIG_GRAPH),
        triples_after_first,
        "a second run must not duplicate triples or otherwise change what the first run wrote"
    );
}

// ── Nothing-to-migrate cases, each distinct ────────────────────────────────────────────────────

#[test]
fn no_rocksdb_files_at_all_is_a_clean_no_op() {
    let datadir = tempfile::tempdir().expect("tempdir, never populated with RocksDB files");
    let config_store = config_store_at(datadir.path());

    let outcome = migrate_rocksdb_datadir_if_needed(&config_store)
        .expect("a datadir with no legacy RocksDB files must not be an error");
    assert!(
        !matches!(outcome, MigrationOutcome::Migrated { .. }),
        "nothing exists to migrate from, so this must not report a migration, got {outcome:?}"
    );
}

#[test]
fn config_store_already_populated_is_left_untouched() {
    let datadir = tempfile::tempdir().expect("tempdir for the legacy datadir");
    build_legacy_rocksdb_datadir(datadir.path());

    let config_store = config_store_at(datadir.path());
    // Seed the new store directly, as if migration (or a hand-entry) had already populated it --
    // distinct company from the fixture's "acme", so a merge would be visible.
    let cg = ConfigGraph::new(&config_store, ConnectorDeclarations::none());
    cg.add_company(&CompanyConfig {
        slug: "already-here".to_string(),
        name: "Already Here Inc".to_string(),
    })
    .expect("seeding the config store directly succeeds");
    let triples_before = graph_quads(config_store.inner(), CONFIG_GRAPH);

    let outcome = migrate_rocksdb_datadir_if_needed(&config_store)
        .expect("migration against an already-populated config store must not error");
    assert!(
        !matches!(outcome, MigrationOutcome::Migrated { .. }),
        "an already-populated config store must not be reported as freshly migrated, got \
         {outcome:?}"
    );
    assert_eq!(
        graph_quads(config_store.inner(), CONFIG_GRAPH),
        triples_before,
        "the config store's own pre-existing data must be exactly what it was -- the legacy \
         RocksDB store's distinct 'acme' company must not have been merged in"
    );
}

#[test]
fn legacy_rocksdb_store_present_but_empty_is_a_clean_no_op() {
    let datadir = tempfile::tempdir().expect("tempdir for the legacy datadir");
    // Create a genuine RocksDB store at this path (so the `CURRENT` marker file exists), but
    // never write anything into it.
    let legacy = Store::open(datadir.path()).expect("legacy RocksDB store opens");
    drop(legacy);

    let config_store = config_store_at(datadir.path());
    let outcome = migrate_rocksdb_datadir_if_needed(&config_store)
        .expect("an empty legacy RocksDB store must not be an error");
    assert!(
        !matches!(outcome, MigrationOutcome::Migrated { .. }),
        "there is nothing to migrate, so this must not report a migration, got {outcome:?}"
    );
    assert!(
        graph_quads(config_store.inner(), CONFIG_GRAPH).is_empty(),
        "the config store must remain empty when the legacy store held nothing"
    );
}

/// A config store holding *only* `PRODUCT_GRAPH` data (D6's every-startup reload, which may well
/// have already run by the time migration runs) must not be mistaken for "already migrated" --
/// mirrors `tests/migration.rs`'s identical concern for the combined-store migration.
#[test]
fn product_graph_data_alone_does_not_count_as_an_already_populated_config_store() {
    let datadir = tempfile::tempdir().expect("tempdir for the legacy datadir");
    let config_graph_triples = build_legacy_rocksdb_datadir(datadir.path());

    let config_store = config_store_at(datadir.path());
    config_store
        .reload_product_graph(
            r#"
            @prefix sh: <http://www.w3.org/ns/shacl#> .
            @prefix ex: <https://contreforts.ds-labs.org/ontologies/example#> .
            ex:ExampleShape a sh:NodeShape .
        "#,
        )
        .expect("loading the reserved product graph succeeds");

    let outcome = migrate_rocksdb_datadir_if_needed(&config_store)
        .expect("migration must still run when only the reserved product graph is populated");
    assert!(
        matches!(outcome, MigrationOutcome::Migrated { .. }),
        "product-graph-only data must not be mistaken for an already-migrated config store, got \
         {outcome:?}"
    );
    assert_eq!(
        graph_quads(config_store.inner(), CONFIG_GRAPH),
        config_graph_triples,
        "the real configuration must have been copied in, exactly"
    );
}
