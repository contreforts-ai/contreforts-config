//! D5's guard, half two -- the actual #18 Q3 invariant, stated precisely in comment 7969:
//! "exactly one config record may name a KB graph IRI -- the KB's own `KnowledgeBaseConfig.graph`
//! -- and no other config record may name one at all." Distinct from
//! `tests/kb_graph_prefix_guard.rs`, which checks a KB's own graph against its own instance;
//! this file checks that **nothing else** ever stores a KB's graph IRI as a value.
//!
//! The Target-KB link (`ConfigGraph::set_connector_target_kb`, D4) is the sharpest, most directly
//! relevant case: its own doc comment in `src/config_graph.rs` states it stores a **label**,
//! deliberately, precisely so it never becomes a second record naming a graph IRI -- and flags
//! that "D5 must settle this ambiguity definitively." Nothing today stops a caller from passing
//! a graph IRI string where a label belongs, since the predicate itself does not validate its
//! argument. That is exactly the gap this file closes: the *task description's* warning that
//! this invariant "is the one most likely to be implemented as a no-op" is concretely about this
//! call succeeding silently.
//!
//! `KnowledgeBaseConfig::kg_instance_label` does not exist yet, so this file does not compile
//! against `develop` -- sanctioned compile-error RED, same as this crate's other new D5 files.

use contreforts_config::{
    CompanyConfig, ConfigGraph, ConfigStore, ForgejoConnectorConfig, KgInstanceConfig,
    KnowledgeBaseConfig,
};
use contreforts_declaration::ConnectorDeclarations;

fn store() -> (tempfile::TempDir, ConfigStore) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config_store");
    let store = ConfigStore::open(&path).expect("store opens at a fresh path");
    (dir, store)
}

const PREFIX: &str = "https://contreforts.ds-labs.org/data/instance/refguard-7e1a/";

fn setup_company_and_kb(cg: &ConfigGraph<'_>) -> String {
    cg.set_kg_instance(&KgInstanceConfig {
        label: "primary".to_string(),
        iri_prefix: PREFIX.to_string(),
    })
    .expect("registering the instance succeeds");
    cg.add_company(&CompanyConfig {
        slug: "acme".to_string(),
        name: "Acme".to_string(),
    })
    .expect("company registers cleanly");
    let kb_graph = format!("{PREFIX}entity/1");
    cg.set_knowledge_base(
        "acme",
        &KnowledgeBaseConfig {
            label: "support".to_string(),
            kg_instance_label: "primary".to_string(),
            graph: Some(kb_graph.clone()),
            vector_store_label: "vs".to_string(),
        },
    )
    .expect("registering the KB succeeds");
    kb_graph
}

/// The control: linking a connector to a KB by its real **label** -- the shape D4 built and
/// intends to be the only legitimate one -- must keep succeeding. Without this, the two
/// rejection tests below would still pass against a guard that refuses every
/// `set_connector_target_kb` call outright.
#[test]
fn linking_a_connector_to_a_kb_by_its_real_label_still_succeeds() {
    let (_dir, store) = store();
    let cg = ConfigGraph::new(&store, ConnectorDeclarations::none());
    setup_company_and_kb(&cg);
    cg.set_forgejo_connector(
        "acme",
        &ForgejoConnectorConfig {
            label: "main".to_string(),
            url: "https://git.example.com".to_string(),
            token: "tok".to_string(),
        },
    )
    .expect("connector registers cleanly");

    cg.set_connector_target_kb("acme", "forgejo", Some("main"), "support")
        .expect("linking a connector to a KB by its real label must still succeed");

    assert_eq!(
        cg.get_connector_target_kb("acme", "forgejo", Some("main"))
            .expect("lookup succeeds"),
        Some("support".to_string())
    );
}

/// The concrete no-op risk: passing a KB's **graph IRI**, not its label, as the Target-KB link's
/// argument. This is precisely a non-`KnowledgeBaseConfig` record (the connector's own config)
/// naming a KB graph IRI -- the invariant's exact violation -- reached through a legitimate,
/// already-existing entry point rather than a raw store bypass (`tests/startup_validation.rs`
/// covers that separately).
#[test]
fn linking_a_connector_to_a_kb_graph_iri_instead_of_a_label_is_rejected() {
    let (_dir, store) = store();
    let cg = ConfigGraph::new(&store, ConnectorDeclarations::none());
    let kb_graph = setup_company_and_kb(&cg);
    cg.set_forgejo_connector(
        "acme",
        &ForgejoConnectorConfig {
            label: "main".to_string(),
            url: "https://git.example.com".to_string(),
            token: "tok".to_string(),
        },
    )
    .expect("connector registers cleanly");

    let err = cg
        .set_connector_target_kb("acme", "forgejo", Some("main"), &kb_graph)
        .expect_err(
            "passing a KB's graph IRI where a label belongs must be rejected -- this predicate \
             is only ever supposed to hold a label, and accepting a graph IRI here creates a \
             second config record naming it, violating #18 Q3",
        );

    let message = err.to_string();
    assert!(
        message.contains(&kb_graph),
        "the error must name the offending graph IRI, got: {message:?}"
    );

    assert!(
        cg.get_connector_target_kb("acme", "forgejo", Some("main"))
            .expect("lookup succeeds")
            .is_none(),
        "the rejected write must not have linked the connector to anything"
    );
}

/// The same violation reached through an ordinary connector field rather than the Target-KB
/// predicate specifically -- proving the guard is a genuine, general property of config writes
/// ("no record but `KnowledgeBaseConfig` may store this value"), not a special case wired only
/// into `set_connector_target_kb`. A Forgejo instance URL has no legitimate reason to ever equal
/// a KB's graph IRI verbatim, so rejecting this coincidence is safe by construction.
#[test]
fn a_connector_field_set_verbatim_to_a_registered_kb_graph_iri_is_rejected() {
    let (_dir, store) = store();
    let cg = ConfigGraph::new(&store, ConnectorDeclarations::none());
    let kb_graph = setup_company_and_kb(&cg);

    let err = cg
        .set_forgejo_connector(
            "acme",
            &ForgejoConnectorConfig {
                label: "main".to_string(),
                url: kb_graph.clone(),
                token: "tok".to_string(),
            },
        )
        .expect_err(
            "a connector field written verbatim as a registered KB's graph IRI must be \
             rejected -- only that KB's own KnowledgeBaseConfig.graph may hold this value",
        );

    let message = err.to_string();
    assert!(
        message.contains(&kb_graph),
        "the error must name the offending value, got: {message:?}"
    );

    assert!(
        cg.get_forgejo_connector("acme", "main")
            .expect("lookup succeeds")
            .is_none(),
        "the rejected write must not have registered the connector at all"
    );
}
