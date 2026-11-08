//! Decides whether a reply may be sent to a source IP right now: at most one
//! per source per window, at most N per second in total, and a capped memory
//! of sources. Pure: the caller passes the time in, so tests need no clock.

use std::collections::HashMap;
use std::net::IpAddr;
use std::time::{Duration, Instant};

// length of the window the global cap counts in
const ONE_SECOND: Duration = Duration::from_secs(1);

/// Rate limiter for replies, per source IP and in total.
pub struct ReplyLimiter {
    per_source: Duration,
    per_second: u32,
    max_sources: usize,
    // source -> when it last got a reply
    last_reply: HashMap<IpAddr, Instant>,
    // start of the current one-second window and replies allowed in it
    second_started: Option<Instant>,
    replies_this_second: u32,
}

impl ReplyLimiter {
    /*
     * per_source: shortest time between two replies to the same source
     * per_second: most replies allowed in any one-second window, all sources together
     * max_sources: most sources remembered; the oldest is dropped past this
     */
    pub fn new(per_source: Duration, per_second: u32, max_sources: usize) -> Self {
        ReplyLimiter {
            per_source: per_source,
            per_second: per_second,
            max_sources: max_sources,
            last_reply: HashMap::new(),
            second_started: None,
            replies_this_second: 0,
        }
    }

    /*
     * says whether one reply to source may go out now, and counts it if so
     *
     * source: the IP the reply would go to
     * now: the current time (an earlier time than before counts as no time passed)
     * returns: true to send, false to stay quiet
     */
    pub fn allow(&mut self, source: IpAddr, now: Instant) -> bool {
        // this source had a reply too recently
        let last = self.last_reply.get(&source);
        match last {
            Some(at) => {
                let since = now.saturating_duration_since(*at);
                if since < self.per_source {
                    return false;
                }
            }
            None => {}
        }

        // start a new one-second window when the last one is over
        let window_over;
        match self.second_started {
            Some(started) => {
                if now.saturating_duration_since(started) >= ONE_SECOND {
                    window_over = true;
                } else {
                    window_over = false;
                }
            }
            None => window_over = true,
        }
        if window_over {
            self.second_started = Some(now);
            self.replies_this_second = 0;
        }

        // the global cap for this second is used up
        if self.replies_this_second >= self.per_second {
            return false;
        }
        self.replies_this_second += 1;

        self.remember(source, now);
        true
    }

    // how many sources are remembered (for tests and logs)
    pub fn sources_remembered(&self) -> usize {
        self.last_reply.len()
    }

    // records the reply time, dropping the oldest source when memory is full
    fn remember(&mut self, source: IpAddr, now: Instant) {
        let is_known = self.last_reply.contains_key(&source);
        if !is_known {
            if self.max_sources == 0 {
                return;
            }
            if self.last_reply.len() >= self.max_sources {
                self.forget_oldest();
            }
        }
        self.last_reply.insert(source, now);
    }

    // removes the source whose last reply is the oldest
    fn forget_oldest(&mut self) {
        let mut oldest: Option<(IpAddr, Instant)> = None;
        for (ip, at) in self.last_reply.iter() {
            match oldest {
                Some((_, oldest_at)) => {
                    if *at < oldest_at {
                        oldest = Some((*ip, *at));
                    }
                }
                None => oldest = Some((*ip, *at)),
            }
        }

        match oldest {
            Some((ip, _)) => {
                self.last_reply.remove(&ip);
            }
            None => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    const WINDOW: Duration = Duration::from_secs(10);

    // the n-th test source, 10.0.x.y
    fn ip(n: u16) -> IpAddr {
        let high = (n / 256) as u8;
        let low = (n % 256) as u8;
        IpAddr::V4(Ipv4Addr::new(10, 0, high, low))
    }

    #[test]
    fn first_reply_to_a_source_is_allowed() {
        let mut limiter = ReplyLimiter::new(WINDOW, 20, 256);
        assert!(limiter.allow(ip(1), Instant::now()));
    }

    #[test]
    fn second_reply_within_window_is_refused() {
        let mut limiter = ReplyLimiter::new(WINDOW, 20, 256);
        let start = Instant::now();
        limiter.allow(ip(1), start);

        assert!(!limiter.allow(ip(1), start + Duration::from_secs(9)));
    }

    #[test]
    fn reply_after_window_is_allowed() {
        let mut limiter = ReplyLimiter::new(WINDOW, 20, 256);
        let start = Instant::now();
        limiter.allow(ip(1), start);

        assert!(limiter.allow(ip(1), start + WINDOW));
    }

    #[test]
    fn refused_reply_does_not_extend_the_window() {
        let mut limiter = ReplyLimiter::new(WINDOW, 20, 256);
        let start = Instant::now();
        limiter.allow(ip(1), start);
        limiter.allow(ip(1), start + Duration::from_secs(5));

        assert!(limiter.allow(ip(1), start + WINDOW));
    }

    #[test]
    fn different_sources_are_limited_separately() {
        let mut limiter = ReplyLimiter::new(WINDOW, 20, 256);
        let now = Instant::now();
        limiter.allow(ip(1), now);

        assert!(limiter.allow(ip(2), now));
        assert!(!limiter.allow(ip(1), now));
    }

    #[test]
    fn global_per_second_cap_applies_across_sources() {
        let mut limiter = ReplyLimiter::new(WINDOW, 3, 256);
        let start = Instant::now();

        assert!(limiter.allow(ip(1), start));
        assert!(limiter.allow(ip(2), start));
        assert!(limiter.allow(ip(3), start + Duration::from_millis(500)));
        assert!(!limiter.allow(ip(4), start + Duration::from_millis(900)));

        // a new second starts a new count
        assert!(limiter.allow(ip(4), start + Duration::from_secs(1)));
    }

    #[test]
    fn refused_by_the_global_cap_does_not_use_up_the_source_window() {
        let mut limiter = ReplyLimiter::new(WINDOW, 1, 256);
        let start = Instant::now();
        limiter.allow(ip(1), start);
        assert!(!limiter.allow(ip(2), start));

        assert!(limiter.allow(ip(2), start + Duration::from_secs(1)));
    }

    #[test]
    fn refused_by_the_source_window_does_not_use_the_global_budget() {
        // ip(1) asks again and again inside its window; ip(2) still gets the one global slot
        let mut limiter = ReplyLimiter::new(WINDOW, 2, 256);
        let now = Instant::now();
        assert!(limiter.allow(ip(1), now));
        for _ in 0..10 {
            assert!(!limiter.allow(ip(1), now));
        }

        assert!(limiter.allow(ip(2), now));
        assert!(
            !limiter.allow(ip(3), now),
            "the global cap of 2 is now used up"
        );
    }

    #[test]
    fn oldest_source_is_dropped_at_max_sources() {
        let mut limiter = ReplyLimiter::new(WINDOW, 100, 3);
        let start = Instant::now();
        limiter.allow(ip(1), start);
        limiter.allow(ip(2), start + Duration::from_millis(1));
        limiter.allow(ip(3), start + Duration::from_millis(2));

        // a fourth source pushes out the oldest, ip(1)
        limiter.allow(ip(4), start + Duration::from_millis(3));

        assert_eq!(limiter.sources_remembered(), 3);
        let later = start + Duration::from_millis(4);
        assert!(
            limiter.allow(ip(1), later),
            "ip(1) was forgotten, so it may get a reply"
        );
        assert!(!limiter.allow(ip(3), later), "ip(3) is still remembered");
    }

    #[test]
    fn memory_never_grows_past_max_sources() {
        let mut limiter = ReplyLimiter::new(WINDOW, 100_000, 256);
        let start = Instant::now();
        for n in 0..1000u16 {
            limiter.allow(ip(n), start + Duration::from_millis(n as u64));
        }
        assert_eq!(limiter.sources_remembered(), 256);
    }

    #[test]
    fn time_going_backwards_does_not_panic() {
        let mut limiter = ReplyLimiter::new(WINDOW, 20, 256);
        let start = Instant::now() + Duration::from_secs(60);
        limiter.allow(ip(1), start);

        // an earlier time counts as 0 s later: still inside the window
        let earlier = start - Duration::from_secs(30);
        assert!(!limiter.allow(ip(1), earlier));
        assert!(limiter.allow(ip(2), earlier));
    }

    #[test]
    fn zero_per_second_refuses_everything() {
        let mut limiter = ReplyLimiter::new(WINDOW, 0, 256);
        assert!(!limiter.allow(ip(1), Instant::now()));
    }

    #[test]
    fn zero_max_sources_still_limits_globally_without_panicking() {
        let mut limiter = ReplyLimiter::new(WINDOW, 1, 0);
        let now = Instant::now();
        assert!(limiter.allow(ip(1), now));
        assert!(!limiter.allow(ip(2), now));
        assert_eq!(limiter.sources_remembered(), 0);
    }
}
