# Decision Records

This file records design decisions that were deliberate trade-offs, not
oversights, so a reviewer who notices the gap does not have to re-litigate it
from scratch. Each record is numbered and, once accepted, is not edited in
place — a changed decision gets a new record that supersedes it and says so.

## ADR 1: Enumeration is off-chain, not an entry point

**Status:** Accepted

### Context

There is no entry point that answers "which streams does address X have?".
`get_stream(id)` returns one stream by id; `stream_count()` returns how many
ids exist in total. Nothing returns the set of ids for a given `sender` or
`recipient`. The only on-chain way to find a party's streams today is to call
`get_stream` for every id from `0` to `stream_count() - 1` and filter
client-side, which is O(every stream ever created on the deployment) and gets
slower with every new stream, regardless of how many belong to the party
asking.

This gap is deliberate, not an oversight. `Created`, `Withdrawn`, and
`Cancelled` all index `sender` and/or `recipient` as event topics specifically
so an off-chain consumer can answer this query without the contract doing it
(see [README.md § Events](../README.md#events)); the indexer and web client
that consume those events live in separate repositories.

### Decision

Enumeration by party stays off-chain, answered by an indexer that subscribes
to `Created`, `Withdrawn`, and `Cancelled` and reconstructs the
address-to-stream-id mapping from their topics. The contract exposes no
`streams_of(address)` or equivalent entry point, and none is planned.

### Alternatives considered

1. **A reverse index stored on-chain** — e.g. a `Vec<u64>` of stream ids kept
   per address, appended to on every `create_stream`. Rejected: every
   creation would have to read and rewrite a collection that only grows, for
   both `sender` and `recipient`, so the cost of `create_stream` increases
   without bound as a party accumulates streams. This is exactly the
   unbounded-storage-growth cost Soroban's fee model is designed to price
   out, and it would eventually make creating or reading a heavy user's
   streams prohibitively expensive or too large to read back in one call.

2. **A paginated view entry point** — `list_streams(address, offset, limit) ->
   Vec<u64>` — backed by the same kind of on-chain index. Rejected for the
   same reason as (1): pagination only changes how the index is read, not the
   fact that the contract would still have to maintain and mutate a per-address
   list on every `create_stream`, `withdraw`, `withdraw_amount`, and `cancel`.

3. **Brute-force client iteration over `stream_count`.** Already possible
   today with no contract change, but it is O(total streams on the
   deployment) per query, paid in RPC round-trips by whoever is asking. Fine
   for a local script or a contract with a handful of streams; not something
   a production client should do for "show me my streams" at scale.

4. **Off-chain indexer on event topics (chosen).** The topics the contract
   already publishes for other reasons make `sender`/`recipient` natively
   filterable by any indexer, with no additional contract state or logic.
   This is the standard Soroban/Stellar pattern: keep the contract minimal
   and emit enough signal for indexers to reconstruct whatever view is
   needed. It also lets independent indexers build different views — by
   sender, by recipient, by token, by status — from the same event stream
   without the contract committing to any one of them.

### Consequences

- A client cannot derive "my streams" from the contract alone; it needs an
  indexer that has processed the event history (or a query to one). There is
  no on-chain fallback beyond the slow brute-force iteration in alternative 3.
- `create_stream`, `withdraw`, `withdraw_amount`, and `cancel` have storage
  and fee costs that do not grow as a party accumulates streams, because no
  per-address collection is read or mutated by any of them.
- Adding enumeration later, if it ever becomes necessary, is additive — a new
  entry point — rather than a retrofit, since no existing stream record needs
  to change shape to support it.

### What would have to change to revisit this

- A concrete case where off-chain indexing is not viable: for example, a
  deployment that must be fully self-contained with no indexer
  infrastructure, or a client environment that cannot run or trust a
  third-party indexer and must verify "my streams" from the contract alone.
- If that case materializes, the shape most consistent with this contract's
  low-footprint design is the paginated view in alternative 2, not an
  unbounded `Vec`, with the storage-growth cost accepted explicitly as a new,
  deliberate trade-off — and the resulting entry point treated as a new
  public interface surface per
  [docs/INTEGRATOR_OPERATIONS.md § Interface Stability Policy](INTEGRATOR_OPERATIONS.md#interface-stability-policy).
- Until one of those is true, this decision stands: enumeration by party is a
  client/indexer concern, not a contract concern.
