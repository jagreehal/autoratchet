---
name: autoratchet-go
description: Write Go that passes AutoRatchet's complexity and lint check (cyclomatic 10, cognitive 15 and no anti-slop findings for new code, and no function may get worse). Use whenever you write, edit or refactor .go code in a repo that uses AutoRatchet (an autoratchet binary or dependency, a jagreehal/autoratchet workflow, or .claude/hooks/complexity-check.sh), and whenever an AutoRatchet hook or CI check reports "regressions detected" (complexity or lint) for Go, even if the user only asks for a feature or a bug fix or mentions gocyclo, gocognit, cyclop or golangci-lint.
---

# Writing Go that passes AutoRatchet

AutoRatchet scores every function on two metrics and compares it with the same function on the
base branch. For each metric:

```go
current <= max(previous, threshold) // previous is 0 for a new function
```

- A **new** function must score at or under the limit: cyclomatic 10 and cognitive 15, unless
  `.golangci.*` at the repo root sets `gocyclo`, `cyclop` or `gocognit` limits.
- A function **under** the limit may grow up to it.
- A function **already over** the limit may not get any worse, not even by one point.
- A function that is **renamed or moved** (another file, or another receiver) counts as new.

So for each function you touch, know how many points you have to spend, and choose shapes that
spend them well.

## Before you edit a function: know its budget

The budget is how many points a function can still gain, on each metric, before the check fails:

```text
budget = max(base score, limit) − current score     # once for cyclomatic, once for cognitive
```

- **Base score**: the function's score at the commit CI compares against, the merge base of the
  target branch and `HEAD`. A new function has none, so its budget is `limit − current score`.
- **Limit**: 10 cyclomatic and 15 cognitive, unless `.golangci.*` at the repo root sets one. A failing report names the limit
  and where it came from.
- **Current score**: its score in your working tree.

So a function with a base score of 18 and a current score of 15, under a limit of 10, has 3 points
left: it may go back up to 18, its old ceiling. At base 18 and current 18 it has none. At base 6
and current 9 it has 1, because a function under the limit may grow up to it.

`measure` prints current scores. For base scores, measure a temporary checkout of the merge base (`npx autoratchet` if the repo installs it with npm):

```bash
autoratchet measure internal/orders   # current scores
# main: the PR's target branch (origin/main if there is no local main)
if base=$(git merge-base main HEAD) && tmp=$(mktemp -d)/base &&
  git worktree add --detach "$tmp" "$base"; then
  if [ -d "$tmp/internal/orders" ]; then autoratchet measure "$tmp/internal/orders"   # base scores
  else echo "internal/orders is new: no base scores"; fi
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

Functions are named as gocyclo names them: `Parse`, `(*Server).Handle`, `(Point).String`, and
`Handler` for `var Handler = func(…) {…}`. **A function literal counts toward the function it sits
in.**

When a budget is zero or less, adding a branch to that function fails the check. Put the new
behaviour where there is room: a new function or method it calls, a map entry, a new type that implements the interface. This is the most common way agents trip the check, so
look before you add.

## What costs points

**Cyclomatic** (as gocyclo scores it): starts at 1. Each `if`, `for`, `&&`, `||` and **each `case`**
of a `switch` or `select` (not `default`) adds 1. Unlike JavaScript and Rust, a long `switch` is
expensive: every case is a point.

**Cognitive** (as gocognit scores it): `if`, `switch`, `select` and `for` cost 1 **plus how deeply
they are nested**; `else if` and `else` cost 1; each run of the same boolean operator costs 1;
labelled `break`/`continue`/`goto` cost 1; recursion within the package costs 1. Function literals
add a level of nesting.

Every `if err != nil` costs a point. That is fine; keep them at the top level, where they cost 1,
rather than inside loops and branches, where they cost more.

## Shapes to write by default

- **Guard clauses and early returns.** Handle the error or edge case and return, so the main path
  stays at the left margin.

  ```go
  func CanEdit(u *User, d *Doc) bool {
  	if !u.Active || d.Locked {
  		return false
  	}
  	if u.Role == "admin" {
  		return true
  	}
  	return d.OwnerID == u.ID
  }
  ```

- **A map for value-to-value mappings**, not a `case` per value:
  `var statusText = map[int]string{200: "ok", 201: "created"}`. In Go this is the main lever on
  cyclomatic scores.
- **An interface, or a map of funcs**, for behaviour per kind, instead of a growing type switch.
- **Named predicates** for compound conditions: `func isHeavyVIP(o *Order) bool`, as real functions
  or methods, not closures (closures still count toward the caller).
- **Loop bodies in their own function** when a `for` holds branching: `for _, it := range items {
  if err := applyItem(o, it); err != nil { return err } }`.
- **One job per function**: parse, validate, compute and format as separate, named steps.

When a function needs a third level of nesting or a fifth `case`, that is the moment to switch
shape, not after the check fails.

## Don't game the metric

- Splitting a function at an arbitrary point: the new piece is new and must meet the limit anyway.
- Moving logic into a function literal: it still counts toward the outer function.
- Renaming or moving a function to escape its history: it is then held to the new-code limit.
- Raising `--threshold` or the golangci-lint settings, or disabling the linters. The check uses the
  stricter of base and working-tree config.

## Lint findings: new code gets none

AutoRatchet also counts anti-slop findings in each function (or the file's `(top level)`), with
the same ratchet and a limit of 0. The report names the rule and line.

- **No `any`/`interface{}` parameters or results**: use a named type, a generic constraint or a
  small interface. Variadic `args ...any` for fmt-style wrappers is fine.
- **No `map[string]any`**: use a struct.
- **No single-value type assertions**: `v, ok := x.(T)` or a type switch.
- **Every use of package `unsafe` needs `// SAFETY: <invariant>`** on the line above it, or above the first line of its statement.

## Override comments are for people

Some functions are fine over the limit: one case per status code, a parser that follows a spec step
by step. Those get an override comment, but **you do not add it**. A new override approves a score,
the agent hook blocks any new one, and CI may need a reviewer's approval. If you think a score is a
false positive, say so to the user, explain why, and show them what they could add:

```go
//nolint:gocyclo // one case per status code, no logic
func StatusText(code int) string {
```

`gocyclo` (or `cyclop`) covers cyclomatic and `gocognit` cognitive; `//nolint:gocyclo,gocognit`
covers both, and `//gocognit:ignore` also covers cognitive. Existing overrides stay in force for
the score they approved, so making such a function worse still fails.

The same goes for a lint finding. If one is a false positive, show the user the directive that
approves it, on the line above, naming the rule:

```go
// autoratchet-disable-next-line anti-slop-go/no-unchecked-type-assertion -- set in init()
cfg := registry["config"].(Config)
```

It approves only the findings it names on that line, once; other new findings still fail.

## Check before you hand back

```bash
autoratchet check --base main internal/orders.go    # what CI runs, uncommitted changes included
```

Exit 0 is a pass. On exit 1 the report shows each regressed function, its metric and
`before → after`; `(new function)` means the limit applied directly. Restructure using the shapes
above and run it again. Exit 3 means a file could not be measured, usually a syntax error. Exits 2
and 4 are tool errors. Then run `go test ./...`.
