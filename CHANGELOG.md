# autoratchet

## 0.1.2

### Patch Changes

- 2123842: `override-approval: review` now checks each approving reviewer's effective repository permission, and counts approvals from people with write or admin access.

## 0.1.1

### Patch Changes

- 01a2852: Measure cognitive complexity in files over 1 MiB, match `disable-line` overrides to the function on their own line, and accept `:!` exclusion pathspecs in `check`.

## 0.1.0

### Minor Changes

- 722c3dc: First release. `autoratchet check --base main` compares every function's cyclomatic and cognitive complexity in your working tree with the merge base, and fails when a function gets worse than the threshold-aware ratchet allows. Includes a TypeScript API, text, JSON and Markdown reports, override approvals that keep measuring, a GitHub Action with PR comments and diff annotations, and Claude Code hooks.

### Patch Changes

- 6662ce9: README lists the action's `approval-label` input. Temporary folders use an `autoratchet-` prefix, and comments no longer describe a PR label as a protected approval.
