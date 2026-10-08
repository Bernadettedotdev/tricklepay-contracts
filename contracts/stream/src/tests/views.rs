#![cfg(test)]

use soroban_sdk::vec;

use crate::{StreamError, StreamStatus};

use super::helpers::StreamTest;

#[test]
fn progress_reports_basis_points() {
    let t = StreamTest::setup(1_000);
    t.set_time(100);
    let id = t.contract.create_stream(
        &t.sender,
        &t.recipient,
        &t.token_address,
        &1_000,
        &100,
        &1_100,
        &100,
    );

    // Nothing vested at the start.
    assert_eq!(t.contract.progress(&id), 0);
    // Halfway is 50 percent, in basis points.
    t.set_time(600);
    assert_eq!(t.contract.progress(&id), 5_000);
    // One second before the end, progress must still be below the maximum.
    t.set_time(1_099);
    assert_eq!(t.contract.progress(&id), 9_990);
    assert!(t.contract.progress(&id) < 10_000);
    // Fully vested at the end.
    t.set_time(1_100);
    assert_eq!(t.contract.progress(&id), 10_000);
}

#[test]
fn progress_never_decreases_as_time_advances() {
    let t = StreamTest::setup(1_000);
    t.set_time(100);
    let id = t.contract.create_stream(
        &t.sender,
        &t.recipient,
        &t.token_address,
        &1_000,
        &100,
        &1_100,
        &100,
    );

    let mut previous = 0;
    for timestamp in [50, 100, 101, 250, 350, 600, 850, 1_099, 1_100, 1_200] {
        t.set_time(timestamp);
        let progress = t.contract.progress(&id);
        assert!(
            progress >= previous,
            "progress decreased from {previous} to {progress} at timestamp {timestamp}"
        );
        previous = progress;
    }
}

#[test]
fn locked_decreases_as_the_stream_vests() {
    let t = StreamTest::setup(1_000);
    t.set_time(100);
    let id = t.contract.create_stream(
        &t.sender,
        &t.recipient,
        &t.token_address,
        &1_000,
        &100,
        &1_100,
        &600,
    );

    // Before the cliff, nothing is vested and the whole amount is locked.
    t.set_time(300);
    assert_eq!(t.contract.locked(&id), 1_000);
    assert_eq!(t.contract.vested(&id), 0);
    assert_eq!(t.contract.locked(&id) + t.contract.vested(&id), 1_000);

    // Mid-stream, vested and locked remain complementary.
    t.set_time(850);
    assert_eq!(t.contract.locked(&id), 250);
    assert_eq!(t.contract.vested(&id), 750);
    assert_eq!(t.contract.locked(&id) + t.contract.vested(&id), 1_000);

    // After the end, all value is vested and none is locked.
    t.set_time(1_200);
    assert_eq!(t.contract.locked(&id), 0);
    assert_eq!(t.contract.vested(&id), 1_000);
    assert_eq!(t.contract.locked(&id) + t.contract.vested(&id), 1_000);
}

#[test]
fn locked_never_goes_negative_across_sampled_times() {
    let t = StreamTest::setup(1_000);
    t.set_time(100);
    let id = t.contract.create_stream(
        &t.sender,
        &t.recipient,
        &t.token_address,
        &1_000,
        &100,
        &1_100,
        &600,
    );

    for now in [100u64, 300, 600, 850, 1_100, 1_200] {
        t.set_time(now);
        let vested = t.contract.vested(&id);
        let locked = t.contract.locked(&id);
        assert!(
            locked >= 0,
            "locked={} vested={} at time={}",
            locked,
            vested,
            now
        );
        assert_eq!(locked, (1_000 - vested).max(0));
    }

    assert_eq!(t.contract.locked(&id), 0);
}

// ── Post-cancellation view correctness ──────────────────────────────────────

/// `locked` and `progress` after cancellation must report 0 and 10 000.
#[test]
fn views_are_correct_on_a_cancelled_stream() {
    let t = StreamTest::setup(1_000);
    t.set_time(100);
    let id = t.contract.create_stream(
        &t.sender,
        &t.recipient,
        &t.token_address,
        &1_000,
        &100,
        &1_100,
        &100,
    );

    t.set_time(600);
    t.contract.cancel(&id);

    assert_eq!(t.contract.locked(&id), 0);
    assert_eq!(t.contract.progress(&id), 10_000);
    assert_eq!(t.contract.status(&id), StreamStatus::Cancelled);
    assert_eq!(t.contract.withdrawable(&id), 500);
}

/// Same four view assertions, but run again after the recipient has drained
/// the remaining vested balance. Once the recipient withdraws, withdrawable
/// must fall to 0 and the other views must stay stable. This also confirms
/// the token balances add up and a second withdraw is rejected.
#[test]
fn views_remain_correct_after_recipient_drains_cancelled_stream() {
    let t = StreamTest::setup(1_000);
    t.set_time(100);
    let id = t.contract.create_stream(
        &t.sender,
        &t.recipient,
        &t.token_address,
        &1_000,
        &100,
        &1_100,
        &100,
    );

    // Cancel at the midpoint and then advance time well past the original end
    // to confirm the frozen state does not change with the clock.
    t.set_time(600);
    t.contract.cancel(&id);
    assert_eq!(t.contract.get_stream(&id).total_amount, 500);
    t.set_time(2_000);

    assert_eq!(t.contract.get_stream(&id).total_amount, 500);
    assert_eq!(t.contract.vested(&id), 500);

    // Recipient drains their share.
    let withdrawn = t.contract.withdraw(&id);
    assert_eq!(withdrawn, 500);

    // Token balances add up to the original total — nothing was lost.
    assert_eq!(t.token.balance(&t.sender), 500);
    assert_eq!(t.token.balance(&t.recipient), 500);
    assert_eq!(t.token.balance(&t.contract.address), 0);

    // Views must remain consistent after the drain.
    assert_eq!(t.contract.locked(&id), 0);
    assert_eq!(t.contract.progress(&id), 10_000);
    assert_eq!(t.contract.status(&id), StreamStatus::Cancelled);

    // withdrawable() must now be 0 — the recipient took everything.
    assert_eq!(t.contract.withdrawable(&id), 0);

    // A second withdraw attempt must be rejected.
    assert_eq!(
        t.contract.try_withdraw(&id),
        Err(Ok(StreamError::NothingToWithdraw))
    );
}

/// Issue #252 — View functions must not modify stored state.
#[test]
fn test_view_functions_do_not_modify_stored_state() {
    let t = StreamTest::setup(1_000);
    t.set_time(100);

    let id = t.contract.create_stream(
        &t.sender,
        &t.recipient,
        &t.token_address,
        &1_000,
        &100,
        &1_100,
        &400,
    );

    t.set_time(600);

    let before = t.contract.get_stream(&id);

    let _ = t.contract.get_stream(&id);
    let _ = t.contract.withdrawable(&id);
    let _ = t.contract.vested(&id);
    let _ = t.contract.locked(&id);
    let _ = t.contract.progress(&id);
    let _ = t.contract.status(&id);
    let _ = t.contract.stream_count();

    let after = t.contract.get_stream(&id);
    assert_eq!(
        after, before,
        "a view call must not modify the stored stream"
    );

    assert_eq!(after.withdrawn, before.withdrawn);
    assert_eq!(after.total_amount, before.total_amount);
    assert_eq!(after.start_time, before.start_time);
    assert_eq!(after.cliff_time, before.cliff_time);
    assert_eq!(after.end_time, before.end_time);
    assert_eq!(after.cancelled, before.cancelled);
}

/// Issue #313 — Every read-only entry point must stay silent on the event
/// stream. An indexer must never record activity that did not really happen.
#[test]
fn test_view_functions_publish_no_events() {
    let t = StreamTest::setup(1_000);
    t.set_time(100);

    let id = t.contract.create_stream(
        &t.sender,
        &t.recipient,
        &t.token_address,
        &1_000,
        &100,
        &1_100,
        &400,
    );

    let _ = t.contract.get_stream(&id);
    assert_eq!(t.event_publishers(), vec![&t.env]);

    let _ = t.contract.withdrawable(&id);
    assert_eq!(t.event_publishers(), vec![&t.env]);

    let _ = t.contract.vested(&id);
    assert_eq!(t.event_publishers(), vec![&t.env]);

    let _ = t.contract.locked(&id);
    assert_eq!(t.event_publishers(), vec![&t.env]);

    let _ = t.contract.progress(&id);
    assert_eq!(t.event_publishers(), vec![&t.env]);

    let _ = t.contract.status(&id);
    assert_eq!(t.event_publishers(), vec![&t.env]);

    let _ = t.contract.stream_count();
    assert_eq!(t.event_publishers(), vec![&t.env]);
}

/// Issue #75 — Third-party view access: read-only functions are public and
/// unauthenticated.
#[test]
fn test_third_party_view_access() {
    use soroban_sdk::testutils::Address as _;
    let t = StreamTest::setup(1_000);
    t.set_time(100);

    let id = t.contract.create_stream(
        &t.sender,
        &t.recipient,
        &t.token_address,
        &1_000,
        &100,
        &1_100,
        &100,
    );

    t.set_time(600);

    let third_party = soroban_sdk::Address::generate(&t.env);
    assert_ne!(third_party, t.sender);
    assert_ne!(third_party, t.recipient);

    let stream = t.contract.get_stream(&id);
    assert_eq!(stream.sender, t.sender);
    assert_eq!(stream.recipient, t.recipient);
    assert_eq!(stream.total_amount, 1_000);

    assert_eq!(t.contract.status(&id), StreamStatus::Streaming);
    assert_eq!(t.contract.withdrawable(&id), 500);
    assert_eq!(t.contract.vested(&id), 500);
    assert_eq!(t.contract.locked(&id), 500);
    assert_eq!(t.contract.progress(&id), 5_000);
    assert_eq!(t.contract.stream_count(), 1);

    let auths = t.env.auths();
    assert!(
        auths.is_empty(),
        "read-only view functions must be unauthenticated and produce empty auth list"
    );
}
