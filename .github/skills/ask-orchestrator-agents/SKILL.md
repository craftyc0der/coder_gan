---
name: ask-orchestrator-agents
description: Send structured work requests to coder_gan agents (coder, tester, reviewer) using .orchestrator message queues.
---

# Ask Orchestrator Agents

Use this skill when you want to assign work to one or more orchestrator agents created by this project.

## When to use

- You want `coder`, `tester`, or `reviewer` to perform a concrete task.
- You want consistent message format and routing.
- You want escalation-ready messages with explicit acceptance criteria.

## Inputs to collect from the user

1. **Recipient agent**: `coder`, `tester`, or `reviewer`
2. **Topic**: short slug like `bugfix-scroll`, `add-tests`, `api-review`
3. **Task**: exact work to perform
4. **Constraints**: scope limits, forbidden paths, deadlines
5. **Definition of done**: how success is validated

## Message file protocol

- Directory: `.orchestrator/messages/to_<recipient>/`
- File name:
  `YYYY-MM-DDTHH-MM-SSZ__from-operator__to-<recipient>__topic-<topic>.md`
- Write atomically: write temp file first, then rename.

## Message template

```md
--- INCOMING MESSAGE ---
FROM: operator
TOPIC: <topic>
---

## Objective

<one-paragraph objective>

## Requested Work

- <task 1>
- <task 2>

## Constraints

- <constraint 1>
- <constraint 2>

## Acceptance Criteria

- <verifiable outcome 1>
- <verifiable outcome 2>

## Deliverables

- <expected files or outputs>

## Reply Path

Write your response to:
.orchestrator/messages/to_coder/
(using standard timestamped naming)
```

## Recipient guidance

- `coder`: implementation requests, refactors, architecture updates.
- `tester`: test creation/expansion, failure repro, edge-case validation.
- `reviewer`: dispute resolution, quality arbitration, decision memos.

## Operator checklist

- Confirm recipient inbox exists.
- Confirm topic is concise and unique.
- Include explicit constraints (especially allowed write areas).
- Include exact acceptance criteria to avoid ambiguity.
- If blocked, escalate to `reviewer` with both positions and evidence.
