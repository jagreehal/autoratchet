export function hot(a: number) {
  if (a === 1) return 1; if (a === 2) return 2; if (a === 3) return 3; if (a === 4) return 4;
  if (a === 5) return 5; if (a === 6) return 6; if (a === 7) return 7; if (a === 8) return 8;
  if (a === 9) return 9; if (a === 10) return 10; if (a === 11) return 11;
  return 0;
}
export function cold(a: number) { return a ? 1 : 0; }
