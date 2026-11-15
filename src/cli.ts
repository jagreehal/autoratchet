#!/usr/bin/env node
import fs from 'node:fs';
import { parseArgs } from 'node:util';
import { comparePaths, verdict, type CompareResult } from './compare.js';
import type { GateOptions } from './gate.js';
import { check } from './git.js';
import { formatMarkdown, formatReport, toJson } from './report.js';

const USAGE = `Usage:
  autoratchet check [--base <ref>] [options] [pathspec...]
      Compare the working tree with merge-base(<ref>, HEAD). Default ref: main.
      Pathspecs limit what is measured, e.g. src ':!src/generated'.

  autoratchet compare --base <dir|json> --head <dir|json> [options]
      Compare two directories (or JSON snapshots) directly.

Options:
  --threshold <n>               maximum cyclomatic score for new functions (default 10)
  --cognitive-threshold <n>     maximum cognitive score for new functions (default 15)
  --require-override-approval   fail on any new override instead of treating it as an approval
  --format <text|json|markdown> output format (default text)
  -o, --output <file>           write the report to a file instead of stdout

Exit codes: 0 pass, 1 regression, 2 bad arguments, 3 measurement incomplete, 4 tool error.`;

/** 0 pass, 1 regression, 3 measurement incomplete. 2 and 4 are set below. */
const EXIT = { pass: 0, fail: 1, incomplete: 3 } as const;
const FORMATS = { text: formatReport, json: toJson, markdown: formatMarkdown } as const;

function number(value: string | undefined, flag: string): number | undefined {
  if (value === undefined) return undefined;
  const n = Number(value);
  if (!Number.isFinite(n) || n < 1)
    throw new UsageError(`${flag} needs a positive number, got "${value}"`);
  return n;
}

class UsageError extends Error {}

async function runCommand(
  command: string,
  paths: string[],
  values: { base?: string; head?: string },
  options: GateOptions,
): Promise<CompareResult> {
  if (command === 'check') return check({ ...options, base: values.base, paths });
  if (command !== 'compare') throw new UsageError(`unknown command "${command}"`);
  if (!values.base || !values.head) throw new UsageError('compare needs --base and --head');
  return comparePaths(values.base, values.head, options);
}

async function run(argv: string[]): Promise<number> {
  const { values, positionals } = parseArgs({
    args: argv,
    allowPositionals: true,
    options: {
      base: { type: 'string' },
      head: { type: 'string' },
      threshold: { type: 'string' },
      'cyclomatic-threshold': { type: 'string' },
      'cognitive-threshold': { type: 'string' },
      'require-override-approval': { type: 'boolean' },
      format: { type: 'string', default: 'text' },
      output: { type: 'string', short: 'o' },
      help: { type: 'boolean', short: 'h' },
    },
  });
  const [command, ...paths] = positionals;
  if (values.help || !command) {
    console.log(USAGE);
    return values.help ? 0 : 2;
  }

  const format = FORMATS[values.format as keyof typeof FORMATS];
  if (!format)
    throw new UsageError(`--format must be text, json or markdown, got "${values.format}"`);
  const options = {
    cyclomaticThreshold: number(values.threshold ?? values['cyclomatic-threshold'], '--threshold'),
    cognitiveThreshold: number(values['cognitive-threshold'], '--cognitive-threshold'),
    requireOverrideApproval: values['require-override-approval'],
  };

  const result = await runCommand(command, paths, values, options);
  const report = format(result);
  if (values.output) fs.writeFileSync(values.output, `${report}\n`);
  else console.log(report);
  return EXIT[verdict(result)];
}

let code: number;
try {
  code = await run(process.argv.slice(2));
} catch (error) {
  const usage =
    error instanceof UsageError || (error as { code?: string }).code?.startsWith('ERR_PARSE_ARGS');
  console.error(`autoratchet: ${error instanceof Error ? error.message : String(error)}`);
  if (usage) console.error(`\n${USAGE}`);
  code = usage ? 2 : 4;
}
process.exit(code);
