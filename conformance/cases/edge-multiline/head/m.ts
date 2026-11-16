export const load = async (
  id: string,
  retry: boolean,
): Promise<number> => {
  if (retry) {
    for (let i = 0; i < 3; i++) {
      if (id) {
        try { return 1; } catch { if (i > 1) return 2; }
      }
    }
  }
  return 0;
};
export class Repo {
  async find(
    id: string,
    deep: boolean,
  ) {
    if (deep) { if (id) { for (;;) { if (id.length > 2) return 1; } } }
    return 0;
  }
}
