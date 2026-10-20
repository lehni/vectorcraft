import { File, readBytes } from './blob.js'
import { setToStringTag } from './define.js'
import { getBody } from './fetch.js'
import { getHandlePath } from './file-system.js'
import { natives } from './natives.js'

// `LocalFile`, a file or directory on the user's computer, on desktop: a
// `File` (and so a `Blob`, for `fetch()`, `FormData` and `createImageBitmap()`)
// of its path, which shows the file as it is now, and can change it.
// `new LocalFile(...parts)` joins its parts (paths or `LocalFile`s, the last
// its name; a relative path is from the script's folder):
// `new LocalFile(script.directory, 'data', 'notes.txt')`.

// MIME types by extension, as browsers give a file's `type`:
const types = {
  txt: 'text/plain',
  md: 'text/markdown',
  csv: 'text/csv',
  html: 'text/html',
  css: 'text/css',
  js: 'text/javascript',
  json: 'application/json',
  xml: 'application/xml',
  pdf: 'application/pdf',
  svg: 'image/svg+xml',
  png: 'image/png',
  jpg: 'image/jpeg',
  jpeg: 'image/jpeg',
  gif: 'image/gif',
  webp: 'image/webp',
  bmp: 'image/bmp',
  tif: 'image/tiff',
  tiff: 'image/tiff'
}

export class LocalFile extends File {
  #path

  constructor(...parts) {
    if (!parts.length) {
      throw new TypeError('Invalid LocalFile: its path is missing.')
    }
    const path = natives.fs.path(...parts.map(String))
    super([], natives.fs.info(path).name)
    this.#path = path
  }

  // The `LocalFile` of a file or directory handle.
  static async from(handle) {
    const path = getHandlePath(handle)
    if (!path) {
      throw new TypeError(
        `Invalid handle: '${handle}', not a FileSystemHandle.`
      )
    }
    return new LocalFile(path)
  }

  get path() {
    return this.#path
  }

  get name() {
    return this.#info.name
  }

  get extension() {
    return this.#info.extension
  }

  get type() {
    return getType(this.#path)
  }

  get parent() {
    const { parent } = this.#info
    return parent ? new LocalFile(parent) : null
  }

  get isDirectory() {
    return this.#info.isDirectory
  }

  // On desktop, every file can be written where it is.
  get writable() {
    return true
  }

  get size() {
    return this.#info.size
  }

  get lastModified() {
    return this.#info.lastModified
  }

  // What it holds now, as its readers read it:
  [readBytes]() {
    return natives.fs.read(this.#path)
  }

  // Text, a `Blob`, a buffer or a view.
  async write(data = '') {
    natives.fs.write(this.#path, getBody(data).bytes, false)
  }

  async append(data = '') {
    natives.fs.write(this.#path, getBody(data).bytes, true)
  }

  async exists() {
    return this.#info.exists
  }

  // The files and directories in the directory, by name.
  async list() {
    return natives.fs.list(this.#path).map(path => new LocalFile(path))
  }

  // The file or directory `name` in this directory.
  async file(name) {
    return new LocalFile(this.#path, String(name))
  }

  async makeDirectory() {
    natives.fs.makeDirectory(this.#path, true)
    return this
  }

  // A name renames it where it is, a path moves it; the `LocalFile` follows.
  async rename(nameOrPath) {
    this.#path = natives.fs.rename(this.#path, String(nameOrPath))
    return this
  }

  async copyTo(fileOrPath) {
    const copy = new LocalFile(fileOrPath)
    natives.fs.copy(this.#path, copy.path)
    return copy
  }

  // A directory with what's in it only when `recursive`.
  async remove({ recursive = false } = {}) {
    natives.fs.remove(this.#path, recursive)
  }

  toString() {
    return this.#path
  }

  get #info() {
    return natives.fs.info(this.#path)
  }
}

setToStringTag(LocalFile)

// The MIME type of the file at `path`, by its extension, or none.
export function getType(path) {
  return types[natives.fs.info(path).extension.toLowerCase()] ?? ''
}
