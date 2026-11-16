function twice(a) { return a > 1 ? 1 : 2; }
export const first = () => { function twice(a) { return a ? 1 : 2; } return twice; };
export const second = () => { function twice(a) { return a ? 1 : 2; } return twice; };
export { twice };
[1, 2].forEach((x) => (x > 1 ? 1 : 2));
[1, 2].forEach((x) => (x > 2 ? 1 : 2));
