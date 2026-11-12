//! Binary size baseline: the same program with no engine.
use std::io::Read;

fn main() {
    let mut bytes = Vec::new();
    std::io::stdin().read_to_end(&mut bytes).unwrap();
    let text: Vec<String> = String::from_utf8_lossy(&bytes)
        .lines()
        .map(String::from)
        .collect();
    println!("{text:?}");
}
