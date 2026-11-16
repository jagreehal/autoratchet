// Time AutoRatchet CLIs on the same inputs and check that they agree.
//
//   node --import tsx conformance/bench.ts --runs 5 \
//     --cli ts="node_modules/.bin/tsx conformance/ts-cli.ts" \
//     --cli rust=target/release/autoratchet \
//     --repo /path/to/a/large/repo --base HEAD~20
//
// Scenarios: a small fixture comparison, and `check` plus `compare` on --repo (read-only:
// AutoRatchet extracts the base commit to a temporary directory).
import { spawnSync } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const argv = process.argv.slice(2);
const flag = (name: string) => {
  const i = argv.indexOf(name);
  return i === -1 ? undefined : argv[i + 1];
};
const runs = Number(flag('--runs') ?? 5);
const repo = flag('--repo');
const base = flag('--base') ?? 'HEAD~20';
const clis = argv
  .flatMap((a, i) => (a === '--cli' ? [argv[i + 1]!] : []))
  .map((spec) => {
    const [name, command] = [spec.slice(0, spec.indexOf('=')), spec.slice(spec.indexOf('=') + 1)];
    const [cmd, ...args] = command
      .split(' ')
      .map((p) => (p.startsWith('.') || p.includes('/') ? path.resolve(p) : p));
    return { name, cmd: cmd!, args };
  });
if (clis.length === 0)
  throw new Error('usage: bench.ts --cli name=command [--cli ...] [--repo dir]');

type Scenario = { name: string; cwd: string; args: string[] };
const fixture = path.join(here, 'cases/fixture-fail');
const scenarios: Scenario[] = [
  { name: 'fixture compare', cwd: fixture, args: ['compare', '--base', 'base', '--head', 'head'] },
];
if (repo) {
  scenarios.push(
    { name: `check --base ${base}`, cwd: repo, args: ['check', '--base', base] },
    {
      name: 'compare repo with itself',
      cwd: repo,
      args: ['compare', '--base', '.', '--head', '.'],
    },
  );
}

/** JSON output with arrays sorted, so implementations that order findings differently agree. */
function canonical(stdout: string): string {
  const sort = (v: unknown): unknown =>
    Array.isArray(v)
      ? v.map(sort).sort((a, b) => JSON.stringify(a).localeCompare(JSON.stringify(b)))
      : v && typeof v === 'object'
        ? Object.fromEntries(
            Object.entries(v)
              .filter(([k]) => k !== 'markdown')
              .sort(([a], [b]) => a.localeCompare(b))
              .map(([k, x]) => [k, sort(x)]),
          )
        : v;
  try {
    return JSON.stringify(sort(JSON.parse(stdout)));
  } catch {
    return stdout;
  }
}

const median = (xs: number[]) => [...xs].sort((a, b) => a - b)[Math.floor(xs.length / 2)]!;

let disagreements = 0;
for (const s of scenarios) {
  console.log(`\n${s.name}`);
  const outputs = new Map<string, string>();
  const medians = new Map<string, number>();
  for (const cli of clis) {
    const times: number[] = [];
    let last = { status: null as number | null, stdout: '' };
    for (let i = 0; i < runs; i++) {
      const start = performance.now();
      // Read the report from a file: a CLI that exits before flushing a pipe would truncate stdout.
      const out = path.join(os.tmpdir(), `autoratchet-bench-${cli.name}.json`);
      fs.rmSync(out, { force: true });
      const r = spawnSync(cli.cmd, [...cli.args, ...s.args, '--format', 'json', '-o', out], {
        cwd: s.cwd,
        encoding: 'utf8',
        maxBuffer: 1 << 30,
      });
      times.push(performance.now() - start);
      last = {
        status: r.status,
        stdout: fs.existsSync(out) ? fs.readFileSync(out, 'utf8') : r.stdout,
      };
    }
    outputs.set(cli.name, canonical(last.stdout));
    medians.set(cli.name, median(times));
    const parsed = JSON.parse(last.stdout || '{}') as { verdict?: string; functions?: number };
    console.log(
      `  ${cli.name.padEnd(6)} median ${median(times).toFixed(0).padStart(6)} ms  min ${Math.min(
        ...times,
      )
        .toFixed(0)
        .padStart(
          6,
        )} ms  exit ${last.status}  ${parsed.functions ?? '?'} functions, ${parsed.verdict ?? '?'}`,
    );
  }
  const [first, ...rest] = [...outputs.values()];
  const agree = rest.every((o) => o === first);
  if (!agree) disagreements++;
  if (clis.length > 1) {
    const [a, b] = [...medians.values()];
    console.log(
      `  outputs ${agree ? 'agree' : 'DIFFER'}; ${clis[0]!.name}/${clis[1]!.name} = ${(a! / b!).toFixed(1)}x`,
    );
  }
}
process.exitCode = disagreements > 0 ? 1 : 0;
