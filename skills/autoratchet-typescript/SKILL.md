---
name: autoratchet-typescript
description: Write TypeScript and JavaScript that passes AutoRatchet's complexity check (cyclomatic 10, cognitive 15 for new code, and no function may get worse). Use whenever you write, edit or refactor .ts, .tsx, .js, .jsx, .mts, .cts, .mjs or .cjs code in a repo that uses AutoRatchet (autoratchet in package.json, a jagreehal/autoratchet workflow, or .claude/hooks/complexity-check.sh), and whenever an AutoRatchet hook or CI check reports "Complexity regressions detected", even if the user only asks for a feature or a bug fix.
---

# Writing TypeScript and JavaScript that passes AutoRatchet

AutoRatchet scores every function on two metrics and compares it with the same function on the
base branch. For each metric:

```ts
current <= Math.max(previous ?? 0, threshold);
```

- A **new** function must score at or under the limit: cyclomatic 10 and cognitive 15, unless
  `.oxlintrc.json` or `biome.json` at the repo root sets another number.
- A function **under** the limit may grow up to it.
- A function **already over** the limit may not get any worse, not even by one point.
- A function that is **renamed or moved** to another file counts as new.

So the job is not to write "simple code" in the abstract. It is to know, for each function you
touch, how many points you have to spend, and to choose shapes that spend them well.

## Before you edit a function: know its budget

The budget is how many points a function can still gain, on each metric, before the check fails:

```text
budget = max(base score, limit) − current score     # once for cyclomatic, once for cognitive
```

- **Base score**: the function's score at the commit CI compares against, the merge base of the
  target branch and `HEAD`. A new function has none, so its budget is `limit − current score`.
- **Limit**: 10 cyclomatic and 15 cognitive, unless `.oxlintrc.json` or `biome.json` at the repo root sets one. A failing report names the limit
  and where it came from.
- **Current score**: its score in your working tree.

So a function with a base score of 18 and a current score of 15, under a limit of 10, has 3 points
left: it may go back up to 18, its old ceiling. At base 18 and current 18 it has none. At base 6
and current 9 it has 1, because a function under the limit may grow up to it.

`measure` prints current scores. For base scores, measure a temporary checkout of the merge base:

```bash
npx autoratchet measure src/payments   # current scores
# main: the PR's target branch (origin/main if there is no local main)
if base=$(git merge-base main HEAD) && tmp=$(mktemp -d)/base &&
  git worktree add --detach "$tmp" "$base"; then
  if [ -d "$tmp/src/payments" ]; then npx autoratchet measure "$tmp/src/payments"   # base scores
  else echo "src/payments is new: no base scores"; fi
  git worktree remove "$tmp"
else
  echo "could not check out the base: no base scores to compare"
fi
```

Both print `{"functions": [{"filePath", "functionName", "line", "cyclomatic", "cognitive"}], "problems": […]}`, with
paths relative to the directory measured, so match functions by `filePath` and `functionName`. A
function missing from the base output is new, and so is every function in a directory the base
does not have: the change adds it, so each budget there is `limit − current score`. Only a
successful checkout can show that a directory is new. Any other failure is not "new": a
missing target ref (try `origin/main`), a failed checkout, exit 4 (the tool failed) or exit 3 with a `problems` list (a file in the base
does not parse) all mean the base could not be measured, so say so and rely on `check`'s report
rather than treating those functions as new. Budget against the merge base, because that is what
CI enforces; the agent hook compares with the last commit, so after a commit it can pass a change
that CI fails.

When a budget is zero or less, adding a branch to that function fails the check. Put the new
behaviour where there is room: a new, well-named function the old one calls, a table entry, or a new case of a handler that already exists. This is the most common way agents trip the check, so
look before you add.

## What costs points

**Cyclomatic** (Oxlint, modified variant): starts at 1. Each `if`, `else if`, ternary, loop
(`for`, `for…of`, `for…in`, `while`, `do`), `catch`, `&&`, `||`, `??`, `?.`, default parameter value
and logical assignment (`??=`, `||=`, `&&=`) adds 1. A whole `switch` adds **1**, however many cases.

**Cognitive** (SonarSource, scored as Biome does): `if`, ternary, `switch`, loops and `catch` cost
1 **plus how deeply they are nested**; `else` and `else if` cost 1; each run of the same logical
operator costs 1 (`a && b && c` is 1, `a && b || c` is 2). `?.`, `??` and defaults are free.

A callback (`items.map(x => …)`) is scored as its own function, named `render>items.map()`, so its
branches do not count against the outer function. Its own nesting still counts inside it.

The takeaway: nesting is what makes cognitive scores explode, and long `&&`/`||`/`?.` chains are
what push cyclomatic scores up.

## Shapes to write by default

Reach for these first. They read better, and they score low without any effort:

- **Guard clauses.** Handle the invalid and edge cases first and return. The main path stays at
  the left margin, where every `if` costs 1 instead of 1 + depth.

  ```ts
  export function canEdit(user: User, doc: Doc): boolean {
    if (!user.active) return false;
    if (doc.locked) return false;
    if (user.role === 'admin') return true;
    return doc.ownerId === user.id;
  }
  ```

- **`switch` over a union**, not an `if`/`else if` ladder on one value. With a discriminated union,
  end with `default: { const _exhaustive: never = x; throw … }` so the compiler catches new cases.
- **A `Map` for value-to-value mappings.** Not an object literal: `obj['constructor']` returns a
  function instead of the default.

  ```ts
  const ZONES = new Map([['GB', 'domestic'], ['FR', 'eu'], ['US', 'americas']]);
  export const zone = (country: string) => ZONES.get(country) ?? 'rest-of-world';
  ```

- **Named predicates** for compound conditions: `withinReturnWindow(r) || coveredByWarranty(r)`
  instead of three lines of `&&` and `||`.
- **A decision table** for ordered first-match-wins rules: `[outcome, (x) => boolean]` pairs and
  `.find()`. Give each entry a key (`['flagged-review', fn]`): table functions are named by key,
  and unkeyed ones by position, so reordering would pair a rule with another rule's history.
- **One job per function.** Validate, compute and format as separate, named steps.

When a function you are writing starts to need a third level of nesting or a fourth branch on the
same value, that is the moment to switch shape, not after the check fails.

## Don't game the metric

The check exists so humans can review agent code. Tricks that lower the number without making the
code clearer defeat that, and some do not even work:

- Splitting a function at an arbitrary line: the new piece is new and must meet the limit anyway.
- Renaming or moving a function to escape its history: it is then held to the stricter new-code
  limit.
- Replacing `if` with `&&` short-circuits or nested ternaries, or hiding logic in a callback.
- Raising `--threshold`, editing the limits in linter config, or switching the rule `off`. The
  check uses the stricter of the base and working-tree config, so loosening it does nothing.

## Override comments are for people

Some functions are fine over the limit: an options bag with ten defaults, a `?.`/`??` fallback
chain over a loose API response. Those get an override comment, but **you do not add it**. A new
override approves a score, the agent hook blocks any new one, and CI may need a reviewer's
approval. If you think a score is a false positive, say so to the user, explain why, and show them
the comment they could add:

```ts
// oxlint-disable-next-line complexity -- options bag: each default is one line, no logic
export function createChart({ width = 640, height = 400 /* … */ } = {}) {
```

`complexity` covers cyclomatic and `sonarjs/cognitive-complexity` covers cognitive; name both,
comma-separated, for both. `eslint-` and `biome-ignore lint/complexity/noExcessiveCognitiveComplexity`
spellings work too. Existing overrides stay in force for the score they approved, so making such a
function worse still fails.

## Check before you hand back

```bash
npx autoratchet check --base main src/payments.ts    # what CI runs, uncommitted changes included
```

Exit 0 is a pass. On exit 1 the report shows each regressed function, its metric and
`before → after`; `(new function)` means the limit applied directly. Restructure using the shapes
above and run it again. Exit 3 means a file could not be measured, usually because it does not
parse: fix the syntax first. Exits 2 and 4 are tool errors, not your code. Then run the tests: a
refactor that changes behaviour is worse than a complexity point.
