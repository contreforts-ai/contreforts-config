//! On-disk durability for [`crate::ConfigStore`]'s in-memory Oxigraph `Store`, now that this
//! crate no longer links oxigraph's RocksDB backend (`Store::open` needs `oxrocksdb-sys`, a
//! `cmake`/C++ toolchain dependency this crate has been asked to stop shipping).
//!
//! One file per persisted named graph -- [`crate::CONFIG_GRAPH`] and each imported-ontology graph
//! ([`graph_relpath`]) -- rather than one file for the whole store, so the "three classes of
//! graph, named separately" split `lib.rs`'s own module doc argues for extends to the storage
//! layer too. [`crate::PRODUCT_GRAPH`] is deliberately excluded: it is rebuilt from the binary's
//! own compiled-in Turtle at every startup ([`crate::ConfigStore::reload_product_graph`]), so a
//! copy on disk would only ever be stale.
//!
//! Every mutating [`crate::ConfigStore`] method serializes its target graph and calls
//! [`atomic_write_with_backups`] before returning `Ok` -- there is no separate flush step to
//! forget, and no window where a caller sees success for a write that has not reached disk.
//! [`atomic_write_with_backups`] keeps the last [`BACKUP_GENERATIONS`] versions of each file; on
//! [`crate::ConfigStore::open`], [`load_graph_with_recovery`] falls back through them, newest
//! first, if the active file fails to parse -- and immediately re-persists whichever backup
//! recovers cleanly, so a corrupted primary file heals itself rather than re-triggering recovery
//! (and the same "which generation was still good" logging) on every subsequent restart.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use oxigraph::io::{RdfFormat, RdfParser};
use oxigraph::model::{GraphName, NamedNode};
use oxigraph::store::Store;

use crate::ConfigStoreError;
use crate::IMPORTED_ONTOLOGY_GRAPH_PREFIX;
use contreforts_core::namespaces::CONFIG_GRAPH;

/// How many prior versions of a persisted graph file [`atomic_write_with_backups`] keeps,
/// newest as `.bak.1` through oldest as `.bak.5` -- the operator's own choice of how much of a
/// crash-and-corruption tail this crate can recover from without falling back to hand-editing.
pub(crate) const BACKUP_GENERATIONS: usize = 5;

/// The path, relative to the config store's datadir, that `graph_iri` persists to -- or `None`
/// for a graph this crate never writes to disk ([`crate::PRODUCT_GRAPH`], or anything else no
/// mutating method ever targets).
///
/// `IMPORTED_ONTOLOGY_GRAPH_PREFIX`'s own doc comment already establishes that the
/// percent-encoded label following it contains no `/`, which is what makes it safe to use
/// directly as a filename component here.
pub(crate) fn graph_relpath(graph_iri: &str) -> Option<PathBuf> {
    if graph_iri == CONFIG_GRAPH {
        Some(PathBuf::from("config_graph.ttl"))
    } else {
        graph_iri
            .strip_prefix(IMPORTED_ONTOLOGY_GRAPH_PREFIX)
            .map(|label| Path::new("ontologies").join(format!("{label}.ttl")))
    }
}

fn backup_path(path: &Path, generation: usize) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(format!(".bak.{generation}"));
    PathBuf::from(name)
}

fn tmp_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(".tmp");
    PathBuf::from(name)
}

/// Write `contents` to `path`, rotating up to [`BACKUP_GENERATIONS`] previous versions first
/// (`.bak.1` = most recent previous, `.bak.5` = oldest kept) and going through a same-directory
/// temp file + rename so a crash mid-write leaves either the old file or the new one intact,
/// never a half-written one at `path` itself.
pub(crate) fn atomic_write_with_backups(path: &Path, contents: &[u8]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    // Oldest first, so nothing is overwritten before it has been moved out of the way.
    for generation in (1..BACKUP_GENERATIONS).rev() {
        let src = backup_path(path, generation);
        if src.exists() {
            fs::rename(&src, backup_path(path, generation + 1))?;
        }
    }
    if path.exists() {
        fs::rename(path, backup_path(path, 1))?;
    }

    let tmp = tmp_path(path);
    let mut file = fs::File::create(&tmp)?;
    file.write_all(contents)?;
    file.sync_all()?;
    drop(file);
    fs::rename(&tmp, path)?;

    // Best-effort: not every platform/filesystem needs (or supports) a directory fsync for the
    // rename above to be durable, and this crate already treats the write itself, not this last
    // step, as the correctness boundary.
    if let Some(parent) = path.parent()
        && let Ok(dir) = fs::File::open(parent)
    {
        let _ = dir.sync_all();
    }

    Ok(())
}

/// Load `graph`'s Turtle from `path` into `store`, clearing `graph` first so a retry (primary,
/// then each backup in turn) never mixes a previous, partially-loaded attempt's quads with the
/// next candidate's -- `Store::load_from_slice` streams straight into the store, so a payload
/// that goes malformed halfway through otherwise leaves a partial parse sitting in `graph`.
fn try_load_graph(store: &Store, path: &Path, graph: &NamedNode) -> Result<(), String> {
    let bytes = fs::read(path).map_err(|e| e.to_string())?;
    store
        .clear_graph(graph)
        .map_err(|e| format!("clearing <{graph}> before load: {e}"))?;
    let parser =
        RdfParser::from_format(RdfFormat::Turtle).with_default_graph(GraphName::NamedNode(graph.clone()));
    store
        .load_from_slice(parser, &bytes)
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Load `graph` from its persisted file at `path` into `store`, called once per persisted graph
/// from [`crate::ConfigStore::open`].
///
/// A missing file is a fresh datadir (or a graph never yet persisted) -- not corruption, and not
/// an error: `graph` simply starts empty. A file that exists but fails to parse *is* corruption:
/// this falls back through `path`'s backups, newest (`.bak.1`) first, and the first one that
/// parses cleanly is both loaded and immediately re-persisted as the new `path` (via
/// [`atomic_write_with_backups`]) so the corruption is healed rather than rediscovered on every
/// later startup. If none of the backups parse either, this refuses to start that graph empty --
/// [`ConfigStoreError::CorruptGraphFile`] -- rather than silently discarding data that might
/// still be recoverable by hand.
pub(crate) fn load_graph_with_recovery(
    store: &Store,
    path: &Path,
    graph: &NamedNode,
) -> Result<(), ConfigStoreError> {
    if !path.exists() {
        return Ok(());
    }

    if let Err(primary_err) = try_load_graph(store, path, graph) {
        tracing::error!(
            path = %path.display(),
            graph = graph.as_str(),
            error = %primary_err,
            "config store: graph file failed to load -- attempting recovery from backup",
        );

        for generation in 1..=BACKUP_GENERATIONS {
            let candidate = backup_path(path, generation);
            if !candidate.exists() {
                continue;
            }
            if try_load_graph(store, &candidate, graph).is_err() {
                continue;
            }

            tracing::warn!(
                path = %path.display(),
                graph = graph.as_str(),
                recovered_from = %candidate.display(),
                "config store: recovered graph from backup after the active file was found corrupt",
            );

            let recovered_bytes = fs::read(&candidate).map_err(|source| ConfigStoreError::PersistWrite {
                graph: graph.as_str().to_string(),
                path: path.to_path_buf(),
                source,
            })?;
            atomic_write_with_backups(path, &recovered_bytes).map_err(|source| {
                ConfigStoreError::PersistWrite {
                    graph: graph.as_str().to_string(),
                    path: path.to_path_buf(),
                    source,
                }
            })?;
            return Ok(());
        }

        return Err(ConfigStoreError::CorruptGraphFile {
            path: path.to_path_buf(),
            attempted: BACKUP_GENERATIONS,
            reason: primary_err,
        });
    }

    Ok(())
}
