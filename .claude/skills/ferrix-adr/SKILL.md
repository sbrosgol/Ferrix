---
name: ferrix-adr
description: Write or update a Ferrix Architecture Decision Record and keep the roadmap, architecture doc and README in step. Use when a change makes or alters a design decision, adds a subsystem or crate, or finishes a roadmap item.
---

# Ferrix ADRs

ADRs live in `docs/adr/NNNN-kebab-title.md`, numbered in sequence (check the folder for the next
number). Match the existing ones (read ADR-0003 and ADR-0008 first):

```markdown
# ADR-NNNN: Title in sentence case

- Status: **Accepted**   (or Proposed / Superseded by ADR-XXXX)
- Date: YYYY-MM-DD
- Supersedes: … (only if it does)

## Context
Why a decision is needed: the user problem and the constraints. Plain language.

## Decision
Numbered points. Bold the key choice in each. Name crates, types and libraries.

## Consequences
- + benefits
- − costs and limits (be honest: dependencies, size, unsupported cases)

## Phases            (optional, for multi-step work)
```

When work lands, **append** an `## Implementation notes (<topic>, YYYY-MM-DD)` section rather than
rewriting the decision: what was built, where, and known gaps. Earlier notes stay.

Keep these in step in the same commit:
- `docs/roadmap.md`: strike through finished items as `~~item~~ (done: …)`; add new sections for
  new tracks; record deferred items and why.
- `docs/architecture.md`: the crate list (§4.1) and goals/non-goals if they change.
- `README.md`: "What works today" and "Not yet".

Style: short sentences, no marketing; explain *why*; British/US spelling as already used in the
file you edit.
