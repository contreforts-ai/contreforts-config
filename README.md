# contreforts-config

Gives configuration its own Oxigraph runtime and datadir, separate from
`contreforts-kg`'s knowledge-graph store, so that wiping or re-syncing a
knowledge-graph instance can never touch hand-entered configuration.
This crate is the target of `contreforts-config`/D2 in
contreforts/contreforts-workspace#58 (phase D of the bootstrap epic,
contreforts/contreforts-workspace#20), and implements the store-separation
design decided in contreforts/contreforts-workspace#18 — its own store, its
own path, its own lifecycle, with no dependency on `contreforts-kg`.
