---
name: prd-creation
description: Guide for creating Product Requirements Documents (PRDs). Use this skill when asked to create a PRD, write requirements for a feature, plan implementation work, or document a work item like step_##.
---

# PRD Creation Guide

This skill helps you create comprehensive Product Requirements Documents for product features. PRDs are stored in the `ai_implementation/` folder and follow a consistent structure.

## Prerequisites

Before writing a PRD:

1. **Get the work item ID** - Check the git branch name (`git branch --show-current`) to confirm the ID (format: `step_##`)
2. **Research the codebase** - Understand the current state of affected areas
3. **Ask clarifying questions** - Never assume; get explicit confirmation on requirements

## File Naming & Location

- **Location**: `ai_implementation/`
- **Filename**: `step_##-short-description.md` (e.g., `step_03-dynamic-exhibits.md`)
- **Format**: Markdown with clear section headers

## Required PRD Sections

Every PRD must include these sections:

### 1. Problem Statement

Describe what problem this feature solves and why it matters.

### 2. Goal

Clear, concise statement of what success looks like.

### 3. Scope

What's in scope and out of scope. For code changes, list:

- Files to be created
- Files to be modified
- Architecture decisions

### 4. Current State (Research Required)

Before writing the PRD, research the relevant code areas:

```markdown
## Current State

### Affected Files

| File            | Current Purpose  | Changes Needed  |
| --------------- | ---------------- | --------------- |
| path/to/file.ts | Current behavior | Planned changes |

### Key Findings

- Finding 1 from code research
- Finding 2 from code research
```

### 5. Technical Design

Detailed implementation plan including:

- Data models / schema changes
- API changes
- Component architecture
- Flow diagrams (where helpful)

### 6. Implementation Checklist

**MANDATORY**: Every PRD must have a checklist with checkboxes:

```markdown
## Implementation Checklist

### Phase 1: Schema/Model Changes

- [ ] Add field X to schema Y
- [ ] Update TypeScript types
- [ ] Update Go models (if applicable)

### Phase 2: Business Logic

- [ ] Implement core logic in ...
- [ ] Add validation for ...

### Phase 3: UI (if applicable)

- [ ] Create component ...
- [ ] Update existing component ...

### Phase 4: Testing

- [ ] Write unit tests
- [ ] Write integration tests
- [ ] Write E2E tests (for frontend)
```

### 7. Testing Plan

**MANDATORY for code-related PRDs**: Include a comprehensive testing plan.

```markdown
## Testing Plan

### Unit Tests

| Test Case | File            | Description      |
| --------- | --------------- | ---------------- |
| test_name | path/to/test.ts | What it verifies |

### Integration Tests (if applicable)

| Test Case      | Description                                 |
| -------------- | ------------------------------------------- |
| Full flow test | Seeds data, runs operation, verifies result |

### E2E Tests (for frontend work)

| Test Case | Steps            | Expected Result |
| --------- | ---------------- | --------------- |
| User flow | 1. Do X, 2. Do Y | Z happens       |
```

### 8. Success Criteria

Bullet list of acceptance criteria that define when the feature is complete.

### 9. Open Questions

Questions that need answers before or during implementation.

---

## Research Process

Before writing the PRD, gather context using these steps:

### Step 1: Understand the Domain

Search for related code to understand the domain:

```
# Find related files
file_search: **/*keyword*
grep_search: "RelatedClassName" or "relatedFunction"
semantic_search: "describe the feature area"
```

### Step 2: Examine Existing Patterns

Identify where similar features are implemented in this codebase.

### Step 3: Check for Existing Tests

Find tests to understand expected behavior e.g.:

```
file_search: **/*keyword*.test.ts
file_search: **/*keyword*_test.go
```

### Step 4: Review Related PRDs

Check `ai_implementation/` for similar PRDs that might inform structure.

---

## Clarifying Questions to Ask

Before finalizing the PRD, clarify:

### For All Features

1. What is the primary user persona affected?
2. Are there any edge cases we should handle explicitly?
3. What's the priority/timeline for this work?
4. Are there any dependencies on other tickets?

### For Frontend Work

1. Is there a design/Figma mockup?
2. Should this be mobile-responsive?
3. Are there accessibility requirements?
4. What internationalization (i18n) keys are needed?

### For Backend/API Work

1. Are there performance constraints?
2. What authentication/authorization is required?
3. Are there rate limiting considerations?
4. Should this work with emulators for testing?

### For Schema Changes

1. Is this additive or breaking?
2. Do we need a migration for existing data?
3. Are there Go models that need updating?

---

## PRD Template

Use this template as a starting point:

```markdown
# PRD: step\_## — Feature Name

## Problem Statement

[What problem does this solve?]

## Goal

[One-sentence description of success]

## Scope

### In Scope

- Item 1
- Item 2

### Out of Scope

- Item 1

## Current State

### Research Findings

[Document what you learned from researching the codebase]

### Affected Files

| File         | Current Purpose | Changes Needed     |
| ------------ | --------------- | ------------------ |
| path/to/file | Description     | Change description |

## Technical Design

### Data Model

[Schema changes, if any]

### Implementation

[Step-by-step implementation approach]

## Implementation Checklist

- [ ] Task 1
- [ ] Task 2
- [ ] Task 3

## Testing Plan

### Unit Tests

| Test Case | File | Description |
| --------- | ---- | ----------- |
| test_name | path | description |

### E2E Tests (if frontend)

| Test | Steps | Expected Result |
| ---- | ----- | --------------- |
| flow | steps | result          |

## Success Criteria

- [ ] Criterion 1
- [ ] Criterion 2

## Open Questions

1. Question 1?
2. Question 2?
```

---

## Workflow Reminders

1. **Always research first** - Don't write a PRD without understanding the current state
2. **Ask questions before assuming** - Clarify requirements with the user
3. **Include checklists** - Every PRD needs actionable, checkable items
4. **Plan testing upfront** - Testing requirements should be part of the design
5. **Update as you go** - Mark checklist items as complete during implementation
