import { functionKey } from './identity.js';

export const DEFAULT_CYCLOMATIC_THRESHOLD = 10;
export const DEFAULT_COGNITIVE_THRESHOLD = 15;

export type Metric = 'cyclomatic' | 'cognitive';
export const METRICS: readonly Metric[] = ['cyclomatic', 'cognitive'];

/**
 * A function passes when its score does not exceed the higher of
 * its previous score and the threshold. New functions use previous=undefined,
 * so they must be ≤ threshold.
 */
export function passesComplexityGate(
  previous: number | undefined,
  current: number,
  threshold: number,
): boolean {
  return current <= Math.max(previous ?? 0, threshold);
}

export type FunctionSnapshot = {
  filePath: string;
  functionName: string;
  cyclomatic: number;
  /** Biome cognitive complexity. Absent in JSON snapshots that predate it: not gated. */
  cognitive?: number;
  /** Line Oxlint reports the function on. Used to match override comments to functions. */
  line?: number;
};

export type Regression = {
  filePath: string;
  functionName: string;
  metric: Metric;
  previous: number | undefined;
  current: number;
  threshold: number;
  kind: 'worsened' | 'new';
  /** Line the function starts on in head, when known. */
  line?: number;
};

export type GateOptions = {
  cyclomaticThreshold?: number;
  cognitiveThreshold?: number;
  /**
   * Strict mode: a new override approves nothing and fails the gate instead.
   * Use it where approval has to come from somewhere an author cannot grant
   * themselves, such as a protected PR label (README, Part 6).
   */
  requireOverrideApproval?: boolean;
};

export function thresholds(options: GateOptions = {}): Record<Metric, number> {
  return {
    cyclomatic: options.cyclomaticThreshold ?? DEFAULT_CYCLOMATIC_THRESHOLD,
    cognitive: options.cognitiveThreshold ?? DEFAULT_COGNITIVE_THRESHOLD,
  };
}

/**
 * @param approved `functionKey\0metric` entries a new override approved on this PR.
 *   Those skip the gate once. Measurement continues, so the next PR that makes
 *   the function worse fails like any other.
 */
export function findRegressions(
  previous: Map<string, FunctionSnapshot>,
  current: Map<string, FunctionSnapshot>,
  options: GateOptions = {},
  approved: ReadonlySet<string> = new Set(),
): Regression[] {
  const limits = thresholds(options);
  return [...current].flatMap(([key, head]) => {
    const base = previous.get(key);
    const skip = (metric: Metric) =>
      approved.has(`${functionKey(head.filePath, head.functionName)}\0${metric}`);
    return METRICS.flatMap((metric) =>
      skip(metric) ? [] : metricRegression(base, head, metric, limits[metric]),
    );
  });
}

/** The regression for one metric of one function, as a 0- or 1-item list. */
function metricRegression(
  base: FunctionSnapshot | undefined,
  head: FunctionSnapshot,
  metric: Metric,
  threshold: number,
): Regression[] {
  const now = head[metric];
  // A base snapshot without this metric (older JSON) gives nothing to compare against.
  if (now === undefined || (base && base[metric] === undefined)) return [];
  if (passesComplexityGate(base?.[metric], now, threshold)) return [];
  return [
    {
      filePath: head.filePath,
      functionName: head.functionName,
      metric,
      previous: base?.[metric],
      current: now,
      threshold,
      kind: base ? 'worsened' : 'new',
      line: head.line,
    },
  ];
}

/**
 * Functions over either threshold on base that head no longer reports.
 * Deleted or renamed is fine. A new disable comment is the case a reviewer must see.
 */
export function findUntracked(
  previous: Map<string, FunctionSnapshot>,
  current: Map<string, FunctionSnapshot>,
  options: GateOptions = {},
): FunctionSnapshot[] {
  const limits = thresholds(options);
  const hot = (s: FunctionSnapshot) =>
    s.cyclomatic > limits.cyclomatic || (s.cognitive ?? 0) > limits.cognitive;
  return [...previous]
    .filter(([key, snap]) => hot(snap) && !current.has(key))
    .map(([, snap]) => snap);
}
