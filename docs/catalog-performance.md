# Catalog Performance Baseline

## Scope

This document records the reproducible local baseline for the public catalog's deterministic
test fixture. It is not a release threshold: a threshold can only be adopted after the same
measurement is repeated on the pinned release runner.

The fixture has 1,000 documents (500 Policies and 500 Runbooks). It verifies these invariants:

- the first immutable commit validates and stores 1,000 document provenance rows;
- a repeated ready commit accepts intentionally invalid fetched bodies without parsing or adding
  document rows;
- a changed commit reuses 1,000 matching checksums and validates exactly one added document,
  producing 1,001 new provenance rows; and
- 1,000 source metadata records are returned through ten exclusive cursor requests of at most
  100 records each (each store request fetches at most 101 rows to determine `next_after`).

The application fixture additionally counts repository work: initial sync performs 1,000 supplied
body writes and zero document lookups; ready-commit reuse performs zero document writes and zero
document lookups; the changed sync performs 1,001 lookups, 1 supplied body write, and 1,000
durable-body reuse rows. The durable byte total grows by the copied body bytes plus the one newly
validated body; this is intentionally distinct from a controller receiving new body input.

## Reproduction

Run the deterministic assertions first:

```bash
cargo test -p fleet-application catalog_application_tests --lib
```

Measure seven already-compiled warm runs without updating this document automatically:

```bash
for benchmark_run in 1 2 3 4 5 6 7; do
  /usr/bin/time -p cargo test -q -p fleet-application \
    one_thousand_document_sync_keeps_ready_and_checksum_reuse_paths_deterministic --lib
done
```

## 2026-08-30 Local Observation

On the current local development machine, the seven end-to-end warm samples were 110ms,
100ms, 100ms, 100ms, 100ms, 100ms, and 100ms. The median was **100ms**. This observation
includes test-process startup and is informative only; it is not comparable across machines.

## GitHub-hosted release gate

`.github/workflows/catalog-performance.yml` uses the explicit GitHub-hosted
`ubuntu-22.04` label and Rust `1.94.0`. It always records the runner image metadata, toolchain,
seven samples, and median in an artifact. `capture` records a candidate only; it does not create
or alter a baseline. A reviewer must inspect the artifact and commit it as
`docs/catalog-performance-baseline.json` before a release can pass the `verify` mode.

The npm release workflow calls `verify`, so a missing baseline, changed runner image/toolchain,
or median above the committed baseline plus `max(25%, 50ms)` fails closed. The release workflow
does not update the baseline. Baseline revisions require the artifact, all seven samples, and a
written reason in the review/commit.

To capture the first baseline after this change is pushed, run the **Catalog performance**
workflow manually with `mode: capture`, download its timing-report artifact, review it, then
commit the artifact JSON at the path above. Run the workflow with `mode: verify` to prove the
committed baseline before creating a release tag. Do not change cache layout, query concurrency,
or SQLite pragmas merely to improve this number.
