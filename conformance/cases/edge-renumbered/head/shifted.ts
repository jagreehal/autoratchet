type Run = (task: () => void) => void;
type Log = (value: boolean) => void;

// Head puts a clean callback before this one, which keeps its one finding and gains another.
export function worse(x: number, a: boolean, b: boolean, run: Run, log: Log) {
  if (b) {
    run(() => {
      log(x === 1);
    });
  }
  if (a) {
    run(() => {
      log(x == 1);
      log(x == 2);
    });
  }
}

// Head puts a clean callback before this one, which stays as it was.
export function same(x: number, a: boolean, b: boolean, run: Run, log: Log) {
  if (b) {
    run(() => {
      log(x === 1);
    });
  }
  if (a) {
    run(() => {
      log(x == 1);
    });
  }
}

// Head cleans this one up, and puts a new one with a finding before it.
export function swapped(x: number, a: boolean, b: boolean, run: Run, log: Log) {
  if (b) {
    run(() => {
      log(x == 1);
    });
  }
  if (a) {
    run(() => {
      log(x === 1);
    });
  }
}

// Head replaces this with another method, whose name only looks like the second of this one's.
export class Worker {
  ['run#2'](x: number, log: Log) {
    log(x == 1);
    // oxlint-disable-next-line eqeqeq -- loose on purpose
    log(x == 2);
  }
}

// Not in the method before it, which head removes: the `>` is part of its name.
export class Quoted {
  ['run>2'](x: number, log: Log) {
    log(x == 1);
  }
}
