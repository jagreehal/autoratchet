export function start(options: { a: boolean; b: boolean }) {
  const session = {
    read(key: string) {
      if (options.a) { if (key) { return 1; } }
      return 0;
    },
    destroy() {
      if (options.b) { for (const x of [1, 2]) { if (x > 1) { return x; } } }
      return 0;
    },
  };
  class Local {
    run(a: boolean) { if (a) { if (options.a) { return 1; } } return 0; }
  }
  return { session, Local };
}
