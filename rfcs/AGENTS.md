# RFC Guide

`rfcs/` contains lightweight Markdown RFCs that extend sandbox-toolkit/WebDAV semantics. An RFC is a reviewable, traceable protocol proposal, not an implementation dump or commit log.

## Naming and status

- Use `NNNN-kebab-case-title.md` (for example, `0001-line-range-get.md`).
- Allocate each number once; do not rename a published RFC when its status changes.
- Start every RFC with a metadata table containing number, title, status, authors, created date, and updated date.
- Use one of: `Draft`, `Accepted`, `Rejected`, `Implementing`, `Complete`, or `Obsolete`. Update the date, status, and changelog together.
- Preserve the number and history after publication. A major incompatible change requires a new RFC that states what it supersedes.

## Required content

Use this order; explicitly mark a section `Not applicable` rather than silently omitting a decision:

1. Abstract
2. Motivation, goals, and non-goals
3. Terminology and conventions
4. Protocol design: syntax, requests, responses, status codes, errors, caching, and interaction with existing semantics
5. ABNF or equivalent grammar, plus successful and failing examples
6. Security, performance, and observability
7. Implementation and rollout
8. Test plan and acceptance criteria
9. Open questions
10. Changelog and references

Specify case, units, bounds, defaults, and failure behavior for every protocol field. Define how bytes, characters, lines, and time are counted. Examples must be real HTTP messages and state the input data or totals.

## Workflow

1. Search existing RFCs, source, and tests before choosing a number or protocol name.
2. Create the next numbered Markdown file and keep one RFC focused on one reviewable change.
3. Resolve trade-offs in review discussion; do not hide unresolved design in code or commit messages.
4. When accepted, update status, dates, and changelog. Keep implementation, tests, and links consistent in the same change.
5. For revisions, preserve history and document compatibility, migration, and client impact.
6. Before merge, check heading levels, tables, code fences, links, examples, and corresponding tests or executable acceptance checks.
7. Every RFC MUST have a matching top-level integration test file under `tests/` named `rfc_NNNN_<snake_case_title>.rs` (for example, `tests/rfc_0001_line_range_get.rs`). The file MUST map to every normative feature, status code, interaction, and documented corner case in the RFC; happy-path-only coverage is insufficient. Add or update the test file in the same change as the RFC, and keep test names and comments traceable to the RFC sections and acceptance criteria. A new or revised RFC is not complete until its corresponding tests pass against the current implementation.

This process follows established repository RFC practices: Rust uses numbered Markdown RFCs moved through repository review; Bytecode Alliance uses RFC templates and pull requests; Kubernetes tracks numbered proposals with lifecycle/status metadata; Ember maintains Markdown RFCs through review and recorded decisions.

References:

- <https://github.com/rust-lang/rfcs>
- <https://github.com/bytecodealliance/rfcs>
- <https://github.com/kubernetes/enhancements/tree/master/keps>
- <https://github.com/emberjs/rfcs>

Keep RFCs at the protocol/decision level. Put changing implementation details in the relevant source module and its `AGENTS.md`.
