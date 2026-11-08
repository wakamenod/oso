---
name: adr
description: Record a design decision as an Architecture Decision Record in docs/dev/adr/. Use when a decision has been made or needs to be proposed, or when a change would contradict an accepted ADR.
---

# Record an ADR

An ADR records one decision: context, decision, consequences and the
alternatives considered. Accepted ADRs are never rewritten; a changed decision
gets a new ADR that supersedes the old one.

## Steps

1. **Find the next number.** List `docs/dev/adr/` and take the highest
   `NNNN-*.md` plus one, zero-padded to four digits.
2. **Check for conflicts.** Read the ADRs the decision touches. If it changes
   an accepted decision, the new ADR supersedes or amends it (see step 5).
3. **Write `docs/dev/adr/NNNN-short-title.md`** from the template in
   `docs/dev/adr/README.md`:
   - Status: `Proposed`. Date: today, `YYYY-MM-DD`.
   - **Context:** the problem and the forces at play, briefly.
   - **Decision:** what is decided, as concrete rules. Link related ADRs.
   - **Consequences:** what follows, good and bad, including follow-up work.
   - **Alternatives considered:** each one with the reason it was rejected.
   - Match the style of existing ADRs: short sentences, bullet lists, English.
4. **Do not add a row anywhere.** There is no ADR table: `just adrs` lists
   the ADRs from their files.
5. **Update related docs** so they agree with the decision: design docs in
   `docs/dev/`, `AGENTS.md` (keep it short), `REVIEW.md`. If the ADR
   supersedes another, set the old one's status to `Superseded by NNNN`. If
   it changes only part of one, give it a `Changes to accepted ADRs` section
   naming each passage it changes, and set the old one's status to
   `Accepted, amended by NNNN` once this ADR is accepted. Do not change
   anything else in the old ADR.
6. **Update `docs/dev/open-questions.md`:** remove items the ADR settles, and
   add new open items it raises under the milestone they block.
7. **Ask for approval.** ADRs are a human gate (ADR 0018). Summarize the
   decision and wait for the author. After approval, change the status to
   `Accepted` in the file.
