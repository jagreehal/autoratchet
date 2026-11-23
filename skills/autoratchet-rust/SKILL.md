---
name: autoratchet-rust
description: Write Rust that passes AutoRatchet's complexity and lint check (cyclomatic 10, cognitive 15 and no anti-slop findings for new code, and no function may get worse). Use whenever you write, edit or refactor .rs code in a repo that uses AutoRatchet (an autoratchet binary or dependency, a jagreehal/autoratchet workflow, or .claude/hooks/complexity-check.sh), and whenever an AutoRatchet hook or CI check reports "regressions detected" (complexity or lint) for Rust, even if the user only asks for a feature or a bug fix or mentions clippy's cognitive_complexity.
---

# Writing Rust that passes AutoRatchet

AutoRatchet scores every function on two metrics and compares it with the same function on the
base branch. For each metric:

```text
current <= max(previous, threshold)      # previous is 0 for a new function
```

- A **new** function must score at or under the limit: cyclomatic 10 and cognitive 15, unless
  `clippy.toml` at the repo root sets `cognitive-complexity-threshold`.
- A function **under** the limit may grow up to it.
- A function **already over** the limit may not get any worse, not even by one point.
- A function that is **renamed or moved** (another file, module or `impl`) counts as new.

So for each function you touch, know how many points you have to spend, and choose shapes that
spend them well.

## Before you edit a function: know its budget

The budget is how many points a function can still gain, on each metric, before the check fails:

```text
budget = max(base score, limit) − current score     # once for cyclomatic, once for cognitive
```

- **Base score**: the function's score at the commit CI compares against, the merge base of the
  target branch and `HEAD`. A new function has none, so its budget is `limit − current score`.
- **Limit**: 10 cyclomatic and 15 cognitive, unless `clippy.toml` at the repo root sets the cognitive one. A failing report names the limit
  and where it came from.
- **Current score**: its score in your working tree.

So a function with a base score of 18 and a current score of 15, under a limit of 10, has 3 points
left: it may go back up to 18, its old ceiling. At base 18 and current 18 it has none. At base 6
and current 9 it has 1, because a function under the limit may grow up to it.

`measure` prints current scores. For base scores, measure a temporary checkout of the merge base (`npx autoratchet` if the repo installs it with npm):

```bash
autoratchet measure src/orders   # current scores
# main: the PR's target branch (origin/main if there is no local main)
if base=$(git merge-base main HEAD) && tmp=$(mktemp -d)/base &&
  git worktree add --detach "$tmp" "$base"; then
  if [ -d "$tmp/src/orders" ]; then autoratchet measure "$tmp/src/orders"   # base scores
  else echo "src/orders is new: no base scores"; fi
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

Functions are named `parse`, `Order::total`, `<Order as Display>::fmt`, `Validate::check`,
`billing::charge`, `charge>retry` (nested fn) and `charge>items.iter().map()` (closure).

When a budget is zero or less, adding a branch to that function fails the check. Put the new
behaviour where there is room: a new function it calls, a method on the type, a new enum variant with its own arm. This is the most common way agents trip the check, so
look before you add.

## What costs points

**Cyclomatic** (modified variant): starts at 1. Each `if` / `if let`, `while` / `while let`, `for`,
`let … else`, match guard (`x if …`), `&&` and `||` adds 1. A whole `match` adds **1**, however many
arms. `?` is free.

**Cognitive** (SonarSource): `if`, `match` and loops cost 1 **plus how deeply they are nested**;
`let … else` also costs 1 plus nesting; `else`, `else if`, labelled `break`/`continue` and each run
of one logical operator cost 1. A
closure is scored as its own function, one level deeper than the code around it.

Code inside macro invocations (`vec![…]`, `assert!(…)`, `macro_rules!`) is not measured.

The takeaway: nested `if let` and `match` inside loops is what makes cognitive scores explode;
`?` and flat `match`es are cheap.

## Shapes to write by default

- **`?` and early returns** for `Option` and `Result`, not nested `if let` / `match`. Use
  `let Some(x) = y else { return … };` to keep the happy path unindented. It is not free (1
  cyclomatic, 1 + nesting cognitive), so use it at the top of the function, not inside loops;
  `?` is the only free option.

  ```rust
  pub fn shipping(order: &Order) -> Result<Rate, Error> {
      let address = order.address.as_ref().ok_or(Error::NoAddress)?;
      let zone = zone_for(&address.country)?;
      Ok(rate(zone, order.weight))
  }
  ```

- **One `match`**, not an `if`/`else if` ladder. Match on a tuple (`match (kind, express)`) instead
  of nesting conditions. Prefer patterns (`"FR" | "DE"`, ranges) over guards, since each guard costs
  a point.
- **An enum with methods** for behaviour that varies by kind: `impl Kind { fn rate(&self) -> u32 }`
  keeps the branching in one cheap `match`, out of the callers.
- **A `match` that returns data**, or a `const` slice of pairs, for value-to-value mappings.
- **Iterator adapters** (`.any()`, `.find()`, `.filter_map()`, `.sum()`) instead of `for` + `if` +
  `break`. Keep closures short; their nesting still counts inside them.
- **Named predicates** for compound conditions: `fn is_heavy_vip(order: &Order) -> bool`.
- **One job per function**: parse, validate, compute and format as separate, named steps.

When a function needs a third level of nesting or a second `match` on related values, that is the
moment to switch shape, not after the check fails.

## Don't game the metric

- Splitting a function at an arbitrary point: the new piece is new and must meet the limit anyway.
- Renaming or moving a function to escape its history: it is then held to the new-code limit.
- Moving logic into a macro to hide it from measurement.
- Raising `--threshold` or `cognitive-complexity-threshold`, or adding a crate- or module-wide
  `#![allow(clippy::cognitive_complexity)]`. The check uses the stricter of base and working-tree
  config, and module-wide allows approve nothing.

## Lint findings: new code gets none

AutoRatchet also counts anti-slop findings in each function (or the file's `(top level)`), with
the same ratchet and a limit of 0. The report names the rule and line.

- **No `.unwrap()`** outside tests (`#[cfg(test)]`, `tests/`, `benches/`, `*_test*.rs`):
  propagate with `?`, handle the case, or `expect("<the invariant that makes this safe>")`.
  `.lock().unwrap()` is flagged too (the rule cannot see that the receiver is a mutex): write `.lock().expect("lock poisoned")`, and the same for `RwLock`'s `read()`/`write()`.
- **Every `unsafe` block and `unsafe impl` needs `// SAFETY: <invariant>`** on the line above it, or above the first line of its statement.
- **No `mem::transmute`**: convert with `from_*`/`to_*` methods or a parser.
- **No `dyn Any` in signatures**: take and return a named type or an enum of the cases.
  The `dyn Any + Send` panic payload is exempt.

## Override attributes and comments are for people

Some functions are fine over the limit: one arm per status code, a parser that follows a spec step
by step. Those get an override, but **you do not add it**. A new override approves a score, the
agent hook blocks any new one, and CI may need a reviewer's approval. If you think a score is a
false positive, say so to the user, explain why, and show them what they could add:

```rust
#[expect(clippy::cognitive_complexity, reason = "follows the RFC 3986 parsing steps in order")]
pub fn parse_uri(input: &str) -> Option<Uri> {
```

```rust
// autoratchet-disable-next-line complexity -- one arm per status code, no logic
pub fn status_text(code: u16) -> &'static str {
```

`clippy::cognitive_complexity` covers cognitive; Rust has no cyclomatic lint, so the comment covers
`complexity`, `cognitive-complexity`, or both. Existing overrides stay in force for the score they
approved, so making such a function worse still fails.

The same goes for a lint finding. If one is a false positive, show the user the directive that
approves it, on the line above, naming the rule:

```rust
// autoratchet-disable-next-line anti-slop-rust/no-unwrap -- the regex is a literal checked by tests
let re = Regex::new(r"^\d+$").unwrap();
```

It approves only the findings it names on that line, once; other new findings still fail.

## Check before you hand back

```bash
autoratchet check --base main src/orders.rs     # what CI runs, uncommitted changes included
```

Exit 0 is a pass. On exit 1 the report shows each regressed function, its metric and
`before → after`; `(new function)` means the limit applied directly. Restructure using the shapes
above and run it again. Exit 3 means a file could not be measured, usually a parse error. Exits 2
and 4 are tool errors. Then run `cargo test` (and `cargo clippy` if the project uses it).
AutoRatchet scores Rust itself, so trust its numbers over Clippy's.
