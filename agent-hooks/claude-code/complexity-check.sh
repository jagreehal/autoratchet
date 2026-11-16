#!/usr/bin/env bash
# Claude Code hook: run AutoRatchet on what Claude changed, against the last commit.
#   complexity-check.sh edited   PostToolUse (Edit|Write|MultiEdit): the one file just changed, in about a second.
#   complexity-check.sh stop     Stop: every changed source file, once per turn, before Claude hands back.
# Silent when clean. On problems: exit 2 with the report on stderr, which Claude reads and acts on.
# Blocks on what CI would fail (a function over its limit that got worse, a new function over a limit,
# a function that crossed a limit, a file that cannot be measured) and on any new complexity override
# comment: a new override approves a score, and an agent may not approve its own. A person commits those.
# Needs: jq, git, and AutoRatchet (node_modules/.bin/autoratchet, or autoratchet on PATH).
set -uo pipefail

mode=${1:-edited}
input=$(cat)
cd "${CLAUDE_PROJECT_DIR:-.}" || exit 0
# The project's npm install, else an autoratchet on PATH (Rust projects install the binary).
ratchet=${AUTORATCHET:-node_modules/.bin/autoratchet}
[ -x "$ratchet" ] || ratchet=$(command -v autoratchet) || exit 0
git rev-parse --verify -q HEAD >/dev/null || exit 0 # no commit to compare with yet

is_source() {
  case "$1" in
    node_modules/* | dist/* | build/* | coverage/* | target/* | */target/* | */generated/* | *.d.ts) return 1 ;;
    *.ts | *.tsx | *.js | *.jsx | *.mts | *.cts | *.mjs | *.cjs | *.rs) return 0 ;;
    *) return 1 ;;
  esac
}

files=()
if [ "$mode" = "stop" ]; then
  # A problem Claude could not fix must not trap it in a loop: report, don't block twice.
  active=$(printf '%s' "$input" | jq -r '.stop_hook_active // false')
  while IFS= read -r f; do
    [ -f "$f" ] && is_source "$f" && files+=("$f")
  done < <(git status --porcelain --untracked-files=all | sed -E 's/^.{3}//; s/.* -> //')
else
  active=false
  f=$(printf '%s' "$input" | jq -r '.tool_input.file_path // .tool_response.filePath // empty')
  f=${f#"$PWD"/}
  [ -n "$f" ] && [ -f "$f" ] && is_source "$f" && files+=("$f")
fi
[ ${#files[@]} -gt 0 ] || exit 0

# shellcheck disable=SC2086 # split on purpose, e.g. "--threshold 12 --cognitive-threshold 18"
report=$("$ratchet" check --base HEAD ${AUTORATCHET_ARGS:-} -- "${files[@]}" 2>&1)
status=$?
# 1 = regression, 3 = something could not be measured (often a file that does not parse yet).
# Both go back to Claude. 2 and 4 mean the tool itself failed: say so, but never block on it.
if [ "$status" -ne 0 ] && [ "$status" -ne 1 ] && [ "$status" -ne 3 ]; then
  printf "autoratchet failed (exit %s), skipping the check:\n%s\n" "$status" "$report" >&2
  exit 0
fi

problems=""
[ "$status" -eq 3 ] && problems+="$report\n\nMake every changed file parse, then the check can measure it.\n"
[ "$status" -eq 1 ] && problems+="$report\n\nFix the code, not the score: guard clauses, a lookup table, a switch over a union (a match over an enum in Rust), named predicates, or a decision table. Do not split a function at an arbitrary point.\n"
if printf '%s' "$report" | grep -qE 'Approved by a new override|sit on no single function'; then
  overrides=$(printf '%s' "$report" | grep -E '^\s+line [0-9]+: |^  [^ ].*:[0-9]+  ')
  problems+="New complexity override comments:\n$overrides\nRemove them. If you believe a score is a false positive, stop and ask the user; a person commits overrides.\n"
fi

[ -z "$problems" ] && exit 0
if [ "$active" = "true" ]; then
  printf "Complexity check still failing after a fix attempt (not blocking again):\n%b" "$problems" >&2
  exit 0
fi
printf "Complexity check failed:\n%b" "$problems" >&2
exit 2
