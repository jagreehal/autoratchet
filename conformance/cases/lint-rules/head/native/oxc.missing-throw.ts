export function f(ok: boolean) {
  if (!ok) {
    new Error('not ok');
  }

  return ok;
}
