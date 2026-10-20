// What the runtime and the bundle share: the natives the runtime passes, and
// what the bundle gives back to it, as `__scriptographer`, which the bundle
// takes off the global object.
export const natives = globalThis.__scriptographer
