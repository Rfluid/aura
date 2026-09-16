---
title: Issue triage before GitHub publishing
status: current
version: 0.1.0
last_updated: 2026-09-16
last_verified: 2026-09-16
source_refs:
  - .agent/skills/diagnose-and-publish-issue.md
  - .github/ISSUE_TEMPLATE/bug_report.yml
  - README.md
  - docs/configuration.md
  - docs/platform-tray-icon.md
owner: "@rfluid"
tags: [memory, pattern, triage, github]
source_task: "Create a robust .agent skill to diagnose Aura issues and create GitHub issues only when appropriate."
---

# Issue triage before GitHub publishing

For Aura support reports, the agent should classify the situation before
opening GitHub issues. The durable pattern is: check relevant repo docs and
source, gather OS/install/config evidence, dedupe existing issues, then decide
between actionable bug, regression, known platform limitation, setup/config
issue, duplicate, documentation gap, or insufficient evidence.

The skill at `.agent/skills/diagnose-and-publish-issue.md` now encodes that
workflow and requires explicit user approval before public GitHub mutation.
