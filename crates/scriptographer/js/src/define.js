// Defining classes as the realm defines its own, and as WebIDL has them.

// Inject the properties of `source` into `target`, getters and setters too:
// not enumerable, as the realm's own methods aren't, unless `attributes` say
// otherwise.
export function inject(target, source, attributes) {
  for (const key of Reflect.ownKeys(source)) {
    Object.defineProperty(target, key, {
      ...Object.getOwnPropertyDescriptor(source, key),
      enumerable: false,
      ...attributes
    })
  }
  return target
}

// The `constants` of `Class`, on it and its prototype.
export function defineConstants(Class, constants) {
  for (const target of [Class, Class.prototype]) {
    inject(target, constants, {
      enumerable: true,
      writable: false,
      configurable: false
    })
  }
}

// Name the instances of `classes` as the realm's own are named, in
// `Object.prototype.toString()`: `[object Blob]`.
export function setToStringTag(...classes) {
  for (const Class of classes) {
    inject(
      Class.prototype,
      { [Symbol.toStringTag]: Class.name },
      { writable: false }
    )
  }
}

// What only the runtime's own code passes the classes scripts can't construct:
export const internal = Symbol('internal')

// A `TypeError` unless the runtime's own code constructs the class `name`,
// saying `how` scripts get its instances.
export function checkInternal(key, name, how) {
  if (key !== internal) {
    throw new TypeError(`Invalid ${name}: ${how}.`)
  }
}

// A `TypeError` when `name()` got fewer than `count` arguments.
export function checkArguments(args, count, name) {
  if (args.length < count) {
    throw new TypeError(`Invalid ${name}(): an argument is missing.`)
  }
}
