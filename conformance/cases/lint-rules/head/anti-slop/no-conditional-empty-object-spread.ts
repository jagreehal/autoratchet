export function f(timeout: number | undefined) {
  return { ...(timeout !== undefined ? { timeout } : {}) };
}
