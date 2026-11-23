export function f() {
  const xs: number[] = [1, 2, 3];

  return xs.filter((x) => x > 1).map((x) => x * 2);
}
