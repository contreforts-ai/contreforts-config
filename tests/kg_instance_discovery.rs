//! Instance discovery (contreforts/contreforts-workspace#58 D8, part 1; #18 Q5, answered
//! 2026-07-27): "consumers resolve an instance from config by label, with a default for
//! single-instance deployments." D8 part 2 (out of this chain's scope) rewires the four
//! `ConfigGraph`-consumer crates to actually call this; this file pins the resolution rule itself,
//! in isolation from any consumer.
//!
//! **The rule must match D5's existing `KnowledgeBaseConfig::kg_instance_label` resolution
//! exactly, so the two cannot drift** (see that field's own doc comment and
//! `ConfigGraph::resolve_kg_instance_label`, both in `src/config_graph.rs`, already merged by the
//! D5 chain):
//! - a named label resolves to that instance, or is a named error saying no such instance is
//!   registered (mirrors [`crate::error::ConfigGraphError::kb_instance_unregistered`]'s case);
//! - no label with **exactly one** instance registered resolves to that instance;
//! - no label with **more than one** registered instance is a named error naming the ambiguity --
//!   never a silent pick of the first (mirrors
//!   [`crate::error::ConfigGraphError::kg_instance_ambiguous`]'s case).
//!
//! **What is deliberately not pinned here:** the **zero-registered-instances** case. D5's own
//! `resolve_kg_instance_label` treats `None` with zero instances registered as "no association
//! yet" (`Ok(None)`) -- a legitimate state for a `KnowledgeBaseConfig`, which does not strictly
//! need to belong to any instance. Discovery has no such escape hatch: a consumer calling this to
//! get a datadir to open a store cannot do anything useful with "no instance" the way a KB record
//! can. Whether that makes zero-registered-instances a named error (consistent with this crate's
//! existing "no silent success on an unusable path" doctrine --
//! `ConfigStoreConfig::from_env`/`per_user_default` already refuse to fall back silently when no
//! OS data dir can be found, `crates/contreforts-config/src/lib.rs`), or whether it falls back to
//! some pre-instance-adoption default outside this crate's own knowledge, is left for a2 to
//! decide -- named here rather than guessed at, per the task's own instruction not to decide
//! genuinely ambiguous points unilaterally.
//!
//! **Whether this shares D5's existing code path:** it should share the *three-way branching
//! logic* (so the two rules cannot silently diverge later), but not the function itself.
//! `resolve_kg_instance_label` is scoped to one `KnowledgeBaseConfig` write (it takes `kb_label`
//! purely to name the offending KB in an error, and returns `Option<String>` because "no
//! association" is valid for a KB) -- a consumer-facing discovery call has no KB in view at all,
//! wants a resolved [`KgInstanceConfig`] back (not just a label, since the whole point is reaching
//! its `datadir`), and cannot treat zero instances as a quiet non-error the way a KB write can.
//! Recorded here for a2 as a recommendation, not a requirement: factor the `Some(label) -> lookup
//! or error` / `None + 1 -> resolve` / `None + N -> ambiguous` match into one shared, private
//! helper that both this call and `resolve_kg_instance_label` delegate to, each supplying its own
//! error wording and its own zero-instances behaviour -- rather than two independently
//! hand-maintained copies of the same three-way match that could drift apart one edit at a time.
//!
//! `ConfigGraph::discover_kg_instance` does not exist yet. This file does not compile against
//! `develop` at `647af20` -- the sanctioned compile-error RED
//! (`crates/contreforts-kg/CONTRIBUTING.md` §3).

use contreforts_config::{ConfigGraph, ConfigStore, KgInstanceConfig};
use contreforts_declaration::ConnectorDeclarations;

fn store() -> (tempfile::TempDir, ConfigStore) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config_store");
    let store = ConfigStore::open(&path).expect("store opens at a fresh path");
    (dir, store)
}

fn register(cg: &ConfigGraph<'_>, label: &str, prefix: &str, datadir: &str) {
    cg.set_kg_instance(&KgInstanceConfig {
        label: label.to_string(),
        iri_prefix: prefix.to_string(),
        datadir: Some(datadir.to_string()),
    })
    .expect("registering the instance succeeds");
}

/// A named label resolving to a genuinely registered instance must hand back that exact instance
/// -- not merely succeed, but return the same record (including its `datadir`, since that is the
/// entire reason a consumer calls this) that was registered.
#[test]
fn a_named_label_resolves_to_the_registered_instance() {
    let (_dir, store) = store();
    let cg = ConfigGraph::new(&store, ConnectorDeclarations::none());
    register(
        &cg,
        "primary",
        "https://contreforts.ds-labs.org/data/instance/discover-p1/",
        "/var/lib/contreforts/kg-instances/discover-primary",
    );
    register(
        &cg,
        "secondary",
        "https://contreforts.ds-labs.org/data/instance/discover-p2/",
        "/var/lib/contreforts/kg-instances/discover-secondary",
    );

    let resolved = cg
        .discover_kg_instance(Some("primary"))
        .expect("a named, registered label must resolve");

    assert_eq!(resolved.label, "primary");
    assert_eq!(
        resolved.datadir,
        Some("/var/lib/contreforts/kg-instances/discover-primary".to_string()),
        "discovery must hand back the exact registered instance, not merely confirm one exists"
    );
}

/// A named label that names no registered instance at all must be a named error saying so --
/// never `Ok` with some other instance substituted, and never a panic.
#[test]
fn a_named_label_naming_no_registered_instance_is_a_named_error() {
    let (_dir, store) = store();
    let cg = ConfigGraph::new(&store, ConnectorDeclarations::none());
    register(
        &cg,
        "primary",
        "https://contreforts.ds-labs.org/data/instance/discover-p3/",
        "/var/lib/contreforts/kg-instances/discover-primary-2",
    );

    let err = cg
        .discover_kg_instance(Some("nonexistent"))
        .expect_err("naming an unregistered instance must be rejected, not silently substituted");

    let message = err.to_string();
    assert!(
        message.contains("nonexistent"),
        "the error must name the label that failed to resolve, got: {message:?}"
    );
}

/// No label, with **exactly one** instance registered, resolves to that instance -- the "default
/// for single-instance deployments" #18 Q5 asks for, so a deployment that has never named more
/// than one instance configures nothing extra to keep working.
#[test]
fn no_label_with_exactly_one_registered_instance_resolves_to_it() {
    let (_dir, store) = store();
    let cg = ConfigGraph::new(&store, ConnectorDeclarations::none());
    register(
        &cg,
        "only",
        "https://contreforts.ds-labs.org/data/instance/discover-only/",
        "/var/lib/contreforts/kg-instances/discover-only",
    );

    let resolved = cg
        .discover_kg_instance(None)
        .expect("with exactly one registered instance, no label must resolve to it by default");

    assert_eq!(resolved.label, "only");
    assert_eq!(
        resolved.datadir,
        Some("/var/lib/contreforts/kg-instances/discover-only".to_string()),
        "the default resolution must hand back the sole instance's real record, datadir included"
    );
}

/// No label, with **more than one** instance registered, must be a named error naming the
/// ambiguity -- never a silent pick of the first (or of whichever SPARQL happens to return
/// first). This is the rule D5 already established for `KnowledgeBaseConfig::kg_instance_label`'s
/// own `None` case with several instances registered; discovery must not drift from it into
/// picking one quietly.
#[test]
fn no_label_with_more_than_one_registered_instance_is_a_named_ambiguity_error() {
    let (_dir, store) = store();
    let cg = ConfigGraph::new(&store, ConnectorDeclarations::none());
    register(
        &cg,
        "alpha",
        "https://contreforts.ds-labs.org/data/instance/discover-alpha/",
        "/var/lib/contreforts/kg-instances/discover-alpha",
    );
    register(
        &cg,
        "beta",
        "https://contreforts.ds-labs.org/data/instance/discover-beta/",
        "/var/lib/contreforts/kg-instances/discover-beta",
    );

    let err = cg.discover_kg_instance(None).expect_err(
        "with more than one registered instance, no label must be rejected as ambiguous rather \
         than silently resolving to whichever instance was registered or listed first",
    );

    let message = err.to_string();
    assert!(
        message.contains('2'),
        "the error should name how many instances are registered so the ambiguity is legible, \
         got: {message:?}"
    );
}
