# autoratchet

## 0.7.0

### Minor Changes

- 54bbcf0: Ratchet lint findings alongside complexity. New functions get no findings, and existing functions may not gain any.

  - JavaScript and TypeScript: 17 anti-slop rules and 87 of Oxlint's built-in rules. These need Node.js and Oxlint's npm package, which the GitHub Action installs.
  - Rust, Python and Go: `anti-slop-rust`, `anti-slop-python` and `anti-slop-go`, built into the binary.
  - Approve a finding with `oxlint-disable-next-line <rule> -- reason`, or `autoratchet-disable-next-line` in Rust, Python and Go.
  - AutoRatchet matches functions that share a name by their code, so you can add, remove or reorder callbacks and each keeps its history.

## 0.6.0

### Minor Changes

- fb8b0bf: Agent skills in `skills/`: `autoratchet-setup`, plus one per language (`autoratchet-typescript`, `autoratchet-rust`, `autoratchet-python`, `autoratchet-go`) for writing code that passes the check. Install with `npx skills add jagreehal/autoratchet`. The Claude Code hook reads `AUTORATCHET_PATHS`, the same pathspecs as the workflow's `path`.

## 0.5.1

### Patch Changes

- 7b331b3: AutoRatchet reads limits from eslint-plugin-sonarjs rules in `.oxlintrc.json`: `sonarjs/cognitive-complexity` for cognitive and `sonarjs/cyclomatic-complexity` for cyclomatic. Override comments accept `sonarjs/cyclomatic-complexity`.

## 0.5.0

### Minor Changes

- 10559b7: AutoRatchet takes complexity limits from your linter config: `.oxlintrc.json`, `biome.json`, Ruff, `clippy.toml` and golangci-lint. Each language gets its own limit, and the report names the file that set it. The action's `threshold` and `cognitive-threshold` inputs now default to your config.

## 0.4.6

### Patch Changes

- 7f2e9e8: `autoratchet --version` (or `-V`) prints the version.

## 0.4.5

### Patch Changes

- 91bd1cb: The GitHub Action reads the pull request's current labels in `label` mode, so adding the approval label and rerunning the check approves the override. The README's workflow runs the check when someone adds or removes a label.
  The Claude Code hook lists an override comment once when it covers both metrics.

## 0.4.4

### Patch Changes

- a1a89b4: The GitHub Action updates its pull request comment on every run, so the comment shows the latest verdict once a fix lands.

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
