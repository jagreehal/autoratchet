import type { CompareResult } from './compare.js';
import { verdict } from './compare.js';
import type { FunctionSnapshot, Regression } from './gate.js';
import type { Approval, Override } from './overrides.js';

/** Format regressions the way the README / CI output quotes them. */
export type ReportInput = {
  regressions: Regression[];
  untracked?: FunctionSnapshot[];
  approvals?: Approval[];
  unmatchedOverrides?: Override[];
  blockedOverrides?: Override[];
  incomplete?: string[];
};

const EMPTY: Required<Omit<ReportInput, 'regressions'>> = {
  untracked: [],
  approvals: [],
  unmatchedOverrides: [],
  blockedOverrides: [],
  incomplete: [],
};

/** Lines for one block of the report, or nothing when it has no items. */
function block<T>(title: string, items: T[], line: (item: T) => string[]): string[] {
  return items.length === 0 ? [] : [title, ...items.flatMap(line)];
}

const overrideLine = (o: Override) => [`  ${o.filePath}:${o.line}  ${o.text}`];

export function formatReport(input: ReportInput): string {
  const r = { ...EMPTY, ...input };
  const failed = r.incomplete.length + r.regressions.length + r.blockedOverrides.length > 0;
  const failures = [
    r.regressions.length > 0 ? [formatRegressions(r.regressions)] : [],
    block(
      'New overrides need approval (--require-override-approval):',
      r.blockedOverrides,
      overrideLine,
    ),
    block(
      'Measurement incomplete. The gate fails until every function can be measured:',
      r.incomplete,
      (p) => [`  ${p}`],
    ),
  ].filter((b) => b.length > 0);
  const notes = [
    block('Approved by a new override. A reviewer must accept each reason:', r.approvals, (a) => [
      `  ${a.override.filePath}::${a.functionName} ${a.override.metric} ${a.score ?? '?'}`,
      `    line ${a.override.line}: ${a.override.text}`,
    ]).concat(
      r.approvals.length > 0
        ? ['Still measured: if one of these gets worse in a later PR, the gate fails.']
        : [],
    ),
    block(
      'Warning: new overrides that sit on no single function (file-wide or misplaced). The gate ignores them:',
      r.unmatchedOverrides,
      overrideLine,
    ),
    block(
      'Warning: over a threshold on base, missing on head (deleted, renamed or moved):',
      r.untracked,
      (s) => [
        `  ${s.filePath}::${s.functionName} (was cyclomatic ${s.cyclomatic}${s.cognitive === undefined ? '' : `, cognitive ${s.cognitive}`})`,
      ],
    ),
  ].filter((b) => b.length > 0);

  const verdictLines = failed
    ? ['Quality gate failed.']
    : ['No complexity regressions detected.', 'Quality gate passed.'];
  return [
    ...failures.map((b) => `${b.join('\n')}\n`),
    verdictLines.join('\n'),
    ...notes.map((b) => `\n${b.join('\n')}`),
  ].join('\n');
}

function formatRegressions(regressions: Regression[]): string {
  // file → function → that function's failing metrics, in first-seen order
  const byFile = new Map<string, Map<string, Regression[]>>();
  const sorted = [...regressions].sort((a, b) => a.filePath.localeCompare(b.filePath));
  for (const reg of sorted) {
    const fns = byFile.get(reg.filePath) ?? new Map<string, Regression[]>();
    fns.set(reg.functionName, [...(fns.get(reg.functionName) ?? []), reg]);
    byFile.set(reg.filePath, fns);
  }

  const lines: string[] = ['Complexity regressions detected:', ''];
  for (const [filePath, fns] of byFile) {
    lines.push(filePath);
    for (const [functionName, regs] of fns) {
      lines.push(`  ${functionName}${regs[0]!.kind === 'new' ? ' (new function)' : ''}`);
      for (const reg of regs) lines.push(`    ${formatMetric(reg)}`);
    }
    lines.push('');
  }
  return lines.join('\n').trimEnd();
}

function formatMetric(reg: Regression): string {
  if (reg.kind === 'new') {
    return `${reg.metric}: ${reg.current} (maximum for new code: ${reg.threshold})`;
  }
  const delta = reg.current - (reg.previous ?? 0);
  return `${reg.metric}: ${reg.previous} → ${reg.current} (+${delta})`;
}

/** Machine-readable result for dashboards, CI annotations and coding agents. */
export function toJson(result: CompareResult & { baseCommit?: string }): string {
  return JSON.stringify(
    {
      verdict: verdict(result),
      baseCommit: result.baseCommit,
      regressions: result.regressions,
      approvals: result.approvals,
      blockedOverrides: result.blockedOverrides,
      unmatchedOverrides: result.unmatchedOverrides,
      incomplete: result.incomplete,
      untracked: result.untracked,
      functions: result.current.size,
      // The same result as a PR comment, so CI needs one run for both.
      markdown: formatMarkdown(result),
    },
    null,
    2,
  );
}

const VERDICT_LINE = {
  pass: '**Passed.** No function got more complex than the ratchet allows.',
  fail: '**Failed.** A function got more complex than the ratchet allows.',
  incomplete:
    '**Failed.** AutoRatchet could not measure every function, so it cannot vouch for this change.',
} as const;

function cell(text: string): string {
  return text.replaceAll('|', '\\|');
}

function regressionTable(regressions: Regression[]): string[] {
  if (regressions.length === 0) return [];
  const rows = regressions.map((r) => {
    const limit = r.kind === 'new' ? r.threshold : Math.max(r.previous ?? 0, r.threshold);
    return `| \`${cell(r.filePath)}::${cell(r.functionName)}\` | ${r.metric} | ${r.previous ?? 'new'} | ${r.current} | ${limit} |`;
  });
  return ['| Function | Metric | Before | After | Limit |', '|---|---|--:|--:|--:|', ...rows, ''];
}

/** A titled bullet list, or nothing when there are no items. */
function section<T>(title: string, items: T[], bullet: (item: T) => string): string[] {
  return items.length === 0 ? [] : [title, '', ...items.map((item) => `- ${bullet(item)}`), ''];
}

const at = (o: Override) => `\`${o.filePath}:${o.line}\` ${cell(o.text)}`;

/** A pull request comment: verdict first, then one table or list per kind of finding. */
export function formatMarkdown(result: CompareResult & { baseCommit?: string }): string {
  const base = result.baseCommit ? ` against \`${result.baseCommit.slice(0, 7)}\`` : '';
  return [
    '## AutoRatchet',
    '',
    VERDICT_LINE[verdict(result)],
    '',
    ...regressionTable(result.regressions),
    ...section('**New overrides need approval:**', result.blockedOverrides, at),
    ...section('**Could not measure:**', result.incomplete, (p) => p),
    ...section(
      '**Approved by a new override** (a reviewer must accept each reason; still measured):',
      result.approvals,
      (a) =>
        `\`${a.override.filePath}::${a.functionName}\` ${a.override.metric} ${a.score ?? '?'}: ${cell(a.override.text)}`,
    ),
    ...section(
      '**Overrides on no single function** (ignored by the gate):',
      result.unmatchedOverrides,
      at,
    ),
    ...section(
      '**Over a limit on base, missing on head** (deleted, renamed or moved):',
      result.untracked,
      (u) => `\`${u.filePath}::${u.functionName}\``,
    ),
    `<sub>${result.current.size} functions measured${base}. Limits for new code: cyclomatic 10, cognitive 15 unless configured.</sub>`,
  ].join('\n');
}
