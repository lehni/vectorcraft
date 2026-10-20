// Name the instances of `classes` as the realm's own are named, in
// `Object.prototype.toString()`: `[object Blob]`.
export function setToStringTag(...classes) {
  for (const Class of classes) {
    Object.defineProperty(Class.prototype, Symbol.toStringTag, {
      value: Class.name,
      configurable: true
    })
  }
}
