//! Performance samples for the benchmarks: key latency, startup and file
//! loading, measured inside the app and read over the control socket
//! (ADR 0025, ADR 0046). Dev builds only.
//!
//! Must not depend on: `app`, winit or the GPU; the app tells it what
//! happened, and it only keeps the numbers. It holds no buffer text, and while a
//! buffer that may hold secrets is shown, no keys and no frames but the first one
//! (ADR 0060). It times no load of such a buffer, and no first screen drawn while one
//! is shown (ADR 0078).

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use oso_core::editor::Editor;
use serde_json::{Value, json};

/// The most key samples kept until they are read, and the most keys waiting
/// for a frame. More are counted as dropped, so a long dev session cannot
/// grow without limit. The limit also keeps a response under 1 MiB.
pub const MAX_KEY_SAMPLES: usize = 16384;

/// What a key did, for the timings.
#[derive(Debug, Clone, Copy)]
pub struct HandledKey {
    /// It needs a frame, so it has one to wait for.
    pub redraw: bool,
    /// A buffer that may hold secrets was shown around it, says [`SecretWatch`].
    pub withheld: bool,
}

/// Says of each key in turn whether a buffer that may hold secrets was shown just
/// before or just after it (ADR 0060). Every way in for keys goes through this.
#[derive(Debug)]
pub struct SecretWatch {
    shown: bool,
}

impl SecretWatch {
    /// Starts watching before a key.
    pub fn new(editor: &Editor) -> Self {
        Self {
            shown: editor.shows_secret(),
        }
    }

    /// Whether one was shown around the key just handled.
    pub fn after_key(&mut self, editor: &Editor) -> bool {
        let now = editor.shows_secret();
        let around = self.shown || now;
        self.shown = now;
        around
    }
}

/// Where the frame being presented spent its time, from when it started: the app tells
/// these just before [`Timings::frame_presented`], so that each key it shows can say where
/// its own time went.
#[derive(Debug, Clone, Copy)]
pub struct FrameSpans {
    /// When the app began the frame, and so took in what the loads had, and when it
    /// handed the frame to the renderer.
    pub started: Instant,
    pub rendering: Instant,
    /// From `rendering`: prepared on the CPU, given its drawable, and presented.
    pub prepared: Duration,
    pub acquired: Duration,
    pub presented: Duration,
}

/// The app's timings. The app owns one and updates it on the UI thread.
#[derive(Debug)]
pub struct Timings {
    started: Instant,
    frames: u64,
    first_frame: Option<FirstFrame>,
    /// When each key still waiting for a frame was received, and when it was handled.
    waiting_keys: Vec<(Instant, Instant)>,
    key_latency_us: Vec<u32>,
    /// For each key sampled, where its latency went, in microseconds: received to handled,
    /// handled to the frame's start, taking in the loads and the status before rendering,
    /// then the renderer's preparing, waiting for its drawable, and presenting. A sample
    /// each, taken by `take` with `key_latency_us`.
    key_spans_us: Vec<[u32; 6]>,
    /// The spans of the frame about to be reported presented.
    frame_spans: Option<FrameSpans>,
    dropped_keys: u64,
    load: LoadTimes,
    /// How far a review's reading was in the last text put in, until a frame shows it.
    review_text: Option<ReviewText>,
    /// Each frame that showed a review's new text, as wall-clock time, for the review
    /// benchmark (ADR 0122). Taken by `take`.
    review_frames: Vec<(u64, ReviewText)>,
}

/// How far a review's reading was in a text put in: files read of those listed, or done.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReviewText {
    pub read: usize,
    pub listed: usize,
    pub done: bool,
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

/// The first load of this process of a file that cannot hold secrets, which the app
/// says by telling of no other (ADR 0078).
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
    /// Its first complete screen came while a buffer that may hold secrets was shown, so it
    /// has none (ADR 0078).
    screen_withheld: bool,
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
            key_spans_us: Vec::new(),
            frame_spans: None,
            dropped_keys: 0,
            load: LoadTimes::default(),
            review_text: None,
            review_frames: Vec::new(),
        }
    }

    /// A key received at `received` was handled. Only a key that needs a frame has
    /// one to wait for. A withheld key leaves nothing, and drops the keys still
    /// waiting, so that no sample spans it. Neither counts as dropped, which would say
    /// how many there were (ADR 0060).
    pub fn key_handled(&mut self, received: Instant, key: HandledKey) {
        if key.withheld {
            self.waiting_keys.clear();
            return;
        }
        if !key.redraw {
            return;
        }
        if self.waiting_keys.len() < MAX_KEY_SAMPLES {
            self.waiting_keys.push((received, Instant::now()));
        } else {
            self.dropped_keys += 1;
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

    /// A review's new text went in, its reading as far as `text` says.
    pub fn review_text(&mut self, text: ReviewText) {
        self.review_text = Some(text);
    }

    /// Where the frame about to be reported presented spent its time.
    pub fn frame_spans(&mut self, spans: FrameSpans) {
        self.frame_spans = Some(spans);
    }

    /// A frame was presented, reflecting every key handled so far. `complete`
    /// is false if glyphs were left for the next frame. `withheld` says a buffer that
    /// may hold secrets is shown: the frame then counts only as the first one, not in
    /// `frames`, and keys waiting for it leave no sample (ADR 0060). If it is the load's
    /// first complete screen, the load has none (ADR 0078).
    pub fn frame_presented(&mut self, complete: bool, withheld: bool) {
        self.first_frame.get_or_insert_with(|| FirstFrame {
            after_start: self.started.elapsed(),
            unix: unix_now(),
        });
        if complete
            && self.load.first_screen.is_none()
            && !self.load.screen_withheld
            && let Some(began) = self.load.began
        {
            if withheld {
                self.load.screen_withheld = true;
            } else {
                self.load.first_screen = Some(began.elapsed());
                self.load.first_screen_after_start = Some(self.started.elapsed());
            }
        }
        let spans = self.frame_spans.take();
        if let Some(text) = self.review_text.take()
            && self.review_frames.len() < MAX_KEY_SAMPLES
        {
            self.review_frames.push((micros(unix_now()), text));
        }
        if withheld {
            self.waiting_keys.clear();
            return;
        }
        self.frames += 1;
        for (received, handled) in self.waiting_keys.drain(..) {
            if self.key_latency_us.len() < MAX_KEY_SAMPLES {
                self.key_latency_us.push(micros_u32(received.elapsed()));
                if let Some(frame) = spans {
                    self.key_spans_us.push([
                        micros_u32(handled.saturating_duration_since(received)),
                        micros_u32(frame.started.saturating_duration_since(handled)),
                        micros_u32(frame.rendering.saturating_duration_since(frame.started)),
                        micros_u32(frame.prepared),
                        micros_u32(frame.acquired.saturating_sub(frame.prepared)),
                        micros_u32(frame.presented.saturating_sub(frame.acquired)),
                    ]);
                }
            } else {
                self.dropped_keys += 1;
            }
        }
    }

    /// The result of `take-timings`. The key samples are handed over, so
    /// the next call reports only newer ones; everything else stays.
    pub fn take(&mut self) -> Value {
        let keys = std::mem::take(&mut self.key_latency_us);
        let spans = std::mem::take(&mut self.key_spans_us);
        let dropped = std::mem::take(&mut self.dropped_keys);
        let reviews: Vec<Value> = std::mem::take(&mut self.review_frames)
            .into_iter()
            .map(|(at, text)| {
                json!({ "unix_us": at, "read": text.read, "listed": text.listed, "done": text.done })
            })
            .collect();
        json!({
            "startup_us": self.first_frame.map(|frame| micros(frame.after_start)),
            "first_frame_unix_us": self.first_frame.map(|frame| micros(frame.unix)),
            "frames": self.frames,
            "keys_waiting": self.waiting_keys.len(),
            "key_latency_us": keys,
            "key_spans_us": spans,
            "dropped_keys": dropped,
            "load": {
                "first_content_us": self.load.first_content.map(micros),
                "first_screen_us": self.load.first_screen.map(micros),
                "first_screen_after_start_us": self.load.first_screen_after_start.map(micros),
                "done_us": self.load.done.map(micros),
            },
            "review_frames": reviews,
        })
    }
}

fn micros_u32(duration: Duration) -> u32 {
    u32::try_from(duration.as_micros()).unwrap_or(u32::MAX)
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
    use super::{FrameSpans, HandledKey, MAX_KEY_SAMPLES, Timings};
    use serde_json::{Value, json};
    use std::time::{Duration, Instant};

    fn timings() -> Timings {
        Timings::new(Instant::now())
    }

    fn key(redraw: bool, withheld: bool) -> HandledKey {
        HandledKey { redraw, withheld }
    }

    #[test]
    fn a_review_text_is_marked_at_the_frame_that_shows_it_and_taken_once() {
        let mut timings = timings();
        timings.frame_presented(true, false);
        let text = super::ReviewText {
            read: 2,
            listed: 5,
            done: false,
        };
        timings.review_text(text);
        timings.frame_presented(true, false);
        timings.frame_presented(true, false);
        let taken = timings.take();
        let frames = taken["review_frames"].as_array().unwrap();
        assert_eq!(frames.len(), 1, "one frame showed it");
        assert_eq!(frames[0]["read"], 2);
        assert_eq!(frames[0]["listed"], 5);
        assert_eq!(frames[0]["done"], false);
        assert!(frames[0]["unix_us"].as_u64().unwrap() > 0);
        assert_eq!(timings.take()["review_frames"], json!([]));
    }

    #[test]
    fn a_key_is_sampled_at_the_first_frame_after_it() {
        let mut timings = timings();
        let received = Instant::now();
        timings.key_handled(received, key(true, false));
        assert_eq!(timings.take()["keys_waiting"], 1);
        std::thread::sleep(Duration::from_millis(2));
        timings.frame_presented(true, false);
        let result = timings.take();
        assert_eq!(result["keys_waiting"], 0);
        let samples = result["key_latency_us"].as_array().unwrap();
        assert_eq!(samples.len(), 1);
        assert!(samples[0].as_u64().unwrap() >= 2000, "{samples:?}");
    }

    #[test]
    fn a_sampled_key_says_where_its_time_went_and_a_withheld_frame_says_nothing() {
        let mut timings = timings();
        let received = Instant::now();
        timings.key_handled(received, key(true, false));
        std::thread::sleep(Duration::from_millis(2));
        let started = Instant::now();
        let spans = FrameSpans {
            started,
            rendering: started + Duration::from_micros(300),
            prepared: Duration::from_micros(400),
            acquired: Duration::from_micros(1400),
            presented: Duration::from_micros(1500),
        };
        timings.frame_spans(spans);
        timings.frame_presented(true, false);
        let taken = timings.take();
        let sample = &taken["key_spans_us"][0];
        assert!(
            sample[1].as_u64().unwrap() >= 2000,
            "waited for the frame: {sample}"
        );
        assert_eq!(sample.as_array().unwrap()[2..], [300, 400, 1000, 100]);
        assert_eq!(timings.take()["key_spans_us"], json!([]), "taken");
        timings.key_handled(Instant::now(), key(true, false));
        timings.frame_spans(spans);
        timings.frame_presented(true, true);
        assert_eq!(timings.take()["key_spans_us"], json!([]));
    }

    #[test]
    fn a_later_frame_does_not_sample_a_key_again() {
        let mut timings = timings();
        timings.key_handled(Instant::now(), key(true, false));
        timings.frame_presented(true, false);
        timings.frame_presented(true, false);
        assert_eq!(
            timings.take()["key_latency_us"].as_array().unwrap().len(),
            1
        );
    }

    #[test]
    fn a_withheld_frame_counts_nothing_but_the_first_one() {
        let mut timings = timings();
        let began = Instant::now();
        timings.load_first_content(began);
        timings.key_handled(Instant::now(), key(true, false));
        timings.frame_presented(true, true);
        timings.key_handled(Instant::now(), key(true, true));
        let taken = timings.take();
        assert_eq!(taken["frames"], 0);
        assert_eq!(taken["keys_waiting"], 0, "the key leaves no sample");
        assert_eq!(taken["key_latency_us"], json!([]));
        assert!(taken["startup_us"].as_u64().is_some(), "the first frame");
        assert_eq!(taken["load"]["first_screen_us"], Value::Null);
        timings.key_handled(Instant::now(), key(true, false));
        timings.frame_presented(true, false);
        let taken = timings.take();
        assert_eq!(taken["frames"], 1, "once it is no longer shown");
        assert_eq!(taken["key_latency_us"].as_array().map(Vec::len), Some(1));
        let load = &taken["load"];
        assert_eq!(load["first_screen_us"], Value::Null, "nor later");
        assert_eq!(load["first_screen_after_start_us"], Value::Null);
        assert!(load["first_content_us"].as_u64().is_some());
    }

    #[test]
    fn a_withheld_key_drops_those_still_waiting_even_with_no_frame_of_its_own() {
        let mut timings = timings();
        timings.key_handled(Instant::now(), key(true, false));
        timings.key_handled(Instant::now(), key(false, true));
        let taken = timings.take();
        assert_eq!(taken["keys_waiting"], 0);
        assert_eq!(taken["dropped_keys"], 0, "it is not counted either");
    }

    #[test]
    fn keys_without_a_redraw_have_no_sample() {
        let mut timings = timings();
        for _ in 0..3 {
            timings.key_handled(Instant::now(), key(false, false));
        }
        timings.frame_presented(true, false);
        assert_eq!(timings.take()["key_latency_us"], json!([]));
    }

    #[test]
    fn take_hands_over_samples_but_keeps_the_rest() {
        let mut timings = timings();
        for _ in 0..2 {
            timings.key_handled(Instant::now(), key(true, false));
        }
        timings.frame_presented(true, false);
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
        timings.frame_presented(true, false);
        let first = timings.take();
        assert!(first["startup_us"].is_u64());
        assert!(first["first_frame_unix_us"].as_u64().unwrap() > 0);
        std::thread::sleep(Duration::from_millis(2));
        timings.frame_presented(true, false);
        let second = timings.take();
        assert_eq!(second["startup_us"], first["startup_us"], "only the first");
    }

    #[test]
    fn samples_and_waiting_keys_are_bounded() {
        let mut timings = timings();
        for _ in 0..MAX_KEY_SAMPLES + 5 {
            timings.key_handled(Instant::now(), key(true, false));
        }
        assert_eq!(timings.waiting_keys.len(), MAX_KEY_SAMPLES);
        timings.frame_presented(true, false);
        for _ in 0..2 {
            timings.key_handled(Instant::now(), key(true, false));
        }
        timings.frame_presented(true, false);
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
        timings.frame_presented(true, false);
        assert_eq!(timings.take()["load"]["first_screen_us"], Value::Null);
        let began = Instant::now();
        timings.load_first_content(began);
        timings.frame_presented(false, false);
        assert_eq!(timings.take()["load"]["first_screen_us"], Value::Null);
        std::thread::sleep(Duration::from_millis(2));
        timings.frame_presented(true, false);
        let load = timings.take()["load"].clone();
        let screen = load["first_screen_us"].clone();
        assert!(screen.as_u64().unwrap() >= 2000, "{load}");
        assert!(screen.as_u64() >= load["first_content_us"].as_u64());
        assert!(load["first_screen_after_start_us"].as_u64() >= screen.as_u64());
        // Measured once.
        std::thread::sleep(Duration::from_millis(2));
        timings.frame_presented(true, false);
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
