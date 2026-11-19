---
name: autoratchet-setup
description: Set up AutoRatchet, the complexity ratchet for TypeScript, JavaScript, Rust, Python and Go, in a repository. Covers installing it, the first check, the GitHub Action, the Claude Code hooks, the per-language skills and limits from the existing linter config. Use when the user asks to add, install, configure or roll out AutoRatchet, or asks for a complexity ratchet that stops functions getting more complex in CI or in agent hooks. Not for tuning an existing linter's complexity rule on its own.
---

# Setting up AutoRatchet

AutoRatchet compares every function with the same function on the target branch. A new function
must stay under the limit (cyclomatic 10, cognitive 15 by default), a function already over it may
not get worse, and there is no baseline file. That is why setup is short: there is nothing to
record and no existing code to fix first.

## Do what was asked, offer the rest

Setup has several independent parts, and each one changes different files. Do only the parts the
user asked for:

| The user asked for | Do |
|--------------------|----|
| "Install AutoRatchet", "try it", "run it locally" | Steps 1–3 |
| "Add it to CI", "a complexity check on PRs" | Steps 1 and 4 (and 5 if limits came up) |
| "Agent hooks", "stop Claude making code more complex" | Steps 1–3 and 6 (the hook runs the local CLI) |
| "The skills", "teach the agent" | Steps 1 and 7 |
| "Set up AutoRatchet" or "roll it out", with no narrower scope | Steps 1–3, then ask which of 4, 6 and 7 they want |

A request about an existing complexity rule (raise Ruff's `max-complexity`, turn on Biome's rule)
is a linter change, not an AutoRatchet setup: make that change and, at most, mention AutoRatchet in
one line. The GitHub Action downloads its own binary, so CI alone adds no dependency: install
locally (step 2) only when the user also wants to run it locally, wants the hooks, or the workflow
they want calls the local CLI. After the steps you did, list the ones you skipped in one line each, so the user can ask
for them. Tell the user what you changed at each step.

## 1. Look at the repo

Find out, before changing anything:

- **Languages**: which of `.ts/.tsx/.js/.jsx/.mts/.cts/.mjs/.cjs`, `.rs`, `.py`, `.go` the repo
  holds (`git ls-files`). Other languages are not measured.
- **Package manager**: is there a root `package.json`, and which lockfile (`pnpm-lock.yaml`,
  `package-lock.json`, `yarn.lock`, `bun.lock`)?
- **Default branch**: `git symbolic-ref refs/remotes/origin/HEAD`, else `main`.
- **Linter limits already set** at the repo root (AutoRatchet reads these, see step 5):
  `.oxlintrc.json`, `biome.json(c)`, `ruff.toml`/`.ruff.toml`/`pyproject.toml`,
  `clippy.toml`/`.clippy.toml`, `.golangci.*`.
- **Generated or vendored code** that git includes but should not be measured (`src/generated`,
  protobuf output, a committed `vendor/`), and whether `.gitignore` covers `node_modules`,
  `target/` and virtualenvs. `check` measures whatever git lists (step 3).

## 2. Install

**A repo with a root `package.json`** (any language, including Rust/Python/Go in a JS monorepo):
add it as a dev dependency with the repo's own package manager, e.g.

```bash
npm install -D autoratchet        # or: pnpm add -D / yarn add -D / bun add -d
```

It needs Node 24 or later. The package ships the native binary and the Oxlint version used for
JS/TS cyclomatic scores, so everyone gets the same numbers.

**Rust, Python or Go with no `package.json`**: use the release binary. Archives are
`autoratchet-<platform>.tar.gz` with `<platform>` one of `darwin-arm64`, `darwin-x64`,
`linux-x64`, `linux-arm64`, `win32-x64`, `win32-arm64`, next to a `SHA256SUMS` file:

```bash
base=https://github.com/jagreehal/autoratchet/releases/latest/download
p=darwin-arm64   # the platform, from the list above
curl -fsSLO "$base/autoratchet-$p.tar.gz" &&
  curl -fsSLO "$base/SHA256SUMS" &&
  grep " ./autoratchet-$p.tar.gz\$" SHA256SUMS | sed 's| \./| |' | shasum -a 256 -c - &&
  tar -xzf "autoratchet-$p.tar.gz"   # then put autoratchet on PATH
```

Keep it one `&&` chain: `tar` must run only if both downloads succeeded and the checksum matched
(a mismatch or a missing line both fail `shasum -c`). On Linux, `sha256sum -c -` does the same.

Verify the checksum; do not skip it. Ask the user where they want the binary if it is not obvious
(`~/.local/bin`, a tools directory). JS/TS without npm also needs the Oxlint 1.93.0 binary, found
via `AUTORATCHET_OXLINT`, `node_modules/.bin` or `PATH`; prefer the npm route for JS/TS.

## 3. First run

```bash
npx autoratchet check --base main          # or: autoratchet check --base main
```

On a clean branch this should pass (exit 0): nothing changed, so nothing regressed. If it exits
3, a file did not parse or could not be measured; show the user which. Exclude generated code with
pathspecs (`npx autoratchet check --base main src ':!src/generated'`) and use the same pathspecs
in CI (`path`) and in the hooks (`AUTORATCHET_PATHS`).

`check` measures what git lists: tracked files, and untracked files git does not ignore. Beyond
that it skips only Go files under a `vendor/` directory. `node_modules`, Cargo's `target/` and
virtualenvs are skipped only because `.gitignore` normally lists them, so check that it does. Any
generated or vendored code git does include (a committed `vendor/` of JavaScript or Python,
protobuf output, a committed build directory) needs its own exclusion, e.g. `':!vendor'`.

To show the user where the code stands today, `autoratchet measure <dir>` prints every function's
scores as JSON; count how many are over the limits. That number is debt they keep, not something
they must fix.

## 4. GitHub Action

Create `.github/workflows/autoratchet.yml`. Pin the action to the version installed in step 2, or,
for CI alone, to the latest release (`npm view autoratchet version` or
`gh release view --repo jagreehal/autoratchet --json tagName -q .tagName`; both only read):

```yaml
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
      - uses: jagreehal/autoratchet@vX.Y.Z
        with:
          path: src                  # omit for the whole repo; space-separated pathspecs
          override-approval: review
```

The action needs no Node and no `fetch-depth` setting. Inputs worth asking about:

- `override-approval`: who may approve a new `// …-disable-next-line complexity -- reason`
  comment. `none` lists it for reviewers, `strict` fails every new one, `review` (recommended)
  needs an approval from someone other than the PR author on the current head commit, `label`
  needs the `complexity-override-approved` label.
- `threshold` / `cognitive-threshold`: leave out so the linter config applies (step 5).
- `comment`, `annotations`, `fail-on-regression`: default `true`.

Tell the user to make the job a **required status check** in branch protection; that is a
repository setting you cannot change from the workflow. With `review`, also suggest requiring a
review from someone else and dismissing stale approvals.

## 5. Limits

Do not invent limits. If the repo's linter already sets one, AutoRatchet uses it for new code in
that language, and the report names the file it came from:

| Language | File | Cyclomatic | Cognitive |
|----------|------|------------|-----------|
| JS/TS | `.oxlintrc.json` | `complexity` `max`, `sonarjs/cyclomatic-complexity` | `sonarjs/cognitive-complexity` |
| JS/TS | `biome.json(c)` | | `noExcessiveCognitiveComplexity` `maxAllowedComplexity` |
| Python | ruff config | `lint.mccabe.max-complexity` | |
| Rust | `clippy.toml` | | `cognitive-complexity-threshold` |
| Go | `.golangci.*` | `gocyclo`/`cyclop` | `gocognit` `min-complexity` |

Only root config is read, only explicit numbers count, and a rule that is `off` is ignored. Keep
the linter rule at `warn` if it should not fail lint. Otherwise the defaults are 10 and 15. Only
set `--threshold` / `--cognitive-threshold` if the user asks for different limits, because a flag
overrides every language's config.

## 6. Claude Code hooks

Only if the user asked for agent hooks (or for a full rollout, and said yes). They make the agent
hear about a regression after each edit and before it finishes. Copy only the hook here; skills
are step 7.

```bash
mkdir -p .claude/hooks
cp node_modules/autoratchet/agent-hooks/claude-code/complexity-check.sh .claude/hooks/   # npm
cp "$src/agent-hooks/claude-code/complexity-check.sh" .claude/hooks/                    # binary install
```

For a binary install, `$src` comes from [the release source](#the-release-source). Then **merge**
the `hooks` block of `agent-hooks/claude-code/settings.json` (from the same place) into
`.claude/settings.json`; do not overwrite settings that are already there. The hooks need `git` and
`jq`; check that `jq` is installed.

To match CI, set two variables in the `env` block of `.claude/settings.json`, and keep them apart:

- `AUTORATCHET_PATHS`: the workflow's `path` pathspecs, e.g. `src :!src/generated`. The hook then
  checks exactly the changed files these select, as CI does. Without it, the hook checks every
  changed source file except under `node_modules`, `dist`, `build`, `coverage`, `target`, `vendor`,
  virtualenvs and `generated` directories, which is narrower than CI with no `path`. If the
  workflow sets `path`, or the repo measures any of those directories, set this.
- `AUTORATCHET_ARGS`: flags only, e.g. `--threshold 12`. Never pathspecs: git combines pathspecs,
  so `src` here would add all of `src` to every check after each edit, and changed files outside
  `src` would still be checked.

## 7. Agent skills

Only if the user asked for them (or for a full rollout, and said yes). Install the skills for the
languages found in step 1, and nothing else: no hooks, no CI. They teach the agent to write code
that passes in the first place. Skills ship from AutoRatchet 0.6.0.

With the [skills CLI](https://github.com/vercel-labs/skills), which needs Node but no
`package.json`, and also targets Cursor, Codex, Copilot and other agents (`-a <agent>`):

```bash
npx skills add jagreehal/autoratchet --skill autoratchet-typescript   # repeat per language
```

Or copy them, from the npm package or, without Node, from [the release source](#the-release-source):

```bash
mkdir -p .claude/skills
cp -r node_modules/autoratchet/skills/autoratchet-typescript .claude/skills/   # npm, JS/TS
cp -r "$src/skills/autoratchet-rust" .claude/skills/                           # release source, Rust
```

The four are `autoratchet-typescript`, `autoratchet-python`, `autoratchet-rust` and
`autoratchet-go`. If the repo has an `AGENTS.md` or `CLAUDE.md`, offer to add one line saying the
complexity check runs on every PR and that new override comments are for people to add, not agents.

### The release source

A binary install has no `node_modules`. Steps 6 and 7 then copy their files from the source
archive of the same version as the binary, so the hook matches the CLI it calls. Download it once,
copy only what the step you are doing needs, and delete it afterwards:

```bash
v=$(autoratchet --version | cut -d' ' -f2)        # e.g. 0.6.0; skills need 0.6.0 or later
src=$(mktemp -d)
curl -fsSL -o "$src/source.tar.gz" "https://github.com/jagreehal/autoratchet/archive/refs/tags/v$v.tar.gz" &&
  tar -xzf "$src/source.tar.gz" -C "$src" --strip-components=1   # copy only if this succeeded
# … the copy commands from step 6 or 7 …
rm -rf "$src"
```

With no binary installed (skills only, no Node), use the latest release's tag instead of `$v`.

## 8. Hand over

Summarise for the user: what was installed and where, the limits in force and where each came
from, and each file you added or changed. List the parts you did not do in one line each (CI,
hooks, skills), and the manual step left if you added CI (making the check required). Do not
commit unless they ask.
