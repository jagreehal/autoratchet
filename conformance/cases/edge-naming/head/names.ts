export function top(a: number) { return a > 1 ? 1 : 2; }
export const arrow = (a: number) => (a > 1 ? 1 : 2);
export default function (a: number) { return a > 1 ? 1 : 2; }

export class Service {
  #secret(a: number) { return a > 1 ? 1 : 2; }
  get value() { return this.#secret(1) > 1 ? 1 : 2; }
  field = (a: number) => (a > 1 ? 1 : 2);
  static make(a: number) { return a > 1 ? new Service() : null; }
  ['computed'](a: number) { return a > 1 ? 1 : 2; }
  initial = 1 > 2 ? 'a' : 'b';
}

export const Named = class Inner { run(a: number) { return a > 1 ? 1 : 2; } };
export const Anon = class { run(a: number) { return a > 1 ? 1 : 2; } };

export const handlers = {
  onSubmit(a: number) { return a > 1 ? 1 : 2; },
  onReset: (a: number) => (a > 1 ? 1 : 2),
  nested: { deep: (a: number) => (a > 1 ? 1 : 2) },
  'quoted-key': function (a: number) { return a > 1 ? 1 : 2; },
  1: (a: number) => (a > 1 ? 1 : 2),
};

export const typed = { go: (a: number) => (a > 1 ? 1 : 2) } satisfies Record<string, unknown>;
export const cast = { go: (a: number) => (a > 1 ? 1 : 2) } as const;

export function outer(items: number[]) {
  function inner(a: number) { return a > 1 ? 1 : 2; }
  const mapped = items.map((i) => (i > 1 ? inner(i) : 0)).filter((i) => (i > 0 ? true : false));
  class Local { m(a: number) { return a > 1 ? 1 : 2; } }
  return [mapped, new Local()];
}

export const RULES = [
  (a: number) => (a > 1 ? 1 : 2),
  ,
  (a: number) => (a > 2 ? 1 : 2),
];
export const KEYED = [
  ['first', (a: number) => (a > 1 ? 1 : 2)],
  ['second', (a: number) => (a > 2 ? 1 : 2)],
];
export const BY_ID = [
  { id: 'vip', test: (a: number) => (a > 1 ? 1 : 2) },
  { outcome: 'deny', test: (a: number) => (a > 2 ? 1 : 2) },
  { other: 1, test: (a: number) => (a > 3 ? 1 : 2) },
];

export function dup(a: number) { return a > 1 ? 1 : 2; }
const exportsObj: Record<string, unknown> = {};
exportsObj.dynamic = function (a: number) { return a > 1 ? 1 : 2; };
setTimeout(() => (Math.random() > 0.5 ? 1 : 2));
new Promise((resolve) => (Math.random() > 0.5 ? resolve(1) : resolve(2)));
