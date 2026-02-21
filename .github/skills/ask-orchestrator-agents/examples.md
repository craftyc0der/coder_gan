# Examples: Ask Orchestrator Agents

## Example 1: Ask coder to implement a fix

**Recipient:** `coder`
**Topic:** `fix-tmux-scroll`

```md
--- INCOMING MESSAGE ---
FROM: operator
TOPIC: fix-tmux-scroll
---

## Objective

Make tmux sessions easier to inspect by enabling scroll support and preserving long output history.

## Requested Work

- Ensure new agent sessions enable tmux mouse mode.
- Increase session history limit.
- Add regression tests that assert session options are configured.

## Constraints

- Do not modify files under orchestrator/src unrelated to session startup.
- Keep changes minimal and backward-compatible.

## Acceptance Criteria

- New sessions allow mouse wheel scrollback in terminal emulator.
- Targeted tests pass.

## Deliverables

- Code changes + test updates.
- Short summary of what changed and why.
```

## Example 2: Ask tester to add tests

**Recipient:** `tester`
**Topic:** `inject-regression`

```md
--- INCOMING MESSAGE ---
FROM: operator
TOPIC: inject-regression
---

## Objective

Add tests to prevent regressions in prompt injection behavior.

## Requested Work

- Add a test that verifies bracketed paste is used.
- Verify Enter is sent after paste.

## Constraints

- Do not alter production logic.
- Keep tests deterministic.

## Acceptance Criteria

- New tests fail before fix and pass after fix.
- Test names clearly describe behavior under test.

## Deliverables

- Test file updates and a brief execution report.
```
