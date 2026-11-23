export function f(input: string | number) {
  return typeof input === 'string' ? input : String(input);
}
