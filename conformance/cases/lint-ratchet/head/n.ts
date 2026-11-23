export function compare(a: number, b: number) {
  // oxlint-disable-next-line eqeqeq -- loose on purpose
  return a == b;
}

export function kind(v: string | number) {
  return typeof v === 'string' ? 'text' : 'number';
}
