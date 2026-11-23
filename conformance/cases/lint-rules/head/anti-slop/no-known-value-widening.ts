type Handler = () => void;

declare const startHandler: Handler;

export function f() {
  const handlers: Record<string, Handler> = { start: startHandler };

  return handlers;
}
