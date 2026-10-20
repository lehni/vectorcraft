// Defining properties as the realm defines its own.

// `values` as data properties of `target`, symbols too, with `attributes`.
export function defineValues(target, values, attributes = {}) {
  for (const key of Reflect.ownKeys(values)) {
    Object.defineProperty(target, key, { value: values[key], ...attributes })
  }
}

// `accessors` (`{ name: { get, set } }`) as properties of `target`, enumerable
// and configurable, as WebIDL's attributes are.
export function defineAccessors(target, accessors) {
  for (const key of Reflect.ownKeys(accessors)) {
    Object.defineProperty(target, key, {
      ...accessors[key],
      enumerable: true,
      configurable: true
    })
  }
}

// Name the instances of `classes` as the realm's own are named, in
// `Object.prototype.toString()`: `[object Blob]`.
export function setToStringTag(...classes) {
  for (const Class of classes) {
    defineValues(
      Class.prototype,
      { [Symbol.toStringTag]: Class.name },
      { configurable: true }
    )
  }
}
