# Third-party code in oso-term

oso-term links libghostty-vt as a static archive that Zig builds from the release pinned in
[`ghostty-vt.lock`](ghostty-vt.lock) ([ADR 0120](../../docs/dev/adr/0120-the-terminal.md)).
It is no crate, so `cargo deny` and `cargo audit` see neither it nor what it bundles. This list
does their work by hand, the way ADR 0111 does for the grammars' C.

## How the list is kept

- **The packages in the dependency tarball:** `build.rs` fails unless its packages are exactly
  the lock's `package` lines, and a test fails unless each of them is in the table below.
- **The library's own report:** the release's `deps/libghostty-vt-third-party.json` names each
  package, its version, commit, licence and whether it is linked or used only to build. The
  table copies it.
- **oso's patch:** `patches/0001-row-copies.patch` changes four files of the library's source
  (`src/terminal/PageList.zig`, `Screen.zig`, `page.zig` and `c/terminal.zig`) so that row
  copies never abort; its lines say so where they change the code. The changed files stay
  under the library's MIT licence, whose text in `licenses/` is unchanged. `build.rs` checks
  the patch and each file it changes against the lock.
- **At a bump:** read the new report and the diff of each linked package, update the lock and
  this table, and copy any licence text that changed into `licenses/`. Drop the patch if the
  new release makes it needless, or rebase it and update its hashes. Then fuzz again with
  `just fuzz-term`, ReleaseFast and ReleaseSafe, an hour each, and on Linux as well, before
  the bump lands.

## libghostty-vt 1.0.1

| Package | Version | Commit | Licence | Use | Zig package (lock) |
|---|---|---|---|---|---|
| libghostty-vt | 1.0.1 with one oso patch | release tarball | MIT | linked | the source tarball |
| simdutf | 9.0.0 | vendored in `pkg/simdutf/vendor` | Apache-2.0 OR MIT | linked | the source tarball |
| highway | 1.2.0 | `66486a10623fa0d72fe91260f96c892e41aceb06` | Apache-2.0 OR BSD-3-Clause | linked | `N-V-__8AAGmZhABbsPJLfbqrh6JTHsXhY6qCaLAQyx25e0XE` |
| Wuffs | 0.4.0-alpha.10 | `7411f488fe2e2c205c3d3b3d28638b7356522930` | Apache-2.0 OR MIT | linked | `N-V-__8AAP5JWgCGP_AD0teWpa4krRvE9VPZzvviGdbmN4jI` |
| uucode | 0.2.0 | `1fb73433bba5d93366c57f23ff2e9d7939746500` | MIT AND Unicode-3.0 | linked | `uucode-0.2.0-ZZjBPm6FVgBcY-79AHV8ckQj86z0JwOyVyj89VeZrEiv` |
| translate_c | 2.0.0 | `02ff0c523fb92a38939b04d0145c4a9602a74d56` | MIT | linked | `translate_c-2.0.0-Q_BUWltOBwBYABF84EUL208pIFeUIwbBs29FmVBaafN5` |
| aro | 0.0.0 | `d0c8c4d9c55daa7ef6e40cf0f630a5b5e900989b` | MIT AND Unicode-3.0 | build only | `aro-0.0.0-JSD1QtuBNwCASyBtNF3pqTl_W3oAJQGEVyFAtrBSE_Pa` |

Every licence is one of `deny.toml`'s allowed licences. Where a package offers a choice, oso
takes the permissive option: MIT for simdutf and Wuffs, BSD-3-Clause for highway.

## Licence texts

`licenses/` holds the texts the tarballs carry: libghostty-vt's, highway's BSD-3-Clause,
Wuffs's MIT, uucode's and translate_c's. Two texts are not in the tarballs and must be added
when oso is packaged, which also ships these notices beside the binary
([`open-questions.md`](../../docs/dev/open-questions.md)):

- simdutf's: the vendored copy carries no licence file; its `build.zig.zon` declares
  Apache-2.0 OR MIT, after the upstream repository.
- uucode's Unicode-3.0 and the UTF-8 decoder notice its `LICENSE.md` points to: its package
  leaves its `licenses/` directory out.
