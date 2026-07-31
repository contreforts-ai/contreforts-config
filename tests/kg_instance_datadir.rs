//! Per-instance **datadir** (contreforts/contreforts-workspace#58 D8, part 1 -- the "Done when"
//! on #58 requires each `contreforts-kg` data instance to have its own datadir, alongside the
//! label and independently-assigned IRI prefix D4 already pinned in `tests/kg_instance.rs`).
//!
//! Split out the same way `crates/contreforts-kg/tests/instance_prefix.rs` was split from
//! `tests/kg_instance.rs` for D4 (see that file's own doc comment): the record's *own*
//! persistence, CRUD and reopen-stability lives here, one property at a time, so this file's
//! diff is reviewable independently of D4's already-merged prefix coverage. `tests/kg_instance.rs`
//! and every other file in this crate that already constructs a `KgInstanceConfig` value is
//! patched (in the same commit as this file) to carry a `datadir` too, since it is a new
//! *required* field -- see this crate's PR description for the full list of touched fixtures and
//! why each one's own datadir was chosen to be distinct from every other instance registered into
//! the same store.
//!
//! `KgInstanceConfig::datadir` does not exist yet. This file does not compile against `develop`
//! at `647af20` -- the sanctioned compile-error RED (`crates/contreforts-kg/CONTRIBUTING.md` §3).
//!
//! What a *different* instance's datadir being used to actually open its own physical store looks
//! like is out of this crate's scope -- `contreforts-kg` owns `GraphStore::open`, so that half of
//! the contract is pinned in `crates/contreforts-kg/tests/instance_store.rs` instead. This file is
//! the configuration side only: the record's own field, its persistence, and the uniqueness
//! guard that keeps two instances from ever being told to write into the same directory.

use contreforts_config::{ConfigGraph, ConfigStore, KgInstanceConfig};
use contreforts_declaration::ConnectorDeclarations;

fn store() -> (tempfile::TempDir, ConfigStore) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config_store");
    let store = ConfigStore::open(&path).expect("store opens at a fresh path");
    (dir, store)
}

/// The plain round trip: a KG instance's `datadir` must come back exactly as it was registered,
/// not merely something truthy -- a guard or a UI reading only `label`/`iri_prefix` back would
/// silently miss a dropped `datadir` the way a no-op field would.
#[test]
fn datadir_round_trips_through_set_and_get() {
    let (_dir, store) = store();
    let cg = ConfigGraph::new(&store, ConnectorDeclarations::none());

    let assigned_datadir = "/var/lib/contreforts/kg-instances/roundtrip".to_string();
    cg.set_kg_instance(&KgInstanceConfig {
        label: "roundtrip".to_string(),
        iri_prefix: "https://contreforts.ds-labs.org/data/instance/roundtrip-1a2b/".to_string(),
        datadir: assigned_datadir.clone(),
    })
    .expect("a fresh instance registers cleanly");

    let got = cg
        .get_kg_instance("roundtrip")
        .expect("lookup succeeds")
        .expect("the instance we just registered is found by its label");

    assert_eq!(
        got.datadir, assigned_datadir,
        "the instance's datadir must round-trip exactly through set_kg_instance/get_kg_instance"
    );

    let listed = cg.list_kg_instances().expect("listing succeeds");
    let listed = listed
        .iter()
        .find(|i| i.label == "roundtrip")
        .expect("the registered instance appears in the listing");
    assert_eq!(
        listed.datadir, assigned_datadir,
        "the datadir must also round-trip through list_kg_instances, not only the single-fetch \
         path -- a caller that reads only one of the two would silently miss the other"
    );
}

/// A KG instance's assigned datadir must survive a store close/reopen -- otherwise the moment the
/// process restarts, nothing remembers where that instance's own knowledge-graph data lives on
/// disk, which is the entire point of recording it at all (contreforts-workspace#58's "Done
/// when": "each instance has its own datadir"). Same tempdir + reopen shape as
/// `tests/kg_instance.rs`'s own `prefix_survives_store_close_and_reopen`.
#[test]
fn datadir_survives_store_close_and_reopen() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config_store");

    let assigned_datadir = "/var/lib/contreforts/kg-instances/reopen".to_string();
    {
        let store = ConfigStore::open(&path).expect("store opens at a fresh path");
        let cg = ConfigGraph::new(&store, ConnectorDeclarations::none());
        cg.set_kg_instance(&KgInstanceConfig {
            label: "durable-datadir".to_string(),
            iri_prefix: "https://contreforts.ds-labs.org/data/instance/reopen-dd-1/".to_string(),
            datadir: assigned_datadir.clone(),
        })
        .expect("registering the instance succeeds");
    } // `store` (and its Arc<Store>) is dropped here, closing the on-disk store.

    let reopened = ConfigStore::open(&path).expect("store reopens at the same path");
    let cg = ConfigGraph::new(&reopened, ConnectorDeclarations::none());
    let found = cg
        .get_kg_instance("durable-datadir")
        .expect("lookup succeeds")
        .expect("the instance registered before close is still there after reopen");

    assert_eq!(
        found.datadir, assigned_datadir,
        "a KG instance's datadir must survive a store close/reopen -- otherwise, after every \
         restart, nothing on disk remembers where that instance's own knowledge-graph data lives"
    );
}

/// Two instances must be able to have different datadirs -- the ordinary case, mirroring
/// `tests/kg_instance.rs`'s own `two_instances_get_independently_assigned_prefixes` for the
/// prefix field. Without this, nothing would distinguish "two instances" from "one instance with
/// two names."
#[test]
fn two_instances_get_distinct_datadirs() {
    let (_dir, store) = store();
    let cg = ConfigGraph::new(&store, ConnectorDeclarations::none());

    cg.set_kg_instance(&KgInstanceConfig {
        label: "east".to_string(),
        iri_prefix: "https://contreforts.ds-labs.org/data/instance/east-4f1/".to_string(),
        datadir: "/var/lib/contreforts/kg-instances/east".to_string(),
    })
    .expect("registering the first instance succeeds");
    cg.set_kg_instance(&KgInstanceConfig {
        label: "west".to_string(),
        iri_prefix: "https://contreforts.ds-labs.org/data/instance/west-8b2/".to_string(),
        datadir: "/var/lib/contreforts/kg-instances/west".to_string(),
    })
    .expect("registering the second instance succeeds");

    let east = cg
        .get_kg_instance("east")
        .expect("lookup succeeds")
        .expect("the first instance is found");
    let west = cg
        .get_kg_instance("west")
        .expect("lookup succeeds")
        .expect("the second instance is found");

    assert_ne!(
        east.datadir, west.datadir,
        "two distinct instances must carry distinct datadirs"
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

/// Re-registering the exact same `(label, iri_prefix, datadir)` triple must stay idempotent --
/// `set_kg_instance`'s own doc comment already promises this for `(label, iri_prefix)`; this
/// proves the new datadir-uniqueness guard (pinned below) does not turn a legitimate repeat
/// registration of the *same* instance into a spurious "datadir already registered" rejection
/// against itself. Without this control, a guard that compared datadir against every registered
/// instance *including this one's own prior record* would reject its own re-registration --
/// exactly the "a guard that examines nothing still reports success" trap's mirror image: a guard
/// that examines the wrong set of instances would report failure where there is none.
#[test]
fn re_registering_the_exact_same_instance_including_datadir_is_idempotent() {
    let (_dir, store) = store();
    let cg = ConfigGraph::new(&store, ConnectorDeclarations::none());

    let config = KgInstanceConfig {
        label: "steady".to_string(),
        iri_prefix: "https://contreforts.ds-labs.org/data/instance/steady-2c3/".to_string(),
        datadir: "/var/lib/contreforts/kg-instances/steady".to_string(),
    };

    cg.set_kg_instance(&config)
        .expect("the first registration succeeds");
    cg.set_kg_instance(&config).expect(
        "re-registering the exact same (label, iri_prefix, datadir) triple must be idempotent, \
         not rejected as a datadir collision against itself",
    );

    let got = cg
        .get_kg_instance("steady")
        .expect("lookup succeeds")
        .expect("the instance is still registered");
    assert_eq!(got.datadir, config.datadir);
    assert_eq!(got.iri_prefix, config.iri_prefix);
}

/// Renaming an instance (`ConfigGraph::rename_kg_instance`, added by D4) must leave its `datadir`
/// intact, exactly as D4 already requires for `iri_prefix` -- the datadir is what
/// `GraphStore::open_for_instance` (`crates/contreforts-kg/tests/instance_store.rs`) actually
/// opens; losing it on rename would silently orphan the instance's own on-disk store the next
/// time anything tried to open it under the new label. Found by exercising this file against a
/// scratch implementation that only carried `iri_prefix` across a rename (mirroring
/// `rename_kg_instance`'s pre-D8 body, which predates this field and only knew to copy `label`
/// and `iri_prefix`) -- this is exactly the "scope of a check, not its logic" trap the task
/// warns about, applied to a rename path rather than a write-time guard.
#[test]
fn renaming_an_instance_leaves_its_datadir_intact() {
    let (_dir, store) = store();
    let cg = ConfigGraph::new(&store, ConnectorDeclarations::none());

    let assigned_datadir = "/var/lib/contreforts/kg-instances/rename-me".to_string();
    cg.set_kg_instance(&KgInstanceConfig {
        label: "before-rename".to_string(),
        iri_prefix: "https://contreforts.ds-labs.org/data/instance/rename-dd-1/".to_string(),
        datadir: assigned_datadir.clone(),
    })
    .expect("a fresh instance registers cleanly");

    cg.rename_kg_instance("before-rename", "after-rename")
        .expect("renaming an existing instance succeeds");

    let after = cg
        .get_kg_instance("after-rename")
        .expect("lookup succeeds")
        .expect("the instance is found under its new label after the rename");

    assert_eq!(
        after.datadir, assigned_datadir,
        "renaming an instance must not change its datadir -- that is the one on-disk location \
         its knowledge-graph data actually lives at, and losing it on rename would silently \
         orphan the instance's own store"
    );
}

/// contreforts-workspace#58 D8 part 1: two instances must never share a **datadir** -- doing so
/// means two independently-assigned IRI prefixes' entity data would land in the same physical
/// Oxigraph store, a data-corrupting collision that no later check would notice (unlike a shared
/// label or prefix, which at least stays internally consistent within one store; a shared datadir
/// means two *processes'* worth of writes interleave on disk). `set_kg_instance` must reject this
/// even when both label and prefix differ, naming the offending datadir and which existing
/// instance (by label) already claims it.
///
/// Per the task's instruction to assert on message content rather than an error variant (a2
/// chooses the error shape for this new check): only `err.to_string()` is inspected below.
#[test]
fn a_second_instance_on_an_already_registered_datadir_is_rejected() {
    let (_dir, store) = store();
    let cg = ConfigGraph::new(&store, ConnectorDeclarations::none());

    let shared_datadir = "/var/lib/contreforts/kg-instances/contested".to_string();
    cg.set_kg_instance(&KgInstanceConfig {
        label: "claimant".to_string(),
        iri_prefix: "https://contreforts.ds-labs.org/data/instance/claimant-9a1/".to_string(),
        datadir: shared_datadir.clone(),
    })
    .expect("registering the first instance succeeds");

    let err = cg
        .set_kg_instance(&KgInstanceConfig {
            label: "squatter".to_string(),
            iri_prefix: "https://contreforts.ds-labs.org/data/instance/squatter-9a2/".to_string(),
            datadir: shared_datadir.clone(),
        })
        .expect_err(
            "a second instance, under a different label and a different prefix, must not \
             silently reuse a datadir already claimed by a different instance -- two instances \
             writing into the same physical store would silently corrupt each other's data",
        );

    let message = err.to_string();
    assert!(
        message.contains(&shared_datadir),
        "the error must name the contested datadir, got: {message:?}"
    );
    assert!(
        message.contains("claimant"),
        "the error must name the existing instance that already claims this datadir, got: \
         {message:?}"
    );

    assert!(
        cg.get_kg_instance("squatter")
            .expect("lookup succeeds")
            .is_none(),
        "the rejected write must not have registered the colliding instance under its own label"
    );
    let still_there = cg
        .get_kg_instance("claimant")
        .expect("lookup succeeds")
        .expect("the original instance must still be registered");
    assert_eq!(
        still_there.datadir, shared_datadir,
        "a rejected write must not have disturbed the original instance's own datadir"
    );
}
