export function lineApproved(x: number, log: (b: boolean) => void) {
  // oxlint-disable-next-line eqeqeq -- reason
  log(x == 2);
}

export function proseIsNot(x: number, log: (b: boolean) => void) {
  // Example syntax: oxlint-disable-next-line eqeqeq -- reason
  log(x == 2);
}

export function docIsNot(x: number, log: (b: boolean) => void) {
  /** oxlint-disable-next-line eqeqeq */
  log(x == 2);
}

export function tripleSlashIsNot(x: number, log: (b: boolean) => void) {
  /// oxlint-disable-next-line eqeqeq
  log(x == 2);
}

export function twoLineBlock(x: number, log: (b: boolean) => void) {
  /* oxlint-disable-next-line eqeqeq
     -- reason */
  log(x == 2);
}

export function rulesOnTheNextLine(x: number, log: (b: boolean) => void) {
  /* oxlint-disable-next-line no-eval,
     eqeqeq */
  log(x == 2);
}

export function restOfTheClosingLine(x: number, log: (b: boolean) => void) {
  /* oxlint-disable-next-line eqeqeq */ log(x == 2);
}

export function notBeforeTheComment(x: number, log: (b: boolean) => void) {
  log(x == 2); /* oxlint-disable-next-line eqeqeq */
  log(false);
}

export function lineBefore(x: number, log: (b: boolean) => void) {
  log(x == 2); // oxlint-disable-line eqeqeq
}

export function lineNotAfter(x: number, log: (b: boolean) => void) {
  /* oxlint-disable-line eqeqeq */ log(x == 2);
}

export function lineBothSides(x: number, log: (b: boolean) => void) {
  log(x == 2); /* oxlint-disable-line eqeqeq */ log(x == 2);
}

export function reasonNamesAnother(x: number, log: (b: boolean) => void) {
  // oxlint-disable-next-line eqeqeq -- not a -disable-line
  log(x == 2);
}
