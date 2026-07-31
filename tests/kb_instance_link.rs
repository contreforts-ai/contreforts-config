//! The association D5 needs before it can guard anything (contreforts/contreforts-workspace#58
//! comment 7969): "today the phrase 'another instance's data' is not a checkable property --
//! nothing says which instance a KB is part of." `KnowledgeBaseConfig` gains a new field naming
//! the `KgInstanceConfig` it belongs to, by **label** -- the same pattern `vector_store_label`
//! and the Target-KB link (`tests/target_kb_link.rs`) already use for every other cross-record
//! reference in this crate, and the only shape that never puts a second graph IRI into a
//! non-`KnowledgeBaseConfig` record (contreforts-workspace#18 Q3).
//!
//! `KnowledgeBaseConfig::kg_instance_label` does not exist yet -- this file does not compile
//! against `develop` at `c081e95`. That is the sanctioned RED (`crates/contreforts-kg/
//! CONTRIBUTING.md` §3): a compile error naming the missing field is evidence enough.
//!
//! This is also the reason `KnowledgeBaseConfig`'s *stored* triples must change, not merely its
//! Rust shape: every `KnowledgeBaseConfig` a store already holds was written before this field
//! existed, so a real deployment's existing data has no instance association either. See the
//! task report for the exact call sites (`src/config_graph.rs`'s `get_knowledge_base`/
//! `list_knowledge_bases`, plus three existing test fixtures in this crate) that must be updated
//! together when this field is added, not just this new file.

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

fn register_instance(cg: &ConfigGraph<'_>, label: &str, iri_prefix: &str) {
    cg.set_kg_instance(&KgInstanceConfig {
        label: label.to_string(),
        iri_prefix: iri_prefix.to_string(),
        // D8 part 1 (contreforts-workspace#58): datadir is a new, required field, unrelated to
        // the KB<->instance association this file pins. Derived from the label so callers of
        // this helper (which never register two instances under the same label -- label
        // uniqueness is D4's own guard) get distinct datadirs for free, never colliding with
        // the new datadir-uniqueness guard pinned in tests/kg_instance_datadir.rs.
        datadir: Some(format!("/var/lib/contreforts/kg-instances/{label}")),
    })
    .expect("registering the instance succeeds");
}

/// The plain round trip: a KB registered against a real instance reports that exact instance
/// back, by label, and is not merely accepted -- `get_knowledge_base` must actually carry the
/// association, not silently drop it the way a no-op guard would make invisible.
#[test]
fn a_kb_belongs_to_its_registered_instance_and_the_association_round_trips() {
    let (_dir, store) = store();
    let cg = ConfigGraph::new(&store, ConnectorDeclarations::none());
    register_instance(
        &cg,
        "primary",
        "https://contreforts.ds-labs.org/data/instance/p1/",
    );

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
            graph: Some("https://contreforts.ds-labs.org/data/instance/p1/kb/support".to_string()),
            vector_store_label: "vs".to_string(),
        },
    )
    .expect("a KB naming a real, registered instance registers cleanly");

    let got = cg
        .get_knowledge_base("acme", "support")
        .expect("lookup succeeds")
        .expect("the KB just registered is found");

    assert_eq!(
        got.kg_instance_label,
        Some("primary".to_string()),
        "the instance association must round-trip exactly, not merely something truthy"
    );

    let listed = cg.list_knowledge_bases("acme").expect("listing succeeds");
    assert_eq!(listed.len(), 1);
    assert_eq!(
        listed[0].kg_instance_label,
        Some("primary".to_string()),
        "the instance association must also appear in the list view, not only the single-fetch \
         path -- a guard or a UI that reads only one of the two would silently miss the other"
    );
}

/// The association must survive a store close/reopen -- otherwise the guard this is built for
/// would only work for the lifetime of one process, never across a restart, which is precisely
/// the moment #19 O2's reserved-graph reload also happens.
#[test]
fn the_instance_association_survives_store_close_and_reopen() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config_store");

    {
        let store = ConfigStore::open(&path).expect("store opens at a fresh path");
        let cg = ConfigGraph::new(&store, ConnectorDeclarations::none());
        register_instance(
            &cg,
            "durable",
            "https://contreforts.ds-labs.org/data/instance/d1/",
        );
        cg.add_company(&CompanyConfig {
            slug: "acme".to_string(),
            name: "Acme".to_string(),
        })
        .expect("company registers cleanly");
        cg.set_knowledge_base(
            "acme",
            &KnowledgeBaseConfig {
                label: "support".to_string(),
                kg_instance_label: Some("durable".to_string()),
                graph: None,
                vector_store_label: "vs".to_string(),
            },
        )
        .expect("registering the KB succeeds");
    } // store closed here

    let reopened = ConfigStore::open(&path).expect("store reopens at the same path");
    let cg = ConfigGraph::new(&reopened, ConnectorDeclarations::none());
    let found = cg
        .get_knowledge_base("acme", "support")
        .expect("lookup succeeds")
        .expect("the KB registered before close is still there after reopen");

    assert_eq!(
        found.kg_instance_label,
        Some("durable".to_string()),
        "the instance association must survive a store close/reopen -- otherwise every \
         KB's own definition would silently lose the fact this guard depends on"
    );
}

/// A KB cannot name an instance that was never registered: the guard's whole predicate ("does
/// this KB's graph fall under its own instance's assigned prefix") is meaningless without a
/// real instance to resolve, so a dangling reference must be refused at the same moment it would
/// otherwise be created, naming the KB and the instance label it wrongly claims.
#[test]
fn a_kb_naming_an_unregistered_instance_is_rejected() {
    let (_dir, store) = store();
    let cg = ConfigGraph::new(&store, ConnectorDeclarations::none());

    cg.add_company(&CompanyConfig {
        slug: "acme".to_string(),
        name: "Acme".to_string(),
    })
    .expect("company registers cleanly");

    let err = cg
        .set_knowledge_base(
            "acme",
            &KnowledgeBaseConfig {
                label: "orphan".to_string(),
                kg_instance_label: Some("never-registered".to_string()),
                graph: None,
                vector_store_label: "vs".to_string(),
            },
        )
        .expect_err(
            "a KB naming an instance that does not exist must be rejected -- there is nothing \
             for the prefix guard to check it against",
        );

    let message = err.to_string();
    assert!(
        message.contains("orphan"),
        "the error must name the KB attempting the dangling reference, got: {message:?}"
    );
    assert!(
        message.contains("never-registered"),
        "the error must name the instance label that does not resolve, got: {message:?}"
    );

    assert!(
        cg.get_knowledge_base("acme", "orphan")
            .expect("lookup succeeds")
            .is_none(),
        "the rejected write must not have registered the KB at all"
    );
}

/// `kg_instance_label: None` resolution's third case (contreforts/contreforts-workspace#58's
/// follow-up ruling 1, not covered by any of a1's own tests above): with more than one
/// registered instance, `None` cannot resolve to "the sole one" -- there isn't one -- and
/// silently picking either would reintroduce exactly the "absence presenting as success" failure
/// this epic keeps paying for. Must be a named error, not a guess, naming the KB attempting the
/// ambiguous write.
#[test]
fn a_kb_with_no_instance_named_and_multiple_instances_registered_is_rejected_as_ambiguous() {
    let (_dir, store) = store();
    let cg = ConfigGraph::new(&store, ConnectorDeclarations::none());
    register_instance(
        &cg,
        "primary",
        "https://contreforts.ds-labs.org/data/instance/multi-p1/",
    );
    register_instance(
        &cg,
        "secondary",
        "https://contreforts.ds-labs.org/data/instance/multi-p2/",
    );
    cg.add_company(&CompanyConfig {
        slug: "acme".to_string(),
        name: "Acme".to_string(),
    })
    .expect("company registers cleanly");

    let err = cg
        .set_knowledge_base(
            "acme",
            &KnowledgeBaseConfig {
                label: "ambiguous".to_string(),
                kg_instance_label: None,
                graph: None,
                vector_store_label: "vs".to_string(),
            },
        )
        .expect_err(
            "with more than one registered instance, `kg_instance_label: None` must be rejected \
             rather than silently picking one",
        );

    let message = err.to_string();
    assert!(
        message.contains("ambiguous"),
        "the error must name the KB attempting the ambiguous write, got: {message:?}"
    );

    assert!(
        cg.get_knowledge_base("acme", "ambiguous")
            .expect("lookup succeeds")
            .is_none(),
        "the rejected write must not have registered the KB at all"
    );
}

/// `kg_instance_label: None` resolution's remaining case: with **zero** registered instances,
/// there is nothing for a KB to belong to yet, and nothing for the prefix guard to check --
/// treated as "no association recorded" rather than an error, so every caller that predates KG
/// instances entirely (every `KnowledgeBaseConfig` stored before D4, and
/// `contreforts-config-api`'s knowledge-base routes, which have never registered one) keeps
/// working unchanged. Distinct from the multi-instance case above, which *is* an error: zero
/// instances means the feature has not been adopted yet, not that a real ambiguity exists.
#[test]
fn a_kb_with_no_instance_named_and_no_instance_registered_is_accepted_with_no_association() {
    let (_dir, store) = store();
    let cg = ConfigGraph::new(&store, ConnectorDeclarations::none());
    cg.add_company(&CompanyConfig {
        slug: "acme".to_string(),
        name: "Acme".to_string(),
    })
    .expect("company registers cleanly");

    cg.set_knowledge_base(
        "acme",
        &KnowledgeBaseConfig {
            label: "docs".to_string(),
            kg_instance_label: None,
            graph: Some("http://example.org/code-graph".to_string()),
            vector_store_label: "vs".to_string(),
        },
    )
    .expect(
        "with zero registered instances, `kg_instance_label: None` must be accepted -- there is \
         nothing registered for this KB to belong to, so the guard has nothing to check",
    );

    let got = cg
        .get_knowledge_base("acme", "docs")
        .expect("lookup succeeds")
        .expect("the KB just registered is found");
    assert_eq!(
        got.kg_instance_label, None,
        "no instance was registered to resolve to, so none must be recorded"
    );
}
