//! Per-instance identity (contreforts/contreforts-workspace#58 D4; #18 Q2): a KG data instance
//! record is a **label** plus an **independently-assigned IRI prefix** -- assigned, not derived
//! from the label, so renaming an instance never rewrites a single stored subject IRI.
//!
//! `contreforts_config::KgInstanceConfig` and `ConfigGraph::{set,get,rename}_kg_instance` do not
//! exist yet. This file does not compile against the current `develop` -- the sanctioned RED
//! (`crates/contreforts-kg/CONTRIBUTING.md` §3, referenced from this crate's own PR description
//! since this crate has no `CONTRIBUTING.md` of its own yet).
//!
//! What the entity-IRI side of this contract (the prefix, once resolved, actually being what
//! `contreforts-kg`'s builders use, and never the label) looks like lives in
//! `crates/contreforts-kg/tests/instance_prefix.rs` instead -- that crate depends on this one (the
//! D3c shim dependency), so it can build a bare `KgInstanceConfig` value with no store involved.
//! This file is the configuration side only: the record's own persistence, CRUD and stability
//! across a store reopen.

use contreforts_config::{ConfigGraph, ConfigStore, KgInstanceConfig};
use contreforts_declaration::ConnectorDeclarations;

fn store() -> (tempfile::TempDir, ConfigStore) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config_store");
    let store = ConfigStore::open(&path).expect("store opens at a fresh path");
    (dir, store)
}

/// The sharpest test in the set -- #18 Q2's entire purpose. Renaming an instance must leave its
/// assigned prefix byte-identical, because every subject IRI that instance's entity data was ever
/// built from is built from that exact string.
#[test]
fn renaming_an_instance_leaves_its_assigned_prefix_byte_identical() {
    let (_dir, store) = store();
    let cg = ConfigGraph::new(&store, ConnectorDeclarations::none());

    let assigned_prefix = "https://contreforts.ds-labs.org/data/instance/f3a9c1e2/".to_string();
    cg.set_kg_instance(&KgInstanceConfig {
        label: "primary".to_string(),
        iri_prefix: assigned_prefix.clone(),
    })
    .expect("a fresh instance registers cleanly");

    let before = cg
        .get_kg_instance("primary")
        .expect("lookup succeeds")
        .expect("the instance we just registered is found by its label");
    assert_eq!(before.iri_prefix, assigned_prefix);

    cg.rename_kg_instance("primary", "primary-renamed")
        .expect("renaming an existing instance succeeds");

    let after = cg
        .get_kg_instance("primary-renamed")
        .expect("lookup succeeds")
        .expect("the instance is found under its new label after the rename");

    assert_eq!(
        after.iri_prefix, assigned_prefix,
        "renaming must not change the assigned prefix -- every subject IRI this instance's \
         entity data was ever built from depends on this string staying byte-identical \
         (contreforts-workspace#18 Q2)"
    );
    assert_eq!(
        after.iri_prefix, before.iri_prefix,
        "the prefix observed before and after the rename must be the exact same bytes, not \
         merely equal-looking strings"
    );

    assert!(
        cg.get_kg_instance("primary")
            .expect("lookup succeeds")
            .is_none(),
        "the old label must no longer resolve an instance -- this was a rename, not a copy \
         left behind under the old name"
    );
}

/// Two instances get different, independently-assigned prefixes. Uses two different labels, so
/// this is the "ordinary" case; `instance_prefix.rs` in `contreforts-kg` covers the sharper
/// same-label variant that a label-derived prefix would fail.
#[test]
fn two_instances_get_independently_assigned_prefixes() {
    let (_dir, store) = store();
    let cg = ConfigGraph::new(&store, ConnectorDeclarations::none());

    cg.set_kg_instance(&KgInstanceConfig {
        label: "north".to_string(),
        iri_prefix: "https://contreforts.ds-labs.org/data/instance/north-7a1/".to_string(),
    })
    .expect("registering the first instance succeeds");
    cg.set_kg_instance(&KgInstanceConfig {
        label: "south".to_string(),
        iri_prefix: "https://contreforts.ds-labs.org/data/instance/south-9c4/".to_string(),
    })
    .expect("registering the second instance succeeds");

    let north = cg
        .get_kg_instance("north")
        .expect("lookup succeeds")
        .expect("the first instance is found");
    let south = cg
        .get_kg_instance("south")
        .expect("lookup succeeds")
        .expect("the second instance is found");

    assert_ne!(
        north.iri_prefix, south.iri_prefix,
        "two distinct instances must carry distinct assigned prefixes"
    );

    let all = cg
        .list_kg_instances()
        .expect("listing all registered instances succeeds");
    assert_eq!(
        all.len(),
        2,
        "both registered instances must be present in the listing, got {all:?}"
    );
}

/// A KG instance's assigned prefix must survive a store close/reopen -- otherwise every IRI it
/// ever wrote becomes unreachable the moment the process restarts. Same tempdir + reopen shape as
/// `tests/store.rs`'s own coverage of `ConfigStore::open`.
#[test]
fn prefix_survives_store_close_and_reopen() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config_store");

    let assigned_prefix = "https://contreforts.ds-labs.org/data/instance/reopen-4e2c/".to_string();
    {
        let store = ConfigStore::open(&path).expect("store opens at a fresh path");
        let cg = ConfigGraph::new(&store, ConnectorDeclarations::none());
        cg.set_kg_instance(&KgInstanceConfig {
            label: "durable".to_string(),
            iri_prefix: assigned_prefix.clone(),
        })
        .expect("registering the instance succeeds");
    } // `store` (and its Arc<Store>) is dropped here, closing the on-disk store.

    let reopened = ConfigStore::open(&path).expect("store reopens at the same path");
    let cg = ConfigGraph::new(&reopened, ConnectorDeclarations::none());
    let found = cg
        .get_kg_instance("durable")
        .expect("lookup succeeds")
        .expect("the instance registered before close is still there after reopen");

    assert_eq!(
        found.iri_prefix, assigned_prefix,
        "a KG instance's assigned prefix must survive a store close/reopen -- otherwise every \
         subject IRI it ever wrote becomes permanently unreachable under its own prefix"
    );
}
