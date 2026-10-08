# Integrator Operations Guide

This guide collects the operational rules client authors need when integrating
with a deployed TricklePay stream contract.

## Redeployment Without Upgradeability

The stream contract is not upgradeable. A defect fix, SDK migration, or
interface change reaches users by deploying a new WASM and creating a new
contract address. There is no admin entry point that can replace code at an
existing address.

Existing streams remain on the deployment that created them. A redeployment
does not copy stream records, token balances, counters, or storage TTL state
from the old contract. Clients must therefore treat the contract id as part of
the stream identity:

```text
stream reference = (network, contract_id, stream_id)
```

Recipients claim from the old deployment by continuing to call `withdraw`,
`withdraw_amount`, and read-only views against that old `contract_id`. Senders
cancel old streams the same way: invoke `cancel` on the deployment that owns
the stream record. New streams should be opened on the new deployment only
after clients and indexers have been updated to recognize its address.

Recommended migration flow:

1. Deploy and verify the new contract address.
2. Publish the new address with the source commit and WASM hash.
3. Update clients to create new streams on the new address.
4. Keep old deployment metadata visible until every old stream is withdrawn,
   cancelled, or intentionally abandoned.
5. Index both old and new addresses during the overlap period.

Do not ask recipients to claim from the new deployment for an old stream. The
new deployment has no storage entry for that stream id and cannot transfer the
tokens locked in the old contract.

## Interface Stability Policy

Current stability expectation: the public interface is intended to be stable
for the lifetime of each deployment. Because deployments are immutable, a
breaking change is delivered as a new contract address rather than an in-place
upgrade.

The following are breaking interface changes:

- Removing, renaming, or changing the parameters or return type of an entry
  point.
- Changing the semantic meaning of an entry point while leaving its signature
  unchanged.
- Reordering, renaming, removing, or changing the type of event fields or
  topics.
- Reusing, renumbering, or changing the meaning of an error code.
- Changing storage semantics in a way that makes previously created streams
  unreadable or unclaimable.

Non-breaking changes include documentation updates, tests, internal refactors
that preserve emitted WASM interface output, and bug fixes that only reject
previously invalid or unsafe inputs when the changelog calls that out.

Every interface change must be communicated in `CHANGELOG.md`. Breaking
changes must identify the changed entry point, event, error code, or behavior,
and must tell integrators whether they need a new generated client, indexer
decoder, or contract address allowlist entry. Before deployment, compare the
interface with:

```bash
stellar contract inspect --wasm target/wasm32v1-none/release/tricklepay_stream.wasm
```

## Recommended Client Retry Behavior

Soroban submission failures are not all equivalent. A network timeout, dropped
HTTP connection, or unavailable RPC response does not prove the transaction
failed; it may have been accepted and applied after the client stopped waiting.

Clients must not blindly rebuild and resubmit a mutating call after an unknown
submission result. Retrying a fresh transaction can double-submit operations
such as `withdraw` once more balance vests, or create an unexpected second
stream if the first creation actually succeeded.

Recommended flow after a failed submission:

1. Preserve the transaction hash or operation intent locally.
2. Query the transaction status from the RPC or Horizon endpoint until it is
   known, expired, or definitively absent.
3. If the transaction succeeded, read the relevant contract view or event:
   `get_stream`, `withdrawable`, `stream_count`, or indexed events depending
   on the operation.
4. If the transaction failed with a contract error, surface that typed error to
   the user and do not retry automatically.
5. If the transaction expired or was never accepted, rebuild with a fresh
   sequence number and resubmit only after checking that the intended state
   change did not already happen.

For `create_stream`, verify whether a `Created` event exists for the sender,
recipient, token, amount, and schedule before resubmitting. For `withdraw` or
`withdraw_amount`, re-read `withdrawable` and the stream's `withdrawn` value
before deciding whether there is still anything to claim. For `cancel`, read
`status` and the `Cancelled` event before trying again.

## Indexer Consistency Requirements

Event topics and payloads are stable for the life of a deployment (see
Interface Stability Policy above), but that alone does not make an indexer
correct. Three things remain the indexer's own responsibility: **replayed
events**, **gaps in the event stream**, and **events it was not written to
understand**. Getting any of these wrong produces an index that looks fine
and is subtly wrong, with no local signal that anything is off.

### Handle repeated events idempotently

A closed ledger on a public Stellar network does not reorganize, but an
indexer can still observe the same event more than once — resubscribing from
an earlier cursor after a restart, replaying a checkpoint during backfill, or
reading the same ledger range from more than one RPC source. Never process an
event by blindly incrementing or appending.

- Key every processed event by a stable identity (ledger sequence,
  transaction, operation, and event index, or whatever equivalent the RPC
  exposes) and skip anything already recorded under that key before applying
  it.
- This matters most for `Withdrawn` and `Cancelled`: re-applying a `Withdrawn`
  event a second time would double the recorded cumulative withdrawn amount
  for a stream that never actually withdrew twice, and that figure would then
  silently disagree with `get_stream`/`withdrawable` on-chain.
- `Created` happens to be idempotent to reapply, since it replaces a record
  with the same data — but dedupe it the same way regardless. Relying on
  per-event-type idempotence instead of one uniform rule is itself a source
  of bugs the moment a future deployment changes what a given event carries.

### Detect and backfill gaps

An indexer can miss events outright: downtime, starting to listen only after
the contract was already deployed, or querying an RPC endpoint whose
retention window for historical events has already passed. The contract has
no push mechanism to flag a missed event, so the indexer has to notice on its
own.

- Treat a `Withdrawn` or `Cancelled` event that names a stream id with no
  corresponding local `Created` record as a signal that the indexer's own
  history has a gap for that id — not as a contract inconsistency — and call
  `get_stream(id)` to recover that stream's fields directly from the
  contract rather than trying to reconstruct history that was never observed.
- Periodically compare the highest stream id the indexer has recorded against
  `stream_count()`. Any gap means ids exist on-chain with no local `Created`
  record, and should be backfilled with `get_stream` for each missing id
  rather than treated as out of scope.
- `get_stream`, `withdrawable`, `vested`, `locked`, `progress`, and `status`
  require no authorization and always reflect current on-chain state
  regardless of how complete the local event history is (see
  [THREAT_MODEL.md § Authorization model](../THREAT_MODEL.md#authorization-model)).
  They are the correct recovery path for a gap, not a reason to mark a stream
  unindexable.

### Ignore, don't crash on, events it does not recognise

Within one deployment the event set is fixed — `Created`, `Withdrawn`,
`Cancelled`, with the fields documented in
[README.md § Events](../README.md#events) — but an indexer built to be
forward-compatible, or one scanning more broadly than a single contract id,
will still encounter things it was not written to decode:

- Events published by other contracts in the same ledger or transaction —
  most commonly the streamed token's own `transfer` event, since
  `create_stream`, `withdraw`, `withdraw_amount`, and `cancel` each trigger
  one. Filter by contract address before attempting to decode anything as a
  stream event.
- A future redeployment's event shape, since changing field order, a field's
  type, or the topic set is a breaking interface change delivered as a new
  contract address rather than an in-place change (see Interface Stability
  Policy above). An indexer tracking a migration across old and new
  deployments (see
  [Redeployment Without Upgradeability](#redeployment-without-upgradeability))
  must associate each event with the deployment whose shape it actually
  matches, not assume every address it tracks decodes identically.
- Skip and log an event your decoder does not recognise rather than crashing
  the process or forcing it into the nearest known shape. A silently
  mis-decoded event is worse than a loudly skipped one — it corrupts every
  derived figure below without leaving a trace.

### Store facts, derive figures

Not everything belongs in the indexer's own storage as a cached number. Split
what is stored from what is computed at read time:

- **Store** — facts an event sets once or appends to: `sender`, `recipient`,
  `token`, `total_amount`, `start_time`, `end_time`, and `cliff_time` (from
  `Created`); the running total of `amount` across every `Withdrawn` event for
  a given stream id; and, once seen, a `Cancelled` event's
  `recipient_amount`/`sender_refund`.
- **Derive at read time, and never cache as of some past moment:** `vested`,
  `withdrawable`, `locked`, `progress`, and `status`. Each is a pure function
  of the stored facts above and the current ledger time — the same formula
  the contract itself runs (see
  [README.md § Implementation details](../README.md#implementation-details)
  for the vesting arithmetic). A cached `withdrawable` value goes stale the
  instant ledger time moves past whenever it was computed, while recomputing
  it from the stored schedule costs nothing and is never wrong. The same
  applies to anything a client renders as "live" — a progress bar, a
  countdown, or a vested-amount display belongs on a value recomputed on
  every render, not one backfilled from the last event the indexer happened
  to see.

## Withdrawing Before Anything Has Vested

`withdraw` is not a no-op when there is nothing to claim: it returns
`StreamError::NothingToWithdraw` (interface code `7`) rather than silently
succeeding or transferring zero tokens. This happens whenever the vested
amount minus what was already withdrawn is zero or less, including:

- Calling `withdraw` before `start_time`, or before `cliff_time` on a stream
  that has one.
- Calling `withdraw` again immediately after a previous withdrawal has
  already claimed everything vested so far.

`withdraw_amount` behaves differently in this situation: because the caller
names an amount, a positive request against zero available balance fails
with `StreamError::InsufficientBalance` (code `8`) instead of
`NothingToWithdraw`. Only the zero-argument `withdraw` entry point returns
`NothingToWithdraw`.

### How a client should present this

This is an expected state, not a failure. A recipient who opens a stream
before the cliff, or right after draining it, will hit this every time. Treat
`NothingToWithdraw` as information to surface in the UI ("Nothing available
to claim yet" / "Already withdrawn everything that has vested"), not as an
error toast, retry prompt, or logged failure. Do not retry the call
automatically; retrying without the underlying state changing will return the
same error.

### How to check beforehand

Call the `withdrawable` view before submitting a `withdraw` transaction:

```ignore
let available = client.withdrawable(&stream_id);
if available > 0 {
    client.withdraw(&stream_id);
}
```

`withdrawable` returns `vested_amount - withdrawn` for the stream, clamped to
a minimum of zero (see [`vesting::withdrawable_amount`]). A result of `0`
means a `withdraw` call would fail with `NothingToWithdraw` right now, so the
client can disable the withdraw action, hide it, or show the informational
state instead of attempting the call and parsing an error. Because vesting is
time-based, `withdrawable` can change between the check and the submitted
transaction landing; treat a `NothingToWithdraw` error that still occurs as
the authoritative answer rather than a bug in the pre-check.

## Maximum Practical Stream Duration

The type system permits `u64` Unix-second timestamps, but the practical maximum
stream duration is much shorter than that. Treat **30 days without any stream
access** as the operational limit imposed by storage rent, and treat streams
longer than 30 days as safe only if a keeper, indexer, wallet, or user action
touches the stream at least once per 24-day window.

The limit comes from the contract's storage TTL policy:

- Stream records live in persistent storage with `ENTRY_TTL = 518_400`
  ledgers, roughly 30 days at five seconds per ledger.
- The contract bumps a stream record back to that target lifetime whenever a
  read or write touches it and the remaining TTL is below
  `BUMP_THRESHOLD = 103_680` ledgers, roughly six days.
- A stream that is not touched for longer than the network TTL can be archived
  by the Soroban storage system.

Beyond that range, the stream schedule is still mathematically valid, but the
record may need off-chain restoration before the contract can read or mutate it.
Clients that support long-running payroll, vesting, or subscriptions should
schedule periodic read calls or another TTL-maintenance process and should
surface restoration requirements if a record has already expired from active
storage.

## Storage Growth Over Time

The section above covers how long *one* stream's record stays alive without
being touched. This section covers the aggregate cost: how the contract's
total storage footprint grows as a deployment accumulates streams over its
lifetime, which is what anyone planning a deployment needs to size for.

### Growth is per stream created, and it never shrinks

Every successful `create_stream` call writes exactly one new persistent
`Stream(id)` entry (`contracts/stream/src/storage.rs`). Ids are assigned from
a monotonic counter and are never reused, so the number of persistent entries
the contract holds is the total count of streams ever created on that
deployment — not the number currently active. Nothing an entry point does
ever removes a `Stream(id)` entry or reclaims the space it occupies:

- There is no `delete_stream` or equivalent entry point.
- A fully withdrawn stream's record is not compacted or cleared — it keeps
  reporting its final state through every view indefinitely (see
  [README.md § Fully withdrawn stream: what each view reports](../README.md#fully-withdrawn-stream-what-each-view-reports)).
- A cancelled stream's record is likewise kept, frozen at the vested amount
  at the moment of cancellation (see
  [THREAT_MODEL.md § Invariants](../THREAT_MODEL.md#invariants)).

So storage grows monotonically with usage and has no mechanism — automatic or
manual — to shrink. A deployment that creates a million streams over its
life holds a million persistent entries forever, regardless of how many of
those streams are still active.

### A settled stream keeps its record, and keeps paying rent

"Settled" — fully vested and fully withdrawn, or cancelled — does not exempt
a stream's entry from the TTL policy described above. A settled record is
just as subject to `ENTRY_TTL` and `BUMP_THRESHOLD` as an active one: if
nothing reads or writes it again, it decays toward archival on exactly the
same schedule as a stream still being drawn down. Settling a stream stops its
*state* from changing; it does not stop the storage clock.

### Archival is per-entry, not a whole-contract event

Archival in Soroban applies to individual ledger entries, not to the contract
as a whole. A deployment with streams created at different times will have
some entries well within their TTL (because they were created or touched
recently) and others already archived (because nothing touched them for
longer than `ENTRY_TTL`), all at the same time. There is no contract-wide
expiry and no risk that activity on one stream affects another's TTL — each
`Stream(id)` entry, including `StreamCount` in instance storage, is extended
independently (`storage::get_stream`, `storage::set_stream`,
`storage::extend_instance_ttl`).

An archived entry is not deleted from the network's perspective — its data
can be restored — but it is no longer directly readable until that
restoration happens, which is an extra, explicit step (and its own fee)
before the contract can read or mutate it. A deployment with many old,
untouched, settled streams should expect a growing fraction of its entries to
be in this state over time, and should not assume that `get_stream` or any
other view will succeed against an arbitrarily old id without a restoration
step first.
