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
