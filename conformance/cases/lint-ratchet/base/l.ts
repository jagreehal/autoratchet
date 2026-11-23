export const pad = 'éééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééé';

export function legacy(input: unknown) { return Reflect.get(globalThis, String(input)); }

export function clean(n: number) { return n + 1; }

export function fixed(x: unknown) { return x; }
