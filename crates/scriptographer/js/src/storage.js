import { checkArguments, internal, setToStringTag } from './define.js'
import { natives } from './natives.js'

// `localStorage` and `sessionStorage` for scripts outside a browser. The
// runtime keeps their items, and a script's `localStorage` in its file. Items
// read and write as properties too (`localStorage.size = '10'`), through a
// `Proxy`.

// Whether a storage, its proxy or its target, is `sessionStorage`:
const sessions = new WeakMap()

export class Storage {
  constructor(key) {
    if (key !== internal) {
      throw new TypeError(
        'Invalid Storage: use localStorage or sessionStorage.'
      )
    }
  }

  get length() {
    return getKeys(this).length
  }

  key(index) {
    return getKeys(this)[index >>> 0] ?? null
  }

  getItem(key) {
    checkArguments(arguments, 1, 'getItem')
    return natives.storage.getItem(getSession(this), String(key))
  }

  // Throws a `QuotaExceededError` when that's more than it holds.
  setItem(key, value) {
    checkArguments(arguments, 2, 'setItem')
    natives.storage.setItem(getSession(this), String(key), String(value))
  }

  removeItem(key) {
    checkArguments(arguments, 1, 'removeItem')
    natives.storage.removeItem(getSession(this), String(key))
  }

  clear() {
    natives.storage.clear(getSession(this))
  }
}

setToStringTag(Storage)

// Reading a property the storage has gives that one, any other string an item,
// as browsers have it. Any string set is an item, even one the storage has
// (`localStorage.key = '1'`), as WebIDL's named property setter has it.
const handler = {
  get(target, key, receiver) {
    return isItemKey(target, key)
      ? target.getItem(key) ?? undefined
      : Reflect.get(target, key, receiver)
  },

  set(target, key, value, receiver) {
    if (typeof key === 'string') {
      target.setItem(key, value)
      return true
    } else {
      return Reflect.set(target, key, value, receiver)
    }
  },

  has(target, key) {
    return (
      typeof key === 'string' && target.getItem(key) !== null ||
      Reflect.has(target, key)
    )
  },

  deleteProperty(target, key) {
    if (isItemKey(target, key)) {
      target.removeItem(key)
      return true
    } else {
      return Reflect.deleteProperty(target, key)
    }
  },

  // The items' keys, then the storage's own, as `Object.keys()` lists them.
  ownKeys(target) {
    return [...new Set([...getKeys(target), ...Reflect.ownKeys(target)])]
  },

  getOwnPropertyDescriptor(target, key) {
    const value = isItemKey(target, key) ? target.getItem(key) : null
    return value !== null
      ? { value, writable: true, enumerable: true, configurable: true }
      : Reflect.getOwnPropertyDescriptor(target, key)
  }
}

export function createStorage(isSession) {
  const target = new Storage(internal)
  const storage = new Proxy(target, handler)
  sessions.set(target, isSession)
  sessions.set(storage, isSession)
  return storage
}

function getSession(storage) {
  const isSession = sessions.get(storage)
  if (isSession === undefined) {
    throw new TypeError(`Invalid storage: '${storage}', not a Storage.`)
  }
  return isSession
}

function getKeys(storage) {
  return natives.storage.keys(getSession(storage))
}

function isItemKey(target, key) {
  return typeof key === 'string' && !(key in target)
}
