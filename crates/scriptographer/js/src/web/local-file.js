import { ReadOnlyFileHandle } from './pickers.js'
import { setToStringTag } from '../to-string-tag.js'

// The handle each `LocalFile` changes its file through. Its instances are made
// with `Reflect.construct(File, ...)`, so its constructor never runs on them.
const handles = new WeakMap()

// A file or directory on the user's computer, on the web: a `File` of the
// browser's, as it was when it came, which reads and changes the file through
// its handle. The web has no paths: `path` and `parent` are `null`.
export class LocalFile extends File {
  constructor() {
    throw new TypeError(
      'Unable to make a LocalFile from a path on the web: ' +
      'Dialog.openFile() and LocalFile.from(handle) give them.'
    )
  }

  static async from(handle) {
    const file =
      handle.kind === 'directory'
        ? new File([], handle.name)
        : await handle.getFile()
    const local = Reflect.construct(
      File,
      [[file], file.name, { type: file.type, lastModified: file.lastModified }],
      LocalFile
    )
    handles.set(local, handle)
    return local
  }

  get path() {
    return null
  }

  get parent() {
    return null
  }

  get extension() {
    const { name } = this
    const index = name.lastIndexOf('.')
    return index > 0 ? name.slice(index + 1) : ''
  }

  get isDirectory() {
    return handles.get(this).kind === 'directory'
  }

  get writable() {
    return !(handles.get(this) instanceof ReadOnlyFileHandle)
  }

  // Read the file as it is now, not as it came:
  async text() {
    return (await handles.get(this).getFile()).text()
  }

  async arrayBuffer() {
    return (await handles.get(this).getFile()).arrayBuffer()
  }

  async bytes() {
    return new Uint8Array(await this.arrayBuffer())
  }

  async write(data) {
    const stream = await handles.get(this).createWritable()
    await stream.write(data)
    await stream.close()
  }

  async append(data) {
    const handle = handles.get(this)
    const { size } = await handle.getFile()
    const stream = await handle.createWritable({ keepExistingData: true })
    await stream.write({ type: 'write', position: size, data })
    await stream.close()
  }

  async exists() {
    const handle = handles.get(this)
    try {
      await (
        handle.kind === 'directory'
          ? handle.keys().next()
          : handle.getFile()
      )
      return true
    } catch (err) {
      if (err.name === 'NotFoundError') return false
      throw err
    }
  }

  async list() {
    const files = []
    for await (const handle of handles.get(this).values()) {
      files.push(await LocalFile.from(handle))
    }
    return files.sort((a, b) =>
      a.name < b.name ? -1 : a.name > b.name ? 1 : 0
    )
  }

  // The file or directory `name` in this directory. A handle can't name what
  // isn't there, so a missing file is made, empty.
  async file(name) {
    const directory = handles.get(this)
    try {
      return await LocalFile.from(await directory.getFileHandle(name))
    } catch (err) {
      if (err.name === 'TypeMismatchError') {
        return LocalFile.from(await directory.getDirectoryHandle(name))
      }
      if (err.name === 'NotFoundError') {
        return LocalFile.from(
          await directory.getFileHandle(name, { create: true })
        )
      }
      throw err
    }
  }

  async makeDirectory() {
    if (this.isDirectory) return this
    throw new DOMException(
      'Unable to make a directory of a file on the web.',
      'NotSupportedError'
    )
  }

  async remove({ recursive = false } = {}) {
    await handles.get(this).remove({ recursive })
  }

  // The renamed file, a `LocalFile` of its own: a `File`'s name can't change.
  async rename(name) {
    const handle = handles.get(this)
    await handle.move(name)
    return LocalFile.from(handle)
  }

  async copyTo(target) {
    if (!(target instanceof LocalFile)) {
      throw new TypeError(`Invalid target: '${target}', not a LocalFile.`)
    }
    await target.write(await this.bytes())
    return target
  }

  toString() {
    return this.name
  }
}

setToStringTag(LocalFile)
