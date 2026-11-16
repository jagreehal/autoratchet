/* oxlint-disable complexity */
export function tiny(a: number) { return a; } // oxlint-disable-line complexity -- trivial
export function big(a: number) {
  if (a === 1) return 1; if (a === 2) return 2; if (a === 3) return 3; if (a === 4) return 4;
  if (a === 5) return 5; if (a === 6) return 6; if (a === 7) return 7; if (a === 8) return 8;
  if (a === 9) return 9; if (a === 10) return 10; if (a === 11) return 11;
  return 0;
}
export const text = '// eslint-disable-next-line complexity';
// eslint-disable-next-line complexity -- options bag, each default one line
export function opts({ a = 1, b = 2, c = 3, d = 4, e = 5, f = 6, g = 7, h = 8, i = 9, j = 10 } = {}) { return a + b + c + d + e + f + g + h + i + j; }
// biome-ignore lint/complexity/noExcessiveCognitiveComplexity: follows the spec step by step
export function spec(a: boolean, b: boolean, c: boolean, d: boolean) {
  if (a) { if (b) { if (c) { if (d) { for (;;) { if (a && b) return 1; } } } } }
  return 0;
}
// biome-ignore-all lint/complexity/noExcessiveCognitiveComplexity: whole file
