/**
 * AutoRatchet: prevent complexity regressions without fixing your existing technical debt first.
 *
 *   import { check, formatReport, verdict } from 'autoratchet';
 *   const result = await check({ base: 'main', paths: ['src'] });
 *   if (verdict(result) !== 'pass') console.log(formatReport(result));
 */
export { check, type CheckOptions, type CheckResult } from './git.js';
export {
  compareSources,
  comparePaths,
  verdict,
  type CompareResult,
  type Source,
} from './compare.js';
export { collectFiles, collectFromDirectory, Snapshots } from './collect.js';
export {
  DEFAULT_COGNITIVE_THRESHOLD,
  DEFAULT_CYCLOMATIC_THRESHOLD,
  findRegressions,
  passesComplexityGate,
  type FunctionSnapshot,
  type GateOptions,
  type Metric,
  type Regression,
} from './gate.js';
export { type Approval, type Override } from './overrides.js';
export { functionKey } from './identity.js';
export { formatMarkdown, formatReport, toJson, type ReportInput } from './report.js';
