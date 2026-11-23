interface User {
  id: string;
}

declare function loadUser(): User;

export function f() {
  const loaded: User = loadUser();
  const stored: unknown = loaded;

  // SAFETY: fixture for the widen-then-assert rule.
  return stored as User;
}
