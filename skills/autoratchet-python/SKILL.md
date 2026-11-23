---
name: autoratchet-python
description: Write Python that passes AutoRatchet's complexity and lint check (cyclomatic 10, cognitive 15 and no anti-slop findings for new code, and no function may get worse). Use whenever you write, edit or refactor .py code in a repo that uses AutoRatchet (an autoratchet binary or dependency, a jagreehal/autoratchet workflow, or .claude/hooks/complexity-check.sh), and whenever an AutoRatchet hook or CI check reports "regressions detected" (complexity or lint) for Python, even if the user only asks for a feature or a bug fix or mentions radon, complexipy or ruff C901.
---

# Writing Python that passes AutoRatchet

AutoRatchet scores every function on two metrics and compares it with the same function on the
base branch. For each metric:

```python
current <= max(previous or 0, threshold)
```

- A **new** function must score at or under the limit: cyclomatic 10 and cognitive 15, unless
  ruff config at the repo root sets `lint.mccabe.max-complexity`.
- A function **under** the limit may grow up to it.
- A function **already over** the limit may not get any worse, not even by one point.
- A function that is **renamed or moved** (another file or class) counts as new.

So for each function you touch, know how many points you have to spend, and choose shapes that
spend them well.

## Before you edit a function: know its budget

The budget is how many points a function can still gain, on each metric, before the check fails:

```text
budget = max(base score, limit) − current score     # once for cyclomatic, once for cognitive
```

- **Base score**: the function's score at the commit CI compares against, the merge base of the
  target branch and `HEAD`. A new function has none, so its budget is `limit − current score`.
- **Limit**: 10 cyclomatic and 15 cognitive, unless ruff config at the repo root sets the cyclomatic one. A failing report names the limit
  and where it came from.
- **Current score**: its score in your working tree.

So a function with a base score of 18 and a current score of 15, under a limit of 10, has 3 points
left: it may go back up to 18, its old ceiling. At base 18 and current 18 it has none. At base 6
and current 9 it has 1, because a function under the limit may grow up to it.

`measure` prints current scores. For base scores, measure a temporary checkout of the merge base (`npx autoratchet` if the repo installs it with npm):

```bash
autoratchet measure app/orders   # current scores
# main: the PR's target branch (origin/main if there is no local main)
if base=$(git merge-base main HEAD) && tmp=$(mktemp -d)/base &&
  git worktree add --detach "$tmp" "$base"; then
  if [ -d "$tmp/app/orders" ]; then autoratchet measure "$tmp/app/orders"   # base scores
  else echo "app/orders is new: no base scores"; fi
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

Functions are top-level functions and methods: `parse`, `Order.total`, `Order.total.setter`,
`Outer.Inner.method`. **A function nested inside another counts as part of it**, and so does a
lambda.

When a budget is zero or less, adding a branch to that function fails the check. Put the new
behaviour where there is room: a new module-level function or method it calls, a dict entry, a new handler class. This is the most common way agents trip the check, so
look before you add.

## What costs points

**Cyclomatic** (as radon scores it): starts at 1. Each `if`, `elif`, `for`, `while`, `except`,
conditional expression (`a if c else b`), comprehension `for` and `if`, `and`, `or`, `assert`, the
`else` of a `for`, `while` or `try`, and **each `case`** of a `match` (a wildcard `case _` is free)
adds 1. An `assert` costs 1 however compound its condition, and a nested function's decisions count
toward the function around it. Unlike JavaScript and Rust, a long
`match` or `elif` chain is expensive: every branch is a point.

**Cognitive** (as complexipy scores it): `if`, `match`, loops, `except` and conditional expressions
cost 1 **plus how deeply they are nested**; `elif` and `else` cost 1; each run of the same boolean
operator costs 1; recursion costs 1.

The takeaway: nesting is what makes cognitive scores explode, and branch-per-value code is what
pushes cyclomatic scores up.

## Shapes to write by default

- **Guard clauses.** Return or `raise` for invalid and edge cases first, so the main path is not
  nested.

  ```python
  def can_edit(user, doc):
      if not user.active:
          return False
      if doc.locked:
          return False
      return user.role == "admin" or doc.owner_id == user.id
  ```

- **A dict for value-to-value mappings**, not `elif` or `match` per value:
  `STATUS = {200: "ok", 201: "created"}` and `STATUS.get(code, "unknown")`. In Python this is the
  main lever on cyclomatic scores.
- **A dispatch dict of callables** for behaviour per kind: `HANDLERS[kind](order)`, or a method
  per class when each case carries real behaviour.
- **Named predicates** for compound conditions: `def is_heavy_vip(order) -> bool:`, at module
  level or as a method, never nested inside the caller (nested ones still count toward it).
- **`any()`, `all()`, `next(…, default)`, `sum()`** instead of `for` + `if` + `break`.
- **One job per function**: parse, validate, compute and format as separate, named functions.

When a function needs a third level of nesting or a fourth branch on the same value, that is the
moment to switch shape, not after the check fails.

## Don't game the metric

- Splitting a function at an arbitrary point: the new piece is new and must meet the limit anyway.
- Moving logic into a nested `def` or a lambda: it still counts toward the outer function.
- Renaming or moving a function to escape its history: it is then held to the new-code limit.
- Raising `--threshold` or `max-complexity`, or removing the rule. The check uses the stricter of
  base and working-tree config.

## Lint findings: new code gets none

AutoRatchet also counts anti-slop findings in each function (or the module's `(top level)`), with
the same ratchet and a limit of 0. The report names the rule and line.

- **No `Any` or `object` parameters, no `Any` returns**: take and return a named type (a
  dataclass, TypedDict or Protocol) and parse raw data where it enters. `*args`/`**kwargs` and
  `TypeGuard`/`TypeIs` functions and dunder methods (`__eq__(self, other: object)`) are exempt.
- **No `dict[str, Any]`** (or `Mapping`/`object` variants): use a TypedDict or dataclass.
- **Every `cast(...)` needs `# SAFETY: <the checked invariant>`** on the line above it, or above the first line of its statement.

## Override comments are for people

Some functions are fine over the limit: one branch per status code, a parser that follows a spec
step by step. Those get an override comment, but **you do not add it**. A new override approves a
score, the agent hook blocks any new one, and CI may need a reviewer's approval. If you think a
score is a false positive, say so to the user, explain why, and show them what they could add:

```python
def status_text(code):  # noqa: C901 -- one branch per status code, no logic
```

```python
# complexipy: ignore -- follows the RFC 3986 parsing steps in order
def parse_uri(text):
```

`# noqa: C901` covers cyclomatic; `# noqa: complexipy` and `# complexipy: ignore` cover cognitive.
Existing overrides stay in force for the score they approved, so making such a function worse
still fails.

The same goes for a lint finding. If one is a false positive, show the user the directive that
approves it, on the line above, naming the rule:

```python
# autoratchet-disable-next-line anti-slop-python/require-safety-comment-for-cast -- validated by the schema above
user = cast(User, payload)
```

It approves only the findings it names on that line, once; other new findings still fail.

## Check before you hand back

```bash
autoratchet check --base main app/orders.py     # what CI runs, uncommitted changes included
```

Exit 0 is a pass. On exit 1 the report shows each regressed function, its metric and
`before → after`; `(new function)` means the limit applied directly. Restructure using the shapes
above and run it again. Exit 3 means a file could not be measured, usually a syntax error. Exits 2
and 4 are tool errors. Then run the tests. AutoRatchet scores Python itself, so trust its numbers
over `ruff check`'s.
