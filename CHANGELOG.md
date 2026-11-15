# autoratchet

## 0.1.0

### Minor Changes

- 722c3dc: First release. `autoratchet check --base main` compares every function's cyclomatic and cognitive complexity in your working tree with the merge base, and fails when a function gets worse than the threshold-aware ratchet allows. Includes a TypeScript API, text, JSON and Markdown reports, override approvals that keep measuring, a GitHub Action with PR comments and diff annotations, and Claude Code hooks.

### Patch Changes

- 6662ce9: README lists the action's `approval-label` input. Temporary folders use an `autoratchet-` prefix, and comments no longer describe a PR label as a protected approval.
