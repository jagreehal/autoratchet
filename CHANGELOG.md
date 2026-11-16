# autoratchet

## 0.3.1

### Patch Changes

- 1547238: The GitHub Action downloads the AutoRatchet release binary, checked against `SHA256SUMS`, and Oxlint only when the paths include JavaScript or TypeScript. It runs without Node.js.

## 0.3.0

### Minor Changes

- a2dd937: AutoRatchet measures Rust. It names every function, method, trait method and closure, scores cyclomatic and cognitive complexity itself, and reads `#[expect(clippy::cognitive_complexity)]` and `// autoratchet-disable-next-line complexity` overrides. A Rust project needs only the binary; Oxlint is needed only for JavaScript and TypeScript.

## 0.2.0

### Minor Changes

- 90d91c2: AutoRatchet is now a native binary written in Rust, with the same CLI, JSON report, GitHub Action and agent hooks. It scores cognitive complexity itself, with the same scores as before and no Biome install, and accepts `sonarjs/cognitive-complexity` override comments. It scores methods nested inside functions on their own, gives the same result on every run, accepts TypeScript overloads and `declare` functions, and handles repositories of any size. A new `measure` command prints every function's scores. The JavaScript API is replaced by the CLI's JSON output and the `autoratchet` Rust crate.

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
