# autoratchet

## 0.4.3

### Patch Changes

- f4f987d: The GitHub Action reports a regression in full: it writes the job summary, marks the function in the diff and posts the pull request comment before it fails the check.
- fb3dcc8: The README's workflow example names the current release, and each release updates it.

## 0.4.2

### Patch Changes

- 18551a2: The npm package description fits npm's 255-character limit, so the package page shows it in full.

## 0.4.1

### Patch Changes

- 2948094: The GitHub Action has a short description that fits the GitHub Marketplace listing. It and the package description name the five languages AutoRatchet measures and the places it runs: on your machine, in AI coding agent hooks and in CI.

## 0.4.0

### Minor Changes

- cd6423b: AutoRatchet measures Python and Go. Python scores follow radon for cyclomatic complexity and complexipy for cognitive complexity; Go scores follow gocyclo and gocognit. Each language reads the override comments its developers write today (`# noqa: C901`, `# complexipy: ignore`, `//nolint:gocyclo,gocognit`, `//gocognit:ignore`), and both run from the AutoRatchet binary alone.

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
