// The error with its stack, where the engine's starts with it, as V8's does.
export function formatError(err) {
  const text = String(err)
  const stack = err?.stack
  return typeof stack === 'string' && stack.startsWith(text) ? stack : text
}
