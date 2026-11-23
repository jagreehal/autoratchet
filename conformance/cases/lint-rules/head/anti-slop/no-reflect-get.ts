export function f(owner: { a: number }) {
  return Reflect.get(owner, 'a');
}
