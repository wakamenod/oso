//! The thread that drops what the UI thread is done with, `oso-free`: freeing a large file's
//! text can take longer than a frame.
//!
//! Must not depend on: editor state; it only drops what it is sent.

/// The thread that drops what the UI thread is done with, or none if it could not start.
pub(super) struct Freer(Option<std::sync::mpsc::Sender<Box<dyn Send>>>);

impl Freer {
    pub(super) fn start() -> Self {
        let (freer, freed) = std::sync::mpsc::channel::<Box<dyn Send>>();
        // Freeing a large file's text can take longer than a frame, so it is done there.
        let started = std::thread::Builder::new()
            .name("oso-free".into())
            .spawn(move || freed.iter().for_each(drop))
            .is_ok();
        Self(started.then_some(freer))
    }

    /// Drops `value` on the freeing thread, or here if it could not start.
    pub(super) fn free<T: Send + 'static>(&self, value: T) {
        if let Some(freer) = &self.0 {
            // A thread that has gone hands it back in the error, and it is dropped here.
            drop(freer.send(Box::new(value)));
        }
    }
}
