//! Performance samples for the benchmarks: key latency, startup and file
//! loading, measured inside the app and read over the control socket
//! (ADR 0025, ADR 0046). Dev builds only.
//!
//! Must not depend on: `app`, winit or the GPU; the app tells it what
//! happened, and it only keeps the numbers. It holds no buffer text.

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

/// The most key samples kept until they are read, and the most keys waiting
/// for a frame. More are counted as dropped, so a long dev session cannot
/// grow without limit. The limit also keeps a response under 1 MiB.
pub const MAX_KEY_SAMPLES: usize = 16384;

/// The app's timings. The app owns one and updates it on the UI thread.
#[derive(Debug)]
pub struct Timings {
    started: Instant,
    frames: u64,
    first_frame: Option<FirstFrame>,
    /// When each key still waiting for a frame was received.
    waiting_keys: Vec<Instant>,
    key_latency_us: Vec<u32>,
    dropped_keys: u64,
    load: LoadTimes,
}

/// The first presented frame. A frame skipped while the window is hidden
/// does not count.
#[derive(Debug, Clone, Copy)]
struct FirstFrame {
    after_start: Duration,
    /// As wall-clock time, so that a benchmark that started the process can
    /// include the time before `main`.
    unix: Duration,
}

/// The first file load of this process.
#[derive(Debug, Default)]
struct LoadTimes {
    /// When that load started, once its first content has arrived.
    began: Option<Instant>,
    /// From the load's start until its first content reached the UI thread.
    first_content: Option<Duration>,
    /// From the load's start until the first complete screen of it was
    /// presented, with every glyph drawn.
    first_screen: Option<Duration>,
    /// From process start until that screen was presented.
    first_screen_after_start: Option<Duration>,
    /// From the load's start until all of it reached the UI thread.
    done: Option<Duration>,
}

impl Timings {
    /// Timings of a process whose `main` began at `started`.
    pub fn new(started: Instant) -> Self {
        Self {
            started,
            frames: 0,
            first_frame: None,
            waiting_keys: Vec::new(),
            key_latency_us: Vec::new(),
            dropped_keys: 0,
            load: LoadTimes::default(),
        }
    }

    /// `count` keys received at `received` were handled. Only keys that
    /// need a frame have a frame to wait for.
    pub fn keys_handled(&mut self, received: Instant, count: usize, redraw: bool) {
        if !redraw {
            return;
        }
        for _ in 0..count {
            if self.waiting_keys.len() < MAX_KEY_SAMPLES {
                self.waiting_keys.push(received);
            } else {
                self.dropped_keys += 1;
            }
        }
    }

    /// The first content of the load that began at `began` arrived.
    pub fn load_first_content(&mut self, began: Instant) {
        if self.load.began.is_none() {
            self.load.began = Some(began);
            self.load.first_content = Some(began.elapsed());
        }
    }

    /// The load that began at `began` finished.
    pub fn load_done(&mut self, began: Instant) {
        if self.load.began == Some(began) {
            self.load.done.get_or_insert_with(|| began.elapsed());
        }
    }

    /// A frame was presented, reflecting every key handled so far. `complete`
    /// is false if glyphs were left for the next frame.
    pub fn frame_presented(&mut self, complete: bool) {
        self.frames += 1;
        self.first_frame.get_or_insert_with(|| FirstFrame {
            after_start: self.started.elapsed(),
            unix: unix_now(),
        });
        for received in self.waiting_keys.drain(..) {
            if self.key_latency_us.len() < MAX_KEY_SAMPLES {
                let micros = received.elapsed().as_micros();
                self.key_latency_us
                    .push(u32::try_from(micros).unwrap_or(u32::MAX));
            } else {
                self.dropped_keys += 1;
            }
        }
        if complete
            && self.load.first_screen.is_none()
            && let Some(began) = self.load.began
        {
            self.load.first_screen = Some(began.elapsed());
            self.load.first_screen_after_start = Some(self.started.elapsed());
        }
    }

    /// The result of `take-timings`. The key samples are handed over, so
    /// the next call reports only newer ones; everything else stays.
    pub fn take(&mut self) -> Value {
        let keys = std::mem::take(&mut self.key_latency_us);
        let dropped = std::mem::take(&mut self.dropped_keys);
        json!({
            "startup_us": self.first_frame.map(|frame| micros(frame.after_start)),
            "first_frame_unix_us": self.first_frame.map(|frame| micros(frame.unix)),
            "frames": self.frames,
            "keys_waiting": self.waiting_keys.len(),
            "key_latency_us": keys,
            "dropped_keys": dropped,
            "load": {
                "first_content_us": self.load.first_content.map(micros),
                "first_screen_us": self.load.first_screen.map(micros),
                "first_screen_after_start_us": self.load.first_screen_after_start.map(micros),
                "done_us": self.load.done.map(micros),
            },
        })
    }
}

fn micros(duration: Duration) -> u64 {
    u64::try_from(duration.as_micros()).unwrap_or(u64::MAX)
}

fn unix_now() -> Duration {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::{MAX_KEY_SAMPLES, Timings};
    use serde_json::{Value, json};
    use std::time::{Duration, Instant};

    fn timings() -> Timings {
        Timings::new(Instant::now())
    }

    #[test]
    fn a_key_is_sampled_at_the_first_frame_after_it() {
        let mut timings = timings();
        let received = Instant::now();
        timings.keys_handled(received, 1, true);
        assert_eq!(timings.take()["keys_waiting"], 1);
        std::thread::sleep(Duration::from_millis(2));
        timings.frame_presented(true);
        let result = timings.take();
        assert_eq!(result["keys_waiting"], 0);
        let samples = result["key_latency_us"].as_array().unwrap();
        assert_eq!(samples.len(), 1);
        assert!(samples[0].as_u64().unwrap() >= 2000, "{samples:?}");
    }

    #[test]
    fn a_later_frame_does_not_sample_a_key_again() {
        let mut timings = timings();
        timings.keys_handled(Instant::now(), 1, true);
        timings.frame_presented(true);
        timings.frame_presented(true);
        assert_eq!(
            timings.take()["key_latency_us"].as_array().unwrap().len(),
            1
        );
    }

    #[test]
    fn keys_without_a_redraw_have_no_sample() {
        let mut timings = timings();
        timings.keys_handled(Instant::now(), 3, false);
        timings.frame_presented(true);
        assert_eq!(timings.take()["key_latency_us"], json!([]));
    }

    #[test]
    fn take_hands_over_samples_but_keeps_the_rest() {
        let mut timings = timings();
        timings.keys_handled(Instant::now(), 2, true);
        timings.frame_presented(true);
        let first = timings.take();
        assert_eq!(first["key_latency_us"].as_array().unwrap().len(), 2);
        let second = timings.take();
        assert_eq!(second["key_latency_us"], json!([]));
        assert_eq!(second["startup_us"], first["startup_us"]);
        assert_eq!(second["frames"], 1);
    }

    #[test]
    fn startup_waits_for_a_presented_frame() {
        let mut timings = timings();
        assert_eq!(timings.take()["startup_us"], Value::Null);
        timings.frame_presented(true);
        let first = timings.take();
        assert!(first["startup_us"].is_u64());
        assert!(first["first_frame_unix_us"].as_u64().unwrap() > 0);
        std::thread::sleep(Duration::from_millis(2));
        timings.frame_presented(true);
        let second = timings.take();
        assert_eq!(second["startup_us"], first["startup_us"], "only the first");
    }

    #[test]
    fn samples_and_waiting_keys_are_bounded() {
        let mut timings = timings();
        timings.keys_handled(Instant::now(), MAX_KEY_SAMPLES + 5, true);
        assert_eq!(timings.waiting_keys.len(), MAX_KEY_SAMPLES);
        timings.frame_presented(true);
        timings.keys_handled(Instant::now(), 2, true);
        timings.frame_presented(true);
        let result = timings.take();
        assert_eq!(
            result["key_latency_us"].as_array().unwrap().len(),
            MAX_KEY_SAMPLES
        );
        assert_eq!(result["dropped_keys"], 7);
        assert_eq!(timings.take()["dropped_keys"], 0);
        // Even the largest samples fit in a response line.
        let line = json!(vec![u32::MAX; MAX_KEY_SAMPLES]).to_string();
        assert!(
            line.len() < crate::dev::protocol::MAX_LINE,
            "{}",
            line.len()
        );
    }

    #[test]
    fn the_first_screen_ends_when_it_is_complete() {
        let mut timings = timings();
        // A frame before the content arrived is not part of the first screen.
        timings.frame_presented(true);
        assert_eq!(timings.take()["load"]["first_screen_us"], Value::Null);
        let began = Instant::now();
        timings.load_first_content(began);
        timings.frame_presented(false);
        assert_eq!(timings.take()["load"]["first_screen_us"], Value::Null);
        std::thread::sleep(Duration::from_millis(2));
        timings.frame_presented(true);
        let load = timings.take()["load"].clone();
        let screen = load["first_screen_us"].clone();
        assert!(screen.as_u64().unwrap() >= 2000, "{load}");
        assert!(screen.as_u64() >= load["first_content_us"].as_u64());
        assert!(load["first_screen_after_start_us"].as_u64() >= screen.as_u64());
        // Measured once.
        std::thread::sleep(Duration::from_millis(2));
        timings.frame_presented(true);
        assert_eq!(timings.take()["load"]["first_screen_us"], screen);
    }

    #[test]
    fn only_the_first_load_is_timed() {
        let mut timings = timings();
        let first = Instant::now();
        std::thread::sleep(Duration::from_millis(2));
        let second = Instant::now();
        timings.load_first_content(first);
        timings.load_first_content(second);
        timings.load_done(second);
        assert_eq!(timings.take()["load"]["done_us"], Value::Null);
        timings.load_done(first);
        let load = timings.take()["load"].clone();
        assert!(load["first_content_us"].as_u64().unwrap() >= 2000, "{load}");
        assert!(load["done_us"].as_u64().unwrap() >= 2000, "{load}");
    }
}
