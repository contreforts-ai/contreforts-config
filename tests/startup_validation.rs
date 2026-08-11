//! D5's second enforcement point (#18 Q3, #19 O2 answered identically): a write-time-only guard
//! is bypassable through the raw SPARQL update route, which is unrestricted today
//! (`contreforts-config-api/src/routes/graph.rs`). This file proves the startup validation pass
//! catches what the guarded write path in `tests/kb_graph_prefix_guard.rs` and
//! `tests/kb_reference_guard.rs` would have rejected, when the store's contents are corrupted by
//! writing directly through `ConfigStore::inner()` -- exactly the shape of write that route
//! performs, and exactly what D5's own design work names as the reason a write-time-only guard
//! is not enough.
//!
//! Per the task's own trap warning ("a guard that examines nothing still reports success"):
//! every rejection test here is paired with a clean-store control proving the same pass reports
//! *no* violations when there genuinely are none -- a pass that always failed would trivially
//! "catch" the corrupted cases too.
//!
//! `ConfigGraph::validate_startup` does not exist yet, and neither does
//! `KnowledgeBaseConfig::kg_instance_label` -- this file does not compile against `develop`.
//! Sanctioned compile-error RED, same as this crate's other new D5/D6 files.
//!
//! The last section (contreforts/contreforts-kg#10) is the one exception to the framing above:
//! invariant 4 refuses a state no write path was ever meant to prevent, reachable by performing
//! the ordinary create-then-link provisioning of a text-mirror connector and stopping after the
//! first step. No `ConfigStore::inner()` corruption is needed to reach it, and the pairing rule
//! still holds -- every rejection there has its own bound-and-accepted control.

use contreforts_config::{
    AgentConfig, CompanyConfig, ConfigGraph, ConfigStore, ForgejoConnectorConfig, KgInstanceConfig,
    KnowledgeBaseConfig, TextMirrorConnectorConfig,
};
use contreforts_core::namespaces::{CONFIG_GRAPH, CORE_NS};
use contreforts_declaration::ConnectorDeclarations;
use oxigraph::model::{GraphName, Literal, NamedNode, Term};

fn store() -> (tempfile::TempDir, ConfigStore) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config_store");
    let store = ConfigStore::open(&path).expect("store opens at a fresh path");
    (dir, store)
}

const PRIMARY_PREFIX: &str = "https://contreforts.ds-labs.org/data/instance/startup-a1/";
const OTHER_PREFIX: &str = "https://contreforts.ds-labs.org/data/instance/startup-b2/";

fn setup(cg: &ConfigGraph<'_>) {
    cg.set_kg_instance(&KgInstanceConfig {
        label: "primary".to_string(),
        iri_prefix: PRIMARY_PREFIX.to_string(),
        // D8 part 1 (contreforts-workspace#58): datadir is a new, required field, unrelated to
        // startup validation's own guard, distinct per instance for the same reason given in
        // tests/kb_graph_prefix_guard.rs.
        datadir: Some("/var/lib/contreforts/kg-instances/startup-primary".to_string()),
    })
    .expect("registering the primary instance succeeds");
    cg.set_kg_instance(&KgInstanceConfig {
        label: "other".to_string(),
        iri_prefix: OTHER_PREFIX.to_string(),
        datadir: Some("/var/lib/contreforts/kg-instances/startup-other".to_string()),
    })
    .expect("registering the other instance succeeds");
    cg.add_company(&CompanyConfig {
        slug: "acme".to_string(),
        name: "Acme".to_string(),
    })
    .expect("company registers cleanly");
}

/// A store with nothing wrong in it must report no violations -- proves the pass actually
/// examines the store's real contents and reaches a real "everything is fine" conclusion, not
/// merely one it happens never to contradict.
#[test]
fn validate_startup_reports_no_violations_for_a_clean_store() {
    let (_dir, store) = store();
    let cg = ConfigGraph::new(&store, ConnectorDeclarations::none());
    setup(&cg);

    cg.set_knowledge_base(
        "acme",
        &KnowledgeBaseConfig {
            label: "support".to_string(),
            kg_instance_label: Some("primary".to_string()),
            graph: Some(format!("{PRIMARY_PREFIX}entity/1")),
            vector_store_label: "vs".to_string(),
        },
    )
    .expect("registering a clean KB succeeds");
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
        .expect("a legitimate label-based link succeeds");

    let result = cg.validate_startup();
    assert!(
        result.is_ok(),
        "a store with no invariant violations must validate cleanly at startup, got: {result:?}"
    );
}

/// Corrupts a KB's stored `graphIri` literal directly through `ConfigStore::inner()` -- bypassing
/// `set_knowledge_base`'s own write-time guard entirely, exactly as the unrestricted raw SPARQL
/// update route would -- so the KB ends up claiming instance `primary` while its graph actually
/// falls under `other`'s prefix. `validate_startup` must catch this even though it never went
/// through the guarded write path, and must name the KB, the corrupted graph IRI, and the
/// instance whose prefix it violates.
#[test]
fn validate_startup_catches_a_kb_graph_corrupted_via_the_raw_store() {
    let (_dir, store) = store();
    let cg = ConfigGraph::new(&store, ConnectorDeclarations::none());
    setup(&cg);

    cg.set_knowledge_base(
        "acme",
        &KnowledgeBaseConfig {
            label: "support".to_string(),
            kg_instance_label: Some("primary".to_string()),
            graph: Some(format!("{PRIMARY_PREFIX}entity/1")),
            vector_store_label: "vs".to_string(),
        },
    )
    .expect("registering the (initially clean) KB succeeds");

    // Bypass: overwrite the KB's `graphIri` literal directly, exactly as the unrestricted raw
    // SPARQL update route could, with no involvement of `set_knowledge_base`'s own guard.
    let kb_iri = contreforts_core::namespaces::knowledge_base_iri("acme", "support");
    let kb_node = NamedNode::new(&kb_iri).expect("valid IRI");
    let graph_pred = NamedNode::new(format!("{CORE_NS}graphIri")).expect("valid IRI");
    let config_graph = NamedNode::new(CONFIG_GRAPH).expect("valid IRI");
    let corrupted_graph = format!("{OTHER_PREFIX}entity/1");

    store
        .remove_quad(
            &kb_node,
            &graph_pred,
            &Term::Literal(Literal::new_simple_literal(format!(
                "{PRIMARY_PREFIX}entity/1"
            ))),
            &config_graph,
        )
        .expect("removing the original graphIri literal succeeds");
    store
        .inner()
        .insert(&oxigraph::model::Quad::new(
            kb_node.clone(),
            graph_pred.clone(),
            Term::Literal(Literal::new_simple_literal(&corrupted_graph)),
            GraphName::NamedNode(config_graph.clone()),
        ))
        .expect("inserting the corrupted graphIri literal directly succeeds");

    // Sanity: the corruption really is invisible to the guarded read path used elsewhere --
    // `get_knowledge_base` has no reason to itself re-validate on every read.
    let got = cg
        .get_knowledge_base("acme", "support")
        .expect("lookup succeeds")
        .expect("the KB is still there");
    assert_eq!(got.graph.as_deref(), Some(corrupted_graph.as_str()));

    let violations = cg.validate_startup().expect_err(
        "a KB whose graph was corrupted to fall under a foreign instance's prefix \
                     must be reported at startup, even though it never went through the \
                     guarded write path",
    );

    let combined = violations.join("\n");
    assert!(
        combined.contains("support"),
        "the report must name the offending KB, got: {combined:?}"
    );
    assert!(
        combined.contains(&corrupted_graph),
        "the report must name the offending graph IRI, got: {combined:?}"
    );
    assert!(
        combined.contains("primary"),
        "the report must name the instance whose prefix was violated, got: {combined:?}"
    );
}

/// The second invariant's bypass: corrupts a connector's Target-KB link directly through
/// `ConfigStore::inner()` so its stored value becomes a KB's graph IRI rather than a label --
/// the exact violation `tests/kb_reference_guard.rs` proves is rejected when reached through
/// `set_connector_target_kb` itself. `validate_startup` must catch this too, since the raw
/// SPARQL route can write this predicate exactly as easily as any other.
#[test]
fn validate_startup_catches_a_target_kb_link_corrupted_to_a_graph_iri_via_the_raw_store() {
    let (_dir, store) = store();
    let cg = ConfigGraph::new(&store, ConnectorDeclarations::none());
    setup(&cg);

    let kb_graph = format!("{PRIMARY_PREFIX}entity/1");
    cg.set_knowledge_base(
        "acme",
        &KnowledgeBaseConfig {
            label: "support".to_string(),
            kg_instance_label: Some("primary".to_string()),
            graph: Some(kb_graph.clone()),
            vector_store_label: "vs".to_string(),
        },
    )
    .expect("registering the KB succeeds");
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
        .expect("the legitimate, label-based link succeeds");

    // Bypass: overwrite the stored target-KB literal to the KB's graph IRI directly, exactly as
    // the unrestricted raw SPARQL update route could -- `set_connector_target_kb` itself would
    // have rejected this (see `tests/kb_reference_guard.rs`).
    let conn_iri = contreforts_core::namespaces::connector_iri("forgejo", "acme", Some("main"));
    let conn_node = NamedNode::new(&conn_iri).expect("valid IRI");
    let target_pred = NamedNode::new(format!("{CORE_NS}targetKnowledgeBase")).expect("valid IRI");
    let config_graph = NamedNode::new(CONFIG_GRAPH).expect("valid IRI");

    store
        .remove_quad(
            &conn_node,
            &target_pred,
            &Term::Literal(Literal::new_simple_literal("support")),
            &config_graph,
        )
        .expect("removing the original label literal succeeds");
    store
        .inner()
        .insert(&oxigraph::model::Quad::new(
            conn_node,
            target_pred,
            Term::Literal(Literal::new_simple_literal(&kb_graph)),
            GraphName::NamedNode(config_graph),
        ))
        .expect("inserting the corrupted graph-IRI literal directly succeeds");

    let violations = cg.validate_startup().expect_err(
        "a Target-KB link corrupted to hold a KB's graph IRI instead of its label must be \
         reported at startup, even though it never went through the guarded write path",
    );

    let combined = violations.join("\n");
    assert!(
        combined.contains(&kb_graph),
        "the report must name the offending graph IRI found outside its own KB's definition, \
         got: {combined:?}"
    );
}

/// Review addendum: `Agent` is not a connector kind, so it is not in `ALL_CONNECTOR_DESCRIPTORS`
/// and the second invariant's original scan never examined `Agent`-typed subjects at all --
/// `ConfigGraph::set_agent`'s `knowledge_base_label` names a KB exactly the way the Target-KB link
/// does, and the same raw-store corruption reproduced here for it must be caught the same way.
#[test]
fn validate_startup_catches_an_agent_knowledge_base_label_corrupted_to_a_graph_iri_via_the_raw_store()
 {
    let (_dir, store) = store();
    let cg = ConfigGraph::new(&store, ConnectorDeclarations::none());
    setup(&cg);

    let kb_graph = format!("{PRIMARY_PREFIX}entity/1");
    cg.set_knowledge_base(
        "acme",
        &KnowledgeBaseConfig {
            label: "support".to_string(),
            kg_instance_label: Some("primary".to_string()),
            graph: Some(kb_graph.clone()),
            vector_store_label: "vs".to_string(),
        },
    )
    .expect("registering the KB succeeds");
    cg.set_agent(
        "acme",
        &AgentConfig {
            label: "bot".to_string(),
            display_name: None,
            knowledge_base_label: "support".to_string(),
            channels: vec![],
        },
    )
    .expect("the legitimate, label-based agent registration succeeds");

    // Bypass: overwrite the stored `usesKnowledgeBase` literal to the KB's graph IRI directly,
    // exactly as the unrestricted raw SPARQL update route could -- `set_agent` itself would have
    // rejected this (see `tests/kb_reference_guard.rs`).
    let agent_iri = contreforts_core::namespaces::agent_iri("acme", "bot");
    let agent_node = NamedNode::new(&agent_iri).expect("valid IRI");
    let uses_kb_pred = NamedNode::new(format!("{CORE_NS}usesKnowledgeBase")).expect("valid IRI");
    let config_graph = NamedNode::new(CONFIG_GRAPH).expect("valid IRI");

    store
        .remove_quad(
            &agent_node,
            &uses_kb_pred,
            &Term::Literal(Literal::new_simple_literal("support")),
            &config_graph,
        )
        .expect("removing the original label literal succeeds");
    store
        .inner()
        .insert(&oxigraph::model::Quad::new(
            agent_node,
            uses_kb_pred,
            Term::Literal(Literal::new_simple_literal(&kb_graph)),
            GraphName::NamedNode(config_graph),
        ))
        .expect("inserting the corrupted graph-IRI literal directly succeeds");

    let violations = cg.validate_startup().expect_err(
        "an Agent's knowledge_base_label corrupted to hold a KB's graph IRI instead of its \
         label must be reported at startup, even though Agent is not a connector kind",
    );

    let combined = violations.join("\n");
    assert!(
        combined.contains(&kb_graph),
        "the report must name the offending graph IRI found outside its own KB's definition, \
         got: {combined:?}"
    );
}

// ── D8 part 2c, item 3 (contreforts-workspace#58, comment 8127, "carried forward rather than
// fixed"): `validate_startup`'s unconditional exemption for a `None`-labelled KB ─────────────────
//
// `src/config_graph.rs`'s `validate_startup` skips a `KnowledgeBaseConfig` whose
// `kg_instance_label` is `None` unconditionally, rather than re-deriving the skip from the
// store's *current* instance count the way `resolve_kg_instance_label` does at write time. A KB
// written while zero instances existed keeps `kg_instance_label: None` forever unless re-saved,
// so if an instance is registered *afterwards*, "another instance's data" becomes a real,
// checkable concept for that KB -- but this pass keeps exempting it anyway.
//
// The asymmetry to preserve, not collapse: at *write* time (`tests/kb_instance_link.rs`'s
// `a_kb_with_no_instance_named_and_no_instance_registered_is_accepted_with_no_association`,
// already pinned there, not duplicated here), `None` with zero instances registered is accepted
// -- vacuously fine, there is nothing else for the KB to point into. It is specifically the
// *startup* pass,
// examining a store where instances now exist, that must stop treating that same `None` as
// nothing to check. The two tests below are the fix and its control: the same KB, the only
// difference is whether an instance was registered after it, in the exact word order the task
// describes ("once instances exist, a `None`-labelled KB is no longer silently exempt").

/// The control, first: a `None`-labelled KB, with zero instances ever registered, must stay
/// exempt -- `validate_startup` reporting no violations here is *existing, correct* behaviour
/// this file's other tests already rely on staying true. Without this control, a fix that simply
/// flagged every `None`-labelled KB unconditionally would also pass the fix test below, for the
/// wrong reason.
#[test]
fn a_none_labelled_kb_stays_exempt_at_startup_while_zero_instances_are_ever_registered() {
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
            label: "legacy".to_string(),
            kg_instance_label: None,
            graph: Some("https://contreforts.example/pre-instance-adoption/entity/1".to_string()),
            vector_store_label: "vs".to_string(),
        },
    )
    .expect(
        "write time: kg_instance_label: None with zero registered instances is accepted -- \
         vacuously fine, there is nothing else for this KB to point into",
    );

    let result = cg.validate_startup();
    assert!(
        result.is_ok(),
        "with zero instances ever registered, a None-labelled KB predating instance adoption \
         must stay exempt at startup -- got: {result:?}"
    );
}

/// The fix: the *same* KB as above, except an instance is registered *after* the KB was written
/// (still without the KB ever being re-saved, so `kg_instance_label` is still `None` in the
/// store). Once at least one instance exists, "another instance's data" is a real, checkable
/// concept for this KB too -- `validate_startup` must no longer report a clean pass for it
/// unconditionally. This is the exact case `src/config_graph.rs`'s own comment on the `None` arm
/// names as unfixed: "if an instance is registered afterwards and this KB is never re-saved, it
/// stays permanently exempt... even though 'another instance's data' is now a real, checkable
/// concept."
#[test]
fn a_none_labelled_kb_is_no_longer_exempt_at_startup_once_an_instance_is_registered_afterward() {
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
            label: "legacy".to_string(),
            kg_instance_label: None,
            graph: Some("https://contreforts.example/pre-instance-adoption/entity/1".to_string()),
            vector_store_label: "vs".to_string(),
        },
    )
    .expect("write time: still accepted, with zero instances registered at write time");

    // An instance is registered *after* the KB was written; the KB itself is never re-saved, so
    // its stored `kgInstanceLabel` triple is still absent (`kg_instance_label: None` on read).
    cg.set_kg_instance(&KgInstanceConfig {
        label: "primary".to_string(),
        iri_prefix: "https://contreforts.ds-labs.org/data/instance/startup-none-exempt/"
            .to_string(),
        datadir: Some("/var/lib/contreforts/kg-instances/startup-none-exempt".to_string()),
    })
    .expect("registering the instance after the fact succeeds");

    // Sanity: the KB really is still stored with kg_instance_label: None -- otherwise this test
    // would not be exercising the exemption at all.
    let stored = cg
        .get_knowledge_base("acme", "legacy")
        .expect("lookup succeeds")
        .expect("the KB is still there");
    assert_eq!(
        stored.kg_instance_label, None,
        "sanity check: the KB must still be unassociated -- this test is about a KB that is \
         never re-saved after an instance appears, not one that was updated"
    );

    let violations = cg.validate_startup().expect_err(
        "once at least one instance is registered, a None-labelled KB with a graph set must no \
         longer be silently exempt from startup validation -- got Ok(()), meaning the exemption \
         is still unconditional",
    );
    let combined = violations.join("\n");
    assert!(
        combined.contains("legacy"),
        "the report must name the offending KB, got: {combined:?}"
    );
}

// ── contreforts/contreforts-kg#10 (decision recorded 2026-08-11): a text-mirror connector must
// name the knowledge base whose corpus it mirrors ────────────────────────────────────────────
//
// A text mirror is one knowledge base's corpus in lexical form, and `TextMirrorConnectorConfig`
// carries no ACL field of its own, deliberately -- its perimeter *is* the knowledge base it is
// linked to. So a mirror linked to nothing has no perimeter at all.
//
// The link is a separate write from the connector (`set_connector_target_kb`, generic over the
// kind, not a struct field), so "created but never linked" is reachable through the ordinary
// create-then-link provisioning path -- unlike this file's other cases, which need the raw
// SPARQL update route to reach. #10 rejected requiring the link at connector write time (it
// would invert that order for every consumer of the framework) and rejected reporting it as a
// state (a third-party operator has no reason to consult a field they do not know exists), and
// chose `validate_startup`, where the other cross-connector invariants already live.
//
// The trap these tests exist to pin, named in the decision itself: the requirement is
// **conditional on the connector existing**. A deployment with no text-mirror connector must
// still start.

/// The mirror's own bounds are `sh:minCount 1` in the declaration and irrelevant to this guard;
/// the values here are the Python tool's measured defaults (#10's opening comment) so they read
/// as a plausible deployment rather than as placeholders.
fn mirror(label: &str) -> TextMirrorConnectorConfig {
    TextMirrorConnectorConfig {
        label: label.to_string(),
        mirror_root: format!("/var/lib/contreforts/mirrors/{label}"),
        max_documents: 120,
        max_excerpts_per_document: 3,
    }
}

/// Registers one clean, instance-associated KB for a mirror to be bound to. Its `graph` falls
/// under `PRIMARY_PREFIX` so invariant 1 stays silent and these tests only ever observe
/// invariant 4.
fn register_kb(cg: &ConfigGraph<'_>, label: &str) {
    cg.set_knowledge_base(
        "acme",
        &KnowledgeBaseConfig {
            label: label.to_string(),
            kg_instance_label: Some("primary".to_string()),
            graph: Some(format!("{PRIMARY_PREFIX}entity/{label}")),
            vector_store_label: "vs".to_string(),
        },
    )
    .expect("registering a clean KB succeeds");
}

/// A text-mirror connector written and never linked -- the exact state
/// `set_text_mirror_connector` leaves behind on its own, with no corruption and no raw SPARQL
/// involved. Startup must refuse it, and the report must name the connector: whoever reads it is
/// operating a deployment they may not have configured, so "a connector is misconfigured" would
/// be useless to them.
#[test]
fn validate_startup_refuses_a_text_mirror_connector_that_names_no_knowledge_base() {
    let (_dir, store) = store();
    let cg = ConfigGraph::new(&store, ConnectorDeclarations::none());
    setup(&cg);
    register_kb(&cg, "support");

    cg.set_text_mirror_connector("acme", &mirror("support-corpus"))
        .expect("writing a text-mirror connector succeeds -- the write itself is not the guard");
    // Deliberately no `set_connector_target_kb`: that omission is the whole test.

    let violations = cg.validate_startup().expect_err(
        "a text-mirror connector bound to no knowledge base has no perimeter at all, and \
         contreforts/contreforts-kg#10 decided a deployment that leaves one unbound must refuse \
         to start -- got Ok(())",
    );
    let combined = violations.join("\n");
    assert!(
        combined.contains("support-corpus"),
        "the report must name the offending connector -- an operator who did not write this \
         configuration cannot find it otherwise, got: {combined:?}"
    );
    assert!(
        combined.contains("acme"),
        "the report must name the company the connector belongs to, since labels are only \
         unique within one, got: {combined:?}"
    );
    assert!(
        combined.contains("set_connector_target_kb"),
        "the report must say what to do about it, not only that something is wrong -- the \
         remediation is the link this connector is missing, got: {combined:?}"
    );
}

/// The control for the test above: the same connector, linked. Without this, a guard that simply
/// rejected every text-mirror connector -- or every store containing one -- would pass the
/// rejection test for entirely the wrong reason, and the legitimate configuration this issue
/// exists to make mandatory would be unstartable.
#[test]
fn validate_startup_accepts_a_text_mirror_connector_bound_to_a_knowledge_base() {
    let (_dir, store) = store();
    let cg = ConfigGraph::new(&store, ConnectorDeclarations::none());
    setup(&cg);
    register_kb(&cg, "support");

    cg.set_text_mirror_connector("acme", &mirror("support-corpus"))
        .expect("writing a text-mirror connector succeeds");
    cg.set_connector_target_kb("acme", "text-mirror", Some("support-corpus"), "support")
        .expect("linking the mirror to the KB whose corpus it mirrors succeeds");

    let result = cg.validate_startup();
    assert!(
        result.is_ok(),
        "a text-mirror connector that names its knowledge base is exactly the configuration \
         contreforts/contreforts-kg#10 requires -- it must start, got: {result:?}"
    );
}

/// The easy one to get wrong. #10's requirement is conditional on the connector existing: a
/// deployment that uses no text mirror at all is complete, not incomplete. Turning invariant 4
/// into "every company must have a bound mirror" would refuse to start every deployment that
/// does not use `kb_grep`.
///
/// The company here also holds an *unbound connector of another kind*, which is legitimate --
/// only a text mirror has no boundary of its own -- so this test additionally fails if the guard
/// is widened from `TEXT_MIRROR` to `ALL_CONNECTOR_DESCRIPTORS`.
#[test]
fn validate_startup_accepts_a_deployment_with_no_text_mirror_connector_at_all() {
    let (_dir, store) = store();
    let cg = ConfigGraph::new(&store, ConnectorDeclarations::none());
    setup(&cg);
    register_kb(&cg, "support");

    cg.set_forgejo_connector(
        "acme",
        &ForgejoConnectorConfig {
            label: "main".to_string(),
            url: "https://git.example.com".to_string(),
            token: "tok".to_string(),
        },
    )
    .expect("connector registers cleanly");
    // No target-KB link on it, and no text-mirror connector anywhere in the store.

    let result = cg.validate_startup();
    assert!(
        result.is_ok(),
        "the requirement is conditional on a text-mirror connector existing -- a deployment \
         holding none must still start, and an unbound connector of another kind is not this \
         invariant's business, got: {result:?}"
    );
}

/// `validate_startup` reports every violation it finds rather than the first, so a single
/// corrupted store surfaces its whole problem in one restart. Two unbound mirrors must both be
/// named -- and the bound third one must not be, which is what fails if the guard stops
/// examining targets and flags every mirror it sees.
#[test]
fn validate_startup_reports_every_unbound_text_mirror_and_only_those() {
    let (_dir, store) = store();
    let cg = ConfigGraph::new(&store, ConnectorDeclarations::none());
    setup(&cg);
    register_kb(&cg, "support");

    for label in ["hr-corpus", "legal-corpus", "support-corpus"] {
        cg.set_text_mirror_connector("acme", &mirror(label))
            .expect("writing a text-mirror connector succeeds");
    }
    cg.set_connector_target_kb("acme", "text-mirror", Some("support-corpus"), "support")
        .expect("linking one of the three succeeds");

    let violations = cg
        .validate_startup()
        .expect_err("two of the three mirrors name no knowledge base -- got Ok(())");
    let combined = violations.join("\n");
    assert!(
        combined.contains("hr-corpus"),
        "the first unbound mirror must be named, got: {combined:?}"
    );
    assert!(
        combined.contains("legal-corpus"),
        "the second unbound mirror must be named too -- reporting only the first would cost one \
         restart per misconfigured connector, which is not how this pass reports its other \
         invariants, got: {combined:?}"
    );
    assert!(
        !combined.contains("support-corpus"),
        "the bound mirror must not be reported -- a guard that flags every mirror regardless of \
         its target would pass this file's rejection test for the wrong reason, got: {combined:?}"
    );
}

/// A `targetKnowledgeBase` literal set to the empty string names a knowledge base no more than
/// an absent one does. Reachable through the unrestricted raw SPARQL update route this whole
/// file exists for -- and, as it happens, through `set_connector_target_kb` itself, which
/// validates the value against registered KB *graphs* (D5's second invariant) but not against
/// emptiness. Treating it as bound would make an empty string the one way past this guard.
#[test]
fn validate_startup_treats_a_blank_target_knowledge_base_on_a_mirror_as_unbound() {
    let (_dir, store) = store();
    let cg = ConfigGraph::new(&store, ConnectorDeclarations::none());
    setup(&cg);
    register_kb(&cg, "support");

    cg.set_text_mirror_connector("acme", &mirror("support-corpus"))
        .expect("writing a text-mirror connector succeeds");
    cg.set_connector_target_kb("acme", "text-mirror", Some("support-corpus"), "   ")
        .expect("a blank target is accepted at write time -- nothing there checks emptiness");

    // Sanity: the link really was stored, so this test is exercising the blank-value branch and
    // not merely the absent-link one the test above already covers.
    let stored = cg
        .get_connector_target_kb("acme", "text-mirror", Some("support-corpus"))
        .expect("reading the link back succeeds");
    assert_eq!(
        stored.as_deref(),
        Some("   "),
        "sanity check: the blank link must actually be in the store"
    );

    let violations = cg
        .validate_startup()
        .expect_err("a blank target knowledge base names nothing -- got Ok(())");
    let combined = violations.join("\n");
    assert!(
        combined.contains("support-corpus"),
        "the report must name the connector whose target is blank, got: {combined:?}"
    );
}
