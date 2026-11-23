interface User {
  id: string;
}

export function f(input: string) {
  // SAFETY: fixture for the chained-assertion rule.
  return input as object as User;
}
