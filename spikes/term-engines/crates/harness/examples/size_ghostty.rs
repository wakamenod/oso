//! Binary size with one engine: read stdin, feed it, print the screen.
use engine::Engine;
use std::io::Read;

fn main() {
    let mut bytes = Vec::new();
    std::io::stdin().read_to_end(&mut bytes).unwrap();
    let mut e = engine_ghostty::GhosttyEngine::new(80, 24, 1000);
    e.feed(&bytes);
    let mut snap = engine::Snapshot::default();
    e.snapshot(&mut snap);
    println!(
        "{:?} {:?} {}",
        e.screen_text(),
        e.drain_effects(),
        snap.cells.len()
    );
}
