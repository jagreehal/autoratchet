// Change functions at random, approve some of their findings, and check what the gate makes of it.
//
//   node --import tsx conformance/fuzz.ts --cli target/release/autoratchet [--runs 300] [--seed 0] [--keep]
//
// Every finding is known by construction, so the right answer is too:
//   - if more unapproved findings are left than before, the gate must fail;
//   - if not, and no old finding went away, it must pass.
// Where an old finding went away, the gate may be stricter than counting; those are only counted.
// A failure prints its seed, language and mode; `--runs 1 --seed N --keep` keeps its files.
import { spawnSync } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';

const argv = process.argv.slice(2);
const flag = (name: string) => {
  const i = argv.indexOf(name);
  return i === -1 ? undefined : argv[i + 1];
};
const cli = path.resolve(flag('--cli') ?? 'target/release/autoratchet');
const runs = Number(flag('--runs') ?? 300);
const firstSeed = Number(flag('--seed') ?? 0);

type Random = (below: number) => number;
/** mulberry32: the same cases for the same seed, everywhere. */
function random(seed: number): Random {
  let state = Math.imul(seed + 1, 0x9e3779b1);
  return (below) => {
    state = (state + 0x6d2b79f5) | 0;
    let t = Math.imul(state ^ (state >>> 15), 1 | state);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) % below;
  };
}

/**
 * What tells one finding from another: its own code, the call it is in, the branch, a block's other
 * code, where its variable was set, or the callback it is in.
 */
const modes = ['code', 'call', 'branch', 'block', 'setting', 'callback'] as const;
type Mode = (typeof modes)[number];

type Language = {
  open: string;
  close: string;
  directive: string;
  filler: (n: number) => string;
  /** A finding about `id`, spelled one of four ways that are the same code. */
  finding: (id: string, style: number) => string;
  assign: (name: string, value: string) => string;
  call: (name: string, args: string[]) => string;
  set: (name: string, value: string) => string;
  branch: (condition: string, body: string[]) => string[];
  block: (body: string[]) => string[];
  callback: (argument: string, body: string[]) => string[];
};

function pick<T>(items: readonly T[], index: number): T {
  for (const [i, item] of items.entries()) if (i === index % items.length) return item;
  throw new Error('nothing to pick from');
}

const spelled = (style: number, ...ways: string[]) => pick(ways, style);
const braces = (condition: string, body: string[]) => [`if ${condition} {`, ...body, '}'];

const languages = {
  ts: {
    open: 'export function f(x: number, y: number, log: (...a: unknown[]) => void) {',
    close: '}',
    directive: '// oxlint-disable-next-line eqeqeq -- ok',
    filler: (n) => `log(y > ${n});`,
    finding: (id, style) =>
      spelled(style, `x == ${id}`, `x==${id}`, `x /* n */ == ${id}`, `x ==\n ${id}`),
    assign: (name, value) => `const ${name} = ${value};`,
    call: (name, args) => `${name}(${args.join(', ')});`,
    set: (name, value) => `${name} = ${value};`,
    branch: (condition, body) => braces(`(${condition})`, body),
    block: (body) => ['{', ...body, '}'],
    callback: (argument, body) => [`run(${argument}, () => {`, ...body, '});'],
  },
  py: {
    open: 'from typing import cast\ndef f(v):',
    close: '    return 0',
    directive:
      '# autoratchet-disable-next-line anti-slop-python/require-safety-comment-for-cast -- ok',
    filler: (n) => `z = ${n}`,
    finding: (id, style) =>
      spelled(style, `cast(int, v${id})`, `cast(int,v${id})`, `cast( int , v${id} )`),
    assign: (name, value) => `${name} = ${value}`,
    call: (name, args) => `${name}(${args.join(', ')})`,
    set: (name, value) => `${name} = ${value}`,
    branch: (condition, body) => [`if ${condition}:`, ...body],
    block: (body) => ['if c:', ...body],
    callback: (argument, body) => [`with run(${argument}):`, ...body],
  },
  go: {
    open: 'package p\n\nfunc f() {',
    close: '}',
    directive: '// autoratchet-disable-next-line anti-slop-go/no-unchecked-type-assertion -- ok',
    filler: (n) => `z = ${n}`,
    finding: (id, style) => spelled(style, `v${id}.(int)`, `v${id}.( int )`, `v${id}.(int )`),
    assign: (name, value) => `${name} := ${value}`,
    call: (name, args) => `${name}(${args.join(', ')})`,
    set: (name, value) => `${name} = ${value}`,
    branch: braces,
    block: (body) => ['{', ...body, '}'],
    callback: (argument, body) => [`run(${argument}, func() {`, ...body, '})'],
  },
  rs: {
    open: 'fn f() {',
    close: '}',
    directive: '// autoratchet-disable-next-line anti-slop-rust/no-unwrap -- ok',
    filler: (n) => `z = ${n};`,
    finding: (id, style) =>
      spelled(style, `v${id}.unwrap()`, `v${id} .unwrap()`, `v${id}.unwrap( )`),
    assign: (name, value) => `let ${name} = ${value};`,
    call: (name, args) => `${name}(${args.join(', ')});`,
    set: (name, value) => `${name} = ${value};`,
    branch: braces,
    block: (body) => ['{', ...body, '}'],
    callback: (argument, body) => [`run(${argument}, || {`, ...body, '});'],
  },
} satisfies { [extension: string]: Language };

/** A finding, by number, or filler when there is none. */
type Item = { id?: number; style: number; approved: boolean };

function statement(language: Language, mode: Mode, item: Item, next: Random): string[] {
  const finding = language.finding(mode === 'code' ? `${item.id}` : '1', item.style);
  const other = next(2) ? [`${next(9)}`] : [];
  const approval = item.approved ? [language.directive] : [];
  if (mode === 'call') return [...approval, language.call(`call${item.id}`, [finding, ...other])];
  const call = [...approval, language.call('log', [finding, ...other])];
  // What sets the variable a finding is about, `x` or `v1`, for some.
  const set = language.set(finding.includes('x') ? 'x' : 'v1', `value.of${item.id}`);
  if (mode === 'branch') return language.branch(`c${item.id}`, call);
  if (mode === 'block') return language.block([language.call('note', [`u${item.id}`]), ...call]);
  if (mode === 'setting') return [set, ...call];
  if (mode === 'callback') return language.callback(`u${item.id}`, call);
  const ways = [
    language.assign(`a${next(1e6)}`, finding),
    language.call('log', [finding, ...other]),
    language.call('log', [`${next(9)}`, finding]),
  ];
  return [...approval, pick(ways, next(ways.length))];
}

const indented = new Set<Mode>(['branch', 'block', 'callback']);

function render(language: Language, mode: Mode, items: Item[], next: Random): string {
  const lines = items.flatMap((item) =>
    item.id === undefined
      ? [language.filler(next(9))]
      : statement(language, mode, item, next).map((line, i) =>
          // Python's blocks: what a branch holds goes deeper than it.
          indented.has(mode) && i > 0 ? `    ${line}` : line,
        ),
  );
  return [language.open, ...lines.map((line) => `    ${line}`), language.close, ''].join('\n');
}

function shuffle<T>(items: readonly T[], next: Random): T[] {
  const shuffled: T[] = [];
  for (const item of items) shuffled.splice(next(shuffled.length + 1), 0, item);
  return shuffled;
}

type Report = { regressions: { filePath: string; metric: string }[] };
type Expected = { fail: boolean; removed: boolean };

const insert = (items: Item[], item: Item, next: Random) =>
  items.splice(next(items.length + 1), 0, item);

const addFiller = (items: Item[], next: Random) => {
  for (let i = next(4); i > 0; i--) insert(items, { style: 0, approved: false }, next);
};

function makeBase(next: Random): Item[] {
  const ids = Array.from({ length: 1 + next(4) }, (_, i) => 100 + i);
  const base: Item[] = ids.map((id) => ({ id, style: 0, approved: next(5) === 0 }));
  addFiller(base, next);
  return shuffle(base, next);
}

/** Remove, add, approve, reorder and respell. */
function makeHead(base: Item[], next: Random): Item[] {
  const gone = next(4) === 0 ? 100 : undefined;
  const head = base.flatMap((item) =>
    gone !== undefined && item.id === gone ? [] : [{ ...item }],
  );
  addFiller(head, next);
  for (let i = next(4); i > 0; i--)
    insert(head, { id: 200 + i, style: 0, approved: next(5) < 3 }, next);
  if (next(2)) for (const item of head) item.approved ||= isOld(item) && next(10) < 3;
  // A directive covers one line, so what is approved is written on one.
  for (const item of head) item.style = item.approved ? 0 : next(4);
  return next(2) ? shuffle(head, next) : head;
}

const isOld = (item: Item) => item.id !== undefined && item.id < 200;

const ids = (items: Item[], keep: (item: Item) => boolean) =>
  new Set(items.flatMap((item) => (item.id !== undefined && keep(item) ? [item.id] : [])));

function expect(base: Item[], head: Item[]): Expected {
  // A directive base had too approves nothing now.
  const before = ids(base, (item) => item.approved);
  const excused = ids(head, (item) => item.approved).difference(before);
  const [old, now] = [ids(base, () => true), ids(head, () => true)];
  const count = (all: Set<number>) => all.size - all.intersection(excused).size;
  return { fail: count(now) > count(old), removed: !old.isSubsetOf(now) };
}

function makeCase(language: Language, mode: Mode, seed: number) {
  const next = random(seed);
  const base = makeBase(next);
  const head = makeHead(base, next);
  return {
    base: render(language, mode, base, next),
    head: render(language, mode, head, next),
    expected: expect(base, head),
  };
}

const work = fs.mkdtempSync(path.join(os.tmpdir(), 'autoratchet-fuzz-'));

/** One comparison for all of a language's cases, a file each: the files that failed on lint. */
function compare(dir: string): Set<string> {
  const args = ['compare', '--base', `${dir}/base`, '--head', `${dir}/head`, '--format', 'json'];
  const result = spawnSync(cli, args, { encoding: 'utf8', maxBuffer: 1 << 30 });
  if (result.status !== 0 && result.status !== 1)
    throw new Error(`${dir}: exit ${result.status}\n${result.stdout}${result.stderr}`);
  const { regressions }: Report = JSON.parse(result.stdout);
  return new Set(regressions.flatMap((r) => (r.metric === 'lint' ? [r.filePath] : [])));
}

function writeCases(dir: string, extension: string, language: Language, mode: Mode) {
  const expected = new Map<string, Expected>();
  for (const side of ['base', 'head']) fs.mkdirSync(path.join(dir, side), { recursive: true });
  for (let seed = firstSeed; seed < firstSeed + runs; seed++) {
    const made = makeCase(language, mode, seed);
    const name = `c${seed}.${extension}`;
    expected.set(name, made.expected);
    fs.writeFileSync(path.join(dir, 'base', name), made.base);
    fs.writeFileSync(path.join(dir, 'head', name), made.head);
  }
  return expected;
}

const totals = { right: 0, stricter: 0 };
const wrong: string[] = [];
for (const [extension, language] of Object.entries(languages))
  for (const mode of modes) {
    const dir = path.join(work, `${extension}-${mode}`);
    const expected = writeCases(dir, extension, language, mode);
    const failed = compare(dir);
    for (const [name, { fail, removed }] of expected) {
      if (failed.has(name) === fail) totals.right++;
      else if (!fail && removed) totals.stricter++;
      else wrong.push(`${fail ? 'bypass' : 'false failure'}: ${mode} ${name}`);
    }
  }

if (argv.includes('--keep')) console.log(work);
else fs.rmSync(work, { recursive: true, force: true });
for (const line of wrong.slice(0, 20)) console.log(line);
console.log(
  `${totals.right} right, ${totals.stricter} stricter than counting, ${wrong.length} wrong`,
);
process.exit(wrong.length > 0 ? 1 : 0);
