import { Blob, File, getBufferBytes } from './blob.js'
import { checkInternal, internal, setToStringTag } from './define.js'
import { getBody } from './fetch.js'
import { getType } from './local-file.js'
import { natives } from './natives.js'

// The File System Access API on desktop, over paths: the pickers ask the app,
// and give Promises of handles, or reject with an `AbortError` when the user
// cancels. A file handle's `getFile()` is a `File` of what it holds now, and
// its `createWritable()` a stream that writes the file on `close()`, as
// browsers do. What's left out: the stream is not a `WritableStream` (no
// `getWriter()` or `pipeTo()`).

// The paths of the handles:
const pathsByHandle = new WeakMap()

// The largest file a stream writes, in bytes:
const most = 512 * 1024 * 1024

export class FileSystemHandle {
  constructor(key, path) {
    checkInternal(key, 'FileSystemHandle', 'the pickers give them')
    pathsByHandle.set(this, path)
  }

  get kind() {
    return this instanceof FileSystemDirectoryHandle ? 'directory' : 'file'
  }

  get name() {
    return natives.fs.info(getPath(this)).name
  }

  async isSameEntry(other) {
    return pathsByHandle.get(other) === getPath(this)
  }

  // Granted: scripts on desktop may change what the user's files.
  async queryPermission() {
    return 'granted'
  }

  async requestPermission() {
    return 'granted'
  }
}

export class FileSystemFileHandle extends FileSystemHandle {
  async getFile() {
    const path = getPath(this)
    const { name, lastModified } = natives.fs.info(path)
    return new File([natives.fs.read(path)], name, {
      type: getType(path),
      lastModified
    })
  }

  // A stream that writes the file on `close()`, starting empty, or with what
  // the file holds.
  async createWritable({ keepExistingData = false } = {}) {
    const path = getPath(this)
    return new FileSystemWritableFileStream(
      internal,
      path,
      keepExistingData ? natives.fs.read(path) : new Uint8Array()
    )
  }
}

export class FileSystemDirectoryHandle extends FileSystemHandle {
  async getFileHandle(name, { create = false } = {}) {
    return getEntry(this, name, false, create)
  }

  async getDirectoryHandle(name, { create = false } = {}) {
    return getEntry(this, name, true, create)
  }

  async removeEntry(name, { recursive = false } = {}) {
    natives.fs.remove(getEntryPath(this, name), recursive)
  }

  // The names from the directory down to `handle`, or `null` outside it.
  async resolve(handle) {
    const path = pathsByHandle.get(handle)
    return path ? natives.fs.relative(getPath(this), path) : null
  }

  // What's in the directory, by name:
  async *entries() {
    for (const path of natives.fs.list(getPath(this))) {
      yield [natives.fs.info(path).name, createHandle(path)]
    }
  }

  async *keys() {
    for await (const [name] of this.entries()) yield name
  }

  async *values() {
    for await (const [, handle] of this.entries()) yield handle
  }

  [Symbol.asyncIterator]() {
    return this.entries()
  }
}

export class FileSystemWritableFileStream {
  #path
  #bytes
  #position = 0
  #isClosed = false

  constructor(key, path, bytes) {
    checkInternal(
      key,
      'FileSystemWritableFileStream',
      'createWritable() gives them'
    )
    this.#path = path
    this.#bytes = bytes
  }

  // Text, a `Blob`, a buffer or a view at the position, or
  // `{ type: 'write' | 'seek' | 'truncate', data, position, size }`.
  async write(data) {
    const isCommand = (
      data &&
      typeof data === 'object' &&
      typeof data.type === 'string' &&
      !(data instanceof Blob) &&
      !getBufferBytes(data)
    )
    const { type, position, size } = isCommand ? data : { type: 'write' }
    if (type === 'seek') {
      return this.seek(position)
    } else if (type === 'truncate') {
      return this.truncate(size)
    } else if (type === 'write') {
      if (isCommand) data = data.data
      if (data == null) {
        throw new DOMException(
          'Invalid write(): the data is missing.',
          'SyntaxError'
        )
      }
      this.#check()
      const { bytes } = getBody(data)
      const start = getCount(position ?? this.#position)
      const end = getCount(start + bytes.length)
      if (this.#bytes.length < end) this.#resize(end)
      this.#bytes.set(bytes, start)
      this.#position = end
    } else {
      throw new TypeError(`Invalid write() command: '${type}'.`)
    }
  }

  // Where the stream writes next.
  async seek(position) {
    if (position == null) {
      throw new DOMException(
        'Invalid seek(): the position is missing.',
        'SyntaxError'
      )
    }
    this.#check()
    this.#position = getCount(position)
  }

  // Its bytes cut, or filled with zeros, to `size`.
  async truncate(size) {
    this.#check()
    this.#resize(getCount(size))
    this.#position = Math.min(this.#position, this.#bytes.length)
  }

  // Write the file, all at once.
  async close() {
    this.#check()
    this.#isClosed = true
    natives.fs.write(this.#path, this.#bytes, false)
  }

  async abort() {
    this.#check()
    this.#isClosed = true
  }

  #check() {
    if (this.#isClosed) {
      throw new TypeError('Invalid stream: it is closed.')
    }
  }

  #resize(size) {
    const bytes = new Uint8Array(size)
    bytes.set(this.#bytes.subarray(0, size))
    this.#bytes = bytes
  }
}

setToStringTag(
  FileSystemHandle,
  FileSystemFileHandle,
  FileSystemDirectoryHandle,
  FileSystemWritableFileStream
)

// The handles of the files picked.
export async function showOpenFilePicker(options = {}) {
  const paths = pick(options.multiple ? 'openFiles' : 'openFile', options)
  return paths.map(path => createHandle(path, false))
}

// The handle of the file to save, which exists from then on, as browsers make
// it.
export async function showSaveFilePicker(options = {}) {
  const start = getStart(options)
  const { suggestedName } = options
  const path =
    suggestedName == null
      ? start
      : start
        ? natives.fs.path(start, String(suggestedName))
        : String(suggestedName)
  const [picked] = pick('saveFile', options, path)
  if (!natives.fs.info(picked).exists) {
    natives.fs.write(picked, new Uint8Array(), false)
  }
  return createHandle(picked, false)
}

export async function showDirectoryPicker(options = {}) {
  const [picked] = pick('chooseDirectory', { startIn: options.startIn })
  return createHandle(picked, true)
}

// The path of `handle`, or `null` when it's not one of these handles.
export function getHandlePath(handle) {
  return pathsByHandle.get(handle) ?? null
}

function getPath(handle) {
  const path = pathsByHandle.get(handle)
  if (!path) {
    throw new TypeError(`Invalid handle: '${handle}', not a FileSystemHandle.`)
  }
  return path
}

function createHandle(path, isDirectory = natives.fs.info(path).isDirectory) {
  const Handle = isDirectory ? FileSystemDirectoryHandle : FileSystemFileHandle
  return new Handle(internal, path)
}

// The path of the entry `name` in the directory: a name, not a path.
function getEntryPath(directory, name) {
  name = String(name)
  if (!name || name === '.' || name === '..' || /[/\\]/.test(name)) {
    throw new TypeError(`Invalid name: '${name}'.`)
  }
  return natives.fs.path(getPath(directory), name)
}

function getEntry(directory, name, isDirectory, create) {
  const path = getEntryPath(directory, name)
  const info = natives.fs.info(path)
  if (info.exists) {
    if (info.isDirectory !== isDirectory) {
      throw new DOMException(
        `Invalid entry: '${path}' is a ${
          info.isDirectory ? 'directory' : 'file'
        }.`,
        'TypeMismatchError'
      )
    }
  } else if (create) {
    if (isDirectory) {
      natives.fs.makeDirectory(path, false)
    } else {
      natives.fs.write(path, new Uint8Array(), false)
    }
  } else {
    throw new DOMException(`Unable to find '${path}'.`, 'NotFoundError')
  }
  return createHandle(path, isDirectory)
}

// A size or position as a count of bytes, up to `most`.
function getCount(value) {
  const count = Number(value)
  if (!(count >= 0 && Number.isFinite(count))) {
    throw new TypeError(`Invalid size: '${value}'.`)
  }
  if (count > most) {
    throw new DOMException(
      `Unable to write ${count} bytes: more than a file may have here.`,
      'QuotaExceededError'
    )
  }
  return Math.trunc(count)
}

// The paths the user picked in the app's dialog of `type`, or an `AbortError`.
function pick(type, options, path = getStart(options)) {
  const answer = natives.ask({ type, path, filters: getFilters(options) })
  const paths = (Array.isArray(answer) ? answer : [answer]).filter(
    path => typeof path === 'string'
  )
  if (!paths.length) {
    throw new DOMException('The user aborted a request.', 'AbortError')
  }
  return paths
}

// The directory the picker starts in: `startIn`, a handle or a `LocalFile` (a
// file's: its directory) or one of the user's directories (`'desktop'`,
// `'documents'`, `'downloads'`, `'music'`, `'pictures'`, `'videos'`).
function getStart({ startIn }) {
  if (startIn == null) {
    return null
  } else if (typeof startIn === 'object') {
    const path = getHandlePath(startIn) ?? startIn.path
    if (typeof path !== 'string') {
      throw new TypeError(
        `Invalid startIn: '${startIn}', not a handle or a LocalFile.`
      )
    }
    const info = natives.fs.info(path)
    return info.isDirectory ? path : info.parent
  } else {
    return natives.fs.userDirectory(String(startIn))
  }
}

// The pickers' `types` (`[{ description, accept: { type: extensions } }]`) as
// the app's file types, `[label, [extensions]]`, with one for every file unless
// `excludeAcceptAllOption`.
function getFilters({ types, excludeAcceptAllOption }) {
  const filters = Array.from(types ?? [], ({ description, accept }) => {
    if (!accept) {
      throw new TypeError('Invalid file type: its accept is missing.')
    }
    const extensions = Object.values(accept)
      .flat()
      .map(extension => {
        extension = String(extension)
        if (!/^\.[^.]/.test(extension)) {
          throw new TypeError(
            `Invalid extension: '${extension}', which starts with '.'.`
          )
        }
        return extension.slice(1)
      })
    return [
      description || extensions.map(extension => `.${extension}`).join(', '),
      extensions
    ]
  })
  if (filters.length && !excludeAcceptAllOption) {
    filters.push(['All Files', []])
  }
  return filters
}
