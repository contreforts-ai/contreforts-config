//! Configuration graph: persists Company entities and their connector settings
//! in a dedicated named graph (`cfdata:graph/config`) inside `contreforts-config`'s own
//! Oxigraph store.
//!
//! Ported from `contreforts-kg/src/config_graph.rs` (contreforts/contreforts-workspace#58,
//! comment 7904, item D3c): the 11 `*ConnectorConfig` structs and enums, the
//! `ConnectorDescriptor` table and generic engine (`write_connector` and the generic get/list
//! machinery), and the thin per-kind `set_*`/`get_*`/`list_*` wrappers, running against
//! [`crate::ConfigStore`] instead of `contreforts_kg::GraphStore`. `contreforts-kg::config_graph`
//! becomes a re-export shim so its four `ConfigGraph` consumers keep compiling; D8 removes that
//! shim once those consumers resolve their instance from this crate directly.
//!
//! RDF vocabulary used
//! -------------------
//! Company-level (never migrated, see `ConnectorNamespace` below): `core:Company`,
//! `core:slug`, `core:name`, `core:hasConnector`.
//!
//! Every connector's own class and field predicates: minted from that connector's
//! declaration in the injected product graph when one exists (contreforts-kg#21) -- e.g.
//! `forgejo:ForgejoConnector`, `forgejo:label`, `forgejo:instanceUrl`, `forgejo:token` -- or
//! `core:` + the same short names for the ten kinds with no declaration yet
//! (`core:ErpNextConnector`, `core:PennylaneConnector`, `core:GitlabConnector`, ...). See
//! `ConnectorNamespace` for exactly how that choice is made and
//! `contreforts_declaration::connector_validation`'s module docs for the case-1/case-2 policy it
//! implements.
//!
//! IRI patterns
//! ------------
//! Company             : `cfdata:company/{slug}`
//! ERPNext connector   : `cfdata:connector/erpnext/{slug}`
//! Pennylane connector : `cfdata:connector/pennylane/{slug}`
//! Forgejo connector   : `cfdata:connector/forgejo/{slug}/{label}`
//! GitLab connector    : `cfdata:connector/gitlab/{slug}/{label}`
//! Group mapping       : `cfdata:mapping/{type}/{slug}/{label}/{group_path}`
//!
//! These *data* IRIs (built by `namespaces::connector_iri`) are untouched by
//! contreforts-kg#21 -- only the *class and predicate* IRIs used at each of those subjects
//! move.

use std::collections::BTreeMap;

use oxigraph::model::*;
use serde::{Deserialize, Serialize};

use contreforts_core::namespaces::{self, CONFIG_GRAPH, CORE_NS, RDF};
use contreforts_declaration::{ConnectorDeclarations, ConnectorIris, ConnectorValidator};

use crate::ConfigStore;
use crate::error::{ConfigGraphError, Result};

// ── Public config types ───────────────────────────────────────────────────────

/// Top-level company entity that groups connectors together.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompanyConfig {
    /// URL-safe identifier used in IRIs (e.g. `"acme"`).
    pub slug: String,
    /// Human-readable display name.
    pub name: String,
}

/// Configuration for one ERPNext instance/company pair.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErpNextConnectorConfig {
    /// The ERPNext *company name* (as stored in ERPNext itself).
    pub company_name: String,
    /// Base URL of the ERPNext instance (e.g. `"https://acme.erpnext.com"`).
    pub url: String,
    pub api_key: String,
    pub api_secret: String,
}

/// Configuration for a Pennylane connection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PennylaneConnectorConfig {
    pub token: String,
    /// Override the default Pennylane API base URL (optional).
    pub base_url: Option<String>,
}

/// Configuration for a Forgejo connection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForgejoConnectorConfig {
    /// Short label identifying this connector instance (e.g. `"main"`, `"staging"`).
    pub label: String,
    /// Base URL of the Forgejo instance (e.g. `"https://git.example.com"`).
    pub url: String,
    /// Personal access token, sent as `Authorization: token <token>`.
    pub token: String,
}

/// Configuration for a GitLab connection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitlabConnectorConfig {
    /// Short label identifying this connector instance (e.g. `"eona"`, `"gitlab-com"`).
    pub label: String,
    /// Base URL of the GitLab instance (e.g. `"https://gitlab.example.com"`).
    pub url: String,
    /// Personal access token, sent as `PRIVATE-TOKEN` header.
    pub token: String,
}

/// Authentication mode for an O365 connector stored in the config graph.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum O365ConnectorAuth {
    ClientCredentials {
        tenant_id: String,
        client_id: String,
        client_secret: String,
    },
    Delegated {
        access_token: String,
        refresh_token: Option<String>,
    },
}

/// Configuration for a Microsoft 365 calendar connection (multi-instance, label-scoped).
/// Can be linked to a company or a customer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct O365ConnectorConfig {
    pub label: String,
    pub auth: O365ConnectorAuth,
    /// User principal name for app-level access (e.g. `"user@contoso.com"`).
    pub user_principal: Option<String>,
    /// Optional customer slug this connector is scoped to.
    pub customer: Option<String>,
}

/// Authentication mode for a CalDAV connector stored in the config graph.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CaldavConnectorAuth {
    Basic { username: String, password: String },
    Bearer { token: String },
}

/// Configuration for a CalDAV calendar connection (multi-instance, label-scoped).
/// Can be linked to a company or a customer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaldavConnectorConfig {
    pub label: String,
    /// CalDAV server base URL.
    pub url: String,
    pub auth: CaldavConnectorAuth,
    /// Optional explicit calendar home path.
    pub calendar_home: Option<String>,
    /// Optional customer slug this connector is scoped to.
    pub customer: Option<String>,
}

/// Configuration for a Matrix agent connector (multi-instance, label-scoped).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MatrixConnectorConfig {
    /// Short label identifying this connector instance (e.g. `"main"`, `"support"`).
    pub label: String,
    /// Matrix homeserver URL (e.g. `"https://matrix.example.com"`).
    pub homeserver_url: String,
    /// Access token for the bot user.
    pub access_token: String,
    /// Device ID for E2EE (optional).
    pub device_id: Option<String>,
    /// Fully-qualified Matrix user ID of the bot (e.g. `"@bot:example.com"`).
    pub user_id: Option<String>,
}

/// TLS mode for an SMTP connection.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SmtpTlsMode {
    None,
    Starttls,
    Tls,
}

/// Configuration for an SMTP agent connector (multi-instance, label-scoped).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SmtpConnectorConfig {
    /// Short label identifying this connector instance (e.g. `"main"`, `"noreply"`).
    pub label: String,
    /// SMTP server hostname or IP.
    pub host: String,
    /// SMTP port (typically 25, 465, or 587).
    pub port: u16,
    /// SMTP login username (optional for unauthenticated relay).
    pub username: Option<String>,
    /// SMTP password.
    pub password: Option<String>,
    /// Envelope / header From address (e.g. `"agent@example.com"`).
    pub from_address: String,
    /// TLS mode: `none`, `starttls`, or `tls`.
    pub tls: SmtpTlsMode,
}

/// Configuration for the Stalwart sidecar pair (`itip-handler` + `summary-handler`),
/// multi-instance, label-scoped, optionally narrowed to one customer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StalwartConnectorConfig {
    pub label: String,
    /// JMAP base URL, e.g. `"https://mail.example.com"`.
    pub jmap_base_url: String,
    pub admin_user: String,
    pub admin_pass: String,
    /// HTTP listen port of the sidecars.
    pub listen_port: u16,
    /// Local state dir for the iTIP scheduler.
    pub state_dir: String,
    /// SQLite path for the summary store.
    pub db_path: String,
    /// MTA used for internal deliveries (host + port).
    pub smtp_local_host: String,
    pub smtp_local_port: u16,
    /// Optional Internet relay MTA (host + port).
    pub smtp_relay_host: Option<String>,
    pub smtp_relay_port: u16,
    /// Domain used as the iMIP anchor (From: domain).
    pub imip_anchor_domain: String,
    pub ollama_url: String,
    pub ollama_model: String,
    pub ollama_timeout_secs: u64,
    /// Optional customer slug this connector is scoped to.
    pub customer: Option<String>,
}

/// Configuration for the Visio generator service (Element Call + Kutt),
/// multi-instance, label-scoped, optionally narrowed to one customer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VisioConnectorConfig {
    pub label: String,
    pub listen_port: u16,
    pub kutt_base_url: String,
    pub kutt_api_key: String,
    pub service_api_key: String,
    pub customer: Option<String>,
}

/// Backend kind for a vector store connector.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum VectorStoreKind {
    Pgvector,
    InMemory,
}

/// The pgvector column type an embedding is stored in — the other half of a
/// geometry, alongside `dimension` (contreforts-kg#8).
///
/// A dimension alone does not describe a geometry: 2048 cannot be indexed as
/// `vector`, and the same number implies different storage, index types and
/// accuracy depending on this field. The limits below are the ones
/// `contreforts-vecdb#6` **measured** against pgvector 0.8.5 on PostgreSQL 16,
/// not values read off a changelog:
///
/// | variant | HNSW builds up to |
/// |---|---|
/// | `Vector` | 2 000 dims |
/// | `Halfvec` | 4 000 dims |
/// | `Bit` | 4 096 dims — measured working, no limit reached |
///
/// Note `vector(1024)` and `halfvec(2048)` are the same 4 096 bytes per row and
/// the same index size: that pair is f32 across half the axes against f16 across
/// all of them, not a size/quality trade-off, and only a measurement separates
/// them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum VectorStoreColumnType {
    /// f32 per axis. The default, because a configuration graph written before
    /// this field existed describes a `vector(1536)` table — see
    /// `VectorStoreConnectorConfig::column_type`.
    #[default]
    Vector,
    /// f16 per axis: half the bytes, twice the indexable axes.
    Halfvec,
    /// One bit per axis. Cheapest to index and the only variant that reaches
    /// 4 096, at the cost of needing exact rescoring in the retrieval path.
    Bit,
}

impl VectorStoreColumnType {
    /// The SQL type name, as it appears in DDL and in `information_schema`.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Vector => "vector",
            Self::Halfvec => "halfvec",
            Self::Bit => "bit",
        }
    }

    /// Largest dimension this column type can carry in an HNSW index, measured
    /// per the table on the type's own documentation.
    pub fn max_indexable_dimension(&self) -> u32 {
        match self {
            Self::Vector => 2_000,
            Self::Halfvec => 4_000,
            Self::Bit => 4_096,
        }
    }
}

fn parse_vector_store_column_type(raw: Option<&str>) -> VectorStoreColumnType {
    match raw {
        Some("halfvec") => VectorStoreColumnType::Halfvec,
        Some("bit") => VectorStoreColumnType::Bit,
        // Anything else — including absent — is `vector`. Deliberately
        // reproducing today's *behaviour* rather than today's intent
        // (contreforts-kg#8): a graph written before this field existed
        // describes a `vector(1536)` table, whatever it was meant to describe.
        _ => VectorStoreColumnType::Vector,
    }
}

/// Configuration for a vector store connector (multi-instance, label-scoped).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VectorStoreConnectorConfig {
    pub label: String,
    pub kind: VectorStoreKind,
    /// PostgreSQL URL (required when kind=Pgvector, ignored otherwise).
    pub url: Option<String>,
    /// Optional table name (Pgvector only; default "rag_chunks").
    pub table: Option<String>,
    /// Embedding dimension. Load-bearing since contreforts-kg#8: validated
    /// against `column_type` when this connector is written, so an unindexable
    /// pair is refused at save time rather than surfacing months later as "the
    /// assistant got slow" when a query silently became a sequential scan.
    pub dimension: u32,
    /// The pgvector column type holding the embedding (contreforts-kg#8).
    ///
    /// `#[serde(default)]` is `Vector`, which reproduces today's behaviour for
    /// any configuration graph written before this field existed — such a graph
    /// describes a `vector(1536)` table. That is deliberately today's behaviour
    /// and not today's *intent*: a store meant to be `halfvec` was still
    /// created as `vector`, and guessing otherwise would silently reinterpret
    /// stored data.
    #[serde(default)]
    pub column_type: VectorStoreColumnType,
    /// PostgreSQL URL with **DDL rights**, used only to provision the backing
    /// table (contreforts/contreforts-workspace#4). Distinct from `url`, which
    /// runs as a restricted role by design (contreforts-vecdb#13): if schema
    /// creation used the query credentials, the read-only model would be
    /// decorative.
    ///
    /// **Secret, and write-only.** `get_vector_store_connector` and
    /// `list_vector_store_connectors` deliberately do **not** read it back —
    /// absent from responses rather than redacted in them. See
    /// `admin_url_is_write_only` for the test that pins it.
    ///
    /// `None` means this store cannot be provisioned through the API, which is
    /// a reportable state rather than an error. The accepted trade recorded on
    /// contreforts/contreforts-workspace#4: storing a DDL credential here makes
    /// config-graph write access a path to arbitrary DDL, taken deliberately
    /// over having an operator run emitted SQL by hand.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub admin_url: Option<String>,
}

/// Refuse a (column type, dimension) pair no HNSW index can be built on.
///
/// The failure being prevented is not a wrong answer — it is a query that
/// silently becomes a sequential scan, noticed months later as "the assistant
/// got slow" (contreforts-vecdb#6, contreforts-kg#8). Checked when the connector
/// is *saved*, because that is the moment an operator is present to be told.
///
/// `InMemory` stores have no pgvector column, so the pair is not theirs to
/// satisfy and is not checked.
fn validate_vector_store_geometry(config: &VectorStoreConnectorConfig) -> Result<()> {
    if config.kind != VectorStoreKind::Pgvector {
        return Ok(());
    }
    if config.dimension == 0 {
        return Err(ConfigGraphError::ConnectorValidation(format!(
            "vector store '{}': dimension must be greater than 0",
            config.label
        )));
    }
    let max = config.column_type.max_indexable_dimension();
    if config.dimension > max {
        return Err(ConfigGraphError::ConnectorValidation(format!(
            "vector store '{}': {}({}) cannot be indexed -- HNSW builds up to {} dimensions for \
             '{}' (measured against pgvector 0.8.5 on PostgreSQL 16). Use a column type that \
             reaches this dimension, or reduce the dimension; leaving it would not give wrong \
             answers, it would silently make every query a sequential scan",
            config.label,
            config.column_type.as_str(),
            config.dimension,
            max,
            config.column_type.as_str(),
        )));
    }
    Ok(())
}

/// Binding of a named graph IRI to a vector store, identified by label within a company.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowledgeBaseConfig {
    pub label: String,
    /// Which [`KgInstanceConfig`] this KB's data belongs to, by **label** -- the association D5's
    /// guard needs before "points into another instance's data" is a checkable property
    /// (contreforts-workspace#58, comment 7969). `Option`, not required: every `KnowledgeBaseConfig`
    /// stored before this field existed has no association either, and this type is deserialized
    /// straight off JSON request bodies elsewhere in the workspace, so a required field would
    /// silently break that contract too.
    ///
    /// Resolution is explicit, never a silent pick, applied by [`ConfigGraph::set_knowledge_base`]:
    /// - `Some(label)` must name an already-registered [`KgInstanceConfig`], or the write is
    ///   rejected -- there is nothing for the prefix guard to check a dangling reference against.
    /// - `None` resolves to the sole registered instance when **exactly one** exists (every
    ///   deployment that has adopted per-instance identity so far).
    /// - `None` with **more than one** registered instance is a named error (see
    ///   [`crate::error::ConfigGraphError::kg_instance_ambiguous`]), not a guess -- silently
    ///   picking one would reintroduce exactly the "absence presenting as success" failure this
    ///   epic keeps paying for.
    /// - `None` with **zero** registered instances is accepted as "no association yet": nothing is
    ///   registered for this KB to belong to, so the prefix guard has nothing to check and simply
    ///   does not apply, preserving every caller that predates KG instances entirely (e.g.
    ///   `contreforts-config-api`'s knowledge-base routes, which have never registered one).
    ///
    /// `get_knowledge_base`/`list_knowledge_bases` return the *resolved* label that was actually
    /// stored, never the `None` that may have been passed in to get there.
    pub kg_instance_label: Option<String>,
    /// Named graph IRI in Oxigraph (e.g. "http://example.org/code-graph").
    /// `None` means "default graph".
    pub graph: Option<String>,
    /// Label of the VectorStore connector to write into.
    pub vector_store_label: String,
}

/// Identity of one `contreforts-kg` data instance (contreforts/contreforts-workspace#58 D4;
/// #18 Q2): a **label** plus an **independently-assigned IRI prefix** -- the prefix is never
/// derived from the label, so renaming an instance (`ConfigGraph::rename_kg_instance`) never
/// rewrites a single subject IRI that instance's entity data was built from. Global, not
/// company-scoped (ruling 2 on contreforts-workspace#58): one instance holds many companies'
/// data, so scoping it per-company would invert that containment.
///
/// Both `label` and `iri_prefix` are enforced unique across all registered instances by
/// `ConfigGraph::set_kg_instance` (ruling 1 on contreforts-workspace#58) -- a shared label
/// would make Q5's "resolve by label, with a default" ambiguous, and a shared prefix would
/// silently merge two instances' entity data into one IRI space.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct KgInstanceConfig {
    pub label: String,
    pub iri_prefix: String,
}

/// Reference to one of the existing channel connectors.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChannelRef {
    /// "matrix" | "smtp" | "o365" | "caldav"
    pub kind: String,
    pub label: String,
}

/// Configuration for an Agent that binds a KnowledgeBase to one or more channels.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    pub label: String,
    pub display_name: Option<String>,
    /// Label of the KnowledgeBase this agent uses.
    pub knowledge_base_label: String,
    pub channels: Vec<ChannelRef>,
}

/// A SPARQL template with placeholders for semantic RAG.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SparqlTemplateConfig {
    pub label: String,
    /// Human-readable description for the LLM.
    pub description: String,
    /// SPARQL pattern with {{placeholders}}.
    pub pattern: String,
}

/// Typed union of all supported connector configurations.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ConnectorConfig {
    #[serde(rename = "erpnext")]
    ErpNext(ErpNextConnectorConfig),
    Pennylane(PennylaneConnectorConfig),
    Forgejo(ForgejoConnectorConfig),
    Gitlab(GitlabConnectorConfig),
    O365(O365ConnectorConfig),
    Caldav(CaldavConnectorConfig),
    Matrix(MatrixConnectorConfig),
    Smtp(SmtpConnectorConfig),
    VectorStore(VectorStoreConnectorConfig),
    Stalwart(StalwartConnectorConfig),
    Visio(VisioConnectorConfig),
}

impl ConnectorConfig {
    pub fn connector_type(&self) -> &str {
        match self {
            Self::ErpNext(_) => "erpnext",
            Self::Pennylane(_) => "pennylane",
            Self::Forgejo(_) => "forgejo",
            Self::Gitlab(_) => "gitlab",
            Self::O365(_) => "o365",
            Self::Caldav(_) => "caldav",
            Self::Matrix(_) => "matrix",
            Self::Smtp(_) => "smtp",
            Self::VectorStore(_) => "vector_store",
            Self::Stalwart(_) => "stalwart",
            Self::Visio(_) => "visio",
        }
    }

    /// For multi-instance connectors, returns the label.
    pub fn label(&self) -> Option<&str> {
        match self {
            Self::Forgejo(c) => Some(&c.label),
            Self::Gitlab(c) => Some(&c.label),
            Self::O365(c) => Some(&c.label),
            Self::Caldav(c) => Some(&c.label),
            Self::Matrix(c) => Some(&c.label),
            Self::Smtp(c) => Some(&c.label),
            Self::VectorStore(c) => Some(&c.label),
            Self::Stalwart(c) => Some(&c.label),
            Self::Visio(c) => Some(&c.label),
            _ => None,
        }
    }

    /// Optional customer slug this connector is scoped to.
    pub fn customer(&self) -> Option<&str> {
        match self {
            Self::O365(c) => c.customer.as_deref(),
            Self::Caldav(c) => c.customer.as_deref(),
            Self::Stalwart(c) => c.customer.as_deref(),
            Self::Visio(c) => c.customer.as_deref(),
            _ => None,
        }
    }

    /// Logical category for display grouping.
    pub fn category(&self) -> &str {
        match self {
            Self::ErpNext(_) => "erp",
            Self::Pennylane(_) => "accounting",
            Self::Forgejo(_) | Self::Gitlab(_) => "git-forge",
            Self::O365(_) | Self::Caldav(_) => "calendar",
            Self::Matrix(_) | Self::Smtp(_) => "agent",
            Self::VectorStore(_) => "vector",
            Self::Stalwart(_) => "sidecar",
            Self::Visio(_) => "sidecar",
        }
    }
}

// ── ConnectorDescriptor ───────────────────────────────────────────────────────
//
// Every connector kind writes/reads through the same three shapes:
//   set:  require_company → remove_subject_from_named_graph → write_type → N× write_literal
//         → link `core:hasConnector`.
//   get:  one SPARQL query per connector node — required fields as a plain graph pattern,
//         optional fields each in their own `OPTIONAL { }` — read back into a `BTreeMap`.
//   list: enumerate the labels of a company's connectors of one type, then `get` each.
//
// `ConnectorDescriptor` names the two facts that differ per kind and never change once
// chosen: the `kind` string threaded through `namespaces::connector_iri`, and the short
// class name under `CORE_NS`. Per-call field lists (which predicates, which values, which
// are required) stay at the call site in each thin `set_*`/`get_*`/`list_*` wrapper, because
// that is exactly where the per-kind Rust struct shape — including the two auth enums that
// pick a different predicate set per variant — already lives; duplicating it into a second,
// parallel static table would not remove any of the duplication this issue exists to remove.

/// Static shape of one connector kind, shared by the write and read paths so the class name
/// and IRI `kind` cannot drift between `set_*`, `get_*` and `list_*` for the same connector.
struct ConnectorDescriptor {
    /// Connector kind as used in `namespaces::connector_iri` and IRIs (e.g. `"forgejo"`).
    kind: &'static str,
    /// Short class name under `CORE_NS` (e.g. `"ForgejoConnector"`).
    type_name: &'static str,
    /// `true` for the two-segment (no-label) IRI shape; `false` for the three-segment,
    /// label-scoped shape. Must match `namespaces::connector_iri`'s own asymmetry exactly —
    /// giving a singleton a label, or vice versa, changes a stored IRI.
    singleton: bool,
}

const ERPNEXT: ConnectorDescriptor = ConnectorDescriptor {
    kind: "erpnext",
    type_name: "ErpNextConnector",
    singleton: true,
};
const PENNYLANE: ConnectorDescriptor = ConnectorDescriptor {
    kind: "pennylane",
    type_name: "PennylaneConnector",
    singleton: true,
};
const FORGEJO: ConnectorDescriptor = ConnectorDescriptor {
    kind: "forgejo",
    type_name: "ForgejoConnector",
    singleton: false,
};
const GITLAB: ConnectorDescriptor = ConnectorDescriptor {
    kind: "gitlab",
    type_name: "GitlabConnector",
    singleton: false,
};
const O365: ConnectorDescriptor = ConnectorDescriptor {
    kind: "o365",
    type_name: "O365Connector",
    singleton: false,
};
const CALDAV: ConnectorDescriptor = ConnectorDescriptor {
    kind: "caldav",
    type_name: "CaldavConnector",
    singleton: false,
};
const MATRIX: ConnectorDescriptor = ConnectorDescriptor {
    kind: "matrix",
    type_name: "MatrixConnector",
    singleton: false,
};
const SMTP: ConnectorDescriptor = ConnectorDescriptor {
    kind: "smtp",
    type_name: "SmtpConnector",
    singleton: false,
};
const VECTOR_STORE: ConnectorDescriptor = ConnectorDescriptor {
    kind: "vector_store",
    type_name: "VectorStoreConnector",
    singleton: false,
};
const STALWART: ConnectorDescriptor = ConnectorDescriptor {
    kind: "stalwart",
    type_name: "StalwartConnector",
    singleton: false,
};
const VISIO: ConnectorDescriptor = ConnectorDescriptor {
    kind: "visio",
    type_name: "VisioConnector",
    singleton: false,
};

/// All eleven connector kinds, used by `remove_connector` to resolve a `connector_type`
/// string to its descriptor without a per-kind match arm.
const ALL_CONNECTOR_DESCRIPTORS: &[ConnectorDescriptor] = &[
    ERPNEXT,
    PENNYLANE,
    FORGEJO,
    GITLAB,
    O365,
    CALDAV,
    MATRIX,
    SMTP,
    VECTOR_STORE,
    STALWART,
    VISIO,
];

/// Every connector kind's `(kind, type_name)` pair, read off `ALL_CONNECTOR_DESCRIPTORS` --
/// the same single source of truth `write_connector` and `remove_connector` already use --
/// rather than a second, parallel list. `ConnectorValidator::new` (contreforts-kg#19) takes
/// this to compute which kinds have no declared shape, independent of any write ever
/// happening (e.g. at process startup, before the first request).
pub fn all_connector_kinds() -> Vec<(&'static str, &'static str)> {
    ALL_CONNECTOR_DESCRIPTORS
        .iter()
        .map(|d| (d.kind, d.type_name))
        .collect()
}

// ── ConnectorNamespace ────────────────────────────────────────────────────────
//
// contreforts-kg#21: which namespace one connector kind's class and field predicates live in.
// `ConnectorDeclarations::connector_iris` (case 1/case 2, see `connector_validation`'s module
// docs) is the only place this is decided; `write_connector` and every `get_*`/`list_*` read
// helper resolve it through the same `ConfigGraph::connector_namespace` call, so a connector's
// stored IRIs and the IRIs a read query looks for can never diverge, and a write can never mix
// the two.
//
// contreforts-kg#23: resolution reads `self.declarations` only, never `self.validator` --
// namespace and SHACL write-validation policy are independent facts on `ConfigGraph`, and a
// caller that turns validation off (or never wires a `ConnectorValidator` at all) still gets
// the same namespace a validating caller with the same declarations would.

/// One connector kind's resolved class/field-predicate source, for one `write_connector` or
/// read call.
enum ConnectorNamespace<'a> {
    /// Case 1: `descriptor.kind` has a declaration among `self.declarations`.
    Declared(&'a ConnectorIris),
    /// Case 2: no declaration -- either these are `ConnectorDeclarations::none()` (nothing was
    /// ever injected, so nothing can be known), or real declarations were supplied but have no
    /// shape for this kind. Both mean the same thing here: the honest, unmigrated `CORE_NS`
    /// state, not a fallback to hide -- see this module's top-level doc comment and
    /// `connector_validation`'s.
    Core,
}

impl<'a> ConnectorNamespace<'a> {
    /// The full `rdf:type` class IRI to write/query for `descriptor`.
    fn class_iri(&self, descriptor: &ConnectorDescriptor) -> String {
        match self {
            Self::Declared(iris) => iris.class_iri.clone(),
            Self::Core => format!("{CORE_NS}{}", descriptor.type_name),
        }
    }

    /// The full predicate IRI for one short field name (e.g. `"instanceUrl"`).
    ///
    /// `Err` only in case 1 when the declaration covers this connector's class but has no
    /// `sh:path` for this particular field: falling back to `CORE_NS` for just that one field
    /// would silently produce exactly the mixed-namespace state contreforts-kg#21 forbids (a
    /// connector whose class is `forgejo:ForgejoConnector` but some field is `core:something`),
    /// so an incomplete declaration must fail loudly instead. See
    /// `declaration_missing_one_fields_sh_path_refuses_the_write_instead_of_mixing_namespaces`
    /// for the regression test covering exactly this case (a declared class whose declaration
    /// omits one field's `sh:path`).
    fn field_iri(&self, predicate: &str) -> Result<String> {
        match self {
            Self::Declared(iris) => iris.field_iris.get(predicate).cloned().ok_or_else(|| {
                ConfigGraphError::ConnectorValidation(format!(
                    "connector's declaration targets a class but has no sh:path for field \
                     '{predicate}' -- refusing to mix its namespace with core: (contreforts-kg#21)"
                ))
            }),
            Self::Core => Ok(format!("{CORE_NS}{predicate}")),
        }
    }

    /// The full `sh:datatype` IRI declared for one short field name, or `None` when there is
    /// none to use -- either this is `Core` (case 2: `write_connector` keeps writing a plain
    /// literal, unchanged, contreforts-kg#25), or it is `Declared` but that particular field's
    /// property shape has no `sh:datatype` constraint (also a plain literal, per the "declared
    /// field with no `sh:datatype` is stored as a plain literal, unchanged" requirement).
    /// Never an error: an absent datatype is a legitimate declaration, not a defect the way an
    /// absent `sh:path` is in `field_iri` above.
    fn field_datatype(&self, predicate: &str) -> Option<&'a str> {
        match self {
            Self::Declared(iris) => iris.field_datatypes.get(predicate).map(String::as_str),
            Self::Core => None,
        }
    }
}

/// Parses one field's lexical value (straight off `fetch_connector`, which already reduces
/// every literal to `Literal::value()` regardless of datatype) as `T`, the read-side half of
/// contreforts-kg#25: "for declared kinds, a stored literal that does not match its declared
/// datatype must be an error, not a default." `ns` is the connector's resolved namespace, the
/// same one `fetch_connector` used to build the query this value came from -- reusing it
/// (rather than re-deriving "declared or not" here) means read and write can never disagree
/// about which kinds are case 1 vs. case 2.
///
/// Case 2 (`ns` is `Core`, i.e. an undeclared kind) keeps today's exact behaviour: a bad or
/// missing value silently becomes `default`, the pre-existing `.parse().ok().unwrap_or(_)`
/// this replaces. Case 1 (`ns` is `Declared`) turns that same parse failure into `Err` instead
/// -- see `declared_kind_read_errors_on_a_mismatched_literal_instead_of_silently_defaulting`
/// for the regression test that exercises this with a synthetic `xsd:integer` declaration (no
/// real declaration has a numeric field yet).
///
/// Only called for fields that are `required: true` in their `fetch_connector` call, so `raw`
/// being `None` here would mean `fetch_connector` matched a row without the predicate its own
/// query required -- structurally unreachable, but `default` is still the safe answer rather
/// than a panic.
fn parse_declared_field<T: std::str::FromStr>(
    ns: &ConnectorNamespace,
    field: &str,
    raw: Option<&String>,
    default: T,
) -> Result<T> {
    let Some(raw) = raw else {
        return Ok(default);
    };
    match raw.parse::<T>() {
        Ok(v) => Ok(v),
        Err(_) => match ns {
            ConnectorNamespace::Declared(_) => {
                Err(ConfigGraphError::DeclaredFieldMismatch(format!(
                    "connector field '{field}' has value {raw:?} that does not match its \
                     declared datatype (contreforts-kg#25)"
                )))
            }
            ConnectorNamespace::Core => Ok(default),
        },
    }
}

// ── Store helpers ─────────────────────────────────────────────────────────────
//
// `contreforts-config`'s measured surface (contreforts-workspace#58, comment 7904) adds exactly
// three primitives to `ConfigStore`: `select`, `inner`, `remove_quad`. Every other write this
// engine performed through `contreforts_kg::GraphStore`'s convenience helpers
// (`insert_in_named_graph`, `remove_subject_from_named_graph`) is reimplemented here, in terms
// of those three primitives only, rather than growing `ConfigStore`'s own public surface with
// methods nothing outside this module needs (ruling 4: no `ConfigStore::in_memory()`, and by the
// same logic, no speculative `ConfigStore` convenience methods either).

fn insert_in_named_graph(
    store: &ConfigStore,
    subject: &NamedNode,
    predicate: &NamedNode,
    object: &Term,
    graph: &NamedNode,
) -> Result<()> {
    store.inner().insert(&Quad::new(
        subject.clone(),
        predicate.clone(),
        object.clone(),
        GraphName::NamedNode(graph.clone()),
    ))?;
    Ok(())
}

/// IRI for a KG instance's own definition record (contreforts-workspace#58 D4), scoped by its
/// **label**. A plain, hand-entered config-graph IRI -- rooted at the fixed `DATA_NS`, like
/// `namespaces::company_iri` and friends -- never one of the entity/instance-data IRIs an
/// instance's own assigned prefix re-derives (comment 7936's table). Not exported: nothing
/// outside this module needs a KG instance's own subject IRI, only the `iri_prefix` field its
/// record carries (consumed by `contreforts_kg::namespaces::InstanceNamespace`).
fn kg_instance_iri(label: &str) -> String {
    format!(
        "{}kg-instance/{}",
        namespaces::DATA_NS,
        urlencoding::encode(label)
    )
}

fn remove_subject_from_named_graph(
    store: &ConfigStore,
    subject: &NamedNode,
    graph: &NamedNode,
) -> Result<()> {
    let quads: Vec<_> = store
        .inner()
        .quads_for_pattern(Some(subject.into()), None, None, Some(graph.into()))
        .collect::<std::result::Result<Vec<_>, _>>()?;
    for quad in quads {
        store.inner().remove(&quad)?;
    }
    Ok(())
}

// ── ConfigGraph ───────────────────────────────────────────────────────────────

/// CRUD interface for the configuration named graph.
pub struct ConfigGraph<'a> {
    store: &'a ConfigStore,
    graph: NamedNode,
    /// contreforts-kg#23: which connector kinds' namespaces are in force -- the *only* input
    /// `connector_namespace` reads. Required by every constructor, so it is a compile error to
    /// build a `ConfigGraph` without stating this; pass `ConnectorDeclarations::none()` to
    /// state "none in force" explicitly. Independent of `validator` below: two `ConfigGraph`s
    /// built with the same `declarations` resolve every connector's namespace identically
    /// whether or not either one also validates writes.
    declarations: ConnectorDeclarations<'a>,
    /// contreforts-kg#19: when present, every connector write additionally passes through it
    /// -- pure write-validation *policy*, no longer entangled with namespace resolution
    /// (contreforts-kg#23; see `declarations` above and this module's top-level doc comment).
    /// `None` for every call site across the workspace today (`new` below) -- wiring a real
    /// validator in is the composition root's job (e.g. `contreforts-config-api`,
    /// `dslabs/erp-sync`, each in their own repo), out of scope for this issue; see
    /// `with_validator`.
    validator: Option<&'a ConnectorValidator>,
}

impl<'a> ConfigGraph<'a> {
    /// `declarations` states, explicitly, which connector kinds have a declared namespace --
    /// there is no overload that omits it (contreforts-kg#23). No SHACL write-validation
    /// happens; see `with_validator` to add it without changing namespace resolution.
    pub fn new(store: &'a ConfigStore, declarations: ConnectorDeclarations<'a>) -> Self {
        Self {
            store,
            graph: NamedNode::new(CONFIG_GRAPH).expect("CONFIG_GRAPH is a valid IRI"),
            declarations,
            validator: None,
        }
    }

    /// Same as `new`, but every connector write additionally passes through `validator` first
    /// -- contreforts-kg#19. `validator` is built once (typically at process startup) from
    /// data handed to it by the caller (e.g. `contreforts-product::PRODUCT_GRAPH_TTL`);
    /// this crate itself never obtains that data on its own (see
    /// `contreforts_declaration::connector_validation`'s module docs for why).
    ///
    /// `declarations` and `validator` are independent parameters (contreforts-kg#23): passing
    /// `validator.declarations()` keeps namespace resolution and SHACL validation in sync with
    /// the same product graph, which is the common case, but nothing requires it -- a caller
    /// could pass a different `ConnectorDeclarations` (or `::none()`) and validation would
    /// still run against `validator`'s own shapes.
    pub fn with_validator(
        store: &'a ConfigStore,
        declarations: ConnectorDeclarations<'a>,
        validator: &'a ConnectorValidator,
    ) -> Self {
        Self {
            store,
            graph: NamedNode::new(CONFIG_GRAPH).expect("CONFIG_GRAPH is a valid IRI"),
            declarations,
            validator: Some(validator),
        }
    }

    // ── write helpers ─────────────────────────────────────────────────────────

    fn node(&self, iri: &str) -> Result<NamedNode> {
        NamedNode::new(iri).map_err(|_| ConfigGraphError::InvalidIri(iri.to_string()))
    }

    fn write_triple(&self, subject: &NamedNode, predicate_iri: &str, object: Term) -> Result<()> {
        let pred = self.node(predicate_iri)?;
        insert_in_named_graph(self.store, subject, &pred, &object, &self.graph)
    }

    /// Writes one field's literal, typed from `datatype_iri` when the declaration supplies one
    /// (contreforts-kg#25) -- a plain (untyped) literal otherwise, exactly as before this
    /// issue. `datatype_iri` is `None` for every case-2 (undeclared kind) write and for a
    /// declared field whose property shape has no `sh:datatype`; see
    /// `ConnectorNamespace::field_datatype`, the single place that decides which.
    fn write_literal(
        &self,
        subject: &NamedNode,
        predicate_iri: &str,
        value: &str,
        datatype_iri: Option<&str>,
    ) -> Result<()> {
        let literal = self.typed_literal(value, datatype_iri)?;
        self.write_triple(subject, predicate_iri, Term::Literal(literal))
    }

    /// Builds one field's literal term: `Literal::new_typed_literal(value, datatype)` when
    /// `datatype_iri` is `Some`, `Literal::new_simple_literal(value)` otherwise. Shared by
    /// `write_literal` (the actual store write) and `connector_instance_graph` (what SHACL
    /// validates) so the two can never encode a value differently -- contreforts-kg#25's
    /// "validation sees literal-for-literal what gets written" requirement, the same guarantee
    /// `write_connector`'s shared `resolved_fields` already gives the predicate IRIs and field
    /// set (contreforts-kg#19).
    fn typed_literal(&self, value: &str, datatype_iri: Option<&str>) -> Result<Literal> {
        Ok(match datatype_iri {
            Some(dt) => Literal::new_typed_literal(value, self.node(dt)?),
            None => Literal::new_simple_literal(value),
        })
    }

    fn write_type(&self, subject: &NamedNode, type_iri: &str) -> Result<()> {
        let type_node = self.node(type_iri)?;
        self.write_triple(subject, &format!("{RDF}type"), Term::NamedNode(type_node))
    }

    /// Resolve `descriptor.kind`'s `ConnectorNamespace` -- case 1 (declared) when
    /// `self.declarations` covers this kind, case 2 (`CORE_NS`) otherwise. The single call
    /// both `write_connector` and every read helper (`fetch_connector`,
    /// `list_connector_labels`) go through, so they can never resolve a connector's IRIs
    /// differently -- contreforts-kg#21. Reads `self.declarations` only, never
    /// `self.validator` -- contreforts-kg#23: whether writes are SHACL-checked must not change
    /// where a connector's triples live.
    fn connector_namespace(&self, descriptor: &ConnectorDescriptor) -> ConnectorNamespace<'a> {
        match self.declarations.connector_iris(descriptor.kind) {
            Some(iris) => ConnectorNamespace::Declared(iris),
            None => ConnectorNamespace::Core,
        }
    }

    // ── Generic connector engine ─────────────────────────────────────────────
    //
    // `set`/`get`/`list` for every connector kind go through exactly one of these three.

    /// Write (or idempotently replace) one connector's triples: the body shape every
    /// `set_*_connector` method shares. `label` selects `connector_iri`'s singleton vs.
    /// label-scoped IRI shape; `fields` is `(predicate, value)` — `None` values are skipped
    /// entirely, so an absent optional field stays absent rather than becoming an
    /// empty-string literal.
    fn write_connector(
        &self,
        company_slug: &str,
        descriptor: &ConnectorDescriptor,
        label: Option<&str>,
        fields: &[(&str, Option<&str>)],
    ) -> Result<()> {
        self.require_company(company_slug)?;

        let conn_iri = namespaces::connector_iri(descriptor.kind, company_slug, label);
        let conn_node = self.node(&conn_iri)?;
        let company_node = self.node(&namespaces::company_iri(company_slug))?;
        let ns = self.connector_namespace(descriptor);
        let type_iri = ns.class_iri(descriptor);

        // Resolved once -- `(predicate IRI, value, declared sh:datatype IRI)` for every field
        // actually present, `None`s already dropped -- and read by both the validation step
        // below and the store-write loop further down. contreforts-kg#19: "if the validated
        // instance is not identical to the written one, the check is theatre" -- sharing this
        // one list, rather than two separately-written field loops, is what rules that out
        // structurally rather than by convention. See `connector_write_validates_exactly_what_it_writes`
        // for the regression test. contreforts-kg#21: `ns.field_iri` is where each field's
        // predicate IRI is resolved -- declared (from the connector's own declaration) or
        // `CORE_NS`, uniformly for the whole connector; see `ConnectorNamespace::field_iri` for
        // why an incomplete declaration errors here rather than falling back per field.
        // contreforts-kg#25: `ns.field_datatype` resolves the same field's declared
        // `sh:datatype`, `None` when there is none to use -- see `ConnectorNamespace::field_datatype`.
        let mut resolved_fields: Vec<(String, &str, Option<&str>)> = Vec::new();
        for (predicate, value) in fields {
            if let Some(v) = value {
                let field_iri = ns.field_iri(predicate)?;
                let datatype_iri = ns.field_datatype(predicate);
                resolved_fields.push((field_iri, v, datatype_iri));
            }
        }

        // D5's second invariant (contreforts-workspace#58, comment 7969; #18 Q3): no config
        // record but a KB's own `KnowledgeBaseConfig.graph` may ever store its graph IRI
        // verbatim. Scoped to every field this generic engine writes -- all eleven connector
        // kinds funnel through here, so this is "one list, one caller" for the write path (the
        // same enumeration `validate_startup` re-checks at startup by scanning every
        // connector-typed subject) -- deliberately not a blanket scan of every stored literal in
        // the store, which would false-positive on `SparqlTemplateConfig.pattern` (free-text
        // SPARQL that may legitimately contain a graph IRI as query text, and which this engine
        // never writes: `set_sparql_template` has its own, separate write path).
        self.reject_kb_graph_reference(fields.iter().filter_map(|(_, v)| *v))?;

        if let Some(validator) = self.validator {
            let instance = self.connector_instance_graph(&conn_iri, &type_iri, &resolved_fields)?;
            if let Err(violations) = validator.validate(&type_iri, &instance) {
                let lines: Vec<String> = violations
                    .iter()
                    .enumerate()
                    .map(|(i, v)| format!("  {}. {v}", i + 1))
                    .collect();
                return Err(ConfigGraphError::ConnectorValidation(format!(
                    "connector '{}' ({} field(s)) violates its declared shape, {} violation(s):\n{}",
                    descriptor.kind,
                    resolved_fields.len(),
                    violations.len(),
                    lines.join("\n"),
                )));
            }
        }

        // Idempotent: wipe existing connector triples first. This is the guarantee that
        // calling `set_*` again replaces rather than accumulates triples for this connector.
        remove_subject_from_named_graph(self.store, &conn_node, &self.graph)?;

        self.write_type(&conn_node, &type_iri)?;
        for (predicate_iri, value, datatype_iri) in &resolved_fields {
            self.write_literal(&conn_node, predicate_iri, value, *datatype_iri)?;
        }

        self.write_triple(
            &company_node,
            &format!("{CORE_NS}hasConnector"),
            Term::NamedNode(conn_node),
        )
    }

    /// Builds the small RDF graph a `write_connector` call is about to write for one
    /// connector: `<conn_iri> a <type_iri>` plus `<conn_iri> <predicate_iri> "value"^^<datatype>`
    /// (or a plain literal when no datatype is declared) for every entry in `resolved_fields` --
    /// exactly the two write loops right below it in `write_connector` produce, from the same
    /// inputs, via the same `typed_literal` helper (contreforts-kg#25). This exists only so
    /// `ConnectorValidator::validate` (contreforts-kg#19) checks the instance that is about to
    /// be stored, not a hand-maintained approximation of it that could silently drift from
    /// what `write_type`/`write_literal` actually do -- see
    /// `connector_instance_graph_matches_typed_write` for the regression test that fails if
    /// the two ever diverge on datatypes specifically.
    fn connector_instance_graph(
        &self,
        conn_iri: &str,
        type_iri: &str,
        resolved_fields: &[(String, &str, Option<&str>)],
    ) -> Result<Graph> {
        let subject = self.node(conn_iri)?;
        let type_node = self.node(type_iri)?;
        let rdf_type = self.node(&format!("{RDF}type"))?;

        let mut instance = Graph::new();
        instance.insert(TripleRef::new(&subject, &rdf_type, &type_node));
        for (predicate_iri, value, datatype_iri) in resolved_fields {
            let predicate = self.node(predicate_iri)?;
            let literal = self.typed_literal(value, *datatype_iri)?;
            instance.insert(TripleRef::new(&subject, &predicate, &literal));
        }
        Ok(instance)
    }

    /// Fetch every requested field for one connector, or `None` if the node does not exist
    /// with the descriptor's type and all `required` fields present. `fields` is
    /// `(predicate, required)`; required predicates sit in the main graph pattern (missing
    /// one means no match at all, mirroring every hand-written `get_*` query this replaces),
    /// optional ones are each wrapped in their own `OPTIONAL { }`.
    fn fetch_connector(
        &self,
        company_slug: &str,
        descriptor: &ConnectorDescriptor,
        label: Option<&str>,
        fields: &[(&str, bool)],
    ) -> Result<Option<BTreeMap<String, String>>> {
        let conn_iri = namespaces::connector_iri(descriptor.kind, company_slug, label);
        let ns = self.connector_namespace(descriptor);
        let type_iri = ns.class_iri(descriptor);

        let mut select_vars = String::new();
        let mut required_clause = String::new();
        let mut optional_clause = String::new();
        for (predicate, required) in fields {
            let pred_iri = ns.field_iri(predicate)?;
            select_vars.push_str(&format!("?{predicate} "));
            if *required {
                required_clause.push_str(&format!(" ; <{pred_iri}> ?{predicate}"));
            } else {
                optional_clause.push_str(&format!(
                    " OPTIONAL {{ <{conn_iri}> <{pred_iri}> ?{predicate} }}"
                ));
            }
        }

        let sparql = format!(
            "SELECT {select_vars}WHERE {{ \
             GRAPH <{CONFIG_GRAPH}> {{ \
               <{conn_iri}> a <{type_iri}>{required_clause} .{optional_clause} \
             }} }}"
        );

        let rows = self.store.select(&sparql)?;
        match rows.first() {
            None => Ok(None),
            Some(row) => {
                let mut values = BTreeMap::new();
                for (predicate, _) in fields {
                    if let Some(v) = col(row, predicate) {
                        values.insert((*predicate).to_string(), v);
                    }
                }
                Ok(Some(values))
            }
        }
    }

    /// Labels of every connector of one type registered for a company, via the same
    /// `<company> hasConnector ?conn . ?conn a <Type> ; label ?label` pattern every
    /// label-scoped `list_*` used to spell out individually.
    fn list_connector_labels(
        &self,
        company_slug: &str,
        descriptor: &ConnectorDescriptor,
    ) -> Result<Vec<String>> {
        let company_iri = namespaces::company_iri(company_slug);
        let ns = self.connector_namespace(descriptor);
        let type_iri = ns.class_iri(descriptor);
        let label_iri = ns.field_iri("label")?;
        // `hasConnector` links a company to its connector: company-level, stays `CORE_NS`
        // regardless of the connector's own namespace -- contreforts-kg#21 does not migrate it.
        let sparql = format!(
            "SELECT ?label WHERE {{ \
             GRAPH <{CONFIG_GRAPH}> {{ \
               <{company_iri}> <{CORE_NS}hasConnector> ?conn . \
               ?conn a <{type_iri}> ; <{label_iri}> ?label \
             }} }}"
        );
        Ok(self
            .store
            .select(&sparql)?
            .into_iter()
            .filter_map(|row| col(&row, "label"))
            .collect())
    }

    // ── Company CRUD ──────────────────────────────────────────────────────────

    /// Register a new company in the config graph.
    /// Calling this again with the same slug is idempotent (overwrites name).
    pub fn add_company(&self, config: &CompanyConfig) -> Result<()> {
        let iri = namespaces::company_iri(&config.slug);
        let subject = self.node(&iri)?;

        // Remove stale name triples to make this idempotent.
        let name_pred = self.node(&format!("{CORE_NS}name"))?;
        let quads: Vec<_> = self
            .store
            .inner()
            .quads_for_pattern(
                Some((&subject).into()),
                Some((&name_pred).into()),
                None,
                Some((&self.graph).into()),
            )
            .collect::<std::result::Result<_, _>>()?;
        for q in quads {
            self.store.inner().remove(&q)?;
        }

        self.write_type(&subject, &format!("{CORE_NS}Company"))?;
        self.write_literal(&subject, &format!("{CORE_NS}slug"), &config.slug, None)?;
        self.write_literal(&subject, &format!("{CORE_NS}name"), &config.name, None)?;
        Ok(())
    }

    /// Return all registered companies.
    pub fn list_companies(&self) -> Result<Vec<CompanyConfig>> {
        let sparql = format!(
            "SELECT ?slug ?name WHERE {{ \
             GRAPH <{CONFIG_GRAPH}> {{ \
               ?c a <{CORE_NS}Company> ; <{CORE_NS}slug> ?slug ; <{CORE_NS}name> ?name \
             }} }}"
        );
        self.store
            .select(&sparql)?
            .into_iter()
            .map(|row| {
                Ok(CompanyConfig {
                    slug: col(&row, "slug")
                        .ok_or_else(|| ConfigGraphError::InvalidIri("missing slug".into()))?,
                    name: col(&row, "name")
                        .ok_or_else(|| ConfigGraphError::InvalidIri("missing name".into()))?,
                })
            })
            .collect()
    }

    /// Fetch a single company by slug, or `None` if not found.
    pub fn get_company(&self, slug: &str) -> Result<Option<CompanyConfig>> {
        let company_iri = namespaces::company_iri(slug);
        let sparql = format!(
            "SELECT ?name WHERE {{ \
             GRAPH <{CONFIG_GRAPH}> {{ \
               <{company_iri}> a <{CORE_NS}Company> ; <{CORE_NS}name> ?name \
             }} }}"
        );
        let rows = self.store.select(&sparql)?;
        match rows.first() {
            None => Ok(None),
            Some(row) => Ok(Some(CompanyConfig {
                slug: slug.to_string(),
                name: col(row, "name")
                    .ok_or_else(|| ConfigGraphError::InvalidIri("missing name".into()))?,
            })),
        }
    }

    /// Remove a company and all its connectors from the config graph.
    pub fn remove_company(&self, slug: &str) -> Result<()> {
        for connector in self.list_connectors(slug)? {
            self.remove_connector(slug, connector.connector_type(), connector.label())?;
        }
        let iri = namespaces::company_iri(slug);
        let subject = self.node(&iri)?;
        remove_subject_from_named_graph(self.store, &subject, &self.graph)
    }

    // ── Connector CRUD ────────────────────────────────────────────────────────
    //
    // Each `set_*`/`get_*`/`list_*` below is a thin, source-compatible wrapper: it builds the
    // field list this one kind needs (including the two auth enums, which pick a different
    // predicate set per variant) and hands it to the generic engine above. None of them touch
    // `require_company`, `remove_subject_from_named_graph`, `write_type` or the
    // `hasConnector` link directly any more — that shape now lives in exactly one place.

    /// Register or replace the ERPNext connector for a company.
    pub fn set_erpnext_connector(
        &self,
        company_slug: &str,
        config: &ErpNextConnectorConfig,
    ) -> Result<()> {
        self.write_connector(
            company_slug,
            &ERPNEXT,
            None,
            &[
                ("companyName", Some(config.company_name.as_str())),
                ("instanceUrl", Some(config.url.as_str())),
                ("apiKey", Some(config.api_key.as_str())),
                ("apiSecret", Some(config.api_secret.as_str())),
            ],
        )
    }

    /// Register or replace the Pennylane connector for a company.
    pub fn set_pennylane_connector(
        &self,
        company_slug: &str,
        config: &PennylaneConnectorConfig,
    ) -> Result<()> {
        self.write_connector(
            company_slug,
            &PENNYLANE,
            None,
            &[
                ("token", Some(config.token.as_str())),
                ("baseUrl", config.base_url.as_deref()),
            ],
        )
    }

    /// Register or replace a Forgejo connector for a company (label-scoped).
    pub fn set_forgejo_connector(
        &self,
        company_slug: &str,
        config: &ForgejoConnectorConfig,
    ) -> Result<()> {
        self.write_connector(
            company_slug,
            &FORGEJO,
            Some(&config.label),
            &[
                ("label", Some(config.label.as_str())),
                ("instanceUrl", Some(config.url.as_str())),
                ("token", Some(config.token.as_str())),
            ],
        )
    }

    /// Register or replace a GitLab connector for a company (label-scoped).
    pub fn set_gitlab_connector(
        &self,
        company_slug: &str,
        config: &GitlabConnectorConfig,
    ) -> Result<()> {
        self.write_connector(
            company_slug,
            &GITLAB,
            Some(&config.label),
            &[
                ("label", Some(config.label.as_str())),
                ("instanceUrl", Some(config.url.as_str())),
                ("token", Some(config.token.as_str())),
            ],
        )
    }

    /// Fetch the ERPNext connector for a company, or `None` if not configured.
    pub fn get_erpnext_connector(
        &self,
        company_slug: &str,
    ) -> Result<Option<ErpNextConnectorConfig>> {
        let fields = self.fetch_connector(
            company_slug,
            &ERPNEXT,
            None,
            &[
                ("companyName", true),
                ("instanceUrl", true),
                ("apiKey", true),
                ("apiSecret", true),
            ],
        )?;
        Ok(fields.map(|f| ErpNextConnectorConfig {
            company_name: f.get("companyName").cloned().unwrap_or_default(),
            url: f.get("instanceUrl").cloned().unwrap_or_default(),
            api_key: f.get("apiKey").cloned().unwrap_or_default(),
            api_secret: f.get("apiSecret").cloned().unwrap_or_default(),
        }))
    }

    /// Fetch the Pennylane connector for a company, or `None` if not configured.
    pub fn get_pennylane_connector(
        &self,
        company_slug: &str,
    ) -> Result<Option<PennylaneConnectorConfig>> {
        let fields = self.fetch_connector(
            company_slug,
            &PENNYLANE,
            None,
            &[("token", true), ("baseUrl", false)],
        )?;
        Ok(fields.map(|f| PennylaneConnectorConfig {
            token: f.get("token").cloned().unwrap_or_default(),
            base_url: f.get("baseUrl").cloned(),
        }))
    }

    /// Fetch a specific Forgejo connector by label, or `None` if not configured.
    pub fn get_forgejo_connector(
        &self,
        company_slug: &str,
        label: &str,
    ) -> Result<Option<ForgejoConnectorConfig>> {
        let fields = self.fetch_connector(
            company_slug,
            &FORGEJO,
            Some(label),
            &[("instanceUrl", true), ("token", true)],
        )?;
        Ok(fields.map(|f| ForgejoConnectorConfig {
            label: label.to_string(),
            url: f.get("instanceUrl").cloned().unwrap_or_default(),
            token: f.get("token").cloned().unwrap_or_default(),
        }))
    }

    /// List all Forgejo connectors for a company.
    pub fn list_forgejo_connectors(
        &self,
        company_slug: &str,
    ) -> Result<Vec<ForgejoConnectorConfig>> {
        self.list_connector_labels(company_slug, &FORGEJO)?
            .into_iter()
            .filter_map(|label| self.get_forgejo_connector(company_slug, &label).transpose())
            .collect()
    }

    /// Fetch a specific GitLab connector by label, or `None` if not configured.
    pub fn get_gitlab_connector(
        &self,
        company_slug: &str,
        label: &str,
    ) -> Result<Option<GitlabConnectorConfig>> {
        let fields = self.fetch_connector(
            company_slug,
            &GITLAB,
            Some(label),
            &[("instanceUrl", true), ("token", true)],
        )?;
        Ok(fields.map(|f| GitlabConnectorConfig {
            label: label.to_string(),
            url: f.get("instanceUrl").cloned().unwrap_or_default(),
            token: f.get("token").cloned().unwrap_or_default(),
        }))
    }

    /// List all GitLab connectors for a company.
    pub fn list_gitlab_connectors(&self, company_slug: &str) -> Result<Vec<GitlabConnectorConfig>> {
        self.list_connector_labels(company_slug, &GITLAB)?
            .into_iter()
            .filter_map(|label| self.get_gitlab_connector(company_slug, &label).transpose())
            .collect()
    }

    /// Register or replace an O365 connector for a company (label-scoped).
    pub fn set_o365_connector(
        &self,
        company_slug: &str,
        config: &O365ConnectorConfig,
    ) -> Result<()> {
        let mut fields: Vec<(&str, Option<&str>)> = vec![("label", Some(config.label.as_str()))];
        match &config.auth {
            O365ConnectorAuth::ClientCredentials {
                tenant_id,
                client_id,
                client_secret,
            } => {
                fields.push(("authMode", Some("client_credentials")));
                fields.push(("tenantId", Some(tenant_id.as_str())));
                fields.push(("clientId", Some(client_id.as_str())));
                fields.push(("clientSecret", Some(client_secret.as_str())));
            }
            O365ConnectorAuth::Delegated {
                access_token,
                refresh_token,
            } => {
                fields.push(("authMode", Some("delegated")));
                fields.push(("token", Some(access_token.as_str())));
                fields.push(("refreshToken", refresh_token.as_deref()));
            }
        }
        fields.push(("userPrincipal", config.user_principal.as_deref()));
        fields.push(("customer", config.customer.as_deref()));

        self.write_connector(company_slug, &O365, Some(&config.label), &fields)
    }

    /// Register or replace a CalDAV connector for a company (label-scoped).
    pub fn set_caldav_connector(
        &self,
        company_slug: &str,
        config: &CaldavConnectorConfig,
    ) -> Result<()> {
        let mut fields: Vec<(&str, Option<&str>)> = vec![
            ("label", Some(config.label.as_str())),
            ("instanceUrl", Some(config.url.as_str())),
        ];
        match &config.auth {
            CaldavConnectorAuth::Basic { username, password } => {
                fields.push(("authMode", Some("basic")));
                fields.push(("username", Some(username.as_str())));
                fields.push(("password", Some(password.as_str())));
            }
            CaldavConnectorAuth::Bearer { token } => {
                fields.push(("authMode", Some("bearer")));
                fields.push(("token", Some(token.as_str())));
            }
        }
        fields.push(("calendarHome", config.calendar_home.as_deref()));
        fields.push(("customer", config.customer.as_deref()));

        self.write_connector(company_slug, &CALDAV, Some(&config.label), &fields)
    }

    /// Fetch a specific O365 connector by label.
    pub fn get_o365_connector(
        &self,
        company_slug: &str,
        label: &str,
    ) -> Result<Option<O365ConnectorConfig>> {
        let fields = self.fetch_connector(
            company_slug,
            &O365,
            Some(label),
            &[
                ("authMode", true),
                ("tenantId", false),
                ("clientId", false),
                ("clientSecret", false),
                ("token", false),
                ("refreshToken", false),
                ("userPrincipal", false),
                ("customer", false),
            ],
        )?;
        let Some(f) = fields else {
            return Ok(None);
        };
        let auth_mode = f.get("authMode").cloned().unwrap_or_default();
        let auth = match auth_mode.as_str() {
            "client_credentials" => O365ConnectorAuth::ClientCredentials {
                tenant_id: f.get("tenantId").cloned().unwrap_or_default(),
                client_id: f.get("clientId").cloned().unwrap_or_default(),
                client_secret: f.get("clientSecret").cloned().unwrap_or_default(),
            },
            _ => {
                let access_token = f.get("token").cloned().ok_or_else(|| {
                    ConfigGraphError::InvalidIri(
                        "o365 delegated connector has no access_token stored".into(),
                    )
                })?;
                O365ConnectorAuth::Delegated {
                    access_token,
                    refresh_token: f.get("refreshToken").cloned(),
                }
            }
        };
        Ok(Some(O365ConnectorConfig {
            label: label.to_string(),
            auth,
            user_principal: f.get("userPrincipal").cloned(),
            customer: f.get("customer").cloned(),
        }))
    }

    /// List all O365 connectors for a company.
    pub fn list_o365_connectors(&self, company_slug: &str) -> Result<Vec<O365ConnectorConfig>> {
        self.list_connector_labels(company_slug, &O365)?
            .into_iter()
            .filter_map(|label| self.get_o365_connector(company_slug, &label).transpose())
            .collect()
    }

    /// Fetch a specific CalDAV connector by label.
    pub fn get_caldav_connector(
        &self,
        company_slug: &str,
        label: &str,
    ) -> Result<Option<CaldavConnectorConfig>> {
        let fields = self.fetch_connector(
            company_slug,
            &CALDAV,
            Some(label),
            &[
                ("instanceUrl", true),
                ("authMode", true),
                ("username", false),
                ("password", false),
                ("token", false),
                ("calendarHome", false),
                ("customer", false),
            ],
        )?;
        let Some(f) = fields else {
            return Ok(None);
        };
        let auth_mode = f.get("authMode").cloned().unwrap_or_default();
        let auth = match auth_mode.as_str() {
            "basic" => CaldavConnectorAuth::Basic {
                username: f.get("username").cloned().unwrap_or_default(),
                password: f.get("password").cloned().unwrap_or_default(),
            },
            _ => CaldavConnectorAuth::Bearer {
                token: f.get("token").cloned().unwrap_or_default(),
            },
        };
        Ok(Some(CaldavConnectorConfig {
            label: label.to_string(),
            url: f.get("instanceUrl").cloned().unwrap_or_default(),
            auth,
            calendar_home: f.get("calendarHome").cloned(),
            customer: f.get("customer").cloned(),
        }))
    }

    /// List all CalDAV connectors for a company.
    pub fn list_caldav_connectors(&self, company_slug: &str) -> Result<Vec<CaldavConnectorConfig>> {
        self.list_connector_labels(company_slug, &CALDAV)?
            .into_iter()
            .filter_map(|label| self.get_caldav_connector(company_slug, &label).transpose())
            .collect()
    }

    /// Upsert a Matrix agent connector for a company.
    pub fn set_matrix_connector(
        &self,
        company_slug: &str,
        config: &MatrixConnectorConfig,
    ) -> Result<()> {
        self.write_connector(
            company_slug,
            &MATRIX,
            Some(&config.label),
            &[
                ("label", Some(config.label.as_str())),
                ("homeserverUrl", Some(config.homeserver_url.as_str())),
                ("accessToken", Some(config.access_token.as_str())),
                ("deviceId", config.device_id.as_deref()),
                ("userId", config.user_id.as_deref()),
            ],
        )
    }

    /// Fetch a specific Matrix connector by label.
    pub fn get_matrix_connector(
        &self,
        company_slug: &str,
        label: &str,
    ) -> Result<Option<MatrixConnectorConfig>> {
        let fields = self.fetch_connector(
            company_slug,
            &MATRIX,
            Some(label),
            &[
                ("homeserverUrl", true),
                ("accessToken", true),
                ("deviceId", false),
                ("userId", false),
            ],
        )?;
        Ok(fields.map(|f| MatrixConnectorConfig {
            label: label.to_string(),
            homeserver_url: f.get("homeserverUrl").cloned().unwrap_or_default(),
            access_token: f.get("accessToken").cloned().unwrap_or_default(),
            device_id: f.get("deviceId").cloned(),
            user_id: f.get("userId").cloned(),
        }))
    }

    /// List all Matrix connectors for a company.
    pub fn list_matrix_connectors(&self, company_slug: &str) -> Result<Vec<MatrixConnectorConfig>> {
        self.list_connector_labels(company_slug, &MATRIX)?
            .into_iter()
            .filter_map(|label| self.get_matrix_connector(company_slug, &label).transpose())
            .collect()
    }

    /// Upsert an SMTP agent connector for a company.
    pub fn set_smtp_connector(
        &self,
        company_slug: &str,
        config: &SmtpConnectorConfig,
    ) -> Result<()> {
        let port_str = config.port.to_string();
        let tls_str = match config.tls {
            SmtpTlsMode::None => "none",
            SmtpTlsMode::Starttls => "starttls",
            SmtpTlsMode::Tls => "tls",
        };
        self.write_connector(
            company_slug,
            &SMTP,
            Some(&config.label),
            &[
                ("label", Some(config.label.as_str())),
                ("smtpHost", Some(config.host.as_str())),
                ("smtpPort", Some(port_str.as_str())),
                ("fromAddress", Some(config.from_address.as_str())),
                ("smtpTls", Some(tls_str)),
                ("smtpUsername", config.username.as_deref()),
                ("smtpPassword", config.password.as_deref()),
            ],
        )
    }

    /// Fetch a specific SMTP connector by label.
    pub fn get_smtp_connector(
        &self,
        company_slug: &str,
        label: &str,
    ) -> Result<Option<SmtpConnectorConfig>> {
        let fields = self.fetch_connector(
            company_slug,
            &SMTP,
            Some(label),
            &[
                ("smtpHost", true),
                ("smtpPort", true),
                ("fromAddress", true),
                ("smtpTls", true),
                ("smtpUsername", false),
                ("smtpPassword", false),
            ],
        )?;
        let ns = self.connector_namespace(&SMTP);
        fields
            .map(|f| {
                let port = parse_declared_field(&ns, "smtpPort", f.get("smtpPort"), 587u16)?;
                let tls = match f.get("smtpTls").map(String::as_str) {
                    Some("tls") => SmtpTlsMode::Tls,
                    Some("starttls") => SmtpTlsMode::Starttls,
                    _ => SmtpTlsMode::None,
                };
                Ok(SmtpConnectorConfig {
                    label: label.to_string(),
                    host: f.get("smtpHost").cloned().unwrap_or_default(),
                    port,
                    from_address: f.get("fromAddress").cloned().unwrap_or_default(),
                    tls,
                    username: f.get("smtpUsername").cloned(),
                    password: f.get("smtpPassword").cloned(),
                })
            })
            .transpose()
    }

    /// List all SMTP connectors for a company.
    pub fn list_smtp_connectors(&self, company_slug: &str) -> Result<Vec<SmtpConnectorConfig>> {
        self.list_connector_labels(company_slug, &SMTP)?
            .into_iter()
            .filter_map(|label| self.get_smtp_connector(company_slug, &label).transpose())
            .collect()
    }

    /// Upsert a VectorStore connector for a company.
    pub fn set_vector_store_connector(
        &self,
        company_slug: &str,
        config: &VectorStoreConnectorConfig,
    ) -> Result<()> {
        let kind_str = match config.kind {
            VectorStoreKind::Pgvector => "pgvector",
            VectorStoreKind::InMemory => "in_memory",
        };
        // contreforts-kg#8: before anything is written, not after.
        validate_vector_store_geometry(config)?;
        let dimension_str = config.dimension.to_string();
        self.write_connector(
            company_slug,
            &VECTOR_STORE,
            Some(&config.label),
            &[
                ("label", Some(config.label.as_str())),
                ("vectorStoreKind", Some(kind_str)),
                ("instanceUrl", config.url.as_deref()),
                ("tableName", config.table.as_deref()),
                ("dimension", Some(dimension_str.as_str())),
                ("columnType", Some(config.column_type.as_str())),
                // Written, never read back -- see the field's own doc comment
                // and `admin_url_is_write_only`. `None` skips the triple
                // entirely rather than storing an empty string.
                ("adminUrl", config.admin_url.as_deref()),
            ],
        )
    }

    /// Fetch a specific VectorStore connector by label.
    pub fn get_vector_store_connector(
        &self,
        company_slug: &str,
        label: &str,
    ) -> Result<Option<VectorStoreConnectorConfig>> {
        let fields = self.fetch_connector(
            company_slug,
            &VECTOR_STORE,
            Some(label),
            &[
                ("vectorStoreKind", true),
                ("dimension", true),
                ("instanceUrl", false),
                ("tableName", false),
                // Optional on read, unlike on write: a graph written before
                // contreforts-kg#8 has no such triple, and its absence means
                // `vector` (see `parse_vector_store_column_type`).
                ("columnType", false),
            ],
        )?;
        let ns = self.connector_namespace(&VECTOR_STORE);
        fields
            .map(|f| {
                let dimension = parse_declared_field(&ns, "dimension", f.get("dimension"), 0u32)?;
                Ok(VectorStoreConnectorConfig {
                    label: label.to_string(),
                    kind: parse_vector_store_kind(f.get("vectorStoreKind").map(String::as_str)),
                    url: f.get("instanceUrl").cloned(),
                    table: f.get("tableName").cloned(),
                    dimension,
                    column_type: parse_vector_store_column_type(
                        f.get("columnType").map(String::as_str),
                    ),
                    // Deliberately NOT read from the graph: `adminUrl` is a DDL
                    // credential (contreforts/contreforts-workspace#4). Not
                    // requested in the field list above, so it is not fetched
                    // at all -- this cannot become a redaction someone forgets
                    // to apply. Whoever needs it reads it by an explicit,
                    // separate path.
                    admin_url: None,
                })
            })
            .transpose()
    }

    /// List all VectorStore connectors for a company.
    /// Read a vector store's **DDL credential**, the one thing
    /// `get_vector_store_connector` deliberately does not return
    /// (contreforts/contreforts-workspace#4).
    ///
    /// A separate method rather than a field on the config, so that reading a
    /// store to display it and reading it to provision a table are different
    /// calls. `get_vector_store_connector` never fetches this triple at all,
    /// which is stronger than redacting it: there is no code path where
    /// forgetting to redact leaks it.
    ///
    /// `Ok(None)` covers both "no such connector" and "configured without an
    /// admin URL". Both mean the same thing to a caller — this store cannot be
    /// provisioned — and distinguishing them here would only invite a caller to
    /// treat one as an error.
    pub fn get_vector_store_admin_url(
        &self,
        company_slug: &str,
        label: &str,
    ) -> Result<Option<String>> {
        let fields = self.fetch_connector(
            company_slug,
            &VECTOR_STORE,
            Some(label),
            &[("adminUrl", false)],
        )?;
        Ok(fields.and_then(|f| f.get("adminUrl").cloned()))
    }

    pub fn list_vector_store_connectors(
        &self,
        company_slug: &str,
    ) -> Result<Vec<VectorStoreConnectorConfig>> {
        self.list_connector_labels(company_slug, &VECTOR_STORE)?
            .into_iter()
            .filter_map(|label| {
                self.get_vector_store_connector(company_slug, &label)
                    .transpose()
            })
            .collect()
    }

    // ── Stalwart sidecar connector CRUD ───────────────────────────────────────

    /// Upsert a Stalwart sidecar connector for a company.
    pub fn set_stalwart_connector(
        &self,
        company_slug: &str,
        config: &StalwartConnectorConfig,
    ) -> Result<()> {
        let listen_port_str = config.listen_port.to_string();
        let smtp_local_port_str = config.smtp_local_port.to_string();
        let smtp_relay_port_str = config.smtp_relay_port.to_string();
        let ollama_timeout_str = config.ollama_timeout_secs.to_string();
        self.write_connector(
            company_slug,
            &STALWART,
            Some(&config.label),
            &[
                ("label", Some(config.label.as_str())),
                ("jmapBaseUrl", Some(config.jmap_base_url.as_str())),
                ("adminUser", Some(config.admin_user.as_str())),
                ("adminPass", Some(config.admin_pass.as_str())),
                ("listenPort", Some(listen_port_str.as_str())),
                ("stateDir", Some(config.state_dir.as_str())),
                ("dbPath", Some(config.db_path.as_str())),
                ("smtpLocalHost", Some(config.smtp_local_host.as_str())),
                ("smtpLocalPort", Some(smtp_local_port_str.as_str())),
                ("smtpRelayHost", config.smtp_relay_host.as_deref()),
                ("smtpRelayPort", Some(smtp_relay_port_str.as_str())),
                ("imipAnchorDomain", Some(config.imip_anchor_domain.as_str())),
                ("ollamaUrl", Some(config.ollama_url.as_str())),
                ("ollamaModel", Some(config.ollama_model.as_str())),
                ("ollamaTimeoutSecs", Some(ollama_timeout_str.as_str())),
                ("customer", config.customer.as_deref()),
            ],
        )
    }

    /// Fetch a specific Stalwart connector by label.
    pub fn get_stalwart_connector(
        &self,
        company_slug: &str,
        label: &str,
    ) -> Result<Option<StalwartConnectorConfig>> {
        let fields = self.fetch_connector(
            company_slug,
            &STALWART,
            Some(label),
            &[
                ("jmapBaseUrl", true),
                ("adminUser", true),
                ("adminPass", true),
                ("listenPort", true),
                ("stateDir", true),
                ("dbPath", true),
                ("smtpLocalHost", true),
                ("smtpLocalPort", true),
                ("smtpRelayHost", false),
                ("smtpRelayPort", true),
                ("imipAnchorDomain", true),
                ("ollamaUrl", true),
                ("ollamaModel", true),
                ("ollamaTimeoutSecs", true),
                ("customer", false),
            ],
        )?;
        let ns = self.connector_namespace(&STALWART);
        fields
            .map(|f| {
                let listen_port =
                    parse_declared_field(&ns, "listenPort", f.get("listenPort"), 8080u16)?;
                let smtp_local_port =
                    parse_declared_field(&ns, "smtpLocalPort", f.get("smtpLocalPort"), 25u16)?;
                let smtp_relay_port =
                    parse_declared_field(&ns, "smtpRelayPort", f.get("smtpRelayPort"), 25u16)?;
                let ollama_timeout_secs = parse_declared_field(
                    &ns,
                    "ollamaTimeoutSecs",
                    f.get("ollamaTimeoutSecs"),
                    30u64,
                )?;
                Ok(StalwartConnectorConfig {
                    label: label.to_string(),
                    jmap_base_url: f.get("jmapBaseUrl").cloned().unwrap_or_default(),
                    admin_user: f.get("adminUser").cloned().unwrap_or_default(),
                    admin_pass: f.get("adminPass").cloned().unwrap_or_default(),
                    listen_port,
                    state_dir: f.get("stateDir").cloned().unwrap_or_default(),
                    db_path: f.get("dbPath").cloned().unwrap_or_default(),
                    smtp_local_host: f.get("smtpLocalHost").cloned().unwrap_or_default(),
                    smtp_local_port,
                    smtp_relay_host: f.get("smtpRelayHost").cloned(),
                    smtp_relay_port,
                    imip_anchor_domain: f.get("imipAnchorDomain").cloned().unwrap_or_default(),
                    ollama_url: f.get("ollamaUrl").cloned().unwrap_or_default(),
                    ollama_model: f.get("ollamaModel").cloned().unwrap_or_default(),
                    ollama_timeout_secs,
                    customer: f.get("customer").cloned(),
                })
            })
            .transpose()
    }

    /// List all Stalwart connectors for a company.
    pub fn list_stalwart_connectors(
        &self,
        company_slug: &str,
    ) -> Result<Vec<StalwartConnectorConfig>> {
        self.list_connector_labels(company_slug, &STALWART)?
            .into_iter()
            .filter_map(|label| {
                self.get_stalwart_connector(company_slug, &label)
                    .transpose()
            })
            .collect()
    }

    // ── Visio connector CRUD ──────────────────────────────────────────────────

    /// Upsert a Visio (Element Call + Kutt) connector for a company.
    pub fn set_visio_connector(
        &self,
        company_slug: &str,
        config: &VisioConnectorConfig,
    ) -> Result<()> {
        let listen_port_str = config.listen_port.to_string();
        self.write_connector(
            company_slug,
            &VISIO,
            Some(&config.label),
            &[
                ("label", Some(config.label.as_str())),
                ("listenPort", Some(listen_port_str.as_str())),
                ("kuttBaseUrl", Some(config.kutt_base_url.as_str())),
                ("kuttApiKey", Some(config.kutt_api_key.as_str())),
                ("serviceApiKey", Some(config.service_api_key.as_str())),
                ("customer", config.customer.as_deref()),
            ],
        )
    }

    /// Fetch a specific Visio connector by label.
    pub fn get_visio_connector(
        &self,
        company_slug: &str,
        label: &str,
    ) -> Result<Option<VisioConnectorConfig>> {
        let fields = self.fetch_connector(
            company_slug,
            &VISIO,
            Some(label),
            &[
                ("listenPort", true),
                ("kuttBaseUrl", true),
                ("kuttApiKey", true),
                ("serviceApiKey", true),
                ("customer", false),
            ],
        )?;
        let ns = self.connector_namespace(&VISIO);
        fields
            .map(|f| {
                let listen_port =
                    parse_declared_field(&ns, "listenPort", f.get("listenPort"), 8080u16)?;
                Ok(VisioConnectorConfig {
                    label: label.to_string(),
                    listen_port,
                    kutt_base_url: f.get("kuttBaseUrl").cloned().unwrap_or_default(),
                    kutt_api_key: f.get("kuttApiKey").cloned().unwrap_or_default(),
                    service_api_key: f.get("serviceApiKey").cloned().unwrap_or_default(),
                    customer: f.get("customer").cloned(),
                })
            })
            .transpose()
    }

    /// List all Visio connectors for a company.
    pub fn list_visio_connectors(&self, company_slug: &str) -> Result<Vec<VisioConnectorConfig>> {
        self.list_connector_labels(company_slug, &VISIO)?
            .into_iter()
            .filter_map(|label| self.get_visio_connector(company_slug, &label).transpose())
            .collect()
    }

    // ── KnowledgeBase CRUD ────────────────────────────────────────────────────

    /// Resolve `kg_instance_label` to a concrete, registered instance label, or `None` when no
    /// instance is registered at all (contreforts-workspace#58 D5; see the field's own doc
    /// comment on [`KnowledgeBaseConfig::kg_instance_label`] for the full three-way rule).
    /// `kb_label` is only used to name the offending KB in an error.
    fn resolve_kg_instance_label(
        &self,
        kb_label: &str,
        kg_instance_label: &Option<String>,
    ) -> Result<Option<String>> {
        match kg_instance_label {
            Some(label) => {
                if self.get_kg_instance(label)?.is_none() {
                    return Err(ConfigGraphError::kb_instance_unregistered(kb_label, label));
                }
                Ok(Some(label.clone()))
            }
            None => {
                let instances = self.list_kg_instances()?;
                match instances.len() {
                    0 => Ok(None),
                    1 => Ok(Some(instances[0].label.clone())),
                    n => Err(ConfigGraphError::kg_instance_ambiguous(kb_label, n)),
                }
            }
        }
    }

    /// Upsert a KnowledgeBase for a company.
    ///
    /// Enforces D5's first invariant (contreforts-workspace#58, comment 7969; #18 Q3): once
    /// `config.kg_instance_label` resolves to a concrete, registered instance (see
    /// [`Self::resolve_kg_instance_label`]), a `config.graph` that does not fall under that
    /// instance's assigned IRI prefix is rejected before anything is written (see
    /// [`ConfigGraphError::kb_graph_prefix_violation`]).
    pub fn set_knowledge_base(
        &self,
        company_slug: &str,
        config: &KnowledgeBaseConfig,
    ) -> Result<()> {
        self.require_company(company_slug)?;

        let resolved_instance =
            self.resolve_kg_instance_label(&config.label, &config.kg_instance_label)?;

        if let (Some(instance_label), Some(graph)) = (&resolved_instance, &config.graph) {
            let instance = self
                .get_kg_instance(instance_label)?
                .expect("just resolved to a registered instance");
            if !graph.starts_with(&instance.iri_prefix) {
                return Err(ConfigGraphError::kb_graph_prefix_violation(
                    &config.label,
                    graph,
                    instance_label,
                ));
            }
        }

        let kb_iri = namespaces::knowledge_base_iri(company_slug, &config.label);
        let kb_node = self.node(&kb_iri)?;
        let company_node = self.node(&namespaces::company_iri(company_slug))?;

        remove_subject_from_named_graph(self.store, &kb_node, &self.graph)?;

        self.write_type(&kb_node, &format!("{CORE_NS}KnowledgeBase"))?;
        self.write_literal(&kb_node, &format!("{CORE_NS}label"), &config.label, None)?;
        if let Some(graph) = &config.graph {
            self.write_literal(&kb_node, &format!("{CORE_NS}graphIri"), graph, None)?;
        }
        if let Some(instance_label) = &resolved_instance {
            self.write_literal(
                &kb_node,
                &format!("{CORE_NS}kgInstanceLabel"),
                instance_label,
                None,
            )?;
        }
        self.write_literal(
            &kb_node,
            &format!("{CORE_NS}vectorStoreLabel"),
            &config.vector_store_label,
            None,
        )?;

        self.write_triple(
            &company_node,
            &format!("{CORE_NS}hasKnowledgeBase"),
            Term::NamedNode(kb_node),
        )
    }

    /// Fetch a specific KnowledgeBase by label.
    pub fn get_knowledge_base(
        &self,
        company_slug: &str,
        label: &str,
    ) -> Result<Option<KnowledgeBaseConfig>> {
        let kb_iri = namespaces::knowledge_base_iri(company_slug, label);
        let sparql = format!(
            "SELECT ?graph ?vsLabel ?instLabel WHERE {{ \
             GRAPH <{CONFIG_GRAPH}> {{ \
               <{kb_iri}> a <{CORE_NS}KnowledgeBase> ; \
                          <{CORE_NS}vectorStoreLabel> ?vsLabel . \
               OPTIONAL {{ <{kb_iri}> <{CORE_NS}graphIri> ?graph }} \
               OPTIONAL {{ <{kb_iri}> <{CORE_NS}kgInstanceLabel> ?instLabel }} \
             }} }}"
        );
        let rows = self.store.select(&sparql)?;
        match rows.first() {
            None => Ok(None),
            Some(row) => Ok(Some(KnowledgeBaseConfig {
                label: label.to_string(),
                kg_instance_label: col(row, "instLabel"),
                graph: col(row, "graph"),
                vector_store_label: col(row, "vsLabel").unwrap_or_default(),
            })),
        }
    }

    /// List all KnowledgeBases for a company.
    pub fn list_knowledge_bases(&self, company_slug: &str) -> Result<Vec<KnowledgeBaseConfig>> {
        let company_iri = namespaces::company_iri(company_slug);
        let sparql = format!(
            "SELECT ?label ?graph ?vsLabel ?instLabel WHERE {{ \
             GRAPH <{CONFIG_GRAPH}> {{ \
               <{company_iri}> <{CORE_NS}hasKnowledgeBase> ?kb . \
               ?kb a <{CORE_NS}KnowledgeBase> ; \
                   <{CORE_NS}label> ?label ; \
                   <{CORE_NS}vectorStoreLabel> ?vsLabel . \
               OPTIONAL {{ ?kb <{CORE_NS}graphIri> ?graph }} \
               OPTIONAL {{ ?kb <{CORE_NS}kgInstanceLabel> ?instLabel }} \
             }} }}"
        );
        self.store
            .select(&sparql)?
            .into_iter()
            .map(|row| {
                Ok(KnowledgeBaseConfig {
                    label: col(&row, "label").unwrap_or_default(),
                    kg_instance_label: col(&row, "instLabel"),
                    graph: col(&row, "graph"),
                    vector_store_label: col(&row, "vsLabel").unwrap_or_default(),
                })
            })
            .collect()
    }

    /// Remove a KnowledgeBase and the company's link to it.
    pub fn remove_knowledge_base(&self, company_slug: &str, label: &str) -> Result<()> {
        let kb_iri = namespaces::knowledge_base_iri(company_slug, label);
        let kb_node = self.node(&kb_iri)?;
        let company_node = self.node(&namespaces::company_iri(company_slug))?;
        let has_kb_pred = self.node(&format!("{CORE_NS}hasKnowledgeBase"))?;

        remove_subject_from_named_graph(self.store, &kb_node, &self.graph)?;
        self.store.remove_quad(
            &company_node,
            &has_kb_pred,
            &Term::NamedNode(kb_node),
            &self.graph,
        )?;
        Ok(())
    }

    // ── KG Instance CRUD (contreforts-workspace#58 D4) ───────────────────────
    //
    // A KG instance record is *configuration* -- it names, and stores, the independently
    // assigned IRI prefix a `contreforts-kg` data instance builds its entity IRIs from
    // (`contreforts_kg::namespaces::InstanceNamespace`). Global, not company-scoped (ruling 2):
    // one instance holds many companies' data. Its own subject IRI is a plain, hand-entered
    // config-graph IRI (`{DATA_NS}kg-instance/{label}`) -- it is not, and must never become, one
    // of the entity/instance-data IRIs re-prefixed by instance assignment (comment 7936's
    // table); `tests/config_iri_invariance.rs` pins the five builders that must stay rooted at
    // the fixed `DATA_NS`, and this record's own subject follows the same rule for the same
    // reason: it is itself hand-entered, not re-derivable by a sync.

    /// Register a new KG instance, or idempotently re-register the exact same
    /// `(label, iri_prefix)` pair. Enforces uniqueness on **both** fields, independently
    /// (contreforts-workspace#58 D4, ruling 1):
    ///
    /// - a different, already-registered instance must not share this `label` (ambiguous
    ///   resolution by label, contreforts-workspace#18 Q5), rejected with
    ///   [`ConfigGraphError::KgInstanceLabelConflict`];
    /// - a different, already-registered instance must not share this `iri_prefix` (silently
    ///   merges two instances' entity data into one IRI space), rejected with
    ///   [`ConfigGraphError::KgInstancePrefixConflict`].
    ///
    /// Renaming an existing instance is a distinct, dedicated operation --
    /// [`Self::rename_kg_instance`] -- not a second call to this method with a new label.
    pub fn set_kg_instance(&self, config: &KgInstanceConfig) -> Result<()> {
        if let Some(existing) = self.get_kg_instance(&config.label)?
            && existing.iri_prefix != config.iri_prefix
        {
            return Err(ConfigGraphError::KgInstanceLabelConflict {
                label: config.label.clone(),
                existing_prefix: existing.iri_prefix,
            });
        }
        if let Some(other_label) = self.label_using_prefix(&config.iri_prefix)?
            && other_label != config.label
        {
            return Err(ConfigGraphError::KgInstancePrefixConflict {
                prefix: config.iri_prefix.clone(),
                existing_label: other_label,
            });
        }

        let iri = kg_instance_iri(&config.label);
        let node = self.node(&iri)?;
        remove_subject_from_named_graph(self.store, &node, &self.graph)?;
        self.write_type(&node, &format!("{CORE_NS}KgInstance"))?;
        self.write_literal(&node, &format!("{CORE_NS}label"), &config.label, None)?;
        self.write_literal(
            &node,
            &format!("{CORE_NS}iriPrefix"),
            &config.iri_prefix,
            None,
        )?;
        Ok(())
    }

    /// Fetch a single KG instance by label, or `None` if not registered.
    pub fn get_kg_instance(&self, label: &str) -> Result<Option<KgInstanceConfig>> {
        let iri = kg_instance_iri(label);
        let sparql = format!(
            "SELECT ?prefix WHERE {{ \
             GRAPH <{CONFIG_GRAPH}> {{ \
               <{iri}> a <{CORE_NS}KgInstance> ; <{CORE_NS}iriPrefix> ?prefix \
             }} }}"
        );
        let rows = self.store.select(&sparql)?;
        match rows.first() {
            None => Ok(None),
            Some(row) => Ok(Some(KgInstanceConfig {
                label: label.to_string(),
                iri_prefix: col(row, "prefix")
                    .ok_or_else(|| ConfigGraphError::InvalidIri("missing iriPrefix".into()))?,
            })),
        }
    }

    /// Return every registered KG instance.
    pub fn list_kg_instances(&self) -> Result<Vec<KgInstanceConfig>> {
        let sparql = format!(
            "SELECT ?label ?prefix WHERE {{ \
             GRAPH <{CONFIG_GRAPH}> {{ \
               ?inst a <{CORE_NS}KgInstance> ; \
                     <{CORE_NS}label> ?label ; \
                     <{CORE_NS}iriPrefix> ?prefix \
             }} }}"
        );
        self.store
            .select(&sparql)?
            .into_iter()
            .map(|row| {
                Ok(KgInstanceConfig {
                    label: col(&row, "label")
                        .ok_or_else(|| ConfigGraphError::InvalidIri("missing label".into()))?,
                    iri_prefix: col(&row, "prefix")
                        .ok_or_else(|| ConfigGraphError::InvalidIri("missing iriPrefix".into()))?,
                })
            })
            .collect()
    }

    /// Rename a registered KG instance: `new_label` resolves it from here on, while its
    /// assigned prefix stays byte-identical (contreforts-workspace#18 Q2 -- this is the entire
    /// reason the prefix is assigned independently of the label rather than derived from it).
    /// Moves the record to a new subject IRI (`{DATA_NS}kg-instance/{new_label}`); the old
    /// label no longer resolves anything afterward. Fails if `old_label` is not registered, or
    /// if `new_label` already names a *different* instance.
    pub fn rename_kg_instance(&self, old_label: &str, new_label: &str) -> Result<()> {
        let existing = self.get_kg_instance(old_label)?.ok_or_else(|| {
            ConfigGraphError::InvalidIri(format!(
                "no KG instance registered under label '{old_label}'"
            ))
        })?;

        if old_label != new_label
            && let Some(collision) = self.get_kg_instance(new_label)?
        {
            return Err(ConfigGraphError::KgInstanceLabelConflict {
                label: new_label.to_string(),
                existing_prefix: collision.iri_prefix,
            });
        }

        let old_node = self.node(&kg_instance_iri(old_label))?;
        remove_subject_from_named_graph(self.store, &old_node, &self.graph)?;

        let new_node = self.node(&kg_instance_iri(new_label))?;
        self.write_type(&new_node, &format!("{CORE_NS}KgInstance"))?;
        self.write_literal(&new_node, &format!("{CORE_NS}label"), new_label, None)?;
        self.write_literal(
            &new_node,
            &format!("{CORE_NS}iriPrefix"),
            &existing.iri_prefix,
            None,
        )?;
        Ok(())
    }

    /// The label of the registered instance currently holding `prefix`, if any. Checked in
    /// Rust over [`Self::list_kg_instances`] rather than embedding `prefix` as a SPARQL string
    /// literal -- an assigned prefix is operator-supplied, and this sidesteps having to escape
    /// it for safe embedding in a query.
    fn label_using_prefix(&self, prefix: &str) -> Result<Option<String>> {
        Ok(self
            .list_kg_instances()?
            .into_iter()
            .find(|instance| instance.iri_prefix == prefix)
            .map(|instance| instance.label))
    }

    // ── KB-reference guard (contreforts-workspace#58 D5, second invariant) ───
    //
    // #18 Q3 / comment 7969: "exactly one config record may name a KB graph IRI -- the KB's own
    // `KnowledgeBaseConfig.graph` -- and no other config record may name one at all." Every
    // registered KB's own graph is the one value nothing *else* may ever store verbatim.
    // `all_registered_kb_graphs` is the single query both write-time callers
    // (`write_connector`, `set_connector_target_kb`) and `validate_startup` use to know what
    // "a registered KB's own graph IRI" currently means -- global across every company, since a
    // connector in one company could just as easily be handed another company's KB graph IRI.

    /// Every currently-registered KB's own `graph`, across every company -- the value set D5's
    /// second invariant reserves to `KnowledgeBaseConfig.graph` alone.
    fn all_registered_kb_graphs(&self) -> Result<Vec<String>> {
        let sparql = format!(
            "SELECT ?graph WHERE {{ \
             GRAPH <{CONFIG_GRAPH}> {{ \
               ?kb a <{CORE_NS}KnowledgeBase> ; <{CORE_NS}graphIri> ?graph \
             }} }}"
        );
        self.store
            .select(&sparql)?
            .into_iter()
            .map(|row| {
                col(&row, "graph")
                    .ok_or_else(|| ConfigGraphError::InvalidIri("missing graphIri".into()))
            })
            .collect()
    }

    /// Reject any of `values` that equals a currently-registered KB's own `graph`, naming the
    /// offending value. Used at every write-time call site of D5's second invariant; see the
    /// section doc comment above.
    fn reject_kb_graph_reference<'v>(
        &self,
        values: impl IntoIterator<Item = &'v str>,
    ) -> Result<()> {
        let kb_graphs = self.all_registered_kb_graphs()?;
        if kb_graphs.is_empty() {
            return Ok(());
        }
        for value in values {
            if kb_graphs.iter().any(|g| g == value) {
                return Err(ConfigGraphError::kb_graph_referenced_elsewhere(value));
            }
        }
        Ok(())
    }

    // ── Target-KB link (contreforts-workspace#58 D4) ─────────────────────────
    //
    // A connector's config names the knowledge base it feeds, one-directionally -- a connector
    // names its target KB, a KB never names its connectors (contreforts-workspace#18 point 3).
    // Stored as a **literal** label on the connector's own subject, not an IRI reference to the
    // KB's node: contreforts-workspace#58 ruling 3 keeps this deliberately ambiguity-proof.
    // #18's own text is unclear on which direction "config gains a Target-KB link" describes --
    // the prose reads config -> KB, but the direction rule is written "KB graph -> config
    // graph, never the reverse". A literal label satisfies both readings at once, because it
    // creates *no RDF edge* across graphs in either direction: this predicate's object is a
    // plain string that happens to match a `KnowledgeBaseConfig::label`, not a `NamedNode`
    // pointing into KB data. D5 must settle this ambiguity definitively when it builds the
    // write-time/startup guard -- a guard cannot enforce a direction the design has not fixed.

    /// Set (or replace) the label of the knowledge base a connector targets. Does not validate
    /// that `kb_label` names a KB that actually exists -- contreforts-workspace#58 D5's guard is
    /// what enforces referential validity; D4 only establishes the link itself.
    pub fn set_connector_target_kb(
        &self,
        company_slug: &str,
        connector_kind: &str,
        label: Option<&str>,
        kb_label: &str,
    ) -> Result<()> {
        self.reject_kb_graph_reference(std::iter::once(kb_label))?;

        let conn_iri = namespaces::connector_iri(connector_kind, company_slug, label);
        let conn_node = self.node(&conn_iri)?;
        let pred_iri = format!("{CORE_NS}targetKnowledgeBase");
        let pred_node = self.node(&pred_iri)?;

        // Idempotent: drop any previously-set target before writing the new one, so re-linking
        // a connector replaces rather than accumulates a second literal.
        let quads: Vec<_> = self
            .store
            .inner()
            .quads_for_pattern(
                Some((&conn_node).into()),
                Some((&pred_node).into()),
                None,
                Some((&self.graph).into()),
            )
            .collect::<std::result::Result<_, _>>()?;
        for quad in quads {
            self.store.inner().remove(&quad)?;
        }

        self.write_literal(&conn_node, &pred_iri, kb_label, None)
    }

    /// The label of the knowledge base a connector targets, or `None` if never linked.
    pub fn get_connector_target_kb(
        &self,
        company_slug: &str,
        connector_kind: &str,
        label: Option<&str>,
    ) -> Result<Option<String>> {
        let conn_iri = namespaces::connector_iri(connector_kind, company_slug, label);
        let sparql = format!(
            "SELECT ?kb WHERE {{ \
             GRAPH <{CONFIG_GRAPH}> {{ \
               <{conn_iri}> <{CORE_NS}targetKnowledgeBase> ?kb \
             }} }}"
        );
        let rows = self.store.select(&sparql)?;
        Ok(rows.first().and_then(|row| col(row, "kb")))
    }

    // ── Startup validation (contreforts-workspace#58 D5) ─────────────────────
    //
    // A write-time-only guard is bypassable through the unrestricted raw SPARQL `update` route
    // (`contreforts-config-api/src/routes/graph.rs`, D7): anything written directly through
    // `ConfigStore::inner()` never passes through `set_knowledge_base`, `write_connector` or
    // `set_connector_target_kb` at all. `validate_startup` re-checks both of D5's invariants
    // directly against the store's actual contents, so a corruption that arrived any other way
    // is still caught -- later than write time, but not never.

    /// Re-check both of D5's invariants against the store's actual contents, independent of
    /// whichever write path (if any) put them there. Returns every violation found, each naming
    /// the offending record, the offending value, and the rule violated -- not merely the first
    /// one, so a single corrupted store surfaces its whole problem at once rather than one
    /// restart at a time.
    ///
    /// Meant to be called once at process startup, alongside [`crate::ConfigStore::reload_product_graph`]
    /// (D6's own startup pass) -- see that method's doc comment for why a startup pass is what
    /// makes an invariant real rather than merely convenient to check at the one write path that
    /// happens to exist today.
    pub fn validate_startup(&self) -> std::result::Result<(), Vec<String>> {
        let mut violations = Vec::new();

        let instances = self
            .list_kg_instances()
            .map_err(|e| vec![format!("failed to list KG instances: {e}")])?;
        let companies = self
            .list_companies()
            .map_err(|e| vec![format!("failed to list companies: {e}")])?;

        // Invariant 1 (contreforts-workspace#58 D5, kb_graph_prefix_guard.rs): a KB's own graph
        // must fall under its claimed instance's registered prefix. Checked directly off
        // `list_knowledge_bases`' raw read -- which performs no validation of its own -- rather
        // than through `set_knowledge_base`, so a KB corrupted via `ConfigStore::inner()` (never
        // touching that guarded write path) is still examined here.
        for company in &companies {
            let kbs = match self.list_knowledge_bases(&company.slug) {
                Ok(kbs) => kbs,
                Err(e) => {
                    violations.push(format!(
                        "failed to list knowledge bases for company '{}': {e}",
                        company.slug
                    ));
                    continue;
                }
            };
            for kb in kbs {
                let Some(graph) = &kb.graph else {
                    continue;
                };
                let Some(instance_label) = &kb.kg_instance_label else {
                    // No association recorded -- nothing registered for this KB to belong to
                    // (see `KnowledgeBaseConfig::kg_instance_label`'s doc comment on the
                    // zero-registered-instances case); there is nothing to check this graph
                    // against.
                    continue;
                };
                match instances.iter().find(|i| &i.label == instance_label) {
                    Some(instance) => {
                        if !graph.starts_with(&instance.iri_prefix) {
                            violations.push(format!(
                                "knowledge base '{}' (company '{}') claims KG instance '{}', \
                                 but its graph '{graph}' does not fall under that instance's \
                                 registered prefix '{}'",
                                kb.label, company.slug, instance_label, instance.iri_prefix
                            ));
                        }
                    }
                    None => violations.push(format!(
                        "knowledge base '{}' (company '{}') names KG instance '{instance_label}', \
                         which is not registered",
                        kb.label, company.slug
                    )),
                }
            }
        }

        // Invariant 2 (contreforts-workspace#58 D5, kb_reference_guard.rs /
        // tests/startup_validation.rs's Target-KB corruption case): no config record but a KB's
        // own `KnowledgeBaseConfig.graph` may store a registered KB's graph IRI verbatim. Scanned
        // directly: every literal object on every connector-typed subject (`ALL_CONNECTOR_DESCRIPTORS`
        // is the same one list `write_connector` and `remove_connector` already use, so this
        // cannot silently diverge from what the write-time guard covers), across every declared
        // or undeclared namespace resolution -- catching a Target-KB link or any other connector
        // field corrupted directly via `ConfigStore::inner()`.
        let kb_graphs = match self.all_registered_kb_graphs() {
            Ok(graphs) => graphs,
            Err(e) => {
                violations.push(format!("failed to list registered KB graphs: {e}"));
                Vec::new()
            }
        };
        if !kb_graphs.is_empty() {
            for descriptor in ALL_CONNECTOR_DESCRIPTORS {
                let type_iri = self.connector_namespace(descriptor).class_iri(descriptor);
                let sparql = format!(
                    "SELECT ?s ?p ?o WHERE {{ \
                     GRAPH <{CONFIG_GRAPH}> {{ \
                       ?s a <{type_iri}> ; ?p ?o . \
                       FILTER(isLiteral(?o)) \
                     }} }}"
                );
                let rows = match self.store.select(&sparql) {
                    Ok(rows) => rows,
                    Err(e) => {
                        violations.push(format!(
                            "failed to scan connector kind '{}' for D5's second invariant: {e}",
                            descriptor.kind
                        ));
                        continue;
                    }
                };
                for row in rows {
                    let Some(o) = col(&row, "o") else { continue };
                    if kb_graphs.iter().any(|g| g == &o) {
                        let s = col(&row, "s").unwrap_or_default();
                        let p = col(&row, "p").unwrap_or_default();
                        violations.push(format!(
                            "connector '{s}' stores '{o}' on predicate <{p}> -- that value is a \
                             registered knowledge base's own graph IRI, which only that KB's own \
                             `KnowledgeBaseConfig.graph` may hold"
                        ));
                    }
                }
            }
        }

        if violations.is_empty() {
            Ok(())
        } else {
            Err(violations)
        }
    }

    // ── Agent CRUD ────────────────────────────────────────────────────────────

    /// Upsert an Agent for a company.
    pub fn set_agent(&self, company_slug: &str, config: &AgentConfig) -> Result<()> {
        self.require_company(company_slug)?;

        let agent_iri = namespaces::agent_iri(company_slug, &config.label);
        let agent_node = self.node(&agent_iri)?;
        let company_node = self.node(&namespaces::company_iri(company_slug))?;

        remove_subject_from_named_graph(self.store, &agent_node, &self.graph)?;

        self.write_type(&agent_node, &format!("{CORE_NS}Agent"))?;
        self.write_literal(&agent_node, &format!("{CORE_NS}label"), &config.label, None)?;
        if let Some(name) = &config.display_name {
            self.write_literal(&agent_node, &format!("{CORE_NS}displayName"), name, None)?;
        }
        self.write_literal(
            &agent_node,
            &format!("{CORE_NS}usesKnowledgeBase"),
            &config.knowledge_base_label,
            None,
        )?;
        for channel in &config.channels {
            let value = format!("{}:{}", channel.kind, channel.label);
            self.write_literal(&agent_node, &format!("{CORE_NS}usesChannel"), &value, None)?;
        }

        self.write_triple(
            &company_node,
            &format!("{CORE_NS}hasAgent"),
            Term::NamedNode(agent_node),
        )
    }

    /// Fetch a specific Agent by label.
    pub fn get_agent(&self, company_slug: &str, label: &str) -> Result<Option<AgentConfig>> {
        let agent_iri = namespaces::agent_iri(company_slug, label);
        let sparql = format!(
            "SELECT ?displayName ?kbLabel ?channel WHERE {{ \
             GRAPH <{CONFIG_GRAPH}> {{ \
               <{agent_iri}> a <{CORE_NS}Agent> ; \
                             <{CORE_NS}usesKnowledgeBase> ?kbLabel . \
               OPTIONAL {{ <{agent_iri}> <{CORE_NS}displayName> ?displayName }} \
               OPTIONAL {{ <{agent_iri}> <{CORE_NS}usesChannel> ?channel }} \
             }} }}"
        );
        let rows = self.store.select(&sparql)?;
        if rows.is_empty() {
            return Ok(None);
        }
        let display_name = col(&rows[0], "displayName");
        let kb_label = col(&rows[0], "kbLabel").unwrap_or_default();
        let mut channels = Vec::new();
        for row in &rows {
            if let Some(value) = col(row, "channel")
                && let Some(ch) = parse_channel_ref(&value)
                && !channels.contains(&ch)
            {
                channels.push(ch);
            }
        }
        Ok(Some(AgentConfig {
            label: label.to_string(),
            display_name,
            knowledge_base_label: kb_label,
            channels,
        }))
    }

    /// List all Agents for a company.
    pub fn list_agents(&self, company_slug: &str) -> Result<Vec<AgentConfig>> {
        let company_iri = namespaces::company_iri(company_slug);
        let sparql = format!(
            "SELECT ?label ?displayName ?kbLabel ?channel WHERE {{ \
             GRAPH <{CONFIG_GRAPH}> {{ \
               <{company_iri}> <{CORE_NS}hasAgent> ?agent . \
               ?agent a <{CORE_NS}Agent> ; \
                      <{CORE_NS}label> ?label ; \
                      <{CORE_NS}usesKnowledgeBase> ?kbLabel . \
               OPTIONAL {{ ?agent <{CORE_NS}displayName> ?displayName }} \
               OPTIONAL {{ ?agent <{CORE_NS}usesChannel> ?channel }} \
             }} }}"
        );
        let rows = self.store.select(&sparql)?;
        let mut by_label: std::collections::BTreeMap<String, AgentConfig> =
            std::collections::BTreeMap::new();
        for row in rows {
            let label = col(&row, "label").unwrap_or_default();
            let entry = by_label
                .entry(label.clone())
                .or_insert_with(|| AgentConfig {
                    label,
                    display_name: col(&row, "displayName"),
                    knowledge_base_label: col(&row, "kbLabel").unwrap_or_default(),
                    channels: Vec::new(),
                });
            if let Some(value) = col(&row, "channel")
                && let Some(ch) = parse_channel_ref(&value)
                && !entry.channels.contains(&ch)
            {
                entry.channels.push(ch);
            }
        }
        Ok(by_label.into_values().collect())
    }

    /// Remove an Agent and the company's link to it.
    pub fn remove_agent(&self, company_slug: &str, label: &str) -> Result<()> {
        let agent_iri = namespaces::agent_iri(company_slug, label);
        let agent_node = self.node(&agent_iri)?;
        let company_node = self.node(&namespaces::company_iri(company_slug))?;
        let has_agent_pred = self.node(&format!("{CORE_NS}hasAgent"))?;

        remove_subject_from_named_graph(self.store, &agent_node, &self.graph)?;
        self.store.remove_quad(
            &company_node,
            &has_agent_pred,
            &Term::NamedNode(agent_node),
            &self.graph,
        )?;
        Ok(())
    }

    // ── SPARQL Template CRUD ──────────────────────────────────────────────────

    /// Upsert a SPARQL template for a company.
    pub fn set_sparql_template(
        &self,
        company_slug: &str,
        config: &SparqlTemplateConfig,
    ) -> Result<()> {
        self.require_company(company_slug)?;

        let template_iri = namespaces::sparql_template_iri(company_slug, &config.label);
        let template_node = self.node(&template_iri)?;
        let company_node = self.node(&namespaces::company_iri(company_slug))?;

        remove_subject_from_named_graph(self.store, &template_node, &self.graph)?;

        self.write_type(&template_node, &format!("{CORE_NS}SparqlTemplate"))?;
        self.write_literal(
            &template_node,
            &format!("{CORE_NS}label"),
            &config.label,
            None,
        )?;
        self.write_literal(
            &template_node,
            &format!("{CORE_NS}description"),
            &config.description,
            None,
        )?;
        self.write_literal(
            &template_node,
            &format!("{CORE_NS}pattern"),
            &config.pattern,
            None,
        )?;

        self.write_triple(
            &company_node,
            &format!("{CORE_NS}hasSparqlTemplate"),
            Term::NamedNode(template_node),
        )
    }

    /// Fetch a specific SPARQL template by label.
    pub fn get_sparql_template(
        &self,
        company_slug: &str,
        label: &str,
    ) -> Result<Option<SparqlTemplateConfig>> {
        let template_iri = namespaces::sparql_template_iri(company_slug, label);
        let sparql = format!(
            "SELECT ?description ?pattern WHERE {{ \
             GRAPH <{CONFIG_GRAPH}> {{ \
               <{template_iri}> a <{CORE_NS}SparqlTemplate> ; \
                                <{CORE_NS}description> ?description ; \
                                <{CORE_NS}pattern> ?pattern \
             }} }}"
        );
        let rows = self.store.select(&sparql)?;
        match rows.first() {
            None => Ok(None),
            Some(row) => Ok(Some(SparqlTemplateConfig {
                label: label.to_string(),
                description: col(row, "description").unwrap_or_default(),
                pattern: col(row, "pattern").unwrap_or_default(),
            })),
        }
    }

    /// List all SPARQL templates for a company.
    pub fn list_sparql_templates(&self, company_slug: &str) -> Result<Vec<SparqlTemplateConfig>> {
        let company_iri = namespaces::company_iri(company_slug);
        let sparql = format!(
            "SELECT ?label ?description ?pattern WHERE {{ \
             GRAPH <{CONFIG_GRAPH}> {{ \
               <{company_iri}> <{CORE_NS}hasSparqlTemplate> ?t . \
               ?t a <{CORE_NS}SparqlTemplate> ; \
                  <{CORE_NS}label> ?label ; \
                  <{CORE_NS}description> ?description ; \
                  <{CORE_NS}pattern> ?pattern \
             }} }}"
        );
        self.store
            .select(&sparql)?
            .into_iter()
            .map(|row| {
                Ok(SparqlTemplateConfig {
                    label: col(&row, "label").unwrap_or_default(),
                    description: col(&row, "description").unwrap_or_default(),
                    pattern: col(&row, "pattern").unwrap_or_default(),
                })
            })
            .collect()
    }

    /// Remove a SPARQL template and the company's link to it.
    pub fn remove_sparql_template(&self, company_slug: &str, label: &str) -> Result<()> {
        let template_iri = namespaces::sparql_template_iri(company_slug, label);
        let template_node = self.node(&template_iri)?;
        let company_node = self.node(&namespaces::company_iri(company_slug))?;
        let has_template_pred = self.node(&format!("{CORE_NS}hasSparqlTemplate"))?;

        remove_subject_from_named_graph(self.store, &template_node, &self.graph)?;
        self.store.remove_quad(
            &company_node,
            &has_template_pred,
            &Term::NamedNode(template_node),
            &self.graph,
        )?;
        Ok(())
    }

    /// Ensure default SPARQL templates are present for a company.
    pub fn ensure_default_templates(&self, company_slug: &str) -> Result<()> {
        let defaults = vec![
            SparqlTemplateConfig {
                label: "find_by_type".into(),
                description: "Find entities of a specific RDF type (e.g., cforts:Invoice).".into(),
                pattern: "SELECT ?s WHERE { ?s a <{{type}}> } LIMIT {{limit}}".into(),
            },
            SparqlTemplateConfig {
                label: "filter_by_property".into(),
                description: "Find entities with a specific property value.".into(),
                pattern: "SELECT ?s WHERE { ?s <{{property}}> {{value}} } LIMIT {{limit}}".into(),
            },
            SparqlTemplateConfig {
                label: "related_entities".into(),
                description: "Find entities related to a specific entity via any predicate.".into(),
                pattern: "SELECT ?p ?o WHERE { <{{iri}}> ?p ?o } LIMIT {{limit}}".into(),
            },
        ];

        for template in defaults {
            if self
                .get_sparql_template(company_slug, &template.label)?
                .is_none()
            {
                self.set_sparql_template(company_slug, &template)?;
            }
        }
        Ok(())
    }

    /// Return all connectors registered for a company.
    pub fn list_connectors(&self, company_slug: &str) -> Result<Vec<ConnectorConfig>> {
        let mut out = Vec::new();
        if let Some(c) = self.get_erpnext_connector(company_slug)? {
            out.push(ConnectorConfig::ErpNext(c));
        }
        if let Some(c) = self.get_pennylane_connector(company_slug)? {
            out.push(ConnectorConfig::Pennylane(c));
        }
        for c in self.list_forgejo_connectors(company_slug)? {
            out.push(ConnectorConfig::Forgejo(c));
        }
        for c in self.list_gitlab_connectors(company_slug)? {
            out.push(ConnectorConfig::Gitlab(c));
        }
        for c in self.list_o365_connectors(company_slug)? {
            out.push(ConnectorConfig::O365(c));
        }
        for c in self.list_caldav_connectors(company_slug)? {
            out.push(ConnectorConfig::Caldav(c));
        }
        for c in self.list_matrix_connectors(company_slug)? {
            out.push(ConnectorConfig::Matrix(c));
        }
        for c in self.list_smtp_connectors(company_slug)? {
            out.push(ConnectorConfig::Smtp(c));
        }
        for c in self.list_vector_store_connectors(company_slug)? {
            out.push(ConnectorConfig::VectorStore(c));
        }
        for c in self.list_stalwart_connectors(company_slug)? {
            out.push(ConnectorConfig::Stalwart(c));
        }
        for c in self.list_visio_connectors(company_slug)? {
            out.push(ConnectorConfig::Visio(c));
        }
        Ok(out)
    }

    /// Remove a specific connector (and the company's link to it).
    ///
    /// For forge connectors (forgejo, gitlab), `label` must be provided.
    /// For single-instance connectors (erpnext, pennylane), `label` is ignored.
    pub fn remove_connector(
        &self,
        company_slug: &str,
        connector_type: &str,
        label: Option<&str>,
    ) -> Result<()> {
        // Was 11 near-identical match arms, one per connector kind, each differing only in
        // the literal kind string passed straight back into `connector_iri`. The kind names
        // and their singleton/labelled shape now live in exactly one place —
        // `ALL_CONNECTOR_DESCRIPTORS` — so this collapses to the two shapes that actually
        // differ: singleton (no label) versus label-scoped (label, defaulting to "default").
        let descriptor = ALL_CONNECTOR_DESCRIPTORS
            .iter()
            .find(|d| d.kind == connector_type)
            .ok_or_else(|| {
                ConfigGraphError::InvalidIri(format!(
                    "unknown connector type '{connector_type}' (expected erpnext|pennylane|forgejo|gitlab|o365|caldav|matrix|smtp|vector_store|stalwart|visio)"
                ))
            })?;
        let conn_iri = if descriptor.singleton {
            namespaces::connector_iri(descriptor.kind, company_slug, None)
        } else {
            namespaces::connector_iri(
                descriptor.kind,
                company_slug,
                Some(label.unwrap_or("default")),
            )
        };

        let conn_node = self.node(&conn_iri)?;
        let company_node = self.node(&namespaces::company_iri(company_slug))?;
        let has_connector_pred = self.node(&format!("{CORE_NS}hasConnector"))?;

        remove_subject_from_named_graph(self.store, &conn_node, &self.graph)?;

        self.store.remove_quad(
            &company_node,
            &has_connector_pred,
            &Term::NamedNode(conn_node),
            &self.graph,
        )?;
        Ok(())
    }

    // ── Group → Customer mapping ────────────────────────────────────────────

    /// Map a forge group/org path to a customer slug.
    pub fn set_group_customer_mapping(
        &self,
        company_slug: &str,
        connector_type: &str,
        connector_label: &str,
        group_path: &str,
        customer_slug: &str,
    ) -> Result<()> {
        let mapping_iri = namespaces::group_mapping_iri(
            connector_type,
            company_slug,
            connector_label,
            group_path,
        );
        let mapping_node = self.node(&mapping_iri)?;

        remove_subject_from_named_graph(self.store, &mapping_node, &self.graph)?;

        self.write_literal(
            &mapping_node,
            &format!("{CORE_NS}groupPath"),
            group_path,
            None,
        )?;
        self.write_literal(
            &mapping_node,
            &format!("{CORE_NS}mapsToCustomer"),
            customer_slug,
            None,
        )?;
        self.write_literal(
            &mapping_node,
            &format!("{CORE_NS}connectorType"),
            connector_type,
            None,
        )?;
        self.write_literal(
            &mapping_node,
            &format!("{CORE_NS}connectorLabel"),
            connector_label,
            None,
        )?;
        Ok(())
    }

    /// Resolve the customer for a group path, walking parent paths.
    ///
    /// For `"parent/child/grandchild"`, tries in order:
    /// `parent/child/grandchild`, `parent/child`, `parent`.
    pub fn get_customer_for_group(
        &self,
        company_slug: &str,
        connector_type: &str,
        connector_label: &str,
        group_path: &str,
    ) -> Result<Option<String>> {
        let mut path = group_path.to_string();
        loop {
            let mapping_iri =
                namespaces::group_mapping_iri(connector_type, company_slug, connector_label, &path);
            let sparql = format!(
                "SELECT ?customer WHERE {{ \
                 GRAPH <{CONFIG_GRAPH}> {{ \
                   <{mapping_iri}> <{CORE_NS}mapsToCustomer> ?customer \
                 }} }}"
            );
            let rows = self.store.select(&sparql)?;
            if let Some(row) = rows.first()
                && let Some(customer) = col(row, "customer")
            {
                return Ok(Some(customer));
            }
            match path.rsplit_once('/') {
                Some((parent, _)) => path = parent.to_string(),
                None => return Ok(None),
            }
        }
    }

    /// List all group→customer mappings for a connector.
    pub fn list_group_mappings(
        &self,
        _company_slug: &str,
        connector_type: &str,
        connector_label: &str,
    ) -> Result<Vec<(String, String)>> {
        let sparql = format!(
            "SELECT ?groupPath ?customer WHERE {{ \
             GRAPH <{CONFIG_GRAPH}> {{ \
               ?m <{CORE_NS}connectorType> \"{connector_type}\" ; \
                  <{CORE_NS}connectorLabel> \"{connector_label}\" ; \
                  <{CORE_NS}groupPath> ?groupPath ; \
                  <{CORE_NS}mapsToCustomer> ?customer \
             }} }}"
        );
        self.store
            .select(&sparql)?
            .into_iter()
            .map(|row| {
                Ok((
                    col(&row, "groupPath").unwrap_or_default(),
                    col(&row, "customer").unwrap_or_default(),
                ))
            })
            .collect()
    }

    // ── helpers ───────────────────────────────────────────────────────────────

    fn require_company(&self, slug: &str) -> Result<()> {
        if self.get_company(slug)?.is_none() {
            return Err(ConfigGraphError::InvalidIri(format!(
                "company '{slug}' not found in config graph — add it first with `config company add`"
            )));
        }
        Ok(())
    }
}

// ── SPARQL result helpers ─────────────────────────────────────────────────────

/// Extract a named column from a SPARQL result row, stripping N-Triples quoting.
fn col(row: &[(String, String)], name: &str) -> Option<String> {
    row.iter()
        .find(|(k, _)| k == name)
        .map(|(_, v)| literal_str(v).to_string())
}

/// Strip N-Triples literal quotes:
/// `"value"` → `value`, `"value"^^<type>` → `value`
fn literal_str(s: &str) -> &str {
    if let Some(inner) = s.strip_prefix('"') {
        // Find the matching closing quote (handles ^^type and @lang suffixes).
        if let Some(pos) = inner.rfind('"') {
            &inner[..pos]
        } else {
            inner
        }
    } else {
        s
    }
}

fn parse_vector_store_kind(s: Option<&str>) -> VectorStoreKind {
    match s {
        Some("pgvector") => VectorStoreKind::Pgvector,
        _ => VectorStoreKind::InMemory,
    }
}

fn parse_channel_ref(s: &str) -> Option<ChannelRef> {
    let (kind, label) = s.split_once(':')?;
    if kind.is_empty() || label.is_empty() {
        return None;
    }
    Some(ChannelRef {
        kind: kind.to_string(),
        label: label.to_string(),
    })
}

#[cfg(test)]
mod tests {
    //! Two private-helper tests carried forward inline from `contreforts-kg/src/config_graph.rs`'s
    //! own `#[cfg(test)]` module (contreforts/contreforts-workspace#58, ruling 2): both call the
    //! private `ConfigGraph::connector_instance_graph` directly, which is unreachable from an
    //! external `tests/` crate. Every other test in that module either moved to
    //! `contreforts-config/tests/config_graph.rs` or is accounted for in this task's report --
    //! see the coverage table there.
    use super::*;
    use contreforts_core::namespaces::CONFIG_GRAPH;

    fn setup() -> (tempfile::TempDir, ConfigStore, &'static str) {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = ConfigStore::open(dir.path().join("config_store")).expect("store opens");
        let cg = ConfigGraph::new(&store, ConnectorDeclarations::none());
        cg.add_company(&CompanyConfig {
            slug: "acme".to_string(),
            name: "Acme".to_string(),
        })
        .unwrap();
        (dir, store, "acme")
    }

    /// Verbatim (content-preserving) reproduction of
    /// `contreforts-connector-forgejo/declaration.ttl:185-224`'s `sh:targetClass`/`sh:path`/
    /// `sh:datatype`/`sh:pattern` -- identical in content to `crates/contreforts-kg/src/
    /// config_graph.rs`'s inline test module's own `FORGEJO_DECLARATION_TTL` const.
    const FORGEJO_DECLARATION_TTL: &str = r#"
        @prefix sh:      <http://www.w3.org/ns/shacl#> .
        @prefix xsd:     <http://www.w3.org/2001/XMLSchema#> .
        @prefix forgejo: <https://contreforts.ds-labs.org/ontologies/forgejo#> .

        forgejo:ForgejoConnectorShape a sh:NodeShape ;
            sh:targetClass forgejo:ForgejoConnector ;
            sh:property [
                sh:path forgejo:label ;
                sh:datatype xsd:string ;
                sh:minCount 1 ;
                sh:maxCount 1 ;
            ] ;
            sh:property [
                sh:path forgejo:instanceUrl ;
                sh:datatype xsd:string ;
                sh:minCount 1 ;
                sh:maxCount 1 ;
                sh:pattern "^https?://[^\\s]+$" ;
            ] ;
            sh:property [
                sh:path forgejo:token ;
                sh:datatype xsd:string ;
                sh:minCount 1 ;
                sh:maxCount 1 ;
            ] .
    "#;

    fn term_plain(term: TermRef<'_>) -> String {
        match term {
            TermRef::NamedNode(n) => n.as_str().to_string(),
            TermRef::Literal(l) => l.value().to_string(),
            other => other.to_string(),
        }
    }

    /// `(predicate, plain object)` pairs of an in-memory validation `Graph`, sorted.
    fn graph_triples(g: &Graph) -> Vec<(String, String)> {
        let mut v: Vec<(String, String)> = g
            .iter()
            .map(|t| (t.predicate.as_str().to_string(), term_plain(t.object)))
            .collect();
        v.sort();
        v
    }

    /// `(predicate, plain object)` pairs actually stored for `subject` in `graph`, sorted --
    /// read straight off the store, independent of `ConfigGraph`'s own query helpers.
    fn stored_triples(
        store: &ConfigStore,
        subject: &NamedNode,
        graph: &NamedNode,
    ) -> Vec<(String, String)> {
        let quads: Vec<_> = store
            .inner()
            .quads_for_pattern(Some(subject.into()), None, None, Some(graph.into()))
            .collect::<std::result::Result<Vec<_>, _>>()
            .unwrap();
        let mut v: Vec<(String, String)> = quads
            .into_iter()
            .map(|q| {
                (
                    q.predicate.as_str().to_string(),
                    term_plain(q.object.as_ref()),
                )
            })
            .collect();
        v.sort();
        v
    }

    #[test]
    // contreforts-kg#19: "if the validated instance is not identical to the written one, the
    // check is theatre." `write_connector` builds both the validated `Graph` and the stored
    // triples from the same `resolved_fields` list (see `write_connector`'s own comment) --
    // this asserts that guarantee holds by rebuilding the instance graph independently (via the
    // same `connector_instance_graph` helper) and comparing it, triple for triple, against what
    // actually landed in the store. A future edit that made the store-write loop diverge from
    // `connector_instance_graph` -- a different literal encoding, or a field written on one path
    // but not the other -- would fail this assertion.
    fn connector_write_validates_exactly_what_it_writes() {
        let (_dir, store, slug) = setup();
        let validator =
            ConnectorValidator::new(FORGEJO_DECLARATION_TTL, &all_connector_kinds()).unwrap();
        let cg = ConfigGraph::with_validator(&store, validator.declarations(), &validator);

        cg.set_forgejo_connector(
            slug,
            &ForgejoConnectorConfig {
                label: "main".into(),
                url: "https://git.example.com".into(),
                token: "tok-abc".into(),
            },
        )
        .unwrap();

        let conn_iri = namespaces::connector_iri("forgejo", slug, Some("main"));
        let conn_node = NamedNode::new(&conn_iri).unwrap();
        let graph_node = NamedNode::new(CONFIG_GRAPH).unwrap();

        // Forgejo's real declaration (`FORGEJO_DECLARATION_TTL`) marks all three fields
        // `sh:datatype xsd:string` -- `None` here is deliberate, not an oversight:
        // `Literal::new_typed_literal(value, xsd:string)` collapses to the exact same
        // `LiteralContent::String` a plain literal is (oxrdf's `Literal::new_typed_literal`),
        // so passing `None` and passing the `xsd:string` IRI produce byte-identical terms --
        // this rebuild is faithful to what `write_connector` actually stores either way.
        const FORGEJO_NS: &str = "https://contreforts.ds-labs.org/ontologies/forgejo#";
        let resolved_fields: Vec<(String, &str, Option<&str>)> = vec![
            (format!("{FORGEJO_NS}label"), "main", None),
            (
                format!("{FORGEJO_NS}instanceUrl"),
                "https://git.example.com",
                None,
            ),
            (format!("{FORGEJO_NS}token"), "tok-abc", None),
        ];
        let instance = cg
            .connector_instance_graph(
                &conn_iri,
                &format!("{FORGEJO_NS}ForgejoConnector"),
                &resolved_fields,
            )
            .unwrap();

        assert_eq!(
            stored_triples(&store, &conn_node, &graph_node),
            graph_triples(&instance),
        );
    }

    /// Deliberately synthetic: no real declaration has a numeric field yet (only forgejo is
    /// declared at all, and every one of its fields is `xsd:string`), so this is the minimum
    /// shape needed to actually exercise the `sh:datatype xsd:integer` path this issue adds.
    const VECTOR_STORE_INTEGER_DECLARATION_TTL: &str = r#"
        @prefix sh:  <http://www.w3.org/ns/shacl#> .
        @prefix xsd: <http://www.w3.org/2001/XMLSchema#> .
        @prefix vs:  <https://contreforts.ds-labs.org/ontologies/vectorstore#> .

        vs:VectorStoreConnectorShape a sh:NodeShape ;
            sh:targetClass vs:VectorStoreConnector ;
            sh:property [
                sh:path vs:label ;
                sh:datatype xsd:string ;
                sh:minCount 1 ;
                sh:maxCount 1 ;
            ] ;
            sh:property [
                sh:path vs:vectorStoreKind ;
                sh:datatype xsd:string ;
                sh:minCount 1 ;
                sh:maxCount 1 ;
            ] ;
            sh:property [
                sh:path vs:instanceUrl ;
            ] ;
            sh:property [
                sh:path vs:tableName ;
            ] ;
            sh:property [
                sh:path vs:dimension ;
                sh:datatype xsd:integer ;
                sh:minCount 1 ;
                sh:maxCount 1 ;
            ] ;
            sh:property [
                sh:path vs:columnType ;
                sh:datatype xsd:string ;
                sh:minCount 1 ;
                sh:maxCount 1 ;
            ] ;
            sh:property [
                sh:path vs:adminUrl ;
                sh:datatype xsd:string ;
                sh:maxCount 1 ;
            ] .
    "#;

    /// `(predicate, datatype IRI)` pairs actually stored for `subject` in `graph`.
    fn stored_datatypes(
        store: &ConfigStore,
        subject: &NamedNode,
        graph: &NamedNode,
    ) -> Vec<(String, String)> {
        let quads: Vec<_> = store
            .inner()
            .quads_for_pattern(Some(subject.into()), None, None, Some(graph.into()))
            .collect::<std::result::Result<Vec<_>, _>>()
            .unwrap();
        let mut v: Vec<(String, String)> = quads
            .into_iter()
            .filter_map(|q| match q.object {
                Term::Literal(l) => Some((
                    q.predicate.as_str().to_string(),
                    l.datatype().as_str().to_string(),
                )),
                _ => None,
            })
            .collect();
        v.sort();
        v
    }

    /// Same as `stored_datatypes`, for an in-memory validation `Graph` rather than the store.
    fn graph_datatypes(g: &Graph) -> Vec<(String, String)> {
        let mut v: Vec<(String, String)> = g
            .iter()
            .filter_map(|t| match t.object {
                TermRef::Literal(l) => Some((
                    t.predicate.as_str().to_string(),
                    l.datatype().as_str().to_string(),
                )),
                _ => None,
            })
            .collect();
        v.sort();
        v
    }

    #[test]
    // The issue's second bullet under "Tests": "The validated instance and the written triples
    // are literal-for-literal identical, including datatypes." Unlike
    // `connector_write_validates_exactly_what_it_writes` above (forgejo, `xsd:string` only --
    // which `Literal::new_typed_literal` collapses to a plain literal regardless of whether
    // `write_connector` threads a datatype through *correctly*, so that test alone cannot catch
    // a broken datatype path), this uses the synthetic `xsd:integer` declaration so a real
    // divergence -- `write_literal` and `connector_instance_graph` encoding `dimension`
    // differently -- would actually fail this assertion.
    fn connector_instance_graph_matches_typed_write() {
        let (_dir, store, slug) = setup();
        let validator =
            ConnectorValidator::new(VECTOR_STORE_INTEGER_DECLARATION_TTL, &all_connector_kinds())
                .unwrap();
        let cg = ConfigGraph::with_validator(&store, validator.declarations(), &validator);

        cg.set_vector_store_connector(
            slug,
            &VectorStoreConnectorConfig {
                label: "primary".into(),
                kind: VectorStoreKind::Pgvector,
                url: None,
                table: Some("chunks".into()),
                dimension: 1536,
                column_type: VectorStoreColumnType::Vector,
                admin_url: None,
            },
        )
        .unwrap();

        let conn_iri = namespaces::connector_iri("vector_store", slug, Some("primary"));
        let conn_node = NamedNode::new(&conn_iri).unwrap();
        let graph_node = NamedNode::new(CONFIG_GRAPH).unwrap();

        const VS_NS: &str = "https://contreforts.ds-labs.org/ontologies/vectorstore#";
        const XSD_STRING: &str = "http://www.w3.org/2001/XMLSchema#string";
        const XSD_INTEGER: &str = "http://www.w3.org/2001/XMLSchema#integer";
        let resolved_fields: Vec<(String, &str, Option<&str>)> = vec![
            (format!("{VS_NS}label"), "primary", Some(XSD_STRING)),
            (
                format!("{VS_NS}vectorStoreKind"),
                "pgvector",
                Some(XSD_STRING),
            ),
            (format!("{VS_NS}tableName"), "chunks", None),
            (format!("{VS_NS}dimension"), "1536", Some(XSD_INTEGER)),
            (format!("{VS_NS}columnType"), "vector", Some(XSD_STRING)),
        ];
        let instance = cg
            .connector_instance_graph(
                &conn_iri,
                &format!("{VS_NS}VectorStoreConnector"),
                &resolved_fields,
            )
            .unwrap();

        // Values (lexical form) match, as before contreforts-kg#25...
        assert_eq!(
            stored_triples(&store, &conn_node, &graph_node),
            graph_triples(&instance),
        );
        // ...and now, crucially, so do the datatypes -- this is the assertion that would catch
        // a `connector_instance_graph`/`write_connector` divergence the lexical-only comparison
        // above cannot.
        assert_eq!(
            stored_datatypes(&store, &conn_node, &graph_node),
            graph_datatypes(&instance),
        );
        // And directly: dimension really is xsd:integer in the store, not xsd:string.
        assert!(
            stored_datatypes(&store, &conn_node, &graph_node)
                .contains(&(format!("{VS_NS}dimension"), XSD_INTEGER.to_string())),
            "dimension must be stored as xsd:integer"
        );
    }
}
