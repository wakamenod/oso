//! The tests of `[lsp]` in [`parse`](super::parse): the built-in table, the person's changes
//! to it, a mode's one server, `options` as JSON, and what is reported of bad ones
//! (ADR 0138, Q3).
//!
//! Must not depend on: `app`, `dispatch` or the file system; these run on strings alone.

use oso_core::buffer::Mode;
use serde_json::json;

use super::lsp::{Lsp, StartWhen};
use super::tests::{Fixture, messages};

fn parsed(text: &str) -> (Lsp, Vec<String>) {
    let f = Fixture::new();
    let loaded = f.parse(text);
    let said = messages(&loaded);
    (loaded.settings.lsp, said)
}

#[test]
fn the_built_in_table_needs_no_configuration() {
    let (lsp, said) = parsed("");
    assert!(said.is_empty());
    assert_eq!(lsp.start, StartWhen::Trusted);
    let rust = lsp.for_mode(Mode::Rust).unwrap();
    assert_eq!(rust.name, "rust-analyzer");
    assert_eq!(rust.command, ["rust-analyzer"]);
    assert!(rust.in_cargo_bin);
    assert_eq!(rust.env, ["CARGO_HOME", "RUSTUP_HOME"]);
    assert_eq!(rust.section.as_deref(), Some("rust-analyzer"));
    assert_eq!(rust.options, Some(json!({"cargo": {"targetDir": true}})));
    assert!(lsp.for_mode(Mode::Toml).is_none());
}

#[test]
fn an_override_changes_only_the_keys_it_names() {
    // An absolute path on this system: `C:` starts one on Windows.
    let program = if cfg!(windows) {
        "C:/ra/bin/rust-analyzer.exe"
    } else {
        "/opt/ra/bin/rust-analyzer"
    };
    let (lsp, said) = parsed(&format!(
        "[lsp]\nstart = \"manual\"\n\
         [lsp.servers.rust-analyzer]\ncommand = [\"{program}\"]\n\
         env = [\"RUSTUP_TOOLCHAIN\", \"CARGO_HOME\"]\n\
         [lsp.servers.rust-analyzer.options]\ncheck.command = \"clippy\"\ncargo.features = [\"a\"]\n",
    ));
    assert_eq!(said, Vec::<String>::new());
    assert_eq!(lsp.start, StartWhen::Manual);
    let rust = lsp.for_mode(Mode::Rust).unwrap();
    assert_eq!(rust.command, [program]);
    assert!(
        !rust.in_cargo_bin,
        "a command of the person's is looked for where it says"
    );
    assert_eq!(rust.env, ["CARGO_HOME", "RUSTUP_HOME", "RUSTUP_TOOLCHAIN"]);
    assert_eq!(
        rust.options,
        Some(json!({
            "cargo": {"targetDir": true, "features": ["a"]},
            "check": {"command": "clippy"},
        }))
    );
}

#[test]
fn an_entry_can_be_turned_off_or_added() {
    let (lsp, said) = parsed(
        "[lsp.servers.rust-analyzer]\nenabled = false\n\
         [lsp.servers.my-server]\nmodes = [\"toml\", \"rust\"]\ncommand = [\"my-server\", \"--stdio\"]\n",
    );
    assert_eq!(said, Vec::<String>::new());
    let mine = lsp.for_mode(Mode::Toml).unwrap();
    assert_eq!(mine.name, "my-server");
    assert_eq!(mine.command, ["my-server", "--stdio"]);
    assert_eq!(mine.section.as_deref(), Some("my-server"));
    assert_eq!(lsp.for_mode(Mode::Rust).unwrap().name, "my-server");
    assert_eq!(lsp.servers.len(), 1);
}

#[test]
fn the_persons_entry_wins_a_mode_and_a_second_is_reported() {
    let (lsp, said) = parsed(
        "[lsp.servers.a]\nmodes = [\"rust\"]\ncommand = [\"a\"]\n\
         [lsp.servers.b]\nmodes = [\"rust\"]\ncommand = [\"b\"]\n",
    );
    assert_eq!(lsp.for_mode(Mode::Rust).unwrap().name, "a");
    assert!(
        !lsp.servers
            .iter()
            .any(|entry| entry.name == "rust-analyzer"),
        "the built-in one lost its only mode"
    );
    assert_eq!(said.len(), 1, "{said:?}");
    assert!(said[0].contains("[lsp.servers.b] names a mode [lsp.servers.a] has already"));
}

#[test]
fn bad_values_are_reported_and_left_out() {
    let (lsp, said) = parsed(
        "[lsp]\nstart = \"always\"\nother = 1\n\
         [lsp.servers.x]\nmodes = [\"fundamental\", \"klingon\"]\ncommand = [\"x\"]\n\
         [lsp.servers.y]\nmodes = [\"toml\"]\ncommand = [\"bin/y\"]\n\
         [lsp.servers.z]\nmodes = [\"markdown\"]\ncommand = []\n\
         [lsp.servers.rust-analyzer]\nenv = [\"PATH\"]\noptions = 3\nfoo = true\n",
    );
    assert_eq!(lsp.start, StartWhen::Trusted);
    let names: Vec<&str> = lsp
        .servers
        .iter()
        .map(|entry| entry.name.as_str())
        .collect();
    assert_eq!(names, ["rust-analyzer"]);
    let all = said.join("\n");
    for part in [
        "[lsp] start takes \"trusted\" or \"manual\", not \"always\"",
        "unknown name \"other\" in [lsp]",
        "[lsp.servers.x] modes takes rust, markdown or toml, skipped",
        "[lsp.servers.x] needs a command and modes, skipped",
        "the program is a name on PATH or an absolute path",
        "from 1 to 64 words",
        "[lsp.servers.rust-analyzer] env cannot pass \"PATH\"",
        "[lsp.servers.rust-analyzer] options takes a table, not a number",
        "unknown name \"foo\" in [lsp.servers.rust-analyzer]",
    ] {
        assert!(all.contains(part), "{part:?} in:\n{all}");
    }
    assert_eq!(lsp.servers[0].env, ["CARGO_HOME", "RUSTUP_HOME"]);
}

#[test]
fn options_nest_only_so_deep() {
    let deep = format!(
        "[lsp.servers.rust-analyzer.options]\n{} = 1\n",
        vec!["a"; 40].join(".")
    );
    let (lsp, said) = parsed(&deep);
    assert!(
        said.iter()
            .any(|line| line.contains("nests more than 32 deep")),
        "{said:?}"
    );
    assert_eq!(
        lsp.for_mode(Mode::Rust).unwrap().options,
        Some(json!({"cargo": {"targetDir": true}})),
        "the built-in settings stay"
    );
    let (lsp, said) =
        parsed("[lsp.servers.rust-analyzer.options]\nwhen = 1979-05-27\nratio = 0.5\nbig = 0x10\n");
    assert!(said.is_empty(), "{said:?}");
    let options = lsp.for_mode(Mode::Rust).unwrap().options.clone().unwrap();
    assert_eq!(options["when"], "1979-05-27");
    assert_eq!(options["ratio"], 0.5);
    assert_eq!(options["big"], 16);
}

#[test]
fn a_bad_name_is_reported() {
    let (_, said) = parsed("[lsp.servers.\"a b\"]\ncommand = [\"x\"]\n");
    assert!(
        said.iter()
            .any(|line| line.contains("a server's name takes letters")),
        "{said:?}"
    );
}
