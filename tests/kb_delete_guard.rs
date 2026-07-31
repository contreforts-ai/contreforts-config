//! D9 (contreforts/contreforts-workspace#58; #18 Q4, answered 2026-07-27): "wipe ≠ delete."
//! This file pins **delete**'s half of that split for a `KnowledgeBaseConfig`: removing a KB's
//! *definition* is safe only when nothing still names it, and unsafe otherwise -- an operator
//! deleting a KB whose connector or agent still points at it would leave that connector/agent
//! silently pointing at nothing, the next sync or chat turn failing in a way that gives no clue
//! why.
//!
//! **Why this lives here, not `contreforts-kg`:** the KB *definition* -- and the connector/agent
//! records that can reference it -- are all `contreforts-config` records (`ConfigGraph`). Nothing
//! about this guard touches KG *data*; it is a pure config-graph referential-integrity question,
//! answered entirely by what `ConfigGraph` itself already knows. Contrast
//! `crates/contreforts-kg/tests/wipe_instance_data.rs`, which is about instance *data* and lives
//! in `contreforts-kg` for the same reason this lives here.
//!
//! **Enumeration of every record type that can reference a `KnowledgeBaseConfig` by label** --
//! verified by `grep -rn "knowledge_base_label\|targetKnowledgeBase\|kb_label" crates/
//! --include='*.rs'` across the whole workspace, not assumed:
//! - a connector, of **any** of the eleven kinds, via `ConfigGraph::set_connector_target_kb`
//!   (`targetKnowledgeBase`, D4) -- one generic predicate shared by every kind, singleton or
//!   label-scoped;
//! - an `AgentConfig`, via `knowledge_base_label` (`usesKnowledgeBase`) -- **not** a connector
//!   kind, which is exactly the shape of bug D5's own guard shipped with (`set_agent` never
//!   called `reject_kb_graph_reference` because `Agent` never went through `write_connector`'s
//!   generic engine -- see `tests/kb_reference_guard.rs`'s own "review addendum" section). A
//!   referential check that walks connector kinds and forgets `Agent` reproduces that exact bug
//!   one level up: it would let a KB an agent still serves vanish out from under it.
//!
//! No other record type stores a KB label or a KB's graph IRI anywhere in the workspace as
//! verified by the grep above (`CompanyConfig`, `SparqlTemplateConfig`, `ChannelRef`, group/
//! customer mappings, and every `*ConnectorConfig`'s own fields hold none).
//!
//! Coverage below deliberately spans **two** connector shapes, not one, to guard against a check
//! that happens to work for whichever kind it was written against: `forgejo` (label-scoped,
//! `Some("main")`) and `erpnext` (singleton, `label: None` -- a different code path through
//! `namespaces::connector_iri`, `descriptor.singleton`). Picking two structurally different
//! kinds, rather than two label-scoped kinds, is deliberately the harder case for a
//! kind-specific check to accidentally still pass.
//!
//! `ConfigGraph::remove_knowledge_base` exists today (`src/config_graph.rs`) but performs **no**
//! referential check at all -- it unconditionally deletes the KB's triples and its company link,
//! even for a label nothing has ever registered. Every refusal test below is therefore a genuine
//! **runtime** RED against current `develop` at `1aaead4c` (the whole file still compiles: every
//! symbol it uses -- `remove_knowledge_base`, `set_connector_target_kb`, `set_agent`, `set_*_
//! connector` -- already exists), confirmed by running `cargo test -p contreforts-config
//! --test kb_delete_guard` against that commit before this file's guard-dependent assertions were
//! written: every `expect_err(..)` call panics with "called `Result::unwrap_err()` on an `Ok`
//! value: ()" (or the delete simply succeeds and the following existence assertion fails),
//! because nothing today stops the delete. See the task report for the exact command and output.

use contreforts_config::{
    AgentConfig, CompanyConfig, ConfigGraph, ConfigStore, ErpNextConnectorConfig,
    ForgejoConnectorConfig, KgInstanceConfig, KnowledgeBaseConfig,
};
use contreforts_declaration::ConnectorDeclarations;

fn store() -> (tempfile::TempDir, ConfigStore) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config_store");
    let store = ConfigStore::open(&path).expect("store opens at a fresh path");
    (dir, store)
}

const PREFIX: &str = "https://contreforts.ds-labs.org/data/instance/kbdel-9c2f/";

fn setup_company_and_kb(cg: &ConfigGraph<'_>, kb_label: &str) -> String {
    cg.set_kg_instance(&KgInstanceConfig {
        label: "primary".to_string(),
        iri_prefix: PREFIX.to_string(),
        datadir: Some("/var/lib/contreforts/kg-instances/kbdel-primary".to_string()),
    })
    .expect("registering the instance succeeds");
    cg.add_company(&CompanyConfig {
        slug: "acme".to_string(),
        name: "Acme".to_string(),
    })
    .expect("company registers cleanly");
    let kb_graph = format!("{PREFIX}entity/{kb_label}");
    cg.set_knowledge_base(
        "acme",
        &KnowledgeBaseConfig {
            label: kb_label.to_string(),
            kg_instance_label: Some("primary".to_string()),
            graph: Some(kb_graph),
            vector_store_label: "vs".to_string(),
        },
    )
    .expect("registering the KB succeeds");
    kb_label.to_string()
}

// ── Baseline: delete must still work when nothing references it ────────────

/// The control every refusal test below depends on: deleting a KB nothing points at must keep
/// succeeding. Without this, a guard that rejects *every* delete outright would still pass the
/// refusal tests below for the wrong reason.
#[test]
fn deleting_an_unreferenced_kb_succeeds() {
    let (_dir, store) = store();
    let cg = ConfigGraph::new(&store, ConnectorDeclarations::none());
    setup_company_and_kb(&cg, "support");

    cg.remove_knowledge_base("acme", "support")
        .expect("deleting a KB nothing references must succeed");

    assert!(
        cg.get_knowledge_base("acme", "support")
            .expect("lookup succeeds")
            .is_none(),
        "the KB must actually be gone after a successful delete"
    );
}

// ── Refusal: a connector still targets the KB ───────────────────────────────

/// `forgejo` case: label-scoped connector. Deleting a KB a Forgejo connector still targets must
/// be refused, and the refusal must name the connector -- not merely say "in use".
#[test]
fn deleting_a_kb_still_targeted_by_a_label_scoped_connector_is_refused_and_names_the_connector() {
    let (_dir, store) = store();
    let cg = ConfigGraph::new(&store, ConnectorDeclarations::none());
    setup_company_and_kb(&cg, "support");
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
        .expect("linking the connector to the KB succeeds");

    let err = cg.remove_knowledge_base("acme", "support").expect_err(
        "deleting a KB a connector still targets must be refused -- the connector would be left \
         pointing at nothing",
    );

    let message = err.to_string();
    assert!(
        message.contains("main") || message.contains("forgejo"),
        "the refusal must name the offending connector (its label 'main' and/or its kind \
         'forgejo'), not merely say the KB is 'in use', got: {message:?}"
    );

    assert!(
        cg.get_knowledge_base("acme", "support")
            .expect("lookup succeeds")
            .is_some(),
        "the refused delete must not have removed the KB"
    );
}

/// `erpnext` case: singleton connector (`label: None`), a structurally different code path
/// through `namespaces::connector_iri` than the label-scoped case above. Proves the guard is not
/// specific to label-scoped connectors.
#[test]
fn deleting_a_kb_still_targeted_by_a_singleton_connector_is_refused_and_names_the_connector() {
    let (_dir, store) = store();
    let cg = ConfigGraph::new(&store, ConnectorDeclarations::none());
    setup_company_and_kb(&cg, "support");
    cg.set_erpnext_connector(
        "acme",
        &ErpNextConnectorConfig {
            company_name: "Acme SAS".to_string(),
            url: "https://acme.erpnext.com".to_string(),
            api_key: "key".to_string(),
            api_secret: "secret".to_string(),
        },
    )
    .expect("connector registers cleanly");
    cg.set_connector_target_kb("acme", "erpnext", None, "support")
        .expect("linking the singleton connector to the KB succeeds");

    let err = cg.remove_knowledge_base("acme", "support").expect_err(
        "deleting a KB a singleton connector still targets must be refused, exactly as for a \
         label-scoped one",
    );

    let message = err.to_string();
    assert!(
        message.contains("erpnext"),
        "the refusal must name the offending connector's kind ('erpnext', its only identity \
         since it is a singleton), got: {message:?}"
    );

    assert!(
        cg.get_knowledge_base("acme", "support")
            .expect("lookup succeeds")
            .is_some(),
        "the refused delete must not have removed the KB"
    );
}

/// Scoping control: a connector targeting a **different** KB must not block deleting this one.
/// Without this, a guard that rejects a delete whenever *any* connector has *any* target
/// (rather than checking whether it targets *this* KB) would still pass every test above.
#[test]
fn deleting_a_kb_not_targeted_by_any_connector_succeeds_even_though_another_kb_is_targeted() {
    let (_dir, store) = store();
    let cg = ConfigGraph::new(&store, ConnectorDeclarations::none());
    setup_company_and_kb(&cg, "support");
    cg.set_knowledge_base(
        "acme",
        &KnowledgeBaseConfig {
            label: "billing".to_string(),
            kg_instance_label: Some("primary".to_string()),
            graph: Some(format!("{PREFIX}entity/billing")),
            vector_store_label: "vs".to_string(),
        },
    )
    .expect("registering the second KB succeeds");
    cg.set_forgejo_connector(
        "acme",
        &ForgejoConnectorConfig {
            label: "main".to_string(),
            url: "https://git.example.com".to_string(),
            token: "tok".to_string(),
        },
    )
    .expect("connector registers cleanly");
    cg.set_connector_target_kb("acme", "forgejo", Some("main"), "billing")
        .expect("linking the connector to the *other* KB succeeds");

    cg.remove_knowledge_base("acme", "support").expect(
        "deleting a KB no connector targets must succeed, even while a connector targets a \
         different KB -- the guard must be scoped to the KB actually being deleted",
    );

    assert!(
        cg.get_knowledge_base("acme", "support")
            .expect("lookup succeeds")
            .is_none()
    );
}

// ── Refusal: an agent still uses the KB ─────────────────────────────────────

/// The trap this phase has already paid for once (`tests/kb_reference_guard.rs`'s review
/// addendum): `Agent` is not a connector kind, so a check that only walks connector kinds misses
/// it entirely. Reproduced here at the delete guard, one level up from where D5's write guard
/// first missed it.
#[test]
fn deleting_a_kb_still_used_by_an_agent_is_refused_and_names_the_agent() {
    let (_dir, store) = store();
    let cg = ConfigGraph::new(&store, ConnectorDeclarations::none());
    setup_company_and_kb(&cg, "support");
    cg.set_agent(
        "acme",
        &AgentConfig {
            label: "bot".to_string(),
            display_name: None,
            knowledge_base_label: "support".to_string(),
            channels: vec![],
        },
    )
    .expect("agent registers cleanly");

    let err = cg.remove_knowledge_base("acme", "support").expect_err(
        "deleting a KB an agent still uses must be refused -- the agent would be left with no \
         knowledge base to answer from",
    );

    let message = err.to_string();
    assert!(
        message.contains("bot"),
        "the refusal must name the offending agent ('bot'), not merely say the KB is 'in use', \
         got: {message:?}"
    );

    assert!(
        cg.get_knowledge_base("acme", "support")
            .expect("lookup succeeds")
            .is_some(),
        "the refused delete must not have removed the KB"
    );
}

// ── A refused delete changes nothing ────────────────────────────────────────

/// Explicit "no partial deletion" check for the connector-reference case: after the refusal,
/// both the KB's own definition *and* the connector's target-KB link must still be exactly as
/// they were -- not merely "the KB still exists" (which the two refusal tests above already
/// check), but that the *reference itself* was not touched either.
#[test]
fn a_kb_delete_refusal_via_connector_leaves_the_kb_and_the_link_intact() {
    let (_dir, store) = store();
    let cg = ConfigGraph::new(&store, ConnectorDeclarations::none());
    setup_company_and_kb(&cg, "support");
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
        .expect("linking succeeds");

    let before = cg
        .get_knowledge_base("acme", "support")
        .expect("lookup succeeds")
        .expect("the KB exists before the attempted delete");

    cg.remove_knowledge_base("acme", "support")
        .expect_err("the delete must be refused");

    let after = cg
        .get_knowledge_base("acme", "support")
        .expect("lookup succeeds")
        .expect("the KB must still exist after the refusal");
    assert_eq!(
        after.vector_store_label, before.vector_store_label,
        "the KB's own definition must be byte-identical after a refused delete, not partially \
         rewritten"
    );
    assert_eq!(after.graph, before.graph);

    assert_eq!(
        cg.get_connector_target_kb("acme", "forgejo", Some("main"))
            .expect("lookup succeeds"),
        Some("support".to_string()),
        "the connector's target-KB link must also still be exactly as it was -- a refused delete \
         must not touch the reference either"
    );
}

/// Same "no partial deletion" check for the agent-reference case.
#[test]
fn a_kb_delete_refusal_via_agent_leaves_the_kb_and_the_agent_intact() {
    let (_dir, store) = store();
    let cg = ConfigGraph::new(&store, ConnectorDeclarations::none());
    setup_company_and_kb(&cg, "support");
    cg.set_agent(
        "acme",
        &AgentConfig {
            label: "bot".to_string(),
            display_name: None,
            knowledge_base_label: "support".to_string(),
            channels: vec![],
        },
    )
    .expect("agent registers cleanly");

    cg.remove_knowledge_base("acme", "support")
        .expect_err("the delete must be refused");

    assert!(
        cg.get_knowledge_base("acme", "support")
            .expect("lookup succeeds")
            .is_some(),
        "the KB must still exist after the refusal"
    );
    let agent = cg
        .get_agent("acme", "bot")
        .expect("lookup succeeds")
        .expect("the agent must still exist after the refusal");
    assert_eq!(
        agent.knowledge_base_label, "support",
        "the agent's own reference to the KB must also be untouched by the refused delete"
    );
}

// ── Delete on an unknown label is a named error, not a silent no-op ────────

/// `ConfigGraph::remove_knowledge_base` today performs no existence check at all: calling it for
/// a label nothing ever registered simply returns `Ok(())`, indistinguishable from having deleted
/// something. That is exactly the "absence presenting as success" failure this epic keeps paying
/// for (contreforts-workspace#58's own recurring language) -- an operator who mistypes a label
/// gets silent success instead of being told the label does not exist.
#[test]
fn deleting_an_unknown_kb_label_is_a_named_error_not_a_silent_noop() {
    let (_dir, store) = store();
    let cg = ConfigGraph::new(&store, ConnectorDeclarations::none());
    cg.add_company(&CompanyConfig {
        slug: "acme".to_string(),
        name: "Acme".to_string(),
    })
    .expect("company registers cleanly");

    let err = cg
        .remove_knowledge_base("acme", "never-registered")
        .expect_err(
            "deleting a KB label that was never registered must be a named error, not a silent \
         Ok(()) that looks exactly like a real deletion",
        );

    let message = err.to_string();
    assert!(
        message.contains("never-registered"),
        "the error must name the label that does not exist, got: {message:?}"
    );
}
