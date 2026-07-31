//! D9 (contreforts/contreforts-workspace#58; #18 Q4): the second half of "wipe ≠ delete" --
//! **deleting a KG instance's definition**, as distinct from wiping its *data* (that half lives
//! in `crates/contreforts-kg/tests/wipe_instance_data.rs`, and does not touch this crate at all).
//!
//! Q4, verbatim: "Deleting an instance is a separate, explicit operation that removes the
//! definition and refuses while connectors still target it." Read literally against D4's actual
//! reference chain, though, no connector ever names a `KgInstanceConfig` directly -- a connector
//! names a **KB** (`ConfigGraph::set_connector_target_kb`), and a KB names its **instance**
//! (`KnowledgeBaseConfig::kg_instance_label`, D5's first invariant). So the one and only record
//! type that can reference a `KgInstanceConfig` by label, verified by `grep -rn
//! "kg_instance_label\|kgInstanceLabel" crates/ --include='*.rs'` across the whole workspace, is
//! `KnowledgeBaseConfig` itself. This file pins that: deleting an instance a KB still belongs to
//! must be refused, naming the KB -- which transitively protects any connector or agent that in
//! turn targets that KB, exactly the way Q4 intends, without this file needing to know anything
//! about connectors or agents at all.
//!
//! `ConfigGraph::remove_kg_instance` does not exist yet -- this file does not compile against
//! `develop` at `1aaead4c`. That is the sanctioned compile-error RED
//! (`crates/contreforts-kg/CONTRIBUTING.md` §3); see the task report for the exact `cargo build`
//! failure text.

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

fn register_instance(cg: &ConfigGraph<'_>, label: &str, prefix: &str, datadir: &str) {
    cg.set_kg_instance(&KgInstanceConfig {
        label: label.to_string(),
        iri_prefix: prefix.to_string(),
        datadir: Some(datadir.to_string()),
    })
    .expect("registering the instance succeeds");
}

// ── Baseline: delete must work when nothing references it ──────────────────

/// The control every refusal test below depends on: deleting an instance no KB belongs to must
/// keep succeeding. Without this, a guard that rejects every delete outright would still pass
/// the refusal test below for the wrong reason.
#[test]
fn deleting_an_instance_with_no_kb_belonging_to_it_succeeds() {
    let (_dir, store) = store();
    let cg = ConfigGraph::new(&store, ConnectorDeclarations::none());
    register_instance(
        &cg,
        "primary",
        "https://contreforts.ds-labs.org/data/instance/instdel-p1/",
        "/var/lib/contreforts/kg-instances/instdel-primary",
    );

    cg.remove_kg_instance("primary")
        .expect("deleting an instance no KB belongs to must succeed");

    assert!(
        cg.get_kg_instance("primary")
            .expect("lookup succeeds")
            .is_none(),
        "the instance must actually be gone after a successful delete"
    );
}

// ── Refusal: a KB still belongs to the instance ─────────────────────────────

/// The one legitimate config -> instance reference (#18's "why the guard is the crux" section:
/// "config may name a KB in the KB's definition record and nowhere else"). Deleting an instance a
/// KB still belongs to must be refused, and the refusal must name the KB -- not merely say the
/// instance is "in use". Protecting this one link transitively protects any connector or agent
/// that targets that KB, without this test (or the guard) needing to know about connectors or
/// agents at all -- that reference chain is `contreforts-config/tests/kb_delete_guard.rs`'s own
/// concern.
#[test]
fn deleting_an_instance_a_kb_still_belongs_to_is_refused_and_names_the_kb() {
    let (_dir, store) = store();
    let cg = ConfigGraph::new(&store, ConnectorDeclarations::none());
    register_instance(
        &cg,
        "primary",
        "https://contreforts.ds-labs.org/data/instance/instdel-p2/",
        "/var/lib/contreforts/kg-instances/instdel-primary-2",
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
            graph: Some(
                "https://contreforts.ds-labs.org/data/instance/instdel-p2/entity/1".to_string(),
            ),
            vector_store_label: "vs".to_string(),
        },
    )
    .expect("registering the KB succeeds");

    let err = cg.remove_kg_instance("primary").expect_err(
        "deleting an instance a KB still belongs to must be refused -- the KB's own prefix guard \
         (D5) would have nothing to check against once its instance is gone",
    );

    let message = err.to_string();
    assert!(
        message.contains("support"),
        "the refusal must name the offending KB ('support'), not merely say the instance is \
         'in use', got: {message:?}"
    );

    assert!(
        cg.get_kg_instance("primary")
            .expect("lookup succeeds")
            .is_some(),
        "the refused delete must not have removed the instance"
    );
}

/// Scoping control: a KB belonging to a **different** instance must not block deleting this one.
/// Without this, a guard that rejects a delete whenever *any* KB belongs to *any* instance
/// (rather than checking whether it belongs to *this* instance) would still pass the refusal
/// test above.
#[test]
fn deleting_an_instance_no_kb_belongs_to_succeeds_even_though_a_different_instance_has_a_kb() {
    let (_dir, store) = store();
    let cg = ConfigGraph::new(&store, ConnectorDeclarations::none());
    register_instance(
        &cg,
        "empty",
        "https://contreforts.ds-labs.org/data/instance/instdel-empty/",
        "/var/lib/contreforts/kg-instances/instdel-empty",
    );
    register_instance(
        &cg,
        "occupied",
        "https://contreforts.ds-labs.org/data/instance/instdel-occupied/",
        "/var/lib/contreforts/kg-instances/instdel-occupied",
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
            kg_instance_label: Some("occupied".to_string()),
            graph: Some(
                "https://contreforts.ds-labs.org/data/instance/instdel-occupied/entity/1"
                    .to_string(),
            ),
            vector_store_label: "vs".to_string(),
        },
    )
    .expect("registering the KB succeeds");

    cg.remove_kg_instance("empty").expect(
        "deleting an instance no KB belongs to must succeed, even while a different instance has \
         a KB belonging to it -- the guard must be scoped to the instance actually being deleted",
    );

    assert!(
        cg.get_kg_instance("empty")
            .expect("lookup succeeds")
            .is_none()
    );
    assert!(
        cg.get_kg_instance("occupied")
            .expect("lookup succeeds")
            .is_some(),
        "the untouched instance must be unaffected"
    );
}

// ── A refused delete changes nothing ────────────────────────────────────────

/// "No partial deletion": after the refusal, both the instance's own definition (label, prefix,
/// datadir) *and* the KB's association to it must be exactly as they were.
#[test]
fn an_instance_delete_refusal_leaves_the_instance_and_the_kbs_association_intact() {
    let (_dir, store) = store();
    let cg = ConfigGraph::new(&store, ConnectorDeclarations::none());
    register_instance(
        &cg,
        "primary",
        "https://contreforts.ds-labs.org/data/instance/instdel-p3/",
        "/var/lib/contreforts/kg-instances/instdel-primary-3",
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
            graph: Some(
                "https://contreforts.ds-labs.org/data/instance/instdel-p3/entity/1".to_string(),
            ),
            vector_store_label: "vs".to_string(),
        },
    )
    .expect("registering the KB succeeds");

    let before = cg
        .get_kg_instance("primary")
        .expect("lookup succeeds")
        .expect("the instance exists before the attempted delete");

    cg.remove_kg_instance("primary")
        .expect_err("the delete must be refused");

    let after = cg
        .get_kg_instance("primary")
        .expect("lookup succeeds")
        .expect("the instance must still exist after the refusal");
    assert_eq!(
        after, before,
        "the instance's own definition (label, iri_prefix, datadir) must be byte-identical after \
         a refused delete, not partially rewritten"
    );

    let kb = cg
        .get_knowledge_base("acme", "support")
        .expect("lookup succeeds")
        .expect("the KB must still exist");
    assert_eq!(
        kb.kg_instance_label,
        Some("primary".to_string()),
        "the KB's own association to the instance must also still be exactly as it was -- a \
         refused delete must not touch the reference either"
    );
}

// ── Delete on an unknown label is a named error, not a silent no-op ────────

/// Mirrors `kb_delete_guard.rs`'s own unknown-label test: deleting a label nothing ever
/// registered must be a named error, not a silent `Ok(())` an operator cannot tell apart from a
/// real deletion.
#[test]
fn deleting_an_unknown_instance_label_is_a_named_error_not_a_silent_noop() {
    let (_dir, store) = store();
    let cg = ConfigGraph::new(&store, ConnectorDeclarations::none());

    let err = cg.remove_kg_instance("never-registered").expect_err(
        "deleting an instance label that was never registered must be a named error, not a \
         silent Ok(()) that looks exactly like a real deletion",
    );

    let message = err.to_string();
    assert!(
        message.contains("never-registered"),
        "the error must name the label that does not exist, got: {message:?}"
    );
}
