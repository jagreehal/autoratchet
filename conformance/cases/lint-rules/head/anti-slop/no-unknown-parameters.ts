export function f(input: unknown) {
  return input !== undefined;
}

export function load(p: Promise<string>): Promise<string> {
  return p.catch((error: unknown) => String(error)).then(String, (error: unknown) => String(error));
}
