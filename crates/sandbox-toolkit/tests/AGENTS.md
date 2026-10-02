# AGENTS.md

## Rules

- Each feature has a design doc `X.md` paired with a test file `x.rs`.
- A design doc names the test group for each feature; the test file organizes tests by those names, and the design doc is the single source of truth.
- A test point that disagrees with the design doc is corrected to the doc, never the reverse.
- A design change and its test-group update land in the same change.
- `tests/` holds end-to-end tests only; unit tests live in the module they cover.
- Committing with tests not yet fully passing is allowed; committing while tests and the design doc are out of sync is forbidden.

## Communicating a design

When the user describes a design, do not write the design doc or code yet.
First present the intended design back for review, in the most intuitive, briefest form the user can understand and check, capped at 20 lines total (diagrams and tables excluded).
Only start writing once the user agrees.

- Conceive the plan against the existing code, judging feasibility by what is already there.
- If a feature cannot fit the current architecture, say so plainly instead of grinding on an over-complex or hacky workaround.
- If the design needs a decision the user has not specified, never pick silently; present the options briefly and ask the user to decide.

## Writing a design doc

A design doc is transcribed from the user's spoken description, not invented. Record only what the user states.

- Shape: a title, an overview, then one section per feature. Each feature section ends in a list naming its test group.
- Voice: short, declarative, RFC-like but relaxed. Every sentence carries a decision; no filler, common knowledge, or restatement of the code.
- Scope: describe behavior as observable outcomes. Do not add edge cases, options, or configuration the user did not raise.
- Sync: editing design content obliges reconciling the paired test list in the same change, so every section and test point covers and expresses the current intent, and stale, missing, or misdescribed groups and cases are added, deleted, or fixed.

## Sync

When the user asks to commit a design doc change, reconcile the paired test file so every changed section and test point matches the doc.
