# 0172. Syntax limits in the worker thread's CPU time

- Status: Accepted
- Date: 2026-11-23

## Context

[ADR 0111](0111-tree-sitter-highlighting.md) stops highlighting in a buffer for good
(`Failure::TooSlow`) when a parse or highlighting takes too long. It counts "the worker's time",
but as built every limit was measured on the wall clock (`Instant`):

- a first parse past 5 s, or a later one past 1 s, in all;
- a piece of highlighting that runs past its slice's end by more than a slice (at least 10 ms);
- a piece of 1 KiB, or a Markdown piece widened past the limit, that the slice stopped after it
  had most of the slice;
- highlighting that takes 2 s in all before it is done.

A thread the system holds back is charged for all the time it waits. On a loaded machine, such
as macOS CI's 3 vCPUs or the author's Mac during `cargo build`, a 1 MiB file has about 1000
pieces of 16 KiB, so only one of them needs to wait more than 20 ms. Then the buffer stays plain
until its mode is set again. The perf benchmark's diagnostic (#439) found this on macOS CI:
"Highlighting stopped in big.rs: it took too long".

## Decision

The author's choice (F1), 2026-11-23, through the coordinator. The coordinator chose the
technical details.

- **All the limits count the worker thread's CPU time:** the parse limits (5 s first, 1 s later)
  and the three highlighting ones above. Their values do not change. The deadline that ends a
  slice stays on the wall clock, so a slice still yields after about 20 ms, however little of
  that time the thread ran.
- **Never more than the wall clock:** no judgement is stricter than before.
  - A piece is judged by its own interval: the CPU time it read, at most the wall time it took.
  - A limit counts the CPU time in all, at most the wall time in all: the parse's and the
    highlighting's totals add both up, and take the lesser. Capping in all rather than at each
    interval keeps a coarse clock's error within one tick, instead of letting it add up.
  - When the clock reads nothing, the interval counts the wall clock, as before.
- **A piece held back:**
  - Pieces still shrink by the wall clock: a piece the slice stopped after it started with half
    a slice left halves the pieces, as before. That costs little, and fits pieces to the time
    the system gives the thread.
  - It fails the document only if it also ran half a slice of CPU time. A piece the system held
    back is done again in the next slice.
  - A piece overruns when its CPU time passes the time left in its slice by more than a slice
    (at least 10 ms).
- **A parse stops at the slice's end alone:** it is no longer cut short where its limit would
  run out, and it fails once its CPU time has passed the limit. So a parse may pass its limit
  by one slice. In exchange, a thread the system holds back does not approach the limit through
  a run of ever shorter calls.
- **The clock:** a new module in oso-platform,
  `oso_platform::thread_time::now() -> Option<Duration>`, gives the calling thread's CPU time,
  user and kernel:
  - macOS and Linux: `clock_gettime(CLOCK_THREAD_CPUTIME_ID)` through `libc`;
  - Windows: `GetThreadTimes` on `GetCurrentThread()`, declared in oso-platform from kernel32,
    which std already links, with no new dependency (ADR 0137 declares a few OS functions the
    same way). It counts in the scheduler's ticks of about 15.6 ms. Capped in all, a limit is off
    by at most a tick; a piece's judgement, capped by its own interval, at most delays a
    decision by a slice;
  - other systems: `None`, the wall clock.
- **How oso-syntax gets it:** oso-syntax depends on `oso-core` only and forbids `unsafe`, so the
  tip crate's pool hands the clock in. The public items change:
  - `Checkout::run(&mut self, queries: &mut Queries, clock: fn() -> Option<Duration>)`;
  - `Document::highlight(&mut self, queries, first, slice, clock: fn() -> Option<Duration>)`;
  - `Document::parse(&mut self, slice, clock: fn() -> Option<Duration>)`.

  `app/syntax/pool.rs` passes `oso_platform::thread_time::now`. Inside, in
  `document/clock.rs`, a private `Stopwatch` reads both clocks at an interval's start and end,
  and `Spent` adds intervals up for a limit.
- **Cost:** two reads of each clock a piece, two a slice, and two each time a slice parses. A thread CPU clock read takes about
  0.1 µs on the author's Mac. On Linux it is a system call, as the vDSO does not serve this
  clock: a few tenths of a µs. A piece's queries take tens of µs at least. The progress callback
  tree-sitter calls during a parse or a query reads only `Instant`, as before.

## Changes to accepted ADRs

Each passage stays in ADR 0111 as written; what holds now follows it. ADR 0111's status gains
`amended by 0172`.

- ADR 0111, "Large files and long lines (Q6)", **Time limits**: "a first parse that passes 5 s
  of the worker's time, or a later one that passes 1 s": the worker's time is the thread's CPU
  time, at most the wall clock's, as above, and a parse may pass its limit by one slice before
  it fails.
- ADR 0111, step 3 as built, **Limits**, the three cases in which "the document fails as too
  slow": each is judged in the thread's CPU time, at most the wall clock's. "If it had most of
  a slice, pieces halve" holds on the wall clock; the first case fails the document only if the
  piece also ran half a slice. The slice's deadline stays on the wall clock. The measurement there, "a document failed after at most about 2.5 s of the worker's
  time", holds in CPU time. On a loaded machine the wall time it takes to fail grows with the
  load.

## Consequences

- A loaded machine no longer leaves a large file plain. Its highlighting takes longer on the
  wall clock instead.
- **Crafted text keeps its bound:** about 2.5 s of the worker thread's CPU time before it fails,
  and still at most one of the pool's two threads, as before.
- **Its wall time is not bounded:** it grows as the thread's share of the machine shrinks. At a
  tenth, the 2 s limit takes about 20 s, and a shown buffer of such text, or a shown review
  side or session reply, keeps its thread all that time. Documents in the lane behind it wait
  as long. That is the load's cost, not the text's.
- **A clock that reads a value but never advances** would turn every limit off. No supported
  system's thread clock does that; one that fails reads `None`, which counts the wall clock.
- **Tests:**
  - `oso-syntax`'s `document/clock_tests.rs`:
    - A still clock (a thread held back all the time) gets past limits the wall clock fails:
      a parse, highlighting's total, and a crafted text's pieces. The pieces shrink but do not
      fail.
    - A clock at a fifth of the wall clock's (a loaded machine) still fails the parse and the
      crafted text.
    - A racing clock counts no more than the wall clock, for an interval and in all. A clock
      that runs backwards counts nothing, and one that reads none counts the wall clock.
  - The other `oso-syntax` tests, and its `syntax_perf` example, pass a clock that reads none,
    so they time on the wall clock, which is never less than CPU time. The crafted ones still
    fail as they did.
  - The pool's test in the tip crate runs on the real clock, and a crafted document there still
    fails as too slow.
  - `oso-platform`'s `thread_time` tests check that a thread's time grows while it works, not
    while it sleeps, and that each thread has its own.
- oso-platform gains a public module and oso-syntax three changed signatures, in the API
  snapshots. `guardrails/layers.txt` lists `thread_time` on oso-platform's lowest layer.

## Alternatives considered

- **Longer wall-clock limits:** they would still fail at enough load, and would let crafted
  text run longer on an idle machine.
- **Retrying after `TooSlow`:** it would hide the failure ADR 0111 means to show, and crafted
  text would cost its bound again at each retry.
- **CPU time for the slice's deadline too:** a thread that waits would then hold its slice past
  20 ms of wall time, so other buffers' jobs would wait behind it. The deadline is about
  turns, not cost.
- **`QueryThreadCycleTime` on Windows:** finer, but it counts cycles, not time. The wall-clock
  cap already bounds `GetThreadTimes`' ticks.
- **The `windows-sys` crate:** already in the lock through other crates, but a direct dependency
  of oso-platform all the same, for two functions.
