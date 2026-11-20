---
name: ui-page-workflow
description: "Three-phase workflow to build or rework an app UI page (e.g. Settings › Add-ons, ticket #22). Phases: functional draft, prototype on an Artifact, implementation with the right style only after the user approves. Use it for every UI/HITL ticket that requires visual review."
---

# UI page flow: draft → prototype → style

Three phases, in order. Do not skip any of them and do not anticipate phase 3.

**Global style:** the style of the whole app is the **Design A** canvas (https://claude.ai/artifact/D8cbBbj8ohZzfMzGAsieRN). The `Main.dc.html` sheet fixes the rules, palette, fonts and parts; there is one sheet per page (`P01`–`P18`). Prototypes and egui styling follow Design A, never a style of their own.

## Phase 1 — Functional draft (in the code)

1. Derive the list of features from the ticket, the spec and the PRD (`docs/prd/`). Write it as a checklist: data shown, interactions, states (empty, waiting, error, "will turn off when finished"…), navigation.
2. Implement the page in egui with **all** the features in the checklist, using the existing widgets as they are. No styling work: simple layout, no new colors or spacing.
3. The logic goes through the host and the testable seams; write the tests (skill `tdd`). `cargo test` green.
4. Commit on the ticket branch with the message `... (draft)`.

## Phase 2 — Prototype on an Artifact

1. The prototype is a sheet of the Design A canvas: update the page's sheet (e.g. `P14-DiskSpace.dc.html`) or add one, following the instructions of the canvas's Design type. Colors, fonts and parts come from `Main.dc.html`.
2. The prototype shows the full page with the features of the phase 1 checklist, interactive where needed (toggles, selection, side panel; `is_interactive` on the sheet). Visual reference: the page's Design A sheet and the `Main.dc.html` sheet.
3. Publish, give the user the link with a short list of the open visual choices, and **stop**: wait for explicit approval. Apply requested corrections by republishing the same artifact.

## Phase 3 — Implementation with the correct style (only after approval)

1. Proceed only with an explicit approval from the user on the prototype. An approval given for an earlier phase does not count.
2. Bring the approved style into egui, i.e. Design A, using the app's theme, colors and common widgets (`src/widgets.rs`); add new common widgets there, not in the page.
3. The phase 1 tests stay green; no feature changes.
4. Verify with `cargo run -- --page "<Page>" --screenshot` and compare with the prototype; report to the user the differences that remain.
5. Commit on the ticket branch.
