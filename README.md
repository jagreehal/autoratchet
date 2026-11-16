# AutoRatchet

**Deterministic complexity checks for AI coding agents and the people who review their code.**

AutoRatchet stops functions in your TypeScript, JavaScript, Rust, Python and Go code from getting
more complex, without asking you to fix the complex code you already have. It gives the same
verdict on your machine, in your coding agent's hooks and on every pull request, so an agent hears
about a regression while it still has the code open. Install it on a ten-year-old codebase today,
and from tomorrow no change can make it worse.

```bash
npm install -D autoratchet
npx autoratchet check --base main
```

```text
Complexity regressions detected:

src/payments.ts
  processPayment
    cyclomatic: 19 → 24 (+5)
    cognitive: 28 → 38 (+10)

Quality gate failed.
```

## Why

A complexity rule on a legacy codebase fails on hundreds of functions on day one, so teams
either never turn it on or record the existing failures as accepted ("bulk suppressions").
Suppressions count *how many* functions are over the limit in each file. A function that was
already over the limit can then get worse forever without changing the count, and CI stays
green.

AutoRatchet compares every function with the same function on your target branch instead:

- **A new function** must stay under the limit.
- **A function already over the limit** may not get any worse.
- **A function under the limit** may grow up to it.
- **Improvements stick.** The target branch is the baseline, so there is no baseline file to
  maintain, and every improvement becomes that function's new ceiling.

It measures two things, because each misses regressions the other catches:

| Metric | Measures | Limit for new code |
|--------|----------|-------------------:|
| Cyclomatic complexity ([Oxlint](https://oxc.rs/docs/guide/usage/linter), modified variant) | Decisions in a function: roughly, testing effort | 10 |
| Cognitive complexity ([SonarSource](https://www.sonarsource.com/docs/CognitiveComplexity.pdf), scored as [Biome](https://biomejs.dev/linter/rules/no-excessive-cognitive-complexity/) does) | How hard the function is to read, weighted by nesting | 15 |

## Use it

Needs a git repository. AutoRatchet is a native binary for macOS, Linux and Windows (x64 and
arm64). It scores cognitive complexity itself, and runs the [Oxlint](https://oxc.rs) version the
npm package ships with for cyclomatic complexity, so your scores match on every machine.

### Locally

```bash
npx autoratchet check --base main              # whole repo
npx autoratchet check --base main src          # only src/
npx autoratchet check --base main src ':!src/generated'
```

`check` compares your working tree, uncommitted changes included, with
`git merge-base main HEAD`. Files git ignores are skipped. Pathspecs work as in any git command.

### In GitHub pull requests

```yaml
# .github/workflows/autoratchet.yml
name: AutoRatchet
on:
  pull_request:
    types: [opened, synchronize, reopened, labeled, unlabeled]
  pull_request_review:
    types: [submitted, dismissed]

permissions:
  contents: read
  pull-requests: write

jobs:
  ratchet:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v7
      - uses: jagreehal/autoratchet@v0.4.5
        with:
          path: src
          override-approval: review
```

The action downloads the AutoRatchet binary for its own version, checked against the release's
`SHA256SUMS`, and the Oxlint binary when the paths include JavaScript or TypeScript. It needs no
Node.js. It fails the check on a regression, marks each regressed function in the PR diff,
writes the report to the job summary, and keeps one comment on the PR updated (it only
comments once there is something to say). The `labeled`, `unlabeled` and `pull_request_review`
triggers run the check again when someone adds the approval label or approves the PR.

| Input | Default | Meaning |
|-------|---------|---------|
| `path` | whole repo | Git pathspecs to measure, space separated |
| `base` | PR base | Ref to compare against |
| `threshold` | `10` | Cyclomatic limit for new functions |
| `cognitive-threshold` | `15` | Cognitive limit for new functions |
| `override-approval` | `none` | `none`, `strict`, `review` or `label` (see [Overrides](#overrides)) |
| `approval-label` | `complexity-override-approved` | The label `label` mode looks for |
| `comment` | `true` | Keep one sticky PR comment |
| `annotations` | `true` | Mark regressions in the diff |
| `fail-on-regression` | `true` | Fail the job on a regression or incomplete measurement |

Make the job a required status check so a failure blocks merging.

### With coding agents

AutoRatchet constrains the output rather than relying on instructions to the author, so it
treats human- and agent-written code the same. For Claude Code, two hooks run the check while
the agent works and hand any regression straight back to it:

```bash
mkdir -p .claude/hooks
cp node_modules/autoratchet/agent-hooks/claude-code/complexity-check.sh .claude/hooks/
cp node_modules/autoratchet/agent-hooks/claude-code/settings.json .claude/settings.json  # or merge its "hooks" block
```

After every edit, the hook checks the file against its last commit. Before the agent finishes,
it checks every changed file. It blocks on regressions, on files that do not parse, and on any
new override comment, because an agent may not approve its own exception. The hooks need `git`
and `jq`. Hooks run on the author's machine, so they are a fast feedback loop; the CI check is
the enforcement.

## Overrides

Sometimes a high score is fine: an options object with ten defaults, a fallback chain of `?.`.
Mark it on the function, with a reason:

```ts
// oxlint-disable-next-line complexity -- options bag: each default is one line, no logic
export function createChart({ width = 640, height = 400 /* … */ } = {}) {
```

```ts
// eslint-disable-next-line sonarjs/cognitive-complexity -- follows the RFC 3986 parsing steps in order
export function parseUri(input: string) {
```

`complexity` covers the cyclomatic score and `sonarjs/cognitive-complexity` the cognitive one;
name both, comma-separated, to cover both. `oxlint-` and `eslint-` spellings both work, and so
does Biome's `biome-ignore lint/complexity/noExcessiveCognitiveComplexity`. Put a
`disable-next-line` comment on the line above the function, or `disable-line` at the end of the
function's first line.

An override **approves a score; it does not stop measurement.** AutoRatchet measures a copy of
your code with every disable comment defused, so:

- The PR that adds the override approves that function's current score on that one metric,
  and the report lists the function, metric, score and reason for a reviewer.
- Every later PR compares against that score. Make the function worse and the check fails.
- A metric the comment does not name stays enforced.
- File-wide disables (`/* oxlint-disable complexity */`, `biome-ignore-all`) approve nothing.

Who may approve a new override, with `override-approval`:

| Mode | A new override passes when |
|------|----------------------------|
| `none` | Always. It is listed in the PR comment for a reviewer. |
| `strict` | Never. Every new override fails the check. |
| `review` | Someone other than the PR author, with write access, approves the PR's current head commit. A new push needs a new approval. |
| `label` | The PR has the `complexity-override-approved` label. Anyone with write access can add labels, including the author, so this keeps out agents whose token cannot label, not people. |

For an approval that the author cannot grant themselves, use `review` and also protect `main`:
require a review from someone else, dismiss stale approvals when new commits arrive, and use
CODEOWNERS so workflow and configuration changes need their owners' approval.

## Outcomes and exit codes

| Exit code | Meaning |
|----------:|---------|
| 0 | Every function measured, no regressions |
| 1 | A regression, or a new override that needs approval |
| 2 | Bad arguments |
| 3 | Measurement incomplete: a file that does not parse, or a score that cannot be matched to a named function |
| 4 | AutoRatchet itself failed |

AutoRatchet fails closed. A function it could not measure is not evidence that the function is
fine.

## Rust

AutoRatchet measures `.rs` files itself, so a Rust project needs only the binary: download it
from a GitHub release (below), or use the npm package if the project already has one. `check`,
the GitHub Action and the agent hooks work the same way; the hook finds `autoratchet` on your
`PATH` when there is no `node_modules`. Cargo's `target/` directory is skipped.

- **Cyclomatic**, the modified variant: 1, plus each `if` and `if let`, `while` and `while let`,
  `for`, `match` (one per match), match guard, `let … else`, `&&` and `||`. `?` does not count.
- **Cognitive**, after SonarSource's specification: `if`, `match` and loops cost 1 plus their
  nesting; `else`, `else if`, labelled `break`/`continue` and each run of one logical operator
  cost 1. A closure scores on its own, one level deeper than the code around it.

Functions are named `parse`, `Order::total`, `<Order as Display>::fmt`, `Validate::check`
(a trait's default method), `billing::charge` (an inline module), `charge>retry` (a function
inside a function) and `charge>items.iter().map()` or `charge>on_error` for closures.

Overrides go on the function:

```rust
#[expect(clippy::cognitive_complexity, reason = "follows the RFC 3986 parsing steps in order")]
pub fn parse_uri(input: &str) -> Option<Uri> {
```

```rust
// autoratchet-disable-next-line complexity -- one arm per status code, no logic
pub fn status_text(code: u16) -> &'static str {
```

`clippy::cognitive_complexity` (in `#[allow]` or `#[expect]`) covers the cognitive score. Rust has
no cyclomatic lint, so a comment covers that one: `complexity`, `cognitive-complexity`, or both,
comma-separated. A crate- or module-wide `#![allow(…)]` approves nothing.

## Python

AutoRatchet measures `.py` files itself and scores them the way Python's own tools do:

- **Cyclomatic** as [radon](https://radon.readthedocs.io/) scores it, plus the decisions in
  functions nested inside the function.
- **Cognitive** as [complexipy](https://github.com/rohaquinlop/complexipy) scores it, nested
  functions and recursion included.

Across five open-source Python projects and 34,000 functions, the scores match radon's and
complexipy's on over 99.9% of functions.

The functions are the ones complexipy reports: top-level functions and class methods, named
`parse`, `Order.total`, `Order.total.setter` (or `.deleter`) and `Outer.Inner.method`. A function
nested inside another one counts as part of it. AutoRatchet skips `@overload` signatures and
virtual environments (directories holding `pyvenv.cfg`).

Overrides use the comments Python developers write today, on the `def` line or on the comment
lines above it:

```python
def status_text(code):  # noqa: C901 -- one branch per status code, no logic
```

```python
# complexipy: ignore -- follows the RFC 3986 parsing steps in order
def parse_uri(text):
```

`# noqa: C901` covers the cyclomatic score; `# noqa: complexipy` and `# complexipy: ignore` the
cognitive one.

## Go

AutoRatchet measures `.go` files itself and scores them the way
[gocyclo](https://github.com/fzipp/gocyclo) and [gocognit](https://github.com/uudashr/gocognit) do,
recursion across the package included. Across five open-source Go projects, the scores match theirs
on every function with a body.

Functions are named as both tools name them: `Parse`, `(*Server).Handle`, `(Point).String`, and
`Handler` for `var Handler = func(…) {…}`. Function literals count toward the function they sit
in. Overrides use golangci-lint's directive, or gocognit's own:

```go
//nolint:gocyclo // one case per status code, no logic
func StatusText(code int) string {
```

`gocyclo` (or `cyclop`) covers the cyclomatic score and `gocognit` the cognitive one; name both,
comma-separated, to cover both. `//gocognit:ignore` also covers the cognitive score. AutoRatchet
skips Go's `vendor/` directory.

For Rust, Python and Go, a comment on the line above a function reading
`autoratchet-disable-next-line complexity, cognitive-complexity -- reason` covers the scores it names.

## Without npm

Each GitHub release has an archive per platform with the `autoratchet` binary, and a `SHA256SUMS`
file. Put the binary on your `PATH`. That is all Rust, Python and Go need. For JavaScript and
TypeScript, also install the [Oxlint](https://github.com/oxc-project/oxc/releases) 1.93.0 binary,
which measures their cyclomatic complexity. AutoRatchet looks for it in `AUTORATCHET_OXLINT`, then
in `node_modules/.bin`, then on `PATH`. Another Oxlint version can score code differently.

## CLI

```text
autoratchet check [--base <ref>] [options] [pathspec...]
autoratchet compare --base <dir|json> --head <dir|json> [options]
autoratchet measure <dir>

--threshold <n>               cyclomatic limit for new functions (default 10)
--cognitive-threshold <n>     cognitive limit for new functions (default 15)
--require-override-approval   fail on any new override instead of approving its score
--format <text|json|markdown> output format (default text)
-o, --output <file>           write the report to a file
-V, --version                 print the version
```

`--format json` is for dashboards, CI annotations and coding agents. It includes the verdict,
every regression with file and line, approvals, measurement problems, and the Markdown report.
`measure` prints every function's scores as JSON; save it and pass the file to `compare` as a
`--base` or `--head`.

The ratchet rule, applied to each metric of each function:

```ts
current <= Math.max(previous ?? 0, threshold);
```

The Rust crate in `crates/autoratchet` exposes the same pieces as a library: `git::check`,
`compare::compare_paths`, `gate::passes_complexity_gate` and the report formatters.

## Development

```bash
pnpm install
pnpm build    # cargo build --release, then stage the binary for bin/autoratchet.js
pnpm test     # Rust unit tests, then the conformance suite
pnpm bench -- --cli rust=target/release/autoratchet --repo ../some-large-repo
```

`conformance/` holds the cases every release must reproduce: each one's scores, names, reports
and exit codes. Add a case under `conformance/cases/<name>/{base,head}`, then
`pnpm conformance --update <name>` and review the expected file it writes.

## How functions are identified

Each function is matched across commits by file path and a qualified name from its syntax
(Rust's are in [Rust](#rust)): `PaymentService.validate`, `handlers.onSubmit`, `createChart>helper`,
`render>items.map()`, `PAYMENT_RULES["flagged-vip-review"]`. Moving a function to another file
or renaming it makes it look new, so it must meet the limits for new code. Callbacks and table
entries without a key are named by position, so reordering them pairs a function with another
function's history; give table entries a key (`['name', fn]`, or an `id`/`key`/`name` property)
to avoid that.

## Limits

- JavaScript, TypeScript (`.js`, `.jsx`, `.ts`, `.tsx` and the `m`/`c` variants), Rust (`.rs`),
  Python (`.py`) and Go (`.go`).
- Rust code inside macro invocations (`vec![…]`, `assert!(…)`, `macro_rules!`) is not measured.
- Two metrics: cyclomatic and cognitive complexity.
- Cognitive scores of 0 and 1 are both recorded as 1, as earlier versions recorded them.
- Getters and setters have no cognitive score of their own: their logic counts toward the
  function around them, as in Biome.
- Class field initialisers that are not functions are not measured.
- Python: the code of a class defined inside a function counts toward that function's cognitive
  score, as in complexipy, and stays out of its cyclomatic score, as in radon.
- Go: AutoRatchet skips body-less declarations (`//go:linkname`), which hold no code to measure.

## License

Apache-2.0. See [LICENSE](LICENSE).
