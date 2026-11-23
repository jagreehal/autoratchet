type Id = string & { readonly brand: unique symbol };

export function f(value: string) {
  return value as Id;
}
