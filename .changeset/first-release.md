---
'autoratchet': minor
---

First release. `autoratchet check --base main` compares every function's cyclomatic and cognitive complexity in your working tree with the merge base, and fails when a function gets worse than the threshold-aware ratchet allows. Includes a TypeScript API, text, JSON and Markdown reports, override approvals that keep measuring, a GitHub Action with PR comments and diff annotations, and Claude Code hooks.
