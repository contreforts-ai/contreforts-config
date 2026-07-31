//! D5's guard, half one (contreforts/contreforts-workspace#58; #18 Q3, precisely restated in
//! comment 7969): "config must not reference a KB graph other than that KB's own definition."
//! Once a KB names its own instance (`tests/kb_instance_link.rs`), "points into another
//! instance's data" becomes a decidable check -- "its graph IRI does not fall under its own
//! instance's assigned prefix" -- and this file pins that check at write time, on
//! `ConfigGraph::set_knowledge_base` itself.
//!
//! `KnowledgeBaseConfig::kg_instance_label` does not exist yet, so this file does not compile
//! against `develop` -- sanctioned compile-error RED, same as `tests/kb_instance_link.rs`.
//!
//! Per the task's own warning about a guard that examines nothing still reporting success: every
//! rejection test here is paired with a happy-path test proving the *same* shape of write is
//! accepted when it does not violate the rule -- a guard that rejected everything would still
//! pass the rejection tests alone.

use contreforts_config::{
    CompanyConfig, ConfigGraph, ConfigStore, KgInstanceConfig, KnowledgeBaseConfig,
};
use contreforts_declaration::ConnectorDeclarations;

fn store() -> (tempfile::TempDir, ConfigStore) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config_store");
    let store = ConfigStore::open(&path).expect("store opens at a fresh path");
    (dir, store)
}

const PRIMARY_PREFIX: &str = "https://contreforts.ds-labs.org/data/instance/primary-a1b2/";
const OTHER_PREFIX: &str = "https://contreforts.ds-labs.org/data/instance/other-c3d4/";

fn setup(cg: &ConfigGraph<'_>) {
    cg.set_kg_instance(&KgInstanceConfig {
        label: "primary".to_string(),
        iri_prefix: PRIMARY_PREFIX.to_string(),
        // D8 part 1 (contreforts-workspace#58): datadir is a new, required field on this
        // fixture, unrelated to the prefix guard this file pins -- distinct per instance so
        // this file's fixtures don't incidentally trip the new datadir-uniqueness guard
        // pinned in tests/kg_instance_datadir.rs.
        datadir: "/var/lib/contreforts/kg-instances/prefixguard-primary".to_string(),
    })
    .expect("registering the primary instance succeeds");
    cg.set_kg_instance(&KgInstanceConfig {
        label: "other".to_string(),
        iri_prefix: OTHER_PREFIX.to_string(),
        datadir: "/var/lib/contreforts/kg-instances/prefixguard-other".to_string(),
    })
    .expect("registering the other instance succeeds");
    cg.add_company(&CompanyConfig {
        slug: "acme".to_string(),
        name: "Acme".to_string(),
    })
    .expect("company registers cleanly");
}

/// The control: a KB whose graph genuinely falls under its own instance's prefix must be
/// accepted. Without this, the two rejection tests below would still pass against a guard that
/// simply refuses every KB with a `graph` set.
#[test]
fn a_kb_graph_within_its_own_instance_prefix_is_accepted() {
    let (_dir, store) = store();
    let cg = ConfigGraph::new(&store, ConnectorDeclarations::none());
    setup(&cg);

    let own_graph = format!("{PRIMARY_PREFIX}entity/1");
    cg.set_knowledge_base(
        "acme",
        &KnowledgeBaseConfig {
            label: "support".to_string(),
            kg_instance_label: Some("primary".to_string()),
            graph: Some(own_graph.clone()),
            vector_store_label: "vs".to_string(),
        },
    )
    .expect("a graph IRI genuinely under the KB's own instance's prefix must be accepted");

    let got = cg
        .get_knowledge_base("acme", "support")
        .expect("lookup succeeds")
        .expect("the accepted KB is stored");
    assert_eq!(got.graph, Some(own_graph));
}

/// The sharpest rejection: a graph IRI that resolves under **another registered instance's**
/// prefix, not merely an arbitrary unregistered one -- proving the check compares against the
/// KB's *own* instance specifically, not just "is this prefix registered to *someone*." The
/// error must name all three of the KB, the offending graph IRI, and the instance whose prefix
/// it violated -- a rejection silent on any of the three is barely better than none
/// (contreforts/contreforts-workspace#58 task description).
#[test]
fn a_kb_graph_under_a_different_registered_instances_prefix_is_rejected_at_write_time() {
    let (_dir, store) = store();
    let cg = ConfigGraph::new(&store, ConnectorDeclarations::none());
    setup(&cg);

    let foreign_graph = format!("{OTHER_PREFIX}entity/1");
    let err = cg
        .set_knowledge_base(
            "acme",
            &KnowledgeBaseConfig {
                label: "support".to_string(),
                kg_instance_label: Some("primary".to_string()),
                graph: Some(foreign_graph.clone()),
                vector_store_label: "vs".to_string(),
            },
        )
        .expect_err(
            "a KB claiming instance 'primary' whose graph actually falls under instance \
             'other's prefix must be rejected",
        );

    let message = err.to_string();
    assert!(
        message.contains("support"),
        "the error must name the offending KB ('support'), got: {message:?}"
    );
    assert!(
        message.contains(&foreign_graph),
        "the error must name the offending graph IRI ({foreign_graph:?}), got: {message:?}"
    );
    assert!(
        message.contains("primary"),
        "the error must name the instance whose prefix was violated ('primary', the KB's own \
         claimed instance -- not 'other', which the graph happens to match instead), got: \
         {message:?}"
    );

    assert!(
        cg.get_knowledge_base("acme", "support")
            .expect("lookup succeeds")
            .is_none(),
        "the rejected write must not have registered the KB at all"
    );
}

/// A graph IRI that matches no registered instance's prefix at all -- the more ordinary
/// mistake (a typo, or an instance created after the fact with a different prefix) -- must be
/// rejected the same way, naming the same three things.
#[test]
fn a_kb_graph_matching_no_registered_prefix_at_all_is_rejected() {
    let (_dir, store) = store();
    let cg = ConfigGraph::new(&store, ConnectorDeclarations::none());
    setup(&cg);

    let stray_graph =
        "https://contreforts.ds-labs.org/data/instance/typo-9f9f/entity/1".to_string();
    let err = cg
        .set_knowledge_base(
            "acme",
            &KnowledgeBaseConfig {
                label: "support".to_string(),
                kg_instance_label: Some("primary".to_string()),
                graph: Some(stray_graph.clone()),
                vector_store_label: "vs".to_string(),
            },
        )
        .expect_err("a graph IRI under no registered prefix at all must be rejected");

    let message = err.to_string();
    assert!(message.contains("support"), "got: {message:?}");
    assert!(message.contains(&stray_graph), "got: {message:?}");
    assert!(message.contains("primary"), "got: {message:?}");
}

/// A KB with no graph set at all (`graph: None`, meaning "default graph") has nothing for the
/// prefix check to examine, and must not be rejected merely for omitting it -- `graph` staying
/// optional is existing, load-bearing behaviour this guard must not change.
#[test]
fn a_kb_with_no_graph_set_is_unaffected_by_the_prefix_guard() {
    let (_dir, store) = store();
    let cg = ConfigGraph::new(&store, ConnectorDeclarations::none());
    setup(&cg);

    cg.set_knowledge_base(
        "acme",
        &KnowledgeBaseConfig {
            label: "support".to_string(),
            kg_instance_label: Some("primary".to_string()),
            graph: None,
            vector_store_label: "vs".to_string(),
        },
    )
    .expect("a KB with no graph set must not be rejected by the prefix guard");
}
