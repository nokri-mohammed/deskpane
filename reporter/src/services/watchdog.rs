//! Runs one action after a delay on a background thread, whatever the main
//! thread is doing at that moment (for example blocked on a read or a name
//! lookup). Used to give a short-lived process a hard deadline.

use std::thread;
use std::time::Duration;

/*
 * starts a background thread that sleeps for delay, then calls on_expire
 *
 * delay: how long to wait
 * on_expire: what to do then, e.g. exit the process
 */
pub fn start_watchdog(delay: Duration, on_expire: fn()) {
    thread::spawn(move || {
        thread::sleep(delay);
        on_expire();
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    static FIRED: AtomicBool = AtomicBool::new(false);
    static EARLY: AtomicBool = AtomicBool::new(false);

    fn mark_fired() {
        FIRED.store(true, Ordering::SeqCst);
    }

    fn mark_early() {
        EARLY.store(true, Ordering::SeqCst);
    }

    #[test]
    fn action_runs_after_the_delay() {
        start_watchdog(Duration::from_millis(50), mark_fired);

        thread::sleep(Duration::from_millis(400));

        assert!(FIRED.load(Ordering::SeqCst));
    }

    #[test]
    fn action_does_not_run_before_the_delay() {
        start_watchdog(Duration::from_secs(30), mark_early);

        thread::sleep(Duration::from_millis(50));

        assert!(!EARLY.load(Ordering::SeqCst));
    }
}
