export function f(ok: boolean) {
  return new Promise((resolve, reject) => {
    if (ok) {
      resolve(1);
    }

    reject(new Error('no'));
  });
}
