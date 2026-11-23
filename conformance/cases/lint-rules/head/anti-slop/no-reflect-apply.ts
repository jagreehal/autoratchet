export function f(operation: (a: number) => number, args: [number]) {
  return Reflect.apply(operation, undefined, args);
}
