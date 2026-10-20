(() => {
  // src/web/pickers.js
  function setupPickers(ask2) {
    const { showDirectoryPicker: nativeDirectoryPicker } = globalThis;
    const askFor = async (question) => {
      const picked = await ask2(question);
      if (!picked) {
        throw new DOMException("The user aborted a request.", "AbortError");
      }
      return picked;
    };
    Object.assign(globalThis, {
      async showOpenFilePicker(options = {}) {
        const picked = await askFor({
          type: options.multiple ? "openFiles" : "openFile",
          options: getOptions(options)
        });
        return picked.map(
          (item) => item instanceof File ? new ReadOnlyFileHandle(item) : item
        );
      },
      async showSaveFilePicker(options = {}) {
        const [picked] = await askFor({
          type: "saveFile",
          options: getOptions(options)
        });
        return typeof picked === "string" ? new DownloadHandle(picked) : picked;
      },
      async showDirectoryPicker(options = {}) {
        if (!nativeDirectoryPicker) {
          throw new DOMException(
            "Unsupported directories: this browser has no directory picker.",
            "NotSupportedError"
          );
        }
        const [picked] = await askFor({
          type: "chooseDirectory",
          options: getOptions(options)
        });
        return picked;
      }
    });
  }
  var ReadOnlyFileHandle = class {
    kind = "file";
    constructor(file) {
      this.file = file;
      this.name = file.name;
    }
    async getFile() {
      return this.file;
    }
    async createWritable() {
      throw new DOMException(
        "Unable to write a picked file: Dialog.saveFile() gives one to write.",
        "NotAllowedError"
      );
    }
    async isSameEntry(other) {
      return other === this;
    }
    async queryPermission() {
      return "granted";
    }
    async requestPermission() {
      return "granted";
    }
  };
  var DownloadHandle = class {
    kind = "file";
    constructor(name) {
      this.name = name;
      this.parts = [];
    }
    async getFile() {
      return new File(this.parts, this.name);
    }
    async createWritable({ keepExistingData = false } = {}) {
      const parts = keepExistingData ? [...this.parts] : [];
      return {
        write: async (data) => {
          if (data?.type && !(data instanceof Blob)) {
            if (data.type !== "write" || data.position != null) {
              throw new DOMException(
                `Unsupported '${data.type}' on a download.`,
                "NotSupportedError"
              );
            }
            data = data.data;
          }
          parts.push(data);
        },
        close: async () => {
          this.parts = parts;
          download(new Blob(parts), this.name);
        },
        abort: async () => {
        }
      };
    }
    async isSameEntry(other) {
      return other === this;
    }
    async queryPermission() {
      return "granted";
    }
    async requestPermission() {
      return "granted";
    }
  };
  function getOptions({ startIn, ...options }) {
    return typeof startIn === "string" ? { ...options, startIn } : options;
  }
  function download(blob, name) {
    const url = URL.createObjectURL(blob);
    const link = document.createElement("a");
    link.href = url;
    link.download = name;
    link.click();
    setTimeout(() => URL.revokeObjectURL(url), 6e4);
  }

  // src/to-string-tag.js
  function setToStringTag(...classes) {
    for (const Class of classes) {
      Object.defineProperty(Class.prototype, Symbol.toStringTag, {
        value: Class.name,
        configurable: true
      });
    }
  }

  // src/web/local-file.js
  var handles = /* @__PURE__ */ new WeakMap();
  var LocalFile = class _LocalFile extends File {
    constructor() {
      throw new TypeError(
        "Unable to make a LocalFile from a path on the web: Dialog.openFile() and LocalFile.from(handle) give them."
      );
    }
    static async from(handle) {
      const file = handle.kind === "directory" ? new File([], handle.name) : await handle.getFile();
      const local = Reflect.construct(
        File,
        [[file], file.name, { type: file.type, lastModified: file.lastModified }],
        _LocalFile
      );
      handles.set(local, handle);
      return local;
    }
    get path() {
      return null;
    }
    get parent() {
      return null;
    }
    get extension() {
      const { name } = this;
      const index = name.lastIndexOf(".");
      return index > 0 ? name.slice(index + 1) : "";
    }
    get isDirectory() {
      return handles.get(this).kind === "directory";
    }
    get writable() {
      return !(handles.get(this) instanceof ReadOnlyFileHandle);
    }
    // Read the file as it is now, not as it came:
    async text() {
      return (await handles.get(this).getFile()).text();
    }
    async arrayBuffer() {
      return (await handles.get(this).getFile()).arrayBuffer();
    }
    async bytes() {
      return new Uint8Array(await this.arrayBuffer());
    }
    async write(data) {
      const stream = await handles.get(this).createWritable();
      await stream.write(data);
      await stream.close();
    }
    async append(data) {
      const handle = handles.get(this);
      const { size } = await handle.getFile();
      const stream = await handle.createWritable({ keepExistingData: true });
      await stream.write({ type: "write", position: size, data });
      await stream.close();
    }
    async exists() {
      const handle = handles.get(this);
      try {
        await (handle.kind === "directory" ? handle.keys().next() : handle.getFile());
        return true;
      } catch (err) {
        if (err.name === "NotFoundError") return false;
        throw err;
      }
    }
    async list() {
      const files = [];
      for await (const handle of handles.get(this).values()) {
        files.push(await _LocalFile.from(handle));
      }
      return files.sort(
        (a, b) => a.name < b.name ? -1 : a.name > b.name ? 1 : 0
      );
    }
    // The file or directory `name` in this directory. A handle can't name what
    // isn't there, so a missing file is made, empty.
    async file(name) {
      const directory = handles.get(this);
      try {
        return await _LocalFile.from(await directory.getFileHandle(name));
      } catch (err) {
        if (err.name === "TypeMismatchError") {
          return _LocalFile.from(await directory.getDirectoryHandle(name));
        }
        if (err.name === "NotFoundError") {
          return _LocalFile.from(
            await directory.getFileHandle(name, { create: true })
          );
        }
        throw err;
      }
    }
    async makeDirectory() {
      if (this.isDirectory) return this;
      throw new DOMException(
        "Unable to make a directory of a file on the web.",
        "NotSupportedError"
      );
    }
    async remove({ recursive = false } = {}) {
      await handles.get(this).remove({ recursive });
    }
    // The renamed file, a `LocalFile` of its own: a `File`'s name can't change.
    async rename(name) {
      const handle = handles.get(this);
      await handle.move(name);
      return _LocalFile.from(handle);
    }
    async copyTo(target) {
      if (!(target instanceof _LocalFile)) {
        throw new TypeError(`Invalid target: '${target}', not a LocalFile.`);
      }
      await target.write(await this.bytes());
      return target;
    }
    toString() {
      return this.name;
    }
  };
  setToStringTag(LocalFile);

  // src/web/index.js
  var { ask } = globalThis.__scriptographer;
  setupPickers(ask);
  globalThis.LocalFile = LocalFile;
})();
