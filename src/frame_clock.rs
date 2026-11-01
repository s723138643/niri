use std::num::NonZeroU64;
use std::time::Duration;

use crate::utils::get_monotonic_time;

#[derive(Debug)]
pub struct FrameClock {
    last_presentation_time: Option<Duration>,
    refresh_interval_ns: Option<NonZeroU64>,
    vrr: bool,
}

impl FrameClock {
    pub fn new(refresh_interval: Option<Duration>, vrr: bool) -> Self {
        let refresh_interval_ns = if let Some(interval) = &refresh_interval {
            assert_eq!(interval.as_secs(), 0);
            Some(NonZeroU64::new(interval.subsec_nanos().into()).unwrap())
        } else {
            None
        };

        Self {
            last_presentation_time: None,
            refresh_interval_ns,
            vrr,
        }
    }

    pub fn refresh_interval(&self) -> Option<Duration> {
        self.refresh_interval_ns
            .map(|r| Duration::from_nanos(r.get()))
    }

    pub fn set_vrr(&mut self, vrr: bool) {
        if self.vrr == vrr {
            return;
        }

        self.vrr = vrr;
        self.last_presentation_time = None;
    }

    pub fn vrr(&self) -> bool {
        self.vrr
    }

    pub fn presented(&mut self, presentation_time: Duration) {
        if presentation_time.is_zero() {
            // Not interested in these.
            return;
        }

        self.last_presentation_time = Some(presentation_time);
    }

    pub fn prev_presentation_time(&self, t: Duration) -> Duration {
        let Some(interval) = self
            .refresh_interval_ns
            .map(|r| Duration::from_nanos(r.get()))
        else {
            return t;
        };

        let next = self.presentation_time_after(t);
        let prev = next.saturating_sub(interval);

        if prev >= t {
            prev.saturating_sub(interval)
        } else {
            prev
        }
    }

    pub fn next_presentation_time(&self) -> Duration {
        self.presentation_time_after(get_monotonic_time())
    }

    /// Returns the next presentation time after `t`.
    ///
    /// In the normal (vblank grid) case this is strictly after `t`: a frame scheduled at `t`
    /// cannot make the presentation at `t`. If the clock has no vblank grid (no refresh
    /// interval, or nothing presented yet), returns `t` unchanged. With VRR, once a full
    /// refresh interval has passed since the last presentation, returns a time that can be
    /// presented immediately.
    pub fn presentation_time_after(&self, mut t: Duration) -> Duration {
        let Some(refresh_interval_ns) = self.refresh_interval_ns else {
            return t;
        };
        let Some(last_presentation_time) = self.last_presentation_time else {
            return t;
        };

        let refresh_interval_ns = refresh_interval_ns.get();

        if t <= last_presentation_time {
            // Got an early VBlank.
            let orig_t = t;
            t += Duration::from_nanos(refresh_interval_ns);

            if t < last_presentation_time {
                // Not sure when this can happen.
                error!(
                    t = ?orig_t,
                    ?last_presentation_time,
                    "got a 2+ early VBlank, {:?} until presentation",
                    last_presentation_time - t,
                );
                t = last_presentation_time + Duration::from_nanos(refresh_interval_ns);
            }
        }

        let since_last = t - last_presentation_time;
        let since_last_ns =
            since_last.as_secs() * 1_000_000_000 + u64::from(since_last.subsec_nanos());
        let to_next_ns = (since_last_ns / refresh_interval_ns + 1) * refresh_interval_ns;

        // If VRR is enabled and more than one frame passed since last presentation, assume that we
        // can present immediately.
        if self.vrr && to_next_ns > refresh_interval_ns {
            t
        } else {
            last_presentation_time + Duration::from_nanos(to_next_ns)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const INTERVAL: Duration = Duration::from_millis(16);

    fn ms(ms: u64) -> Duration {
        Duration::from_millis(ms)
    }

    /// A clock with a 16ms refresh interval and a presentation at 100ms, so the vblank grid is
    /// 100ms, 116ms, 132ms, ...
    fn clock(vrr: bool) -> FrameClock {
        let mut clock = FrameClock::new(Some(INTERVAL), vrr);
        clock.presented(ms(100));
        clock
    }

    #[test]
    fn presentation_time_after_without_refresh_interval() {
        let clock = FrameClock::new(None, false);
        assert_eq!(clock.presentation_time_after(ms(123)), ms(123));
    }

    #[test]
    fn presentation_time_after_before_first_presentation() {
        let clock = FrameClock::new(Some(INTERVAL), false);
        assert_eq!(clock.presentation_time_after(ms(123)), ms(123));
    }

    #[test]
    fn presentation_time_after_returns_the_next_vblank() {
        let clock = clock(false);

        assert_eq!(clock.presentation_time_after(ms(101)), ms(116));
        assert_eq!(clock.presentation_time_after(ms(115)), ms(116));
        assert_eq!(clock.presentation_time_after(ms(117)), ms(132));
        assert_eq!(clock.presentation_time_after(ms(200)), ms(212));
    }

    #[test]
    fn presentation_time_after_is_strictly_after_the_argument() {
        let clock = clock(false);

        // When the argument lands exactly on a vblank, the "early VBlank" handling resolves to
        // the vblank after the next one instead of the argument itself.
        assert_eq!(clock.presentation_time_after(ms(116)), ms(132));
        assert_eq!(clock.presentation_time_after(ms(100)), ms(132));

        // Less than one interval before the last presentation time, one interval is added
        // before looking for the next vblank.
        assert_eq!(clock.presentation_time_after(ms(99)), ms(116));
    }

    #[test]
    fn presentation_time_after_with_vrr_presents_immediately() {
        let clock = clock(true);

        // Less than one refresh interval since the last presentation: wait for the next vblank.
        assert_eq!(clock.presentation_time_after(ms(101)), ms(116));
        assert_eq!(clock.presentation_time_after(ms(115)), ms(116));

        // One or more full intervals: assume that we can present immediately.
        assert_eq!(clock.presentation_time_after(ms(116)), ms(116));
        assert_eq!(clock.presentation_time_after(ms(150)), ms(150));
    }

    #[test]
    fn prev_presentation_time_without_refresh_interval() {
        let clock = FrameClock::new(None, false);
        assert_eq!(clock.prev_presentation_time(ms(123)), ms(123));
    }

    #[test]
    fn prev_presentation_time_returns_the_vblank_before_the_deadline() {
        let clock = clock(false);

        // Between vblanks, the previous vblank.
        assert_eq!(clock.prev_presentation_time(ms(101)), ms(100));
        assert_eq!(clock.prev_presentation_time(ms(115)), ms(100));
        assert_eq!(clock.prev_presentation_time(ms(117)), ms(116));
        assert_eq!(clock.prev_presentation_time(ms(131)), ms(116));
        assert_eq!(clock.prev_presentation_time(ms(201)), ms(196));

        // On a vblank, the vblank before it, so that a frame for the vblank at `t` can still be
        // rendered and queued in time.
        assert_eq!(clock.prev_presentation_time(ms(116)), ms(100));
        assert_eq!(clock.prev_presentation_time(ms(132)), ms(116));
    }

    #[test]
    fn prev_presentation_time_with_vrr() {
        let clock = clock(true);

        // Within one interval of the last presentation, still the previous vblank.
        assert_eq!(clock.prev_presentation_time(ms(115)), ms(100));
        assert_eq!(clock.prev_presentation_time(ms(116)), ms(100));

        // Further out, VRR assumes that we can present immediately, so the wakeup lands one
        // refresh interval before the deadline.
        assert_eq!(clock.prev_presentation_time(ms(150)), ms(134));
    }
}
