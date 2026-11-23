export function f(n: number) {
  let out = 0;

  switch (n) {
    case 1:
      out += 1;
    case 2:
      out += 2;
      break;
  }

  return out;
}
