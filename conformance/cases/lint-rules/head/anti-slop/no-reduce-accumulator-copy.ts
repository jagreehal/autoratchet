export function f(items: number[]) {
  return items.reduce((acc, item) => acc.concat([item]), []);
}
