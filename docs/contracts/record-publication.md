# Retained authority and publication

## Requirements

- Optimizations preserve durable session evidence and dependency-safe retention.
- Saved Mekugi sessions remain resumable after execution stops; resume restores retained facts,
  never live processes, grants, or execution authority.
- Resume is product-local (Mekugi and stock Codex resume only their own sessions).

## Boundary contract

- **Authority & provenance**: filesystem operands resolve from admitted request workspace metadata,
  never application cwd. Evidence reads keep caller authority; cross-thread inputs preserve
  authenticated source provenance (agent text never becomes human input).
- **Ordered publication**: dependencies are written and validated before referencing records or
  handles become usable. Cross-owner operations (`recordtransactions`) borrow one non-nested
  transaction scoped to evidence publication, input adoption, or fork initialization, never
  spanning terminal or network I/O. Failed writes may leave conservative retention but never expose
  missing evidence. This guarantees ordered reader visibility across restarts, not crash-atomic
  multi-file or power-loss durability.
- **Lease safety**: snapshot leases bridge validation to publication locks without deadlock; session
  leases protect retained evidence until consumer delivery completes. Compaction removes ancestry
  from one request view without retiring records needed by another branch.
- **Owner boundaries**: Native `mekugi-app-server` owns namespaces,
  catalogs, leases, retention, and helper delivery (`codex-arg0`);
  [Change evidence](change-evidence.md) owns capture reconciliation.

## Current approach (replaceable)

- Retained host call records live in the native `mekugi-app-server` storage layer.
- Fork initialization retains inherited records before cloning indexes and publishes the new
  namespace last so interrupted setup is retryable.
