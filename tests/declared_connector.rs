//! Phase F, W6 (contreforts/contreforts-config#11): the generic declared-vocabulary connector
//! read/write — `ConfigGraph::get_declared_connector` / `set_declared_connector` — plus the
//! structural change they rest on: `write_connector` must stop wiping the whole connector
//! subject.
//!
//! **This file does not compile against `develop`.** Neither method exists yet, and
//! `write_connector`'s delete window is still the whole subject. That is the sanctioned RED
//! (`crates/contreforts-kg/CONTRIBUTING.md` §3).
//!
//! ## The two contracts this file pins, and why they are not the same contract
//!
//! **A. The delete window must equal the write window.** `write_connector` calls
//! `remove_subject_from_named_graph` (`src/config_graph.rs`), which is
//! `quads_for_pattern(Some(subject), None, None, Some(graph))` with **no predicate filter** —
//! it deletes every quad on the connector subject. But the *write* window is the declared field
//! set plus `rdf:type`, and `set_connector_target_kb` writes `{CORE_NS}targetKnowledgeBase` onto
//! that same subject. `targetKnowledgeBase` appears in no connector's `declaration.ttl` (verified:
//! `grep -rn 'targetKnowledgeBase' crates/contreforts-connector-*/declaration.ttl` → zero hits),
//! so it is in no kind's `ConnectorIris::field_iris`, so no read-merge keyed on declared fields
//! can carry it forward. Re-saving a connector therefore destroys its target-KB link — and D5's
//! KB-delete guard (`tests/kb_delete_guard.rs`) keys on exactly that link, so the knowledge base
//! silently becomes deletable. This is **pre-existing for the eleven typed setters too**: every
//! test in the tree links the KB *after* the connector write and never re-saves.
//!
//! **B. Omitting a field and clearing a field are different user actions.** They must not
//! collapse into the same write. `write_connector` drops `None` values before anything else and
//! then wipes, so a generic route forwarding "field absent from the request body" straight
//! through as `None` deletes the stored value. W7 serves `GET /connector-values` with secrets
//! elided (D8), so "absent" is the *normal* state of every secret on a form round trip. Hence
//! the patch type is `BTreeMap<String, Option<String>>`, not `BTreeMap<String, String>`:
//!
//! | request state | meaning | merge behaviour |
//! |---|---|---|
//! | key **absent** | the client is not talking about this field | **unchanged** |
//! | key present, `Some(v)`, `v != ""` | set it | write `v` |
//! | key present, `None` | the user emptied the field and saved | **clear** |
//! | key present, `Some("")` | a browser sends `""`, not JSON `null` | empty literal for a declared `xsd:string`/`xsd:anyURI` field; `Err(ConnectorValidation)` for any other declared datatype, **independent of whether a validator is wired** |
//!
//! The `Some("")` rule is not decoration. `typed_literal` builds
//! `Literal::new_typed_literal(value, datatype)` with **no lexical check**, so `""^^xsd:integer`
//! is constructible; with no validator wired it is stored, and every subsequent read of that
//! connector then fails, because `parse_declared_field` returns `Err(DeclaredFieldMismatch)` for
//! declared kinds rather than defaulting. The connector becomes unreadable with no path back
//! through the same form. Four stalwart fields are `xsd:integer`
//! (`crates/contreforts-connector-stalwart/declaration.ttl:191,259,291,352`).
//!
//! ## Design fork settled here — option (a)
//!
//! contreforts-config#11's "Unsettled 1" asks how `ConfigGraph` learns the per-variant field
//! sets, and lists three options. **This file is written against option (a), the issue's own
//! recommended default: W4's `VariantRule`s are a parameter to `set_declared_connector`.**
//! Evidence for the choice, all re-verified against the tree:
//!
//! - `contreforts_declaration::VariantRule` is already public and re-exported
//!   (`declaration/src/lib.rs:85-88`), carrying exactly `discriminant_field`,
//!   `discriminant_value`, `shown`, `required`, `hidden` — field ids only, no RDF.
//! - `contreforts-config` already depends on `contreforts-declaration` (`Cargo.toml`), so this
//!   needs **no** change to a type `contreforts-declaration` owns — option (b) would have made
//!   W6 a three-PR item with a second core pointer bump, contradicting the issue's own framing.
//! - `ConnectorIris` carries only `class_iri` / `field_iris` / `field_datatypes`
//!   (`declaration/src/connector_validation.rs`) — nothing about `sh:xone` — and `ConfigGraph`
//!   holds no `Declaration` and no TTL, so option (c) (re-parse) would give this crate a second
//!   parsing path. Rejected by the issue and not attempted.
//!
//! Consequence, stated so it is not discovered later: the caller supplies the rules, so a caller
//! that supplies none gets no reconciliation. `variant_reconciliation_is_a_no_op_for_a_kind_with_no_variants`
//! pins that this is *correct* for the five of seven kinds that declare no `sh:xone` (erpnext,
//! pennylane, forgejo, gitlab, stalwart) — and PR 2 derives the rules for the other two from
//! `form_schemas(PRODUCT_GRAPH_TTL)`, i.e. from the real declarations, never by hand.
//!
//! ## Fixtures
//!
//! Synthetic Turtle inline, per this crate's own precedent (`tests/config_graph.rs`'s
//! `FORGEJO_DECLARATION_TTL`): `contreforts-config` must not depend on any connector crate.
//! Every fixture below is split into a `*_TOP_LEVEL` const (the target node shape, whose direct
//! `sh:property` entries are exactly what `ConnectorIris::field_iris` reads) and a
//! `*_VARIANTS` const (the `sh:xone` alternatives, which `field_iris` does **not** read). That
//! split is not cosmetic: it is what lets
//! `field_iris_key_set_equals_the_declared_top_level_sh_path_set` scan the top-level block
//! textually and compare it to `field_iris` with no second hand-maintained list.
//!
//! **The residual weakness, not papered over:** the headline data-loss guard here runs against
//! an o365 *lookalike*, whose safety depends on two facts a copy can silently stop mirroring —
//! no flat `sh:minCount` on `o365:refreshToken`, and `sh:minCount 0` inside the delegated
//! alternative. Both verified against `crates/contreforts-connector-o365/declaration.ttl`
//! (`:348-358` flat, `:453` delegated) on 2026-08-01. PR 2 (`contreforts-config-api`'s
//! `product/tests/`) carries the same guard against the *real* declaration, which is the only
//! thing that detects drift.

use std::collections::{BTreeMap, BTreeSet};

use contreforts_config::{
    CaldavConnectorAuth, CaldavConnectorConfig, CompanyConfig, ConfigGraph, ConfigGraphError,
    ConfigStore, ForgejoConnectorConfig, all_connector_kinds,
};
use contreforts_core::namespaces::{self, CONFIG_GRAPH, CORE_NS};
use contreforts_declaration::{ConnectorDeclarations, ConnectorValidator, VariantRule};

// ── Fixtures ─────────────────────────────────────────────────────────────────

const PREFIXES: &str = r#"
    @prefix sh:      <http://www.w3.org/ns/shacl#> .
    @prefix xsd:     <http://www.w3.org/2001/XMLSchema#> .
    @prefix rdfs:    <http://www.w3.org/2000/01/rdf-schema#> .
    @prefix o365:    <https://contreforts.ds-labs.org/ontologies/o365#> .
    @prefix caldav:  <https://contreforts.ds-labs.org/ontologies/caldav#> .
    @prefix forgejo: <https://contreforts.ds-labs.org/ontologies/forgejo#> .
    @prefix erpnext: <https://contreforts.ds-labs.org/ontologies/erpnext#> .
    @prefix stalwart: <https://contreforts.ds-labs.org/ontologies/stalwart#> .
"#;

/// The o365 node shape's nine direct `sh:property` entries, content-faithful to
/// `crates/contreforts-connector-o365/declaration.ttl`'s own flat block (`:230-360`), trimmed of
/// `sh:name`/`sh:description`/`sh:order`/`sh:group`/`contreforts:*`.
///
/// **The two facts this fixture exists to mirror, and which the whole class-(a) data-loss guard
/// rests on:** `o365:refreshToken` carries **no flat `sh:minCount`** here, exactly as in the real
/// file, and `O365_VARIANTS` gives it `sh:minCount 0` inside the delegated alternative. Together
/// they mean a `PUT` that omits `refreshToken` validates **cleanly** — so nothing but the merge
/// stands between the user and a destroyed refresh token.
const O365_TOP_LEVEL: &str = r#"
    o365:O365ConnectorShape a sh:NodeShape ;
        sh:targetClass o365:O365Connector ;
        sh:property [ sh:path o365:label ;         sh:datatype xsd:string ; sh:minCount 1 ; sh:maxCount 1 ] ;
        sh:property [ sh:path o365:userPrincipal ; sh:datatype xsd:string ; sh:maxCount 1 ] ;
        sh:property [ sh:path o365:customer ;      sh:datatype xsd:string ; sh:maxCount 1 ] ;
        sh:property [ sh:path o365:authMode ;      sh:minCount 1 ; sh:maxCount 1 ] ;
        sh:property [ sh:path o365:tenantId ;      sh:datatype xsd:string ; sh:maxCount 1 ] ;
        sh:property [ sh:path o365:clientId ;      sh:datatype xsd:string ; sh:maxCount 1 ] ;
        sh:property [ sh:path o365:clientSecret ;  sh:datatype xsd:string ; sh:maxCount 1 ] ;
        sh:property [ sh:path o365:token ;         sh:datatype xsd:string ; sh:maxCount 1 ] ;
        sh:property [ sh:path o365:refreshToken ;  sh:datatype xsd:string ; sh:maxCount 1 ] ;
        sh:xone ( o365:ClientCredentialsShape o365:DelegatedShape ) .
"#;

/// `crates/contreforts-connector-o365/declaration.ttl:429-457`, verbatim in content.
const O365_VARIANTS: &str = r#"
    o365:ClientCredentialsShape a sh:NodeShape ;
        sh:property [ sh:path o365:authMode ; sh:in ( "client_credentials" ) ; sh:minCount 1 ; sh:maxCount 1 ] ;
        sh:property [ sh:path o365:tenantId ;     sh:minCount 1 ; sh:maxCount 1 ; sh:datatype xsd:string ] ;
        sh:property [ sh:path o365:clientId ;     sh:minCount 1 ; sh:maxCount 1 ; sh:datatype xsd:string ] ;
        sh:property [ sh:path o365:clientSecret ; sh:minCount 1 ; sh:maxCount 1 ; sh:datatype xsd:string ] ;
        sh:property [ sh:path o365:token ;        sh:maxCount 0 ] ;
        sh:property [ sh:path o365:refreshToken ; sh:maxCount 0 ] .

    o365:DelegatedShape a sh:NodeShape ;
        sh:property [ sh:path o365:authMode ; sh:in ( "delegated" ) ; sh:minCount 1 ; sh:maxCount 1 ] ;
        sh:property [ sh:path o365:token ;        sh:minCount 1 ; sh:maxCount 1 ; sh:datatype xsd:string ] ;
        sh:property [ sh:path o365:refreshToken ; sh:minCount 0 ; sh:maxCount 1 ; sh:datatype xsd:string ] ;
        sh:property [ sh:path o365:tenantId ;     sh:maxCount 0 ] ;
        sh:property [ sh:path o365:clientId ;     sh:maxCount 0 ] ;
        sh:property [ sh:path o365:clientSecret ; sh:maxCount 0 ] .
"#;

/// `crates/contreforts-connector-caldav/declaration.ttl`'s eight direct properties.
const CALDAV_TOP_LEVEL: &str = r#"
    caldav:CaldavConnectorShape a sh:NodeShape ;
        sh:targetClass caldav:CaldavConnector ;
        sh:property [ sh:path caldav:label ;        sh:datatype xsd:string ; sh:minCount 1 ; sh:maxCount 1 ] ;
        sh:property [ sh:path caldav:instanceUrl ;  sh:datatype xsd:string ; sh:minCount 1 ; sh:maxCount 1 ] ;
        sh:property [ sh:path caldav:calendarHome ; sh:datatype xsd:string ; sh:maxCount 1 ] ;
        sh:property [ sh:path caldav:customer ;     sh:datatype xsd:string ; sh:maxCount 1 ] ;
        sh:property [ sh:path caldav:authMode ;     sh:minCount 1 ; sh:maxCount 1 ] ;
        sh:property [ sh:path caldav:username ;     sh:datatype xsd:string ; sh:maxCount 1 ] ;
        sh:property [ sh:path caldav:password ;     sh:datatype xsd:string ; sh:maxCount 1 ] ;
        sh:property [ sh:path caldav:token ;        sh:datatype xsd:string ; sh:maxCount 1 ] ;
        sh:xone ( caldav:BasicAuthShape caldav:BearerAuthShape ) .
"#;

/// `crates/contreforts-connector-caldav/declaration.ttl:352-375`, verbatim in content.
const CALDAV_VARIANTS: &str = r#"
    caldav:BasicAuthShape a sh:NodeShape ;
        sh:property [ sh:path caldav:authMode ; sh:in ( "basic" ) ; sh:minCount 1 ; sh:maxCount 1 ] ;
        sh:property [ sh:path caldav:username ; sh:minCount 1 ; sh:maxCount 1 ; sh:datatype xsd:string ] ;
        sh:property [ sh:path caldav:password ; sh:minCount 1 ; sh:maxCount 1 ; sh:datatype xsd:string ] ;
        sh:property [ sh:path caldav:token ; sh:maxCount 0 ] .

    caldav:BearerAuthShape a sh:NodeShape ;
        sh:property [ sh:path caldav:authMode ; sh:in ( "bearer" ) ; sh:minCount 1 ; sh:maxCount 1 ] ;
        sh:property [ sh:path caldav:token ; sh:minCount 1 ; sh:maxCount 1 ; sh:datatype xsd:string ] ;
        sh:property [ sh:path caldav:username ; sh:maxCount 0 ] ;
        sh:property [ sh:path caldav:password ; sh:maxCount 0 ] .
"#;

/// `crates/contreforts-connector-forgejo/declaration.ttl`'s three direct properties — a kind
/// with **no** `sh:xone`, which is what makes it the right fixture for the "reconciliation is a
/// no-op" pin and for the target-KB guard (no variant machinery in the way).
const FORGEJO_TOP_LEVEL: &str = r#"
    forgejo:ForgejoConnectorShape a sh:NodeShape ;
        sh:targetClass forgejo:ForgejoConnector ;
        sh:property [ sh:path forgejo:label ;       sh:datatype xsd:string ; sh:minCount 1 ; sh:maxCount 1 ] ;
        sh:property [ sh:path forgejo:instanceUrl ; sh:datatype xsd:string ; sh:minCount 1 ; sh:maxCount 1 ] ;
        sh:property [ sh:path forgejo:token ;       sh:datatype xsd:string ; sh:minCount 1 ; sh:maxCount 1 ] .
"#;

/// `crates/contreforts-connector-erpnext/declaration.ttl`'s four direct properties. The
/// **singleton** kind (`ConnectorDescriptor.singleton == true`), and the only one in this file
/// with no `label` property at all — both facts the label-normalisation tests turn on.
const ERPNEXT_TOP_LEVEL: &str = r#"
    erpnext:ErpNextConnectorShape a sh:NodeShape ;
        sh:targetClass erpnext:ErpNextConnector ;
        sh:property [ sh:path erpnext:companyName ; sh:datatype xsd:string ; sh:minCount 1 ; sh:maxCount 1 ] ;
        sh:property [ sh:path erpnext:instanceUrl ; sh:datatype xsd:string ; sh:minCount 1 ; sh:maxCount 1 ] ;
        sh:property [ sh:path erpnext:apiKey ;      sh:datatype xsd:string ; sh:minCount 1 ; sh:maxCount 1 ] ;
        sh:property [ sh:path erpnext:apiSecret ;   sh:datatype xsd:string ; sh:minCount 1 ; sh:maxCount 1 ] .
"#;

/// Three of stalwart's sixteen properties, enough to drive the `xsd:integer` half of the
/// `Some("")` rule. `stalwart:listenPort` is `sh:datatype xsd:integer` in the real file
/// (`declaration.ttl:191`), as are `smtpLocalPort`, `smtpRelayPort` and `ollamaTimeoutSecs`.
const STALWART_TOP_LEVEL: &str = r#"
    stalwart:StalwartConnectorShape a sh:NodeShape ;
        sh:targetClass stalwart:StalwartConnector ;
        sh:property [ sh:path stalwart:label ;      sh:datatype xsd:string ;  sh:minCount 1 ; sh:maxCount 1 ] ;
        sh:property [ sh:path stalwart:adminUser ;  sh:datatype xsd:string ;  sh:minCount 1 ; sh:maxCount 1 ] ;
        sh:property [ sh:path stalwart:listenPort ; sh:datatype xsd:integer ; sh:minCount 1 ; sh:maxCount 1 ] .
"#;

/// Every fixture above, in one graph — the shape a composition root actually hands in.
fn all_declarations_ttl() -> String {
    format!(
        "{PREFIXES}{O365_TOP_LEVEL}{O365_VARIANTS}{CALDAV_TOP_LEVEL}{CALDAV_VARIANTS}\
         {FORGEJO_TOP_LEVEL}{ERPNEXT_TOP_LEVEL}{STALWART_TOP_LEVEL}"
    )
}

/// Only forgejo — so every *other* kind resolves to `ConnectorNamespace::Core`, which is what
/// the "a kind with no declaration is an `Err`, not a `CORE_NS` fallback" tests need.
fn forgejo_only_ttl() -> String {
    format!("{PREFIXES}{FORGEJO_TOP_LEVEL}")
}

// ── Variant rules (option (a): the caller supplies them) ─────────────────────
//
// Hand-written here, deliberately, and only here: these mirror what
// `contreforts_declaration::form_schemas` derives from the real declarations, and PR 2 asserts
// exactly that by deriving them from `PRODUCT_GRAPH_TTL` instead of restating them. Written by
// hand in this crate because `form_schemas` runs the full D2/D15/entityKind lint suite, which a
// deliberately-trimmed fixture cannot satisfy — the fixture is trimmed to isolate `field_iris`,
// not to be a lint-clean declaration.
//
// `shown` is the alternative's own field ids minus the discriminator; `required` is those of
// `shown` the alternative declares `sh:minCount > 0` for; `hidden` is
// (union of every alternative's field ids) - (this alternative's) - discriminator.

fn caldav_variants() -> Vec<VariantRule> {
    vec![
        VariantRule {
            discriminant_field: "authMode".to_string(),
            discriminant_value: "basic".to_string(),
            shown: vec!["username".to_string(), "password".to_string()],
            required: vec!["username".to_string(), "password".to_string()],
            hidden: vec!["token".to_string()],
        },
        VariantRule {
            discriminant_field: "authMode".to_string(),
            discriminant_value: "bearer".to_string(),
            shown: vec!["token".to_string()],
            required: vec!["token".to_string()],
            hidden: vec!["username".to_string(), "password".to_string()],
        },
    ]
}

fn o365_variants() -> Vec<VariantRule> {
    vec![
        VariantRule {
            discriminant_field: "authMode".to_string(),
            discriminant_value: "client_credentials".to_string(),
            shown: vec![
                "tenantId".to_string(),
                "clientId".to_string(),
                "clientSecret".to_string(),
            ],
            required: vec![
                "tenantId".to_string(),
                "clientId".to_string(),
                "clientSecret".to_string(),
            ],
            hidden: vec!["token".to_string(), "refreshToken".to_string()],
        },
        VariantRule {
            discriminant_field: "authMode".to_string(),
            discriminant_value: "delegated".to_string(),
            shown: vec!["token".to_string(), "refreshToken".to_string()],
            required: vec!["token".to_string()],
            hidden: vec![
                "tenantId".to_string(),
                "clientId".to_string(),
                "clientSecret".to_string(),
            ],
        },
    ]
}

// ── Scaffolding ──────────────────────────────────────────────────────────────

fn store() -> (tempfile::TempDir, ConfigStore) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config_store");
    let store = ConfigStore::open(&path).expect("store opens at a fresh path");
    (dir, store)
}

fn validator_for(ttl: &str) -> ConnectorValidator {
    ConnectorValidator::new(ttl, &all_connector_kinds())
        .expect("fixture declarations parse as well-formed SHACL")
}

fn add_company(store: &ConfigStore, slug: &str) {
    ConfigGraph::new(store, ConnectorDeclarations::none())
        .add_company(&CompanyConfig {
            slug: slug.to_string(),
            name: slug.to_string(),
        })
        .expect("company registers");
}

fn patch(entries: &[(&str, Option<&str>)]) -> BTreeMap<String, Option<String>> {
    entries
        .iter()
        .map(|(k, v)| ((*k).to_string(), v.map(str::to_string)))
        .collect()
}

/// Every `(predicate, object)` pair stored on `subject_iri` in `CONFIG_GRAPH`, sorted — read
/// through `ConfigStore::select`, the same primitive `tests/config_graph.rs::stored_triples`
/// uses. Deliberately **not** routed through `get_declared_connector`: a merge bug that made the
/// reader and the writer agree on a wrong answer would be invisible to a reader-only assertion.
fn stored_triples(store: &ConfigStore, subject_iri: &str) -> Vec<(String, String)> {
    let sparql =
        format!("SELECT ?p ?o WHERE {{ GRAPH <{CONFIG_GRAPH}> {{ <{subject_iri}> ?p ?o }} }}");
    let mut rows: Vec<(String, String)> = store
        .select(&sparql)
        .expect("wildcard triple query succeeds")
        .into_iter()
        .map(|row| {
            let p = row.iter().find(|(k, _)| k == "p").unwrap().1.clone();
            let o = row.iter().find(|(k, _)| k == "o").unwrap().1.clone();
            (p, o)
        })
        .collect();
    rows.sort();
    rows
}

/// How many triples `subject_iri` carries with predicate `predicate_iri`. The direct-store
/// counterpart to "the map has no such key": a clear must remove the *triple*, not merely stop
/// being reported.
fn triple_count(store: &ConfigStore, subject_iri: &str, predicate_iri: &str) -> usize {
    stored_triples(store, subject_iri)
        .into_iter()
        .filter(|(p, _)| p == predicate_iri)
        .count()
}

fn o365_pred(local: &str) -> String {
    format!("https://contreforts.ds-labs.org/ontologies/o365#{local}")
}

fn caldav_pred(local: &str) -> String {
    format!("https://contreforts.ds-labs.org/ontologies/caldav#{local}")
}

fn stalwart_pred(local: &str) -> String {
    format!("https://contreforts.ds-labs.org/ontologies/stalwart#{local}")
}

fn err_text(e: &ConfigGraphError) -> String {
    e.to_string()
}

// ─────────────────────────────────────────────────────────────────────────────
// A. The delete window must equal the write window
// ─────────────────────────────────────────────────────────────────────────────

/// **RED before change A.** The generic route's version of the guard: link a target KB, then
/// re-save the connector through `set_declared_connector`, and the `core:targetKnowledgeBase`
/// triple must still be there.
///
/// Asserts on a **direct store query**, not on either method's return value: neither
/// `get_declared_connector` nor `get_forgejo_connector` reads `targetKnowledgeBase` at all
/// (it is in no `field_iris` and in no typed getter's field list), so a reader-side assertion
/// could not see this triple whether it survived or not.
#[test]
fn set_declared_connector_preserves_the_target_kb_link() {
    let (_dir, store) = store();
    add_company(&store, "acme");
    let ttl = all_declarations_ttl();
    let validator = validator_for(&ttl);
    let cg = ConfigGraph::with_validator(&store, validator.declarations(), &validator);

    cg.set_forgejo_connector(
        "acme",
        &ForgejoConnectorConfig {
            label: "main".to_string(),
            url: "https://git.example.com".to_string(),
            token: "t0".to_string(),
        },
    )
    .expect("forgejo connector stores");
    cg.set_connector_target_kb("acme", "forgejo", Some("main"), "kb-alpha")
        .expect("target KB links");

    let conn_iri = namespaces::connector_iri("forgejo", "acme", Some("main"));
    let target_pred = format!("{CORE_NS}targetKnowledgeBase");
    assert_eq!(
        triple_count(&store, &conn_iri, &target_pred),
        1,
        "precondition: the target-KB link is stored before the re-save"
    );

    cg.set_declared_connector(
        "acme",
        "forgejo",
        Some("main"),
        &patch(&[("token", Some("t1"))]),
        &[],
    )
    .expect("re-saving one field through the generic route succeeds");

    assert_eq!(
        triple_count(&store, &conn_iri, &target_pred),
        1,
        "set_declared_connector destroyed the connector's core:targetKnowledgeBase link -- \
         the delete window is still the whole subject, not the declared field set. D5's \
         KB-delete guard keys on exactly this triple, so the knowledge base has silently \
         become deletable. Stored now: {:?}",
        stored_triples(&store, &conn_iri)
    );
}

/// **RED before change A**, and the half the phase-F plan missed entirely: change A fixes the
/// eleven *typed* setters too. Same scenario, `set_forgejo_connector` as the re-save.
#[test]
fn set_forgejo_connector_preserves_the_target_kb_link() {
    let (_dir, store) = store();
    add_company(&store, "acme");
    let ttl = all_declarations_ttl();
    let validator = validator_for(&ttl);
    let cg = ConfigGraph::with_validator(&store, validator.declarations(), &validator);

    let config = ForgejoConnectorConfig {
        label: "main".to_string(),
        url: "https://git.example.com".to_string(),
        token: "t0".to_string(),
    };
    cg.set_forgejo_connector("acme", &config)
        .expect("forgejo connector stores");
    cg.set_connector_target_kb("acme", "forgejo", Some("main"), "kb-alpha")
        .expect("target KB links");

    let conn_iri = namespaces::connector_iri("forgejo", "acme", Some("main"));
    let target_pred = format!("{CORE_NS}targetKnowledgeBase");

    cg.set_forgejo_connector(
        "acme",
        &ForgejoConnectorConfig {
            token: "t1".to_string(),
            ..config
        },
    )
    .expect("re-saving through the typed setter succeeds");

    assert_eq!(
        triple_count(&store, &conn_iri, &target_pred),
        1,
        "set_forgejo_connector destroyed the connector's core:targetKnowledgeBase link. This \
         is pre-existing and unpinned: every existing test links the KB *after* the connector \
         write and never re-saves. Stored now: {:?}",
        stored_triples(&store, &conn_iri)
    );
}

/// The deliberate scope limit, pinned so it is a decision rather than an accident: change A
/// narrows the delete window **only** for a kind whose namespace resolves to
/// `ConnectorNamespace::Declared`. A `Core` kind has no declared field set to narrow to, so its
/// write keeps wiping the whole subject — and therefore still destroys a target-KB link.
///
/// If a later change extends the narrowing to `Core` kinds, this test fails and the extension
/// is a reviewed event rather than a silent behaviour change.
#[test]
fn an_undeclared_kind_still_wipes_the_whole_subject() {
    let (_dir, store) = store();
    add_company(&store, "acme");
    // forgejo declared, gitlab NOT -- so gitlab resolves to `Core`.
    let ttl = forgejo_only_ttl();
    let validator = validator_for(&ttl);
    let cg = ConfigGraph::with_validator(&store, validator.declarations(), &validator);

    let config = contreforts_config::GitlabConnectorConfig {
        label: "main".to_string(),
        url: "https://gitlab.example.com".to_string(),
        token: "t0".to_string(),
    };
    cg.set_gitlab_connector("acme", &config)
        .expect("gitlab connector stores");
    cg.set_connector_target_kb("acme", "gitlab", Some("main"), "kb-alpha")
        .expect("target KB links");

    let conn_iri = namespaces::connector_iri("gitlab", "acme", Some("main"));
    let target_pred = format!("{CORE_NS}targetKnowledgeBase");
    assert_eq!(triple_count(&store, &conn_iri, &target_pred), 1);

    cg.set_gitlab_connector(
        "acme",
        &contreforts_config::GitlabConnectorConfig {
            token: "t1".to_string(),
            ..config
        },
    )
    .expect("re-saving an undeclared kind succeeds");

    assert_eq!(
        triple_count(&store, &conn_iri, &target_pred),
        0,
        "an undeclared (`ConnectorNamespace::Core`) kind is expected to keep today's \
         whole-subject wipe -- it has no declared field set to narrow the delete window to. \
         A non-zero count here means the narrowing was extended to `Core` kinds; that may be \
         desirable, but it is a behaviour change that must be reviewed, not inherited."
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// B. Omit vs. clear -- the two must not collapse into the same write
// ─────────────────────────────────────────────────────────────────────────────

/// **RED, the headline data-loss guard, validator wired.** Store an o365 `delegated` connector
/// with a `refreshToken`; patch *without* the `refreshToken` key; the stored token must survive
/// byte-equal.
///
/// This is the class-(a) exemplar: `o365:refreshToken` is optional in **every** applicable
/// shape (no flat `sh:minCount`; `sh:minCount 0` in the delegated alternative), so the omitting
/// patch validates cleanly and SHACL stops nothing. The plan's own proposed RED test used
/// caldav's `password` instead — that one passes **vacuously** against unfixed code, because
/// omitting `password` on a `basic` caldav fails both `sh:xone` alternatives and `set` returns
/// `Err`, so the password survives by accident of validation rather than by merge. See
/// `omitting_caldav_password_under_basic_is_rejected_by_validation_not_saved_by_the_merge`
/// below, which pins that distinction rather than relying on it.
#[test]
fn omitting_a_key_keeps_the_stored_secret_with_a_validator_wired() {
    let (_dir, store) = store();
    add_company(&store, "acme");
    let ttl = all_declarations_ttl();
    let validator = validator_for(&ttl);
    let cg = ConfigGraph::with_validator(&store, validator.declarations(), &validator);

    cg.set_declared_connector(
        "acme",
        "o365",
        Some("main"),
        &patch(&[
            ("label", Some("main")),
            ("authMode", Some("delegated")),
            ("token", Some("access-abc")),
            ("refreshToken", Some("refresh-secret-xyz")),
        ]),
        &o365_variants(),
    )
    .expect("the initial delegated o365 connector stores");

    // The form round trip W7 will actually perform: `GET` elides secrets (D8), so the client
    // never received `refreshToken` and cannot send it back. The key is ABSENT, not `None`.
    cg.set_declared_connector(
        "acme",
        "o365",
        Some("main"),
        &patch(&[
            ("label", Some("main")),
            ("authMode", Some("delegated")),
            ("token", Some("access-abc")),
            ("userPrincipal", Some("user@example.com")),
        ]),
        &o365_variants(),
    )
    .expect("a patch omitting the elided secret must be accepted, not rejected");

    let read = cg
        .get_declared_connector("acme", "o365", Some("main"))
        .expect("read succeeds")
        .expect("the connector exists");
    assert_eq!(
        read.get("refreshToken").map(String::as_str),
        Some("refresh-secret-xyz"),
        "omitting the `refreshToken` key deleted the stored refresh token. `absent` means \
         `the client is not talking about this field`, not `clear it` -- W7 elides secrets on \
         GET, so absent is the NORMAL state of every secret on a form round trip. Read back: {read:?}"
    );
    assert_eq!(
        triple_count(
            &store,
            &namespaces::connector_iri("o365", "acme", Some("main")),
            &o365_pred("refreshToken"),
        ),
        1,
        "the reader claimed the refresh token survived, but the store holds no \
         o365:refreshToken triple for that subject"
    );
    assert_eq!(
        read.get("userPrincipal").map(String::as_str),
        Some("user@example.com"),
        "the patch's own new value must still be applied -- carrying stored values forward \
         must not swallow the patch"
    );
}

/// **RED, the same shape with no validator wired** — the class-(b) blast radius. Built through
/// `ConfigGraph::new(store, validator.declarations())`, which sets `validator: None` while
/// keeping declarations in force.
///
/// Deliberately **not** `ConnectorDeclarations::none()`: that makes `connector_iris(kind)`
/// return `None` for every kind, which the "a kind with no declaration is an `Err`" rule
/// requires to fail before any merge happens — so a `none()`-based version of this test could
/// never reach the merge at all. (contreforts-config#11's own phrasing asks for `none()` here
/// and is unimplementable as written; this is the correction.)
///
/// caldav's `password` is the right field here precisely *because* it is the one SHACL would
/// have saved: with the validator off, nothing saves it but the merge.
#[test]
fn omitting_a_key_keeps_the_stored_secret_with_no_validator_wired() {
    let (_dir, store) = store();
    add_company(&store, "acme");
    let ttl = all_declarations_ttl();
    let validator = validator_for(&ttl);
    let cg = ConfigGraph::new(&store, validator.declarations());

    cg.set_declared_connector(
        "acme",
        "caldav",
        Some("main"),
        &patch(&[
            ("label", Some("main")),
            ("instanceUrl", Some("https://dav.example.com")),
            ("authMode", Some("basic")),
            ("username", Some("alice")),
            ("password", Some("hunter2")),
        ]),
        &caldav_variants(),
    )
    .expect("the initial basic caldav connector stores");

    cg.set_declared_connector(
        "acme",
        "caldav",
        Some("main"),
        &patch(&[
            ("label", Some("main")),
            ("instanceUrl", Some("https://dav2.example.com")),
            ("authMode", Some("basic")),
            ("username", Some("alice")),
        ]),
        &caldav_variants(),
    )
    .expect("a patch omitting the elided password is accepted with no validator wired");

    let conn_iri = namespaces::connector_iri("caldav", "acme", Some("main"));
    let read = cg
        .get_declared_connector("acme", "caldav", Some("main"))
        .expect("read succeeds")
        .expect("the connector exists");
    assert_eq!(
        read.get("password").map(String::as_str),
        Some("hunter2"),
        "omitting the `password` key deleted the stored password. With no validator wired \
         nothing but the merge stands between the user and this loss -- and `ConfigGraph::new` \
         leaves `validator: None`, so this is the DEFAULT configuration, not an exotic one. \
         Read back: {read:?}"
    );
    assert_eq!(
        triple_count(&store, &conn_iri, &caldav_pred("password")),
        1,
        "the reader claimed the password survived, but the store holds no caldav:password \
         triple for that subject"
    );
    assert_eq!(
        read.get("instanceUrl").map(String::as_str),
        Some("https://dav2.example.com"),
        "the patch's own updated value must win over the stored one"
    );
}

/// The other half of the omit/clear pair, and it must fail if the omit behaviour is substituted
/// for it. `refreshToken` present with value `None` is a user who emptied the field and saved:
/// the triple must go.
#[test]
fn an_explicit_none_clears_the_field() {
    let (_dir, store) = store();
    add_company(&store, "acme");
    let ttl = all_declarations_ttl();
    let validator = validator_for(&ttl);
    let cg = ConfigGraph::with_validator(&store, validator.declarations(), &validator);

    cg.set_declared_connector(
        "acme",
        "o365",
        Some("main"),
        &patch(&[
            ("label", Some("main")),
            ("authMode", Some("delegated")),
            ("token", Some("access-abc")),
            ("refreshToken", Some("refresh-secret-xyz")),
        ]),
        &o365_variants(),
    )
    .expect("the initial delegated o365 connector stores");

    // Key PRESENT, value None. The only difference from
    // `omitting_a_key_keeps_the_stored_secret_with_a_validator_wired`'s second patch.
    cg.set_declared_connector(
        "acme",
        "o365",
        Some("main"),
        &patch(&[
            ("label", Some("main")),
            ("authMode", Some("delegated")),
            ("token", Some("access-abc")),
            ("refreshToken", None),
        ]),
        &o365_variants(),
    )
    .expect("clearing an optional secret succeeds");

    let read = cg
        .get_declared_connector("acme", "o365", Some("main"))
        .expect("read succeeds")
        .expect("the connector exists");
    assert!(
        !read.contains_key("refreshToken"),
        "an explicit `refreshToken -> None` did not clear the field: the reader still reports \
         {:?}. `clear` and `omit` have collapsed into the same write, which is exactly the \
         defect the `Option<String>` patch type exists to make impossible.",
        read.get("refreshToken")
    );
    assert_eq!(
        triple_count(
            &store,
            &namespaces::connector_iri("o365", "acme", Some("main")),
            &o365_pred("refreshToken"),
        ),
        0,
        "the reader reported no refreshToken, but an o365:refreshToken triple is still stored \
         for that subject -- the field is invisible to this reader and live in the store"
    );
    assert_eq!(
        read.get("token").map(String::as_str),
        Some("access-abc"),
        "clearing one field must not disturb its neighbours"
    );
}

/// `Some(v)` with a non-empty `v` sets the field. The third of the four request states.
#[test]
fn an_explicit_some_sets_the_field() {
    let (_dir, store) = store();
    add_company(&store, "acme");
    let ttl = all_declarations_ttl();
    let validator = validator_for(&ttl);
    let cg = ConfigGraph::with_validator(&store, validator.declarations(), &validator);

    cg.set_declared_connector(
        "acme",
        "o365",
        Some("main"),
        &patch(&[
            ("label", Some("main")),
            ("authMode", Some("delegated")),
            ("token", Some("access-abc")),
            ("refreshToken", Some("refresh-old")),
        ]),
        &o365_variants(),
    )
    .unwrap();

    cg.set_declared_connector(
        "acme",
        "o365",
        Some("main"),
        &patch(&[("refreshToken", Some("refresh-new"))]),
        &o365_variants(),
    )
    .expect("a one-key patch setting a value succeeds");

    let read = cg
        .get_declared_connector("acme", "o365", Some("main"))
        .unwrap()
        .unwrap();
    assert_eq!(
        read.get("refreshToken").map(String::as_str),
        Some("refresh-new"),
        "an explicit `refreshToken -> Some(\"refresh-new\")` was not written. Read back: {read:?}"
    );
    assert_eq!(
        triple_count(
            &store,
            &namespaces::connector_iri("o365", "acme", Some("main")),
            &o365_pred("refreshToken"),
        ),
        1,
        "exactly one o365:refreshToken triple must remain -- a merge that appends rather than \
         replaces would leave two"
    );
}

/// The plan's own proposed RED test, kept as a scenario but asserting the **correct** outcome.
/// Omitting `password` from a `basic` caldav is rejected by SHACL, not saved by the merge:
/// alternative 1 requires `caldav:password sh:minCount 1`, alternative 2 requires
/// `caldav:token sh:minCount 1` and forbids `username`/`password`, so the instance satisfies
/// neither and `sh:xone` fails.
///
/// This test exists to keep that distinction honest. Without it, someone reading
/// `omitting_a_key_keeps_the_stored_secret_with_no_validator_wired` could reasonably conclude
/// the validator-on case works the same way, and write a "guard" that is really testing SHACL.
///
/// Note the write must still be **rejected**, not partially applied: nothing may be deleted
/// before validation runs.
#[test]
fn omitting_caldav_password_under_basic_is_rejected_by_validation_not_saved_by_the_merge() {
    let (_dir, store) = store();
    add_company(&store, "acme");
    let ttl = all_declarations_ttl();
    let validator = validator_for(&ttl);
    let cg = ConfigGraph::with_validator(&store, validator.declarations(), &validator);

    cg.set_declared_connector(
        "acme",
        "caldav",
        Some("main"),
        &patch(&[
            ("label", Some("main")),
            ("instanceUrl", Some("https://dav.example.com")),
            ("authMode", Some("basic")),
            ("username", Some("alice")),
            ("password", Some("hunter2")),
        ]),
        &caldav_variants(),
    )
    .unwrap();

    // Explicitly CLEARING the password (not omitting it) under `basic` leaves the instance
    // satisfying neither alternative.
    let result = cg.set_declared_connector(
        "acme",
        "caldav",
        Some("main"),
        &patch(&[("password", None)]),
        &caldav_variants(),
    );
    let err = result.expect_err(
        "clearing caldav:password while authMode is `basic` must be rejected: it satisfies \
         neither sh:xone alternative",
    );
    assert!(
        matches!(err, ConfigGraphError::ConnectorValidation(_)),
        "expected ConnectorValidation, got {err:?}"
    );

    let conn_iri = namespaces::connector_iri("caldav", "acme", Some("main"));
    assert_eq!(
        triple_count(&store, &conn_iri, &caldav_pred("password")),
        1,
        "a rejected write deleted the stored password anyway -- validation must run BEFORE \
         anything is removed. Stored now: {:?}",
        stored_triples(&store, &conn_iri)
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// The `Some("")` rule
// ─────────────────────────────────────────────────────────────────────────────

/// A browser posts `""`, not JSON `null`. On a declared `xsd:string` field that is a legitimate
/// empty value and must round-trip as `Some("")` — *not* be silently treated as a clear, which
/// would make the empty string a third spelling of "delete".
#[test]
fn an_empty_string_on_a_string_field_stores_an_empty_literal() {
    let (_dir, store) = store();
    add_company(&store, "acme");
    let ttl = all_declarations_ttl();
    let validator = validator_for(&ttl);
    let cg = ConfigGraph::new(&store, validator.declarations());

    cg.set_declared_connector(
        "acme",
        "o365",
        Some("main"),
        &patch(&[
            ("label", Some("main")),
            ("authMode", Some("delegated")),
            ("token", Some("access-abc")),
            ("userPrincipal", Some("user@example.com")),
        ]),
        &o365_variants(),
    )
    .unwrap();

    cg.set_declared_connector(
        "acme",
        "o365",
        Some("main"),
        &patch(&[("userPrincipal", Some(""))]),
        &o365_variants(),
    )
    .expect("an empty string on an xsd:string field is a legitimate value");

    let conn_iri = namespaces::connector_iri("o365", "acme", Some("main"));
    let read = cg
        .get_declared_connector("acme", "o365", Some("main"))
        .unwrap()
        .unwrap();
    assert_eq!(
        read.get("userPrincipal").map(String::as_str),
        Some(""),
        "`Some(\"\")` on an xsd:string field must store an empty literal and read back as \
         `Some(\"\")`. Reading back {:?} instead means the empty string was folded into the \
         clear path -- a third, undocumented spelling of `delete`.",
        read.get("userPrincipal")
    );
    assert_eq!(
        triple_count(&store, &conn_iri, &o365_pred("userPrincipal")),
        1,
        "an empty literal is still a triple; storing nothing is the clear behaviour, not the \
         empty-string behaviour"
    );
}

/// The rule that stops a connector from being written into an unreadable state. `""^^xsd:integer`
/// is constructible — `typed_literal` performs no lexical check — and once stored, every
/// subsequent read of that connector fails, because `parse_declared_field` returns
/// `Err(DeclaredFieldMismatch)` for declared kinds rather than defaulting. There is no path back
/// through the same form.
///
/// **With no validator wired**, deliberately: that is the configuration `ConfigGraph::new`
/// produces, and it is the one where nothing else would catch this.
#[test]
fn an_empty_string_on_an_integer_field_is_rejected_and_stores_nothing() {
    let (_dir, store) = store();
    add_company(&store, "acme");
    let ttl = all_declarations_ttl();
    let validator = validator_for(&ttl);
    let cg = ConfigGraph::new(&store, validator.declarations());

    cg.set_declared_connector(
        "acme",
        "stalwart",
        Some("main"),
        &patch(&[
            ("label", Some("main")),
            ("adminUser", Some("admin")),
            ("listenPort", Some("8080")),
        ]),
        &[],
    )
    .expect("a well-formed stalwart connector stores");

    let err = cg
        .set_declared_connector(
            "acme",
            "stalwart",
            Some("main"),
            &patch(&[("listenPort", Some(""))]),
            &[],
        )
        .expect_err(
            "`Some(\"\")` on an xsd:integer field must be an Err even with no validator wired",
        );

    assert!(
        matches!(err, ConfigGraphError::ConnectorValidation(_)),
        "expected ConnectorValidation, got {err:?}"
    );
    let msg = err_text(&err);
    assert!(
        msg.contains("listenPort"),
        "the error must name the offending field so a form can point at it; got: {msg}"
    );
    assert!(
        msg.contains("integer"),
        "the error must name the declared datatype it could not accept; got: {msg}"
    );

    let conn_iri = namespaces::connector_iri("stalwart", "acme", Some("main"));
    let stored: Vec<(String, String)> = stored_triples(&store, &conn_iri)
        .into_iter()
        .filter(|(p, _)| p == &stalwart_pred("listenPort"))
        .collect();
    assert_eq!(
        stored,
        vec![(stalwart_pred("listenPort"), "8080".to_string())],
        "the rejected write must leave the previously-stored listenPort untouched -- storing \
         `\"\"^^xsd:integer` makes every subsequent read of this connector fail with \
         DeclaredFieldMismatch, with no path back through the same form"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Variant reconciliation
// ─────────────────────────────────────────────────────────────────────────────

/// Switching caldav `basic` → `bearer` must drop `username`/`password` from the merged field set
/// before the write. Without this step the merge's own "absent key = unchanged" rule carries
/// them forward, the bearer alternative's `sh:maxCount 0` markers reject the instance, and the
/// switch is simply impossible through the generic route.
#[test]
fn a_variant_switch_drops_the_target_variants_excluded_fields() {
    let (_dir, store) = store();
    add_company(&store, "acme");
    let ttl = all_declarations_ttl();
    let validator = validator_for(&ttl);
    let cg = ConfigGraph::with_validator(&store, validator.declarations(), &validator);

    cg.set_declared_connector(
        "acme",
        "caldav",
        Some("main"),
        &patch(&[
            ("label", Some("main")),
            ("instanceUrl", Some("https://dav.example.com")),
            ("authMode", Some("basic")),
            ("username", Some("alice")),
            ("password", Some("hunter2")),
        ]),
        &caldav_variants(),
    )
    .expect("the initial basic caldav connector stores");

    cg.set_declared_connector(
        "acme",
        "caldav",
        Some("main"),
        &patch(&[("authMode", Some("bearer")), ("token", Some("bearer-tok"))]),
        &caldav_variants(),
    )
    .expect(
        "switching to bearer must succeed: the target variant's excluded fields are dropped \
         from the merged set before validation",
    );

    let conn_iri = namespaces::connector_iri("caldav", "acme", Some("main"));
    assert_eq!(
        triple_count(&store, &conn_iri, &caldav_pred("username")),
        0,
        "caldav:username survived the switch to bearer. The bearer alternative declares \
         `sh:maxCount 0` for it, so carrying it forward makes the sh:xone fail -- and, worse, \
         leaves a stale credential stored for an auth mode that does not use it. Stored: {:?}",
        stored_triples(&store, &conn_iri)
    );
    assert_eq!(
        triple_count(&store, &conn_iri, &caldav_pred("password")),
        0,
        "caldav:password survived the switch to bearer -- a stale stored secret under an auth \
         mode that cannot use it. Stored: {:?}",
        stored_triples(&store, &conn_iri)
    );
    let read = cg
        .get_declared_connector("acme", "caldav", Some("main"))
        .unwrap()
        .unwrap();
    assert_eq!(read.get("token").map(String::as_str), Some("bearer-tok"));
    assert_eq!(
        read.get("instanceUrl").map(String::as_str),
        Some("https://dav.example.com"),
        "reconciliation must drop only the target variant's EXCLUDED fields -- a field no \
         alternative mentions is not variant-governed and must survive"
    );
}

/// The mirror image, in the other direction and on the other connector: o365 `delegated` →
/// `client_credentials` must leave zero `token`/`refreshToken` triples. Two directions on two
/// kinds, because a reconciliation implemented as "drop whatever the *source* variant showed"
/// passes the caldav case and fails here.
#[test]
fn a_variant_switch_drops_excluded_fields_in_the_other_direction_too() {
    let (_dir, store) = store();
    add_company(&store, "acme");
    let ttl = all_declarations_ttl();
    let validator = validator_for(&ttl);
    let cg = ConfigGraph::with_validator(&store, validator.declarations(), &validator);

    cg.set_declared_connector(
        "acme",
        "o365",
        Some("main"),
        &patch(&[
            ("label", Some("main")),
            ("authMode", Some("delegated")),
            ("token", Some("access-abc")),
            ("refreshToken", Some("refresh-secret-xyz")),
        ]),
        &o365_variants(),
    )
    .expect("the initial delegated o365 connector stores");

    cg.set_declared_connector(
        "acme",
        "o365",
        Some("main"),
        &patch(&[
            ("authMode", Some("client_credentials")),
            ("tenantId", Some("tid")),
            ("clientId", Some("cid")),
            ("clientSecret", Some("csec")),
        ]),
        &o365_variants(),
    )
    .expect("switching to client_credentials must succeed");

    let conn_iri = namespaces::connector_iri("o365", "acme", Some("main"));
    assert_eq!(
        triple_count(&store, &conn_iri, &o365_pred("token")),
        0,
        "o365:token survived the switch to client_credentials, whose alternative declares \
         `sh:maxCount 0` for it. Stored: {:?}",
        stored_triples(&store, &conn_iri)
    );
    assert_eq!(
        triple_count(&store, &conn_iri, &o365_pred("refreshToken")),
        0,
        "o365:refreshToken survived the switch to client_credentials -- a stale stored secret \
         under an auth mode that cannot use it. Stored: {:?}",
        stored_triples(&store, &conn_iri)
    );
}

/// **The variant test that carries the store-level assertions.**
///
/// The two tests above are real, but with a validator wired SHACL rejects the un-reconciled
/// write *before* anything is stored — so their `triple_count == 0` assertions never execute
/// when reconciliation is broken, and the only thing proven is "the switch is impossible
/// without it". That is a weaker claim than it looks, and it is exactly the shape of vacuity
/// this project has been bitten by: a guard that passes because something upstream caught the
/// problem first.
///
/// With **no validator wired** — `ConfigGraph::new`'s default, and the configuration every
/// caller in the tree uses today — nothing rejects the un-reconciled write. It simply succeeds
/// and leaves `caldav:username`/`caldav:password` stored under an auth mode that cannot use
/// them: a stale credential, retrievable by anyone who can read the config graph, for an
/// account the user believes they stopped using. Here reconciliation is the *only* mechanism,
/// so the assertions below are the ones that fire.
#[test]
fn a_variant_switch_drops_excluded_fields_with_no_validator_wired() {
    let (_dir, store) = store();
    add_company(&store, "acme");
    let ttl = all_declarations_ttl();
    let validator = validator_for(&ttl);
    let cg = ConfigGraph::new(&store, validator.declarations());

    cg.set_declared_connector(
        "acme",
        "caldav",
        Some("main"),
        &patch(&[
            ("label", Some("main")),
            ("instanceUrl", Some("https://dav.example.com")),
            ("authMode", Some("basic")),
            ("username", Some("alice")),
            ("password", Some("hunter2")),
        ]),
        &caldav_variants(),
    )
    .expect("the initial basic caldav connector stores");

    cg.set_declared_connector(
        "acme",
        "caldav",
        Some("main"),
        &patch(&[("authMode", Some("bearer")), ("token", Some("bearer-tok"))]),
        &caldav_variants(),
    )
    .expect("switching to bearer succeeds -- with no validator wired nothing rejects it");

    let conn_iri = namespaces::connector_iri("caldav", "acme", Some("main"));
    assert_eq!(
        triple_count(&store, &conn_iri, &caldav_pred("username")),
        0,
        "caldav:username is still stored after the switch to bearer. With no validator wired \
         nothing rejects this write, so reconciliation is the only thing that drops it -- and \
         what is left behind is a live credential for an auth mode the connector no longer \
         uses. Stored: {:?}",
        stored_triples(&store, &conn_iri)
    );
    assert_eq!(
        triple_count(&store, &conn_iri, &caldav_pred("password")),
        0,
        "caldav:password is still stored after the switch to bearer -- a stale secret, \
         readable by anyone who can read the config graph, for an account the user believes \
         they stopped using. Stored: {:?}",
        stored_triples(&store, &conn_iri)
    );
    assert_eq!(
        triple_count(&store, &conn_iri, &caldav_pred("token")),
        1,
        "and the new variant's own field must have been written"
    );
}

/// Five of the seven declared kinds (erpnext, pennylane, forgejo, gitlab, stalwart) declare no
/// `sh:xone` at all. For them reconciliation is a no-op, and this says so rather than leaving it
/// to be inferred: an implementation that dropped fields when handed an empty rule set — or that
/// required a non-empty one — would break every one of those five.
#[test]
fn variant_reconciliation_is_a_no_op_for_a_kind_with_no_variants() {
    let (_dir, store) = store();
    add_company(&store, "acme");
    let ttl = all_declarations_ttl();
    let validator = validator_for(&ttl);
    let cg = ConfigGraph::with_validator(&store, validator.declarations(), &validator);

    cg.set_declared_connector(
        "acme",
        "forgejo",
        Some("main"),
        &patch(&[
            ("label", Some("main")),
            ("instanceUrl", Some("https://git.example.com")),
            ("token", Some("t0")),
        ]),
        &[],
    )
    .expect("a no-variant kind stores with an empty rule set");

    cg.set_declared_connector(
        "acme",
        "forgejo",
        Some("main"),
        &patch(&[("token", Some("t1"))]),
        &[],
    )
    .expect("and updates with an empty rule set");

    let read = cg
        .get_declared_connector("acme", "forgejo", Some("main"))
        .unwrap()
        .unwrap();
    assert_eq!(
        read,
        BTreeMap::from([
            ("label".to_string(), "main".to_string()),
            (
                "instanceUrl".to_string(),
                "https://git.example.com".to_string()
            ),
            ("token".to_string(), "t1".to_string()),
        ]),
        "with no variant rules every declared field must survive the merge untouched -- \
         reconciliation is a no-op, not a filter that empties the set"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Label: the IRI segment and the declared field must stay equal
// ─────────────────────────────────────────────────────────────────────────────

/// `label` is simultaneously an IRI segment and a declared `sh:path` on five of the seven kinds.
/// The typed setters keep the two equal by construction (`set_caldav_connector` passes
/// `Some(&config.label)` and `("label", Some(config.label))` from the same field). A generic
/// patch can break that: `{label: Some("new")}` against IRI segment `old` would write subject
/// `…/caldav/acme/old` carrying `caldav:label "new"`, after which `list_connector_labels`
/// (which reads the **field**) reports `new`, `get_caldav_connector(company, "new")` returns
/// `Ok(None)`, and `remove_connector(company, "caldav", Some("new"))` (which addresses by the
/// **IRI segment**) deletes nothing. The stored secret is stranded at an unaddressable IRI.
#[test]
fn a_patch_renaming_the_label_is_rejected_naming_both_values() {
    let (_dir, store) = store();
    add_company(&store, "acme");
    let ttl = all_declarations_ttl();
    let validator = validator_for(&ttl);
    let cg = ConfigGraph::with_validator(&store, validator.declarations(), &validator);

    cg.set_declared_connector(
        "acme",
        "forgejo",
        Some("old"),
        &patch(&[
            ("label", Some("old")),
            ("instanceUrl", Some("https://git.example.com")),
            ("token", Some("t0")),
        ]),
        &[],
    )
    .unwrap();

    let err = cg
        .set_declared_connector(
            "acme",
            "forgejo",
            Some("old"),
            &patch(&[("label", Some("new"))]),
            &[],
        )
        .expect_err("renaming a connector through this path must be rejected, not performed");

    let msg = err_text(&err);
    assert!(
        msg.contains("old") && msg.contains("new"),
        "the error must name BOTH the IRI segment and the patch's label so the caller can see \
         which one it got wrong; got: {msg}"
    );

    let old_iri = namespaces::connector_iri("forgejo", "acme", Some("old"));
    assert_eq!(
        stored_triples(&store, &old_iri)
            .into_iter()
            .filter(|(p, _)| p.ends_with("#label"))
            .map(|(_, o)| o)
            .collect::<Vec<_>>(),
        vec!["old".to_string()],
        "the stored subject must be untouched by a rejected rename"
    );
    let new_iri = namespaces::connector_iri("forgejo", "acme", Some("new"));
    assert!(
        stored_triples(&store, &new_iri).is_empty(),
        "a rejected rename must not have written anything at the new IRI either"
    );
}

/// **The one deliberate exception to "clear and omit are different actions", pinned so it is a
/// decision rather than an accident.** `label` is the only declared field for which an explicit
/// `None` does *not* clear: the auto-fill below re-establishes it from the normalised label
/// argument, so `{label: None}` and omitting `label` entirely produce the same write.
///
/// That collapse is intentional and it is the safe direction. Actually honouring the clear would
/// leave the connector subject in place at `…/forgejo/acme/main` carrying no `forgejo:label`
/// triple, and `list_connector_labels` reads the **field**, not the IRI segment — so the
/// connector, and the secret stored on it, would vanish from every listing while remaining in
/// the store. That is the same stranded-connector failure the rename rejection above exists to
/// prevent, reached by a different route. A patch cannot be allowed to reach it.
///
/// **No validator wired, deliberately.** With one, `label`'s own `sh:minCount 1` rejects a write
/// that dropped the label before anything is stored, so the assertions below would be pinning
/// SHACL rather than the auto-fill — and `ConfigGraph::new` leaves `validator: None`, which is
/// the default configuration. Under a mutation that honours the clear, this test must fail at
/// the assertion showing the connector missing from the typed lister, not at an `.expect`.
#[test]
fn an_explicit_none_on_label_is_absorbed_rather_than_stranding_the_connector() {
    let (_dir, store) = store();
    add_company(&store, "acme");
    let ttl = all_declarations_ttl();
    let validator = validator_for(&ttl);
    let cg = ConfigGraph::new(&store, validator.declarations());

    cg.set_declared_connector(
        "acme",
        "forgejo",
        Some("main"),
        &patch(&[
            ("label", Some("main")),
            ("instanceUrl", Some("https://git.example.com")),
            ("token", Some("t0")),
        ]),
        &[],
    )
    .expect("the initial forgejo connector stores");

    // Key PRESENT, value None -- for every other field this is "clear it".
    cg.set_declared_connector(
        "acme",
        "forgejo",
        Some("main"),
        &patch(&[("label", None), ("token", Some("t1"))]),
        &[],
    )
    .expect("a patch clearing `label` must be absorbed, not rejected and not performed");

    let conn_iri = namespaces::connector_iri("forgejo", "acme", Some("main"));
    assert_eq!(
        stored_triples(&store, &conn_iri)
            .into_iter()
            .filter(|(p, _)| p.ends_with("#label"))
            .map(|(_, o)| o)
            .collect::<Vec<_>>(),
        vec!["main".to_string()],
        "`{{label: None}}` cleared the declared `label` field. The connector subject is still \
         stored at {conn_iri} but carries no label triple, so `list_connector_labels` -- which \
         reads the FIELD, not the IRI segment -- can no longer see it, and the token stored on \
         it is invisible to every listing. Stored now: {:?}",
        stored_triples(&store, &conn_iri)
    );
    assert_eq!(
        cg.list_forgejo_connectors("acme")
            .unwrap()
            .into_iter()
            .map(|c| c.label)
            .collect::<Vec<_>>(),
        vec!["main".to_string()],
        "the typed lister must still see the connector after a `{{label: None}}` patch"
    );
    let read = cg
        .get_declared_connector("acme", "forgejo", Some("main"))
        .unwrap()
        .expect("the connector must still be readable");
    assert_eq!(
        read.get("token").map(String::as_str),
        Some("t1"),
        "and the rest of the patch must still have been applied: {read:?}"
    );
}

/// The other side of the same rule, and the reason a rename can be rejected without making the
/// generic route unusable: on a **create**, a patch that omits `label` gets the normalised label
/// argument written into the declared `label` field.
///
/// Settled here rather than left open, on the evidence of the typed setters: `set_caldav_connector`
/// passes `Some(&config.label)` as the IRI segment and `("label", Some(config.label))` as the
/// field, from one and the same value — "the IRI segment and the field are equal by construction"
/// is the tree's existing rule, and auto-filling is the only way a generic patch can honour it
/// without demanding the caller state the same string twice. The alternative (write no `label`
/// triple) produces a connector `list_connector_labels` cannot see and `get_<kind>_connector`
/// cannot read, which is the exact failure mode the rename rejection above exists to prevent.
#[test]
fn a_create_that_omits_label_takes_it_from_the_normalised_label_argument() {
    let (_dir, store) = store();
    add_company(&store, "acme");
    let ttl = all_declarations_ttl();
    let validator = validator_for(&ttl);
    let cg = ConfigGraph::with_validator(&store, validator.declarations(), &validator);

    cg.set_declared_connector(
        "acme",
        "forgejo",
        Some("main"),
        &patch(&[
            ("instanceUrl", Some("https://git.example.com")),
            ("token", Some("t0")),
        ]),
        &[],
    )
    .expect("a create that omits `label` must succeed, not fail its own sh:minCount 1");

    let read = cg
        .get_declared_connector("acme", "forgejo", Some("main"))
        .unwrap()
        .unwrap();
    assert_eq!(
        read.get("label").map(String::as_str),
        Some("main"),
        "the declared `label` field must be filled from the IRI segment. Anything else leaves \
         a connector `list_connector_labels` cannot see and `get_forgejo_connector` cannot read. \
         Read back: {read:?}"
    );
    assert_eq!(
        cg.list_forgejo_connectors("acme")
            .unwrap()
            .into_iter()
            .map(|c| c.label)
            .collect::<Vec<_>>(),
        vec!["main".to_string()],
        "and the typed lister -- which reads the FIELD, not the IRI segment -- must see it"
    );
}

/// Label normalisation, label-scoped half: `namespaces::connector_iri` switches between the
/// two-segment and three-segment shape purely on `label.is_some()`, with **no** reference to
/// `descriptor.singleton`. `write_connector` does not normalise — it passes the caller's
/// `Option` straight through. So `set_declared_connector("acme", "forgejo", None, …)` would
/// write a forgejo connector at the *singleton* IRI, which `get_forgejo_connector` can never
/// read and `remove_connector` can never delete. `remove_connector` already normalises; both
/// new methods must do it identically.
///
/// **No validator wired, deliberately.** With one, a non-normalising implementation is stopped
/// by `label`'s own `sh:minCount 1` before it writes anything, and the IRI assertions below
/// never run — the test would then be pinning SHACL rather than normalisation. Validation is
/// also not what protects this in production: `ConfigGraph::new` leaves `validator: None`, and
/// the two singleton kinds (`erpnext`, `pennylane`) declare no `label` property at all, so
/// there is no `sh:minCount` to catch the mirror-image mistake in
/// `a_singleton_kind_forces_the_label_to_none` either.
#[test]
fn a_label_scoped_kind_normalises_a_missing_label_to_default() {
    let (_dir, store) = store();
    add_company(&store, "acme");
    let ttl = all_declarations_ttl();
    let validator = validator_for(&ttl);
    let cg = ConfigGraph::new(&store, validator.declarations());

    cg.set_declared_connector(
        "acme",
        "forgejo",
        None,
        &patch(&[
            ("instanceUrl", Some("https://git.example.com")),
            ("token", Some("t0")),
        ]),
        &[],
    )
    .expect("a label-scoped kind written with label: None normalises to \"default\"");

    let default_iri = namespaces::connector_iri("forgejo", "acme", Some("default"));
    let singleton_iri = namespaces::connector_iri("forgejo", "acme", None);
    assert!(
        !stored_triples(&store, &default_iri).is_empty(),
        "nothing was written at the label-scoped `default` IRI {default_iri}"
    );
    assert!(
        stored_triples(&store, &singleton_iri).is_empty(),
        "a forgejo connector was written at the two-segment SINGLETON IRI {singleton_iri}. \
         `get_forgejo_connector` can never read it and `remove_connector` can never delete it \
         -- W7's route is `/connector-values/{{kind}}[/{{label}}]`, so this is one URL away. \
         Stored there: {:?}",
        stored_triples(&store, &singleton_iri)
    );
    assert!(
        cg.get_forgejo_connector("acme", "default")
            .unwrap()
            .is_some(),
        "the typed getter must find what the generic writer wrote"
    );
    // And the reader normalises the same way.
    assert!(
        cg.get_declared_connector("acme", "forgejo", None)
            .unwrap()
            .is_some(),
        "get_declared_connector must normalise `None` to `default` too, or reads and writes \
         address different subjects"
    );
}

/// Label normalisation, singleton half — the mirror image, which a normaliser that only handled
/// one direction would fail. `erpnext` is `singleton: true`, so a label argument must be forced
/// to `None`; otherwise `set_declared_connector("acme", "erpnext", Some("x"), …)` writes a
/// three-segment IRI `get_erpnext_connector` never looks at.
#[test]
fn a_singleton_kind_forces_the_label_to_none() {
    let (_dir, store) = store();
    add_company(&store, "acme");
    let ttl = all_declarations_ttl();
    let validator = validator_for(&ttl);
    let cg = ConfigGraph::with_validator(&store, validator.declarations(), &validator);

    cg.set_declared_connector(
        "acme",
        "erpnext",
        Some("x"),
        &patch(&[
            ("companyName", Some("Acme Ltd")),
            ("instanceUrl", Some("https://erp.example.com")),
            ("apiKey", Some("k")),
            ("apiSecret", Some("s")),
        ]),
        &[],
    )
    .expect("a singleton kind ignores the label argument rather than rejecting it");

    let singleton_iri = namespaces::connector_iri("erpnext", "acme", None);
    let labelled_iri = namespaces::connector_iri("erpnext", "acme", Some("x"));
    assert!(
        stored_triples(&store, &labelled_iri).is_empty(),
        "an erpnext connector was written at the three-segment LABEL-SCOPED IRI \
         {labelled_iri}, which `get_erpnext_connector` never reads and `remove_connector` \
         never deletes. Stored there: {:?}",
        stored_triples(&store, &labelled_iri)
    );
    assert!(
        !stored_triples(&store, &singleton_iri).is_empty(),
        "nothing was written at the singleton IRI {singleton_iri}"
    );

    let typed = cg
        .get_erpnext_connector("acme")
        .unwrap()
        .expect("the typed getter must read the same subject the generic writer wrote");
    assert_eq!(typed.company_name, "Acme Ltd");
    assert_eq!(typed.api_secret, "s");

    assert!(
        cg.get_declared_connector("acme", "erpnext", Some("x"))
            .unwrap()
            .is_some(),
        "and the generic reader must normalise the same way"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Refusals: unknown key, unknown kind, undeclared kind, absent company
// ─────────────────────────────────────────────────────────────────────────────

/// Any patch key not in `field_iris` is an `Err` naming the key. **Not** silently projected onto
/// the declared set: for a form-driven `PUT` of a secret, the silent branch is a write the
/// client then reads back as success while the value was never stored anywhere.
#[test]
fn an_unknown_patch_key_is_rejected_naming_the_key() {
    let (_dir, store) = store();
    add_company(&store, "acme");
    let ttl = all_declarations_ttl();
    let validator = validator_for(&ttl);
    let cg = ConfigGraph::with_validator(&store, validator.declarations(), &validator);

    let err = cg
        .set_declared_connector(
            "acme",
            "forgejo",
            Some("main"),
            &patch(&[
                ("label", Some("main")),
                ("instanceUrl", Some("https://git.example.com")),
                ("token", Some("t0")),
                ("tokenn", Some("typo-value")),
            ]),
            &[],
        )
        .expect_err("an undeclared patch key must be rejected, not discarded");

    let msg = err_text(&err);
    assert!(
        msg.contains("tokenn"),
        "the error must name the offending key -- a typo silently dropped is a secret the \
         client believes it saved; got: {msg}"
    );

    assert!(
        stored_triples(
            &store,
            &namespaces::connector_iri("forgejo", "acme", Some("main"))
        )
        .is_empty(),
        "a rejected patch must write nothing at all"
    );
}

/// An unrecognised `kind` is a named `Err` from **both** methods, never a silent no-op and never
/// an `Ok(None)` that a route would render as "not configured yet".
#[test]
fn an_unknown_kind_is_rejected_naming_the_kind_from_both_methods() {
    let (_dir, store) = store();
    add_company(&store, "acme");
    let ttl = all_declarations_ttl();
    let validator = validator_for(&ttl);
    let cg = ConfigGraph::with_validator(&store, validator.declarations(), &validator);

    let read_err = cg
        .get_declared_connector("acme", "nosuchkind", Some("main"))
        .expect_err("get_declared_connector must reject an unknown kind");
    assert!(
        err_text(&read_err).contains("nosuchkind"),
        "got: {}",
        err_text(&read_err)
    );

    let write_err = cg
        .set_declared_connector(
            "acme",
            "nosuchkind",
            Some("main"),
            &patch(&[("token", Some("t"))]),
            &[],
        )
        .expect_err("set_declared_connector must reject an unknown kind");
    assert!(
        err_text(&write_err).contains("nosuchkind"),
        "got: {}",
        err_text(&write_err)
    );
}

/// A kind that exists in `ALL_CONNECTOR_DESCRIPTORS` but has **no declaration** — i.e. one that
/// resolves to `ConnectorNamespace::Core` — is a named `Err` from both methods, **not** a
/// `CORE_NS` fallback. A generic route over an undeclared kind has no vocabulary to key on: it
/// would accept any field name at all and store it under `core:`, producing exactly the
/// mixed-namespace state `ConnectorNamespace::field_iri` already refuses per-field.
#[test]
fn a_kind_with_no_declaration_is_rejected_by_both_methods() {
    let (_dir, store) = store();
    add_company(&store, "acme");
    // Only forgejo is declared here, so gitlab resolves to `Core`.
    let ttl = forgejo_only_ttl();
    let validator = validator_for(&ttl);
    let cg = ConfigGraph::with_validator(&store, validator.declarations(), &validator);

    let read_err = cg
        .get_declared_connector("acme", "gitlab", Some("main"))
        .expect_err("get_declared_connector must reject an undeclared kind");
    assert!(
        err_text(&read_err).contains("gitlab"),
        "the error must name the kind; got: {}",
        err_text(&read_err)
    );

    let write_err = cg
        .set_declared_connector(
            "acme",
            "gitlab",
            Some("main"),
            &patch(&[("token", Some("t"))]),
            &[],
        )
        .expect_err("set_declared_connector must reject an undeclared kind");
    assert!(
        err_text(&write_err).contains("gitlab"),
        "the error must name the kind; got: {}",
        err_text(&write_err)
    );

    assert!(
        stored_triples(
            &store,
            &namespaces::connector_iri("gitlab", "acme", Some("main"))
        )
        .is_empty(),
        "a rejected write must have stored nothing under core: either"
    );
}

/// A declared kind with nothing stored reads as `Ok(None)` — **not** `Ok(Some(empty))`, which a
/// route would render as an existing connector with every field blank.
#[test]
fn a_declared_but_absent_connector_reads_as_ok_none() {
    let (_dir, store) = store();
    add_company(&store, "acme");
    let ttl = all_declarations_ttl();
    let validator = validator_for(&ttl);
    let cg = ConfigGraph::with_validator(&store, validator.declarations(), &validator);

    assert_eq!(
        cg.get_declared_connector("acme", "o365", Some("main"))
            .expect("reading an absent connector is not an error"),
        None,
        "an unconfigured connector must be `Ok(None)`, not `Ok(Some({{}}))` -- marking every \
         field optional on the generic read must not lose the existence check that \
         `<conn> a <type>` still performs"
    );
}

/// A partially-populated connector must still read back rather than silently matching nothing —
/// the reason every field is marked `required: false` on the generic read. Here `refreshToken`,
/// `userPrincipal`, `customer`, `tenantId`, `clientId` and `clientSecret` are all absent, and a
/// reader that put any of them in the main graph pattern would return `Ok(None)`.
#[test]
fn a_partially_populated_connector_still_reads_back() {
    let (_dir, store) = store();
    add_company(&store, "acme");
    let ttl = all_declarations_ttl();
    let validator = validator_for(&ttl);
    let cg = ConfigGraph::with_validator(&store, validator.declarations(), &validator);

    cg.set_declared_connector(
        "acme",
        "o365",
        Some("main"),
        &patch(&[
            ("label", Some("main")),
            ("authMode", Some("delegated")),
            ("token", Some("access-abc")),
        ]),
        &o365_variants(),
    )
    .unwrap();

    let read = cg
        .get_declared_connector("acme", "o365", Some("main"))
        .unwrap()
        .expect("a connector with six of its nine declared fields absent must still read back");
    assert_eq!(
        read,
        BTreeMap::from([
            ("label".to_string(), "main".to_string()),
            ("authMode".to_string(), "delegated".to_string()),
            ("token".to_string(), "access-abc".to_string()),
        ]),
        "the map must contain exactly the stored fields -- no empty-string placeholders for \
         the absent ones, which would be indistinguishable from a stored empty literal"
    );
}

/// Both methods call `require_company` first. Today only `write_connector` does; `fetch_connector`
/// does not, so a read against a nonexistent company returns `Ok(None)` — indistinguishable from
/// "company exists, connector not configured". W7 serves both from one route.
#[test]
fn a_nonexistent_company_errors_from_both_methods() {
    let (_dir, store) = store();
    add_company(&store, "acme");
    let ttl = all_declarations_ttl();
    let validator = validator_for(&ttl);
    let cg = ConfigGraph::with_validator(&store, validator.declarations(), &validator);

    let read_err = cg
        .get_declared_connector("nosuchco", "forgejo", Some("main"))
        .expect_err(
            "reading a connector for an unregistered company must be an Err, not Ok(None) -- \
             `not configured` and `no such company` are different answers and a route renders \
             them differently",
        );
    assert!(
        err_text(&read_err).contains("nosuchco"),
        "got: {}",
        err_text(&read_err)
    );

    let write_err = cg
        .set_declared_connector(
            "nosuchco",
            "forgejo",
            Some("main"),
            &patch(&[("token", Some("t"))]),
            &[],
        )
        .expect_err("writing for an unregistered company must be an Err");
    assert!(
        err_text(&write_err).contains("nosuchco"),
        "got: {}",
        err_text(&write_err)
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// `field_iris` is not silently a subset
// ─────────────────────────────────────────────────────────────────────────────

/// `ConnectorIris::field_iris`'s own doc records that it skips any `sh:property` whose `sh:path`
/// is not a single simple predicate IRI (inverse/sequence paths), and **nothing downstream can
/// detect the skip**. Both new methods key entirely on `field_iris`, so a skipped property is a
/// field neither reads nor writes, with no error anywhere — the first exotic path in any
/// declaration would produce exactly that, silently.
///
/// This test compares `field_iris`'s key set against the local names scanned textually out of
/// each fixture's **top-level** const. That the fixtures are split into `*_TOP_LEVEL` /
/// `*_VARIANTS` is what makes the scan exact: `field_iris` reads the target shape's *direct*
/// `sh:property` entries only, so the `sh:xone` alternatives' own `sh:path`s must not be in the
/// scanned text either.
#[test]
fn field_iris_key_set_equals_the_declared_top_level_sh_path_set() {
    let ttl = all_declarations_ttl();
    let validator = validator_for(&ttl);
    let declarations = validator.declarations();

    for (kind, top_level) in [
        ("o365", O365_TOP_LEVEL),
        ("caldav", CALDAV_TOP_LEVEL),
        ("forgejo", FORGEJO_TOP_LEVEL),
        ("erpnext", ERPNEXT_TOP_LEVEL),
        ("stalwart", STALWART_TOP_LEVEL),
    ] {
        let declared: BTreeSet<String> = top_level
            .split("sh:path ")
            .skip(1)
            .map(|rest| {
                let token = rest
                    .split_whitespace()
                    .next()
                    .expect("a `sh:path` is followed by a term");
                token
                    .split(':')
                    .nth(1)
                    .expect("every fixture `sh:path` is a `prefix:localName` pair")
                    .to_string()
            })
            .collect();
        assert!(
            !declared.is_empty(),
            "the textual scan of {kind}'s top-level fixture found no sh:path at all -- the \
             scan itself is broken, so this test would pass for the wrong reason"
        );

        let iris = declarations
            .connector_iris(kind)
            .unwrap_or_else(|| panic!("{kind} must resolve to a declared namespace"));
        let mapped: BTreeSet<String> = iris.field_iris.keys().cloned().collect();

        assert_eq!(
            mapped,
            declared,
            "{kind}: `ConnectorIris::field_iris` does not cover its declaration's direct \
             `sh:path` set. Missing from field_iris: {:?}; present in field_iris but not \
             declared at the top level: {:?}. A field missing here is one \
             `get_declared_connector`/`set_declared_connector` neither read nor write, with \
             no error raised anywhere -- the equality holds today by coincidence (no \
             declaration uses an inverse or sequence path), not by construction.",
            declared.difference(&mapped).collect::<Vec<_>>(),
            mapped.difference(&declared).collect::<Vec<_>>(),
        );
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Generic write -> typed read
// ─────────────────────────────────────────────────────────────────────────────

/// The bullet that stops a green suite from meaning nothing, in this crate's synthetic form
/// (PR 2 does it for all seven kinds against the real declarations). The typed getters mark most
/// fields `required: true`, and `fetch_connector` puts required predicates in the **main** graph
/// pattern — so one missing predicate means no row at all. `get_declared_connector` marks every
/// field optional by design and would still return the connector. Without this assertion the two
/// readers can disagree and only the lenient one is under test.
#[test]
fn a_generic_write_is_readable_by_the_typed_getter() {
    let (_dir, store) = store();
    add_company(&store, "acme");
    let ttl = all_declarations_ttl();
    let validator = validator_for(&ttl);
    let cg = ConfigGraph::with_validator(&store, validator.declarations(), &validator);

    cg.set_declared_connector(
        "acme",
        "caldav",
        Some("main"),
        &patch(&[
            ("label", Some("main")),
            ("instanceUrl", Some("https://dav.example.com")),
            ("authMode", Some("basic")),
            ("username", Some("alice")),
            ("password", Some("hunter2")),
            ("calendarHome", Some("/calendars/alice/")),
        ]),
        &caldav_variants(),
    )
    .unwrap();

    // The second write is deliberately PARTIAL -- one field, the shape a form PUT actually
    // takes. A complete patch would make this test pass whether or not the merge carries
    // anything forward; a partial one only reads back through the typed getter if it does,
    // because `get_caldav_connector` marks `instanceUrl`/`authMode`/`username`/`password`
    // required and puts them in the main graph pattern.
    cg.set_declared_connector(
        "acme",
        "caldav",
        Some("main"),
        &patch(&[("calendarHome", Some("/calendars/alice/"))]),
        &caldav_variants(),
    )
    .unwrap();

    let typed = cg
        .get_caldav_connector("acme", "main")
        .expect("the typed read succeeds")
        .expect(
            "the typed getter found no caldav connector where the generic writer just wrote \
             one -- the typed getter marks most fields required and puts them in the main \
             graph pattern, so one predicate the generic write did not produce means no row \
             at all",
        );
    assert_eq!(typed.label, "main");
    assert_eq!(typed.url, "https://dav.example.com");
    assert_eq!(typed.calendar_home.as_deref(), Some("/calendars/alice/"));
    assert!(
        matches!(
            &typed.auth,
            CaldavConnectorAuth::Basic { username, password }
                if username == "alice" && password == "hunter2"
        ),
        "the generic write did not reconstruct into the typed auth union: {:?}",
        typed.auth
    );
}

/// The divergence between the two readers, pinned as a **known and deliberate** asymmetry
/// rather than left to be discovered.
///
/// `get_declared_connector` marks every field `required: false` by design, so a
/// partially-populated connector still reads back. The typed getters do not:
/// `get_caldav_connector` marks `instanceUrl` required, and `fetch_connector` puts required
/// predicates in the **main** graph pattern, so one missing predicate means no row at all.
/// Clearing a field the typed getter requires therefore makes the connector vanish from the
/// typed reader while the generic reader still returns it.
///
/// This is reachable only with no validator wired (SHACL's own `sh:minCount 1` stops it
/// otherwise) — which is `ConfigGraph::new`'s default. Recording it here means a future change
/// to either reader's requiredness policy is a visible, reviewed event; leaving it unpinned is
/// how the two readers silently drift apart with only the lenient one under test.
#[test]
fn clearing_a_typed_getters_required_field_hides_the_connector_from_it_but_not_from_the_generic_reader()
 {
    let (_dir, store) = store();
    add_company(&store, "acme");
    let ttl = all_declarations_ttl();
    let validator = validator_for(&ttl);
    let cg = ConfigGraph::new(&store, validator.declarations());

    cg.set_declared_connector(
        "acme",
        "caldav",
        Some("main"),
        &patch(&[
            ("label", Some("main")),
            ("instanceUrl", Some("https://dav.example.com")),
            ("authMode", Some("basic")),
            ("username", Some("alice")),
            ("password", Some("hunter2")),
        ]),
        &caldav_variants(),
    )
    .unwrap();
    assert!(cg.get_caldav_connector("acme", "main").unwrap().is_some());

    cg.set_declared_connector(
        "acme",
        "caldav",
        Some("main"),
        &patch(&[("instanceUrl", None)]),
        &caldav_variants(),
    )
    .expect("with no validator wired, clearing a required field is not rejected");

    assert!(
        cg.get_caldav_connector("acme", "main").unwrap().is_none(),
        "the typed getter marks `instanceUrl` required and puts it in the main graph pattern, \
         so clearing it must make the connector unmatched there. If this now returns \
         `Some(_)`, the typed getter's requiredness changed and W7's route contract changed \
         with it."
    );
    let generic = cg
        .get_declared_connector("acme", "caldav", Some("main"))
        .unwrap()
        .expect(
            "the generic reader marks every field optional by design, so it must STILL return \
             the connector -- otherwise a user who cleared one field can no longer see, or \
             repair, any of the others through the same form",
        );
    assert_eq!(
        generic.get("password").map(String::as_str),
        Some("hunter2"),
        "and the remaining fields must still be readable: {generic:?}"
    );
    assert!(!generic.contains_key("instanceUrl"));
}

/// And the reverse: what a typed setter writes, the generic reader reads back — same subject,
/// same declared vocabulary, no per-kind match arm on either side.
#[test]
fn a_typed_write_is_readable_by_the_generic_getter() {
    let (_dir, store) = store();
    add_company(&store, "acme");
    let ttl = all_declarations_ttl();
    let validator = validator_for(&ttl);
    let cg = ConfigGraph::with_validator(&store, validator.declarations(), &validator);

    cg.set_caldav_connector(
        "acme",
        &CaldavConnectorConfig {
            label: "main".to_string(),
            url: "https://dav.example.com".to_string(),
            auth: CaldavConnectorAuth::Basic {
                username: "alice".to_string(),
                password: "hunter2".to_string(),
            },
            calendar_home: None,
            customer: None,
        },
    )
    .unwrap();

    let read = cg
        .get_declared_connector("acme", "caldav", Some("main"))
        .unwrap()
        .expect("the generic reader must find what the typed setter wrote");
    assert_eq!(
        read,
        BTreeMap::from([
            ("label".to_string(), "main".to_string()),
            (
                "instanceUrl".to_string(),
                "https://dav.example.com".to_string()
            ),
            ("authMode".to_string(), "basic".to_string()),
            ("username".to_string(), "alice".to_string()),
            ("password".to_string(), "hunter2".to_string()),
        ]),
        "the generic reader must report exactly the fields the typed setter stored -- no more \
         (the two `None` optionals must not appear) and no fewer"
    );
}
