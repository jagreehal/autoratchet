export function step(text: string): void;
export function step(text: string, data: unknown): void;
export function step(text: string, data?: unknown): void {
  if (data) { console.log(text, data); } else { console.log(text); }
}
export declare function external(a: number): number;
export abstract class Shape {
  abstract area(): number;
  scale(factor: number): void;
  scale(factor: number, origin: number): void;
  scale(factor: number, origin?: number): void {
    if (origin) { console.log(factor, origin); }
  }
}
