//! Decides what a reporter does about repeated "id in use" replies: warn on
//! the first one, stay quiet while it lasts, and back off once it has lasted
//! past a grace period. A long quiet gap ends the episode. Pure: the caller
//! passes the time in.

use std::time::{Duration, Instant};

/// What to do after one report cycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictStep {
    // no conflict this cycle
    Quiet,
    // first conflict of an episode: print the error once
    Warn,
    // the conflict goes on, but not yet past the grace period: say nothing
    Silent,
    // the conflict lasted past the grace period: print once and pause
    BackOff,
}

/// Tracks one episode of conflicts.
pub struct ConflictTracker {
    grace: Duration,
    max_gap: Duration,
    // first and latest conflict of the current run; None outside a run
    first: Option<Instant>,
    last: Option<Instant>,
    // the Warn line was already printed in this episode
    warned: bool,
}

impl ConflictTracker {
    /*
     * grace: how long conflicts must keep coming before backing off
     * max_gap: a longer quiet time than this ends the episode
     */
    pub fn new(grace: Duration, max_gap: Duration) -> Self {
        ConflictTracker {
            grace: grace,
            max_gap: max_gap,
            first: None,
            last: None,
            warned: false,
        }
    }

    /*
     * records one report cycle
     *
     * conflict: true when this cycle got at least one "id in use" reply
     * now: the current time (an earlier time than before counts as no time passed)
     * returns: what the caller should do (see ConflictStep)
     */
    pub fn on_cycle(&mut self, conflict: bool, now: Instant) -> ConflictStep {
        // the episode is over once the last conflict is older than max_gap
        let mut episode_over = false;
        match self.last {
            Some(last) => {
                let gap = now.saturating_duration_since(last);
                if gap > self.max_gap {
                    episode_over = true;
                }
            }
            None => {}
        }
        if episode_over {
            self.first = None;
            self.last = None;
            self.warned = false;
        }

        if !conflict {
            return ConflictStep::Quiet;
        }

        // the first conflict of a run starts the grace period
        self.last = Some(now);
        let first = match self.first {
            Some(f) => f,
            None => {
                self.first = Some(now);
                now
            }
        };

        let lasted = now.saturating_duration_since(first);
        if lasted >= self.grace {
            return ConflictStep::BackOff;
        }

        if !self.warned {
            self.warned = true;
            return ConflictStep::Warn;
        }
        ConflictStep::Silent
    }

    /*
     * call when reporting starts again after a back-off pause: the grace
     * period counts again from now, and the episode goes on without a new
     * Warn, so a lasting conflict logs one line per back-off
     *
     * now: the time reporting starts again
     */
    pub fn after_back_off(&mut self, now: Instant) {
        self.first = Some(now);
        self.last = Some(now);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GRACE: Duration = Duration::from_secs(16);
    const MAX_GAP: Duration = Duration::from_secs(25);

    // the time `secs` seconds after start
    fn at(start: Instant, secs: u64) -> Instant {
        start + Duration::from_secs(secs)
    }

    fn tracker() -> ConflictTracker {
        ConflictTracker::new(GRACE, MAX_GAP)
    }

    #[test]
    fn no_conflict_is_quiet() {
        let mut t = tracker();
        assert_eq!(t.on_cycle(false, Instant::now()), ConflictStep::Quiet);
    }

    #[test]
    fn single_conflict_warns_once_and_does_not_back_off() {
        let mut t = tracker();
        assert_eq!(t.on_cycle(true, Instant::now()), ConflictStep::Warn);
    }

    #[test]
    fn conflict_lasting_past_grace_backs_off() {
        // a real duplicate: replies at 0, 10 and 20 s (the daemon's rate limit)
        let mut t = tracker();
        let start = Instant::now();

        assert_eq!(t.on_cycle(true, at(start, 0)), ConflictStep::Warn);
        assert_eq!(t.on_cycle(true, at(start, 10)), ConflictStep::Silent);
        assert_eq!(t.on_cycle(true, at(start, 20)), ConflictStep::BackOff);
    }

    #[test]
    fn conflict_reaching_exactly_grace_backs_off() {
        let mut t = tracker();
        let start = Instant::now();
        t.on_cycle(true, at(start, 0));

        assert_eq!(t.on_cycle(true, at(start, 16)), ConflictStep::BackOff);
    }

    #[test]
    fn cycles_without_replies_inside_the_gap_keep_the_run() {
        let mut t = tracker();
        let start = Instant::now();
        t.on_cycle(true, at(start, 0));
        t.on_cycle(false, at(start, 4));
        t.on_cycle(false, at(start, 8));

        assert_eq!(t.on_cycle(true, at(start, 20)), ConflictStep::BackOff);
    }

    #[test]
    fn two_replies_ten_seconds_apart_do_not_back_off() {
        // the IP-change case: refused twice, then accepted once the old IP goes offline
        let mut t = tracker();
        let start = Instant::now();

        assert_eq!(t.on_cycle(true, at(start, 0)), ConflictStep::Warn);
        assert_eq!(t.on_cycle(true, at(start, 10)), ConflictStep::Silent);
        assert_eq!(t.on_cycle(false, at(start, 14)), ConflictStep::Quiet);
    }

    #[test]
    fn gap_longer_than_max_gap_restarts_the_run_and_warns_again() {
        let mut t = tracker();
        let start = Instant::now();
        t.on_cycle(true, at(start, 0));

        // 26 s of quiet ends the episode: the next conflict is new
        assert_eq!(t.on_cycle(false, at(start, 26)), ConflictStep::Quiet);
        assert_eq!(t.on_cycle(true, at(start, 30)), ConflictStep::Warn);
        assert_eq!(t.on_cycle(true, at(start, 40)), ConflictStep::Silent);
        assert_eq!(t.on_cycle(true, at(start, 46)), ConflictStep::BackOff);
    }

    #[test]
    fn conflict_after_a_long_gap_without_a_quiet_cycle_restarts_the_run() {
        let mut t = tracker();
        let start = Instant::now();
        t.on_cycle(true, at(start, 0));

        assert_eq!(t.on_cycle(true, at(start, 26)), ConflictStep::Warn);
    }

    #[test]
    fn gap_of_exactly_max_gap_keeps_the_run() {
        let mut t = tracker();
        let start = Instant::now();
        t.on_cycle(true, at(start, 0));

        assert_eq!(t.on_cycle(true, at(start, 25)), ConflictStep::BackOff);
    }

    #[test]
    fn after_back_off_a_lasting_conflict_backs_off_again_without_a_new_warning() {
        // at most one log line per back-off: the BackOff step itself
        let mut t = tracker();
        let start = Instant::now();
        t.on_cycle(true, at(start, 0));
        t.on_cycle(true, at(start, 10));
        t.on_cycle(true, at(start, 20));

        // 300 s pause, then reporting again
        t.after_back_off(at(start, 320));
        assert_eq!(t.on_cycle(true, at(start, 320)), ConflictStep::Silent);
        assert_eq!(t.on_cycle(true, at(start, 330)), ConflictStep::Silent);
        assert_eq!(t.on_cycle(true, at(start, 340)), ConflictStep::BackOff);
    }

    #[test]
    fn after_back_off_a_conflict_that_went_away_ends_the_episode() {
        let mut t = tracker();
        let start = Instant::now();
        t.on_cycle(true, at(start, 0));
        t.on_cycle(true, at(start, 20));
        t.after_back_off(at(start, 320));

        // no replies for longer than the gap: the clash is over
        assert_eq!(t.on_cycle(false, at(start, 346)), ConflictStep::Quiet);
        // a later clash warns again
        assert_eq!(t.on_cycle(true, at(start, 400)), ConflictStep::Warn);
    }

    #[test]
    fn time_going_backwards_does_not_panic() {
        let mut t = tracker();
        let start = Instant::now() + Duration::from_secs(60);
        t.on_cycle(true, start);

        let earlier = start - Duration::from_secs(30);
        assert_eq!(t.on_cycle(true, earlier), ConflictStep::Silent);
        assert_eq!(t.on_cycle(false, earlier), ConflictStep::Quiet);
    }
}
