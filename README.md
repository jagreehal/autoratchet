# AutoRatchet

**Prevent code quality regressions, one pull request at a time.**

AutoRatchet stops functions in your TypeScript and JavaScript code from getting more complex,
without asking you to fix the complex code you already have. Install it on a ten-year-old
codebase today, and from tomorrow no pull request can make it worse.

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
| Cognitive complexity ([Biome](https://biomejs.dev/linter/rules/no-excessive-cognitive-complexity/)) | How hard the function is to read, weighted by nesting | 15 |

New to these metrics? The companion guide,
[Cyclomatic and cognitive complexity, explained](https://github.com/jagreehal/cyclomatic-and-cognitive-complexity-explained),
teaches both from scratch with runnable examples, including false positives worth overriding
and how quality gates get gamed.

## Use it

Needs Node.js 24 or newer and a git repository.

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
      - uses: actions/setup-node@v7
        with:
          node-version: 24
      - uses: jagreehal/autoratchet@v0.1.0
        with:
          path: src
          override-approval: review
```

The action fails the check on a regression, marks each regressed function in the PR diff,
writes the report to the job summary, and keeps one comment on the PR updated (it only
comments once there is something to say).

| Input | Default | Meaning |
|-------|---------|---------|
| `path` | whole repo | Git pathspecs to measure, space separated |
| `base` | PR base | Ref to compare against |
| `threshold` | `10` | Cyclomatic limit for new functions |
| `cognitive-threshold` | `15` | Cognitive limit for new functions |
| `override-approval` | `none` | `none`, `strict`, `review` or `label` (see [Overrides](#overrides)) |
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
// biome-ignore lint/complexity/noExcessiveCognitiveComplexity: follows the RFC 3986 parsing steps in order
export function parseUri(input: string) {
```

An override **approves a score; it does not stop measurement.** AutoRatchet measures a copy of
your code with every disable comment defused, so:

- The PR that adds the override approves that function's current score on that one metric,
  and the report lists the function, metric, score and reason for a reviewer.
- Every later PR compares against that score. Make the function worse and the check fails.
- The other metric stays enforced. An Oxlint comment covers cyclomatic only, `biome-ignore`
  covers cognitive only.
- File-wide disables (`/* oxlint-disable */`, `biome-ignore-all`) approve nothing.

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

## CLI

```text
autoratchet check [--base <ref>] [options] [pathspec...]
autoratchet compare --base <dir|json> --head <dir|json> [options]

--threshold <n>               cyclomatic limit for new functions (default 10)
--cognitive-threshold <n>     cognitive limit for new functions (default 15)
--require-override-approval   fail on any new override instead of approving its score
--format <text|json|markdown> output format (default text)
-o, --output <file>           write the report to a file
```

`--format json` is for dashboards, CI annotations and coding agents. It includes the verdict,
every regression with file and line, approvals, measurement problems, and the Markdown report.

## API

```ts
import { check, formatReport, verdict } from 'autoratchet';

const result = await check({ base: 'main', paths: ['src'] });
if (verdict(result) !== 'pass') {
  console.error(formatReport(result));
  process.exitCode = 1;
}
```

`compareSources` and `comparePaths` compare two sets of files directly, and `passesComplexityGate`
is the ratchet rule on its own:

```ts
current <= Math.max(previous ?? 0, threshold);
```

## How functions are identified

Each function is matched across commits by file path and a qualified name from its syntax:
`PaymentService.validate`, `handlers.onSubmit`, `createChart>helper`,
`render>items.map()`, `PAYMENT_RULES["flagged-vip-review"]`. Moving a function to another file
or renaming it makes it look new, so it must meet the limits for new code. Callbacks and table
entries without a key are named by position, so reordering them pairs a function with another
function's history; give table entries a key (`['name', fn]`, or an `id`/`key`/`name` property)
to avoid that.

## Limits

- JavaScript and TypeScript only (`.js`, `.jsx`, `.ts`, `.tsx` and the `m`/`c` variants).
- Two metrics: cyclomatic and cognitive complexity.
- Biome does not report cognitive scores of 0 or 1, so both are recorded as 1.
- Class field initialisers that are not functions are not measured.

## License

Apache-2.0. See [LICENSE](LICENSE).
