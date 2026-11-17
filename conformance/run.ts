// Conformance suite: runs one AutoRatchet CLI over every case and compares the results with
// the expected files, or writes them with --update.
//
//   node --import tsx conformance/run.ts --cli target/release/autoratchet [--update] [filter]
//
// The expected results were first written by the TypeScript implementation (0.1.x) and are the
// contract the Rust implementation keeps. Oxlint comes from node_modules unless
// AUTORATCHET_OXLINT says otherwise.
//
// Each case records the exit code, the JSON report, the text report and the per-function
// scores of both sides. Arrays compare as sets: report order is not part of the contract.
import { execFileSync, spawnSync } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const casesDir = path.join(here, 'cases');
const expectedDir = path.join(here, 'expected');

const argv = process.argv.slice(2);
const cliArg = argv[argv.indexOf('--cli') + 1];
if (!argv.includes('--cli') || !cliArg)
  throw new Error('usage: run.ts --cli "<command>" [--update] [filter]');
const update = argv.includes('--update');
const filter = argv.filter((a, i) => !a.startsWith('--') && argv[i - 1] !== '--cli')[0];
const [cmd, ...cmdArgs] = cliArg
  .split(' ')
  .map((p) => (p.startsWith('.') || p.includes('/') ? path.resolve(p) : p));

type Run = { exit: number | null; stdout: string; stderr: string };

// Git cases run outside this repository: point them at its Oxlint.
const bin = path.join(here, '..', 'node_modules', '.bin');
const toolEnv = {
  ...process.env,
  AUTORATCHET_OXLINT: process.env.AUTORATCHET_OXLINT || path.join(bin, 'oxlint'),
};

function cli(args: string[], cwd: string): Run {
  const r = spawnSync(cmd!, [...cmdArgs, ...args], {
    cwd,
    env: toolEnv,
    encoding: 'utf8',
    maxBuffer: 1 << 30,
  });
  return { exit: r.status, stdout: r.stdout, stderr: r.stderr };
}

/** Sort arrays and object keys, and markdown/text lines, so order differences do not count. */
function normalise(value: unknown): unknown {
  if (Array.isArray(value)) {
    return value.map(normalise).sort((a, b) => JSON.stringify(a).localeCompare(JSON.stringify(b)));
  }
  if (value && typeof value === 'object') {
    return Object.fromEntries(
      Object.entries(value)
        .sort(([a], [b]) => a.localeCompare(b))
        .map(([k, v]) => [k, k === 'markdown' ? sortedLines(String(v)) : normalise(v)]),
    );
  }
  return value;
}

function sortedLines(text: string): string[] {
  return text
    .split('\n')
    .filter((l) => l.trim() !== '')
    .sort();
}

function json(run: Run, what: string): unknown {
  try {
    return normalise(JSON.parse(run.stdout));
  } catch {
    return { unparsable: what, exit: run.exit, stdout: run.stdout, stderr: run.stderr };
  }
}

function caseResult(cwd: string, extra: string[], base = 'base', head = 'head'): unknown {
  const compare = cli(
    ['compare', '--base', base, '--head', head, '--format', 'json', ...extra],
    cwd,
  );
  const text = cli(['compare', '--base', base, '--head', head, ...extra], cwd);
  return {
    exit: compare.exit,
    textExit: text.exit,
    report: json(compare, 'compare'),
    text: sortedLines(text.stdout),
    measureBase: json(cli(['measure', base], cwd), 'measure base'),
    measureHead: json(cli(['measure', head], cwd), 'measure head'),
  };
}

function checkResult(repo: string, args: string[]): unknown {
  const run = cli(['check', '--format', 'json', ...args], repo);
  return { exit: run.exit, report: json(run, 'check') };
}

// Git cases build repositories with fixed dates and authors, so commit ids are stable.
const GIT_ENV = {
  ...process.env,
  GIT_AUTHOR_NAME: 't',
  GIT_AUTHOR_EMAIL: 't@t',
  GIT_COMMITTER_NAME: 't',
  GIT_COMMITTER_EMAIL: 't@t',
  GIT_AUTHOR_DATE: '2026-01-01T00:00:00Z',
  GIT_COMMITTER_DATE: '2026-01-01T00:00:00Z',
  GIT_CONFIG_GLOBAL: '/dev/null',
  GIT_CONFIG_NOSYSTEM: '1',
};

function git(repo: string, ...args: string[]): void {
  execFileSync('git', args, { cwd: repo, env: GIT_ENV, stdio: 'pipe' });
}

function write(root: string, files: Record<string, string>): void {
  for (const [file, text] of Object.entries(files)) {
    fs.mkdirSync(path.dirname(path.join(root, file)), { recursive: true });
    fs.writeFileSync(path.join(root, file), text);
  }
}

const read = (rel: string) => fs.readFileSync(path.join(casesDir, rel), 'utf8');

function newRepo(tmp: string, name: string, files: Record<string, string>): string {
  const repo = path.join(tmp, name);
  fs.mkdirSync(repo, { recursive: true });
  git(repo, 'init', '-q', '-b', 'main');
  write(repo, files);
  git(repo, 'add', '-A');
  git(repo, 'commit', '-qm', 'base');
  return repo;
}

const gitCases: Record<string, (tmp: string) => unknown> = {
  'git-working-tree': (tmp) => {
    const repo = newRepo(tmp, 'wt', {
      'src/payments.ts': read('fixture-fail/base/payments.ts'),
      '.gitignore': 'generated/\n',
    });
    write(repo, {
      'src/payments.ts': read('fixture-fail/head/payments.ts'),
      'src/refunds.ts': read('fixture-fail/head/refunds.ts'),
      'generated/refunds.ts': read('fixture-fail/head/refunds.ts'),
      'other/refunds.ts': read('fixture-fail/head/refunds.ts'),
    });
    return {
      all: checkResult(repo, ['--base', 'main']),
      src: checkResult(repo, ['--base', 'main', 'src']),
      excluded: checkResult(repo, ['--base', 'main', '.', ':!other']),
    };
  },
  'git-merge-base': (tmp) => {
    const repo = newRepo(tmp, 'mb', { 'a.ts': read('edge-untracked/base/u.ts') });
    git(repo, 'checkout', '-qb', 'feature');
    write(repo, { 'b.ts': read('edge-unicode/head/text.ts') });
    git(repo, 'add', '-A');
    git(repo, 'commit', '-qm', 'feature');
    git(repo, 'checkout', '-q', 'main');
    // main moves on after the branch: the check must ignore this commit.
    write(repo, { 'a.ts': read('edge-untracked/head/u.ts') });
    git(repo, 'commit', '-qam', 'main moves');
    git(repo, 'checkout', '-q', 'feature');
    git(repo, 'rm', '-q', 'a.ts');
    return { check: checkResult(repo, ['--base', 'main']) };
  },
  'git-override': (tmp) => {
    const repo = newRepo(tmp, 'ov', { 'o.ts': read('edge-overrides/base/o.ts') });
    write(repo, { 'o.ts': read('edge-overrides/head/o.ts') });
    return {
      check: checkResult(repo, ['--base', 'main']),
      strict: checkResult(repo, ['--base', 'main', '--require-override-approval']),
    };
  },
  'git-config': (tmp) => {
    // Limits come from the stricter of the base commit's config and the working tree's:
    // loosening .oxlintrc.json has no effect yet, a new ruff.toml limit applies at once.
    const repo = newRepo(tmp, 'config', {
      '.oxlintrc.json': read('config-oxlint/base/.oxlintrc.json'),
    });
    write(repo, {
      '.oxlintrc.json': read('config-oxlint-loosened/head/.oxlintrc.json'),
      'ruff.toml': '[lint.mccabe]\nmax-complexity = 8\n',
      'score.ts': read('config-oxlint/head/score.ts'),
      'score.py': read('config-ruff/head/score.py'),
    });
    return {
      check: checkResult(repo, ['--base', 'main']),
      flag: checkResult(repo, ['--base', 'main', '--threshold', '10']),
    };
  },
  'git-go-vendor': (tmp) => {
    // Vendored Go dependencies are someone else's code: tracked or not, check leaves them out.
    const simple =
      'package lib\n\nfunc Lib(a bool) int {\n\tif a {\n\t\treturn 1\n\t}\n\treturn 0\n}\n';
    const nested =
      'package lib\n\nfunc Lib(a, b, c bool, xs []int) int {\n\tif a {\n\t\tif b {\n\t\t\tif c {\n\t\t\t\tfor _, x := range xs {\n\t\t\t\t\tif x > 0 && b || c {\n\t\t\t\t\t\treturn 1\n\t\t\t\t\t}\n\t\t\t\t}\n\t\t\t}\n\t\t}\n\t}\n\treturn 0\n}\n';
    const repo = newRepo(tmp, 'vendor', {
      'main.go': 'package main\n\nfunc main() {}\n',
      'vendor/example.com/lib/lib.go': simple,
    });
    write(repo, { 'vendor/example.com/lib/lib.go': nested });
    return { check: checkResult(repo, ['--base', 'main']) };
  },
  'large-file': (tmp) => {
    const dir = path.join(tmp, 'large');
    const nested =
      'export function f(a, b, c) { if (a) { if (b) { if (c) { for (const x of a) { if (x) { while (b) { if (c) return 1; } } } } } } return 0; }\n';
    write(dir, {
      'base/big.ts': 'export function f() { return 0; }\n',
      'head/big.ts': `${nested}// ${'x'.repeat(1_100_000)}\n`,
    });
    return caseResult(dir, []);
  },
  'usage-errors': (tmp) => ({
    help: cli(['--help'], tmp).exit,
    noCommand: cli([], tmp).exit,
    unknownCommand: cli(['nope'], tmp).exit,
    badFormat: cli(['compare', '--base', 'a', '--head', 'b', '--format', 'xml'], tmp).exit,
    badThreshold: cli(['compare', '--base', 'a', '--head', 'b', '--threshold', '0'], tmp).exit,
    missingHead: cli(['compare', '--base', 'a'], tmp).exit,
    missingDir: cli(['compare', '--base', 'nope', '--head', 'nope'], tmp).exit,
  }),
};

const results = new Map<string, unknown>();
for (const name of fs.readdirSync(casesDir).sort()) {
  if (filter && !name.includes(filter)) continue;
  const dir = path.join(casesDir, name);
  const argsFile = path.join(dir, 'args');
  const extra = fs.existsSync(argsFile)
    ? fs.readFileSync(argsFile, 'utf8').trim().split(/\s+/)
    : [];
  results.set(name, caseResult(dir, extra));
}
const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'autoratchet-conformance-'));
try {
  for (const [name, run] of Object.entries(gitCases)) {
    if (filter && !name.includes(filter)) continue;
    results.set(name, run(tmp));
  }
} finally {
  fs.rmSync(tmp, { recursive: true, force: true });
}

fs.mkdirSync(expectedDir, { recursive: true });
let failed = 0;
for (const [name, actual] of results) {
  const file = path.join(expectedDir, `${name}.json`);
  const text = `${JSON.stringify(actual, null, 2)}\n`;
  if (update) {
    fs.writeFileSync(file, text);
    continue;
  }
  const expected = fs.existsSync(file) ? fs.readFileSync(file, 'utf8') : '';
  if (expected === text) continue;
  failed++;
  const out = path.join(os.tmpdir(), `autoratchet-actual-${name}.json`);
  fs.writeFileSync(out, text);
  console.log(`FAIL ${name}\n  diff -u ${file} ${out}`);
}
console.log(
  update
    ? `wrote ${results.size} expected results`
    : `${results.size - failed}/${results.size} cases match`,
);
process.exitCode = failed > 0 ? 1 : 0;
