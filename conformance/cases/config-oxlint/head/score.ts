export function score(x0: boolean, x1: boolean, x2: boolean, x3: boolean, x4: boolean, x5: boolean, x6: boolean, x7: boolean): number {
  let n = 0;
  if (x0) n++;
  if (x1) n++;
  if (x2) n++;
  if (x3) n++;
  if (x4) n++;
  if (x5) n++;
  if (x6) n++;
  if (x7) n++;
  return n;
}
