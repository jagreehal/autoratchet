/**
 * Lower-level pieces of the collector, for tools built on AutoRatchet (such as the
 * guide's examples, which also measure classic cyclomatic complexity).
 * Not covered by semver: these can change in any release.
 */
export {
  assignCognitive,
  COMPLEXITY_RE,
  functionNameAt,
  runBiomeCognitive,
  runOxlint,
  type CognitiveHit,
  type FunctionSpan,
  type OxlintDiagnostic,
} from './collect.js';
export { analyseFile, namedFunctions, type NamedFunction } from './functions.js';
export { functionKey, normalizePath, parseFunctionName } from './identity.js';
export { findOverridesIn, isSourceFile, OVERRIDE_RE } from './overrides.js';
