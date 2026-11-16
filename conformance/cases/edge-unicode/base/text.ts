const GREETING = 'héllo wörld 🎉 — ünïcödé';
export function greet(name: string, loud: boolean) {
  if (loud) return `${GREETING.toUpperCase()} ${name}!`;
  return `${GREETING} ${name}`;
}
