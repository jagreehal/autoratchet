// eslint-disable-next-line sonarjs/cognitive-complexity -- follows the spec step by step
export function parse(a: boolean, b: boolean, c: boolean, d: boolean) {
  if (a) { if (b) { if (c) { if (d) { for (;;) { if (a && b) return 1; } } } } }
  return 0;
}
// eslint-disable-next-line complexity, sonarjs/cognitive-complexity -- generated state machine
export function both(s: number, t: boolean) {
  if (t) { if (s === 1) { if (s > 0) { while (t) { if (s) return 1; } } } }
  if (s === 1) return 1; if (s === 2) return 2; if (s === 3) return 3; if (s === 4) return 4;
  if (s === 5) return 5; if (s === 6) return 6; if (s === 7) return 7; if (s === 8) return 8;
  return 0;
}
