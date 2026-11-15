import fs from 'node:fs/promises';
import path from 'node:path';
import { collectFiles, Snapshots } from './collect.js';
import {
  findRegressions,
  findUntracked,
  METRICS,
  type FunctionSnapshot,
  type GateOptions,
  type Regression,
} from './gate.js';
import { functionKey } from './identity.js';
import {
  approvalKey,
  findOverridesIn,
  sourceFiles,
  matchOverrides,
  newOverrides,
  type Approval,
  type Override,
} from './overrides.js';

export type CompareResult = {
  previous: Map<string, FunctionSnapshot>;
  current: Map<string, FunctionSnapshot>;
  regressions: Regression[];
  untracked: FunctionSnapshot[];
  /** New overrides on a function: each approves one metric's current score, once. Directories only. */
  approvals: Approval[];
  /** New overrides on no single function (file-wide or misplaced). The gate ignores them. */
  unmatchedOverrides: Override[];
  /** With requireOverrideApproval: every new override, each one a gate failure. */
  blockedOverrides: Override[];
  /** Anything either side could not measure. Non-empty means the gate fails. */
  incomplete: string[];
};

/** The three outcomes: measured and clean, measured with failures, or not fully measured. */
export function verdict(result: CompareResult): 'pass' | 'fail' | 'incomplete' {
  if (result.incomplete.length > 0) return 'incomplete';
  if (result.regressions.length > 0 || result.blockedOverrides.length > 0) return 'fail';
  return 'pass';
}

/** One side of a comparison: source files under a root, or a JSON snapshot file. */
export type Source = { root: string; files: string[] } | { json: string };

export async function comparePaths(
  basePath: string,
  headPath: string,
  options: GateOptions = {},
): Promise<CompareResult> {
  return compareSources(await sourceFor(basePath), await sourceFor(headPath), options);
}

export async function compareSources(
  base: Source,
  head: Source,
  options: GateOptions = {},
): Promise<CompareResult> {
  const previous = await snapshotsOf(base);
  const current = await snapshotsOf(head);
  const added =
    'root' in base && 'root' in head
      ? newOverrides(findOverridesIn(base.root, base.files), findOverridesIn(head.root, head.files))
      : [];
  const strict = options.requireOverrideApproval === true;
  const matched = matchOverrides(added, current);
  const approvals = strict ? [] : matched.approvals;
  const approved = new Set(
    approvals.map((a) => approvalKey(a.override.filePath, a.functionName, a.override.metric)),
  );
  const problems = (side: string, snaps: Map<string, FunctionSnapshot>) =>
    snaps instanceof Snapshots ? snaps.problems.map((p) => `${side}: ${p}`) : [];
  return {
    previous,
    current,
    regressions: sortRegressions(findRegressions(previous, current, options, approved)),
    untracked: findUntracked(previous, current, options),
    approvals,
    unmatchedOverrides: strict ? [] : matched.unmatched,
    blockedOverrides: strict ? added : [],
    incomplete: [...problems('base', previous), ...problems('head', current)],
  };
}

/** Oxlint lints files in parallel: sort so text, JSON and PR comments come out the same every run. */
function sortRegressions(regressions: Regression[]): Regression[] {
  const metricOrder = (r: Regression) => METRICS.indexOf(r.metric);
  return [...regressions].sort(
    (a, b) =>
      a.filePath.localeCompare(b.filePath) ||
      a.functionName.localeCompare(b.functionName) ||
      metricOrder(a) - metricOrder(b),
  );
}

async function sourceFor(target: string): Promise<Source> {
  const abs = path.resolve(target);
  const stat = await fs.stat(abs);
  if (stat.isDirectory()) return { root: abs, files: sourceFiles(abs) };
  if (abs.endsWith('.json')) return { json: abs };
  throw new Error(`Expected a directory of sources or a .json snapshot file, got: ${target}`);
}

async function snapshotsOf(source: Source): Promise<Map<string, FunctionSnapshot>> {
  return 'json' in source
    ? loadJsonSnapshots(source.json)
    : collectFiles(source.root, source.files);
}

async function loadJsonSnapshots(filePath: string): Promise<Map<string, FunctionSnapshot>> {
  const raw = JSON.parse(await fs.readFile(filePath, 'utf8')) as FunctionSnapshot[];
  const map = new Map<string, FunctionSnapshot>();
  for (const snap of raw) {
    map.set(functionKey(snap.filePath, snap.functionName), snap);
  }
  return map;
}
