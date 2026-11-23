export const pad = 'éééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééééé';

export function legacy(input: unknown) { return Reflect.get(globalThis, String(input)); }

export function clean(n: number) { return Reflect.get(globalThis, String(n)); }

export function fixed(x: number) { return x; }

export function added(v: string) {
  const id = v.trim();

  // oxlint-disable-next-line anti-slop/require-safety-comment-for-type-assertion -- branded at the boundary
  return id as Id;
}

export const later = (v: string) => {
  // SAFETY: callers pass a value parseId already checked.
  return v as Id;
};

type Id = string & { readonly brand: unique symbol };
