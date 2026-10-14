use super::wait_for_renewal;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

#[tokio::test(start_paused = true)]
async fn renewal_waits_until_deadline_and_cancellation_interrupts_the_timer() {
    let shutdown = CancellationToken::new();
    let timer = wait_for_renewal(600, 0, &shutdown);
    tokio::pin!(timer);
    assert!(
        tokio::time::timeout(Duration::from_secs(599), &mut timer)
            .await
            .is_err()
    );
    tokio::time::advance(Duration::from_secs(1)).await;
    assert!(!timer.await);
    let timer = wait_for_renewal(600, 0, &shutdown);
    tokio::pin!(timer);
    assert!(
        tokio::time::timeout(Duration::from_secs(1), &mut timer)
            .await
            .is_err()
    );
    shutdown.cancel();
    assert!(
        tokio::time::timeout(Duration::from_secs(1), timer)
            .await
            .expect("cancellation")
    );
}

#[tokio::test(start_paused = true)]
async fn overdue_certificate_does_not_wait() {
    assert!(!wait_for_renewal(10, 11, &CancellationToken::new()).await);
}

#[test]
fn renewal_deadline_is_two_thirds_of_actual_lifetime() {
    for (start, end, expected) in [
        (100, 100 + 7 * 86400, 100 + 7 * 86400 * 2 / 3),
        (100, 100 + 30 * 86400, 100 + 30 * 86400 * 2 / 3),
        (100, 110, 106),
    ] {
        assert_eq!(
            super::validate::renewal_at(start, end).expect("valid lifetime"),
            expected
        );
    }
    assert!(super::validate::renewal_at(5, 5).is_err());
    assert!(super::validate::renewal_at(5, 4).is_err());
    assert!(super::validate::renewal_at(i64::MIN, i64::MAX).is_err());
}
