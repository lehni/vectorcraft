// The standard pickers for scripts on the web. Browsers show their pickers only
// in a click, so we ask the app, whose dialog shows the browser's own in the
// click on its Choose…. Where a browser has none (Firefox, Safari), a picked
// file comes as a handle that only reads it, and a file to save as one that
// downloads it.
export function setupPickers(ask) {
  const { showDirectoryPicker: nativeDirectoryPicker } = globalThis

  const askFor = async question => {
    const picked = await ask(question)
    if (!picked) {
      throw new DOMException('The user aborted a request.', 'AbortError')
    }
    return picked
  }

  Object.assign(globalThis, {
    async showOpenFilePicker(options = {}) {
      const picked = await askFor({
        type: options.multiple ? 'openFiles' : 'openFile',
        options: getOptions(options)
      })
      return picked.map(item =>
        item instanceof File ? new ReadOnlyFileHandle(item) : item
      )
    },

    async showSaveFilePicker(options = {}) {
      const [picked] = await askFor({
        type: 'saveFile',
        options: getOptions(options)
      })
      return typeof picked === 'string' ? new DownloadHandle(picked) : picked
    },

    async showDirectoryPicker(options = {}) {
      if (!nativeDirectoryPicker) {
        throw new DOMException(
          'Unsupported directories: this browser has no directory picker.',
          'NotSupportedError'
        )
      }
      const [picked] = await askFor({
        type: 'chooseDirectory',
        options: getOptions(options)
      })
      return picked
    }
  })
}

// A file the user picked where the browser has no handles: it can only be read.
export class ReadOnlyFileHandle {
  kind = 'file'

  constructor(file) {
    this.file = file
    this.name = file.name
  }

  async getFile() {
    return this.file
  }

  async createWritable() {
    throw new DOMException(
      'Unable to write a picked file: Dialog.saveFile() gives one to write.',
      'NotAllowedError'
    )
  }

  async isSameEntry(other) {
    return other === this
  }

  async queryPermission() {
    return 'granted'
  }

  async requestPermission() {
    return 'granted'
  }
}

// A file to save where the browser has no handles: closing what writes to it
// downloads it, under its name.
export class DownloadHandle {
  kind = 'file'

  constructor(name) {
    this.name = name
    this.parts = []
  }

  async getFile() {
    return new File(this.parts, this.name)
  }

  async createWritable({ keepExistingData = false } = {}) {
    const parts = keepExistingData ? [...this.parts] : []
    return {
      write: async data => {
        // A download is written from start to end: no seeking.
        if (data?.type && !(data instanceof Blob)) {
          if (data.type !== 'write' || data.position != null) {
            throw new DOMException(
              `Unsupported '${data.type}' on a download.`,
              'NotSupportedError'
            )
          }
          data = data.data
        }
        parts.push(data)
      },
      close: async () => {
        this.parts = parts
        download(new Blob(parts), this.name)
      },
      abort: async () => {}
    }
  }

  async isSameEntry(other) {
    return other === this
  }

  async queryPermission() {
    return 'granted'
  }

  async requestPermission() {
    return 'granted'
  }
}

// The options as the app takes them, as JSON: a handle to start in means
// nothing to it.
function getOptions({ startIn, ...options }) {
  return typeof startIn === 'string' ? { ...options, startIn } : options
}

function download(blob, name) {
  const url = URL.createObjectURL(blob)
  const link = document.createElement('a')
  link.href = url
  link.download = name
  link.click()
  // Once the download has its own copy:
  setTimeout(() => URL.revokeObjectURL(url), 60000)
}
