(() => {
  var __defProp = Object.defineProperty;
  var __name = (target, value) => __defProp(target, "name", { value, configurable: true });
  var __export = (target, all) => {
    for (var name in all)
      __defProp(target, name, { get: all[name], enumerable: true });
  };

  // src/natives.js
  var natives = globalThis.__scriptographer;

  // src/define.js
  function inject(target, source, attributes) {
    for (const key of Reflect.ownKeys(source)) {
      Object.defineProperty(target, key, {
        ...Object.getOwnPropertyDescriptor(source, key),
        enumerable: false,
        ...attributes
      });
    }
    return target;
  }
  __name(inject, "inject");
  function defineConstants(Class, constants2) {
    for (const target of [Class, Class.prototype]) {
      inject(target, constants2, {
        enumerable: true,
        writable: false,
        configurable: false
      });
    }
  }
  __name(defineConstants, "defineConstants");
  function setToStringTag(...classes) {
    for (const Class of classes) {
      inject(
        Class.prototype,
        { [Symbol.toStringTag]: Class.name },
        { writable: false }
      );
    }
  }
  __name(setToStringTag, "setToStringTag");
  var internal = /* @__PURE__ */ Symbol("internal");
  function checkInternal(key, name, how) {
    if (key !== internal) {
      throw new TypeError(`Invalid ${name}: ${how}.`);
    }
  }
  __name(checkInternal, "checkInternal");
  function checkArguments(args, count, name) {
    if (args.length < count) {
      throw new TypeError(`Invalid ${name}(): an argument is missing.`);
    }
  }
  __name(checkArguments, "checkArguments");

  // src/abort.js
  var abort_exports = {};
  __export(abort_exports, {
    AbortController: () => AbortController,
    AbortSignal: () => AbortSignal
  });

  // src/events.js
  var events_exports = {};
  __export(events_exports, {
    CloseEvent: () => CloseEvent,
    CustomEvent: () => CustomEvent,
    ErrorEvent: () => ErrorEvent,
    Event: () => Event,
    EventTarget: () => EventTarget,
    MessageEvent: () => MessageEvent,
    defineEventHandlers: () => defineEventHandlers,
    fire: () => fire
  });

  // src/global-scope.js
  var global_scope_exports = {};
  __export(global_scope_exports, {
    atob: () => atob,
    btoa: () => btoa,
    performance: () => performance,
    queueMicrotask: () => queueMicrotask,
    reportError: () => reportError
  });

  // src/format-error.js
  function formatError(err) {
    const text = String(err);
    const stack = err?.stack;
    return typeof stack === "string" && stack.startsWith(text) ? stack : text;
  }
  __name(formatError, "formatError");

  // src/global-scope.js
  var performance = {
    timeOrigin: Date.now(),
    // The milliseconds since the runtime started.
    now() {
      return natives.now();
    }
  };
  function queueMicrotask(callback) {
    if (typeof callback !== "function") {
      throw new TypeError(`Invalid callback: '${callback}', not a function.`);
    }
    Promise.resolve().then(() => {
      try {
        callback();
      } catch (err) {
        reportError(err);
      }
    });
  }
  __name(queueMicrotask, "queueMicrotask");
  function reportError(err) {
    console.error(`Uncaught ${formatError(err)}`);
  }
  __name(reportError, "reportError");
  var alphabet = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
  function atob(data) {
    let text = String(data).replace(/[\t\n\f\r ]/g, "");
    if (text.length % 4 === 0) text = text.replace(/==?$/, "");
    if (text.length % 4 === 1 || /[^A-Za-z0-9+/]/.test(text)) {
      throw new DOMException(
        "Invalid base64: the string is not correctly encoded.",
        "InvalidCharacterError"
      );
    }
    let bits = 0;
    let count = 0;
    let result = "";
    for (const char of text) {
      bits = bits << 6 | alphabet.indexOf(char);
      count += 6;
      if (count >= 8) {
        count -= 8;
        result += String.fromCharCode(bits >> count & 255);
      }
    }
    return result;
  }
  __name(atob, "atob");
  function btoa(data) {
    const text = String(data);
    if (/[^\0-\xff]/.test(text)) {
      throw new DOMException(
        "Invalid character: btoa() takes characters up to 255 only.",
        "InvalidCharacterError"
      );
    }
    let result = "";
    for (let i = 0; i < text.length; i += 3) {
      const [a, b, c] = [0, 1, 2].map((j) => text.charCodeAt(i + j));
      const bits = a << 16 | (b || 0) << 8 | (c || 0);
      result += alphabet[bits >> 18 & 63] + alphabet[bits >> 12 & 63] + (i + 1 < text.length ? alphabet[bits >> 6 & 63] : "=") + (i + 2 < text.length ? alphabet[bits & 63] : "=");
    }
    return result;
  }
  __name(btoa, "btoa");

  // src/events.js
  var phases = { NONE: 0, CAPTURING_PHASE: 1, AT_TARGET: 2, BUBBLING_PHASE: 3 };
  var listenersByTarget = /* @__PURE__ */ new WeakMap();
  var states = /* @__PURE__ */ new WeakMap();
  var handlersByTarget = /* @__PURE__ */ new WeakMap();
  var EventTarget = class {
    static {
      __name(this, "EventTarget");
    }
    addEventListener(type, callback, options) {
      if (callback && typeof callback === "object" || typeof callback === "function") {
        const { capture, once, passive, signal } = getOptions(options);
        if (!signal?.aborted) {
          type = String(type);
          let listeners = listenersByTarget.get(this);
          if (!listeners) {
            listeners = [];
            listenersByTarget.set(this, listeners);
          }
          if (!find(listeners, type, callback, capture)) {
            const listener = {
              type,
              callback,
              capture,
              once,
              passive,
              removed: false
            };
            listeners.push(listener);
            signal?.addEventListener("abort", () => remove(this, listener));
          }
        }
      }
    }
    removeEventListener(type, callback, options) {
      const { capture } = getOptions(options);
      const listener = find(
        listenersByTarget.get(this),
        String(type),
        callback,
        capture
      );
      if (listener) remove(this, listener);
    }
    dispatchEvent(event) {
      const state = states.get(event);
      if (!state) {
        throw new TypeError(`Invalid event: '${event}', not an Event.`);
      }
      if (state.dispatching) {
        throw new DOMException(
          "Unable to dispatch an event that is being dispatched.",
          "InvalidStateError"
        );
      }
      Object.assign(state, {
        dispatching: true,
        target: this,
        currentTarget: this,
        eventPhase: phases.AT_TARGET
      });
      for (const listener of [...listenersByTarget.get(this) ?? []]) {
        if (!listener.removed && listener.type === state.type) {
          if (listener.once) remove(this, listener);
          const { callback } = listener;
          state.passive = listener.passive;
          try {
            if (typeof callback === "function") {
              callback.call(this, event);
            } else if (typeof callback.handleEvent === "function") {
              callback.handleEvent(event);
            } else {
              throw new TypeError("Invalid listener: it has no handleEvent().");
            }
          } catch (err) {
            reportError(err);
          }
          state.passive = false;
          if (state.stopImmediately) break;
        }
      }
      Object.assign(state, {
        dispatching: false,
        stop: false,
        stopImmediately: false,
        currentTarget: null,
        eventPhase: phases.NONE
      });
      return !state.defaultPrevented;
    }
  };
  var Event = class {
    static {
      __name(this, "Event");
    }
    constructor(type, init) {
      if (arguments.length === 0) {
        throw new TypeError("Invalid event: its type is missing.");
      }
      const { bubbles, cancelable, composed } = init ?? {};
      states.set(this, {
        type: String(type),
        bubbles: !!bubbles,
        cancelable: !!cancelable,
        composed: !!composed,
        defaultPrevented: false,
        eventPhase: phases.NONE,
        target: null,
        currentTarget: null,
        timeStamp: globalThis.performance?.now() ?? 0,
        isTrusted: false
      });
    }
    get srcElement() {
      return getState(this).target;
    }
    get returnValue() {
      return !getState(this).defaultPrevented;
    }
    set returnValue(value) {
      if (!value) this.preventDefault();
    }
    get cancelBubble() {
      return !!getState(this).stop;
    }
    set cancelBubble(value) {
      if (value) getState(this).stop = true;
    }
    // A passive listener can't cancel an event:
    preventDefault() {
      const state = getState(this);
      if (state.cancelable && !state.passive) state.defaultPrevented = true;
    }
    stopPropagation() {
      getState(this).stop = true;
    }
    stopImmediatePropagation() {
      Object.assign(getState(this), { stop: true, stopImmediately: true });
    }
    // The target while the event is dispatched, as there's no tree around it.
    composedPath() {
      const { currentTarget } = getState(this);
      return currentTarget ? [currentTarget] : [];
    }
    // The legacy way, before the event is dispatched.
    initEvent(type, bubbles = false, cancelable = false) {
      const state = getState(this);
      if (!state.dispatching) {
        Object.assign(state, {
          type: String(type),
          bubbles: !!bubbles,
          cancelable: !!cancelable,
          stop: false,
          stopImmediately: false,
          defaultPrevented: false
        });
      }
    }
  };
  var CustomEvent = class extends Event {
    static {
      __name(this, "CustomEvent");
    }
    constructor(type, init) {
      super(...arguments);
      setMembers(this, init, { detail: null });
    }
    initCustomEvent(type, bubbles, cancelable, detail = null) {
      this.initEvent(type, bubbles, cancelable);
      const state = getState(this);
      if (!state.dispatching) state.detail = detail;
    }
  };
  var MessageEvent = class extends Event {
    static {
      __name(this, "MessageEvent");
    }
    constructor(type, init) {
      super(...arguments);
      setMembers(this, init, {
        data: null,
        origin: "",
        lastEventId: "",
        source: null,
        ports: []
      });
    }
  };
  var CloseEvent = class extends Event {
    static {
      __name(this, "CloseEvent");
    }
    constructor(type, init) {
      super(...arguments);
      setMembers(this, init, { wasClean: false, code: 0, reason: "" });
    }
  };
  var ErrorEvent = class extends Event {
    static {
      __name(this, "ErrorEvent");
    }
    constructor(type, init) {
      super(...arguments);
      setMembers(this, init, {
        message: "",
        filename: "",
        lineno: 0,
        colno: 0,
        error: null
      });
    }
  };
  var fields = {
    [Event.name]: [
      "type",
      "bubbles",
      "cancelable",
      "composed",
      "defaultPrevented",
      "eventPhase",
      "target",
      "currentTarget",
      "timeStamp",
      "isTrusted"
    ],
    [CustomEvent.name]: ["detail"],
    [MessageEvent.name]: ["data", "origin", "lastEventId", "source", "ports"],
    [CloseEvent.name]: ["wasClean", "code", "reason"],
    [ErrorEvent.name]: ["message", "filename", "lineno", "colno", "error"]
  };
  for (const Class of [
    Event,
    CustomEvent,
    MessageEvent,
    CloseEvent,
    ErrorEvent
  ]) {
    for (const name of fields[Class.name]) {
      inject(
        Class.prototype,
        {
          get [name]() {
            return getState(this)[name];
          }
        },
        { enumerable: true }
      );
    }
  }
  defineConstants(Event, phases);
  setToStringTag(
    EventTarget,
    Event,
    CustomEvent,
    MessageEvent,
    CloseEvent,
    ErrorEvent
  );
  function defineEventHandlers(target, ...types2) {
    for (const type of types2) {
      inject(
        target,
        {
          get [`on${type}`]() {
            return handlersByTarget.get(this)?.[type] ?? null;
          },
          set [`on${type}`](handler2) {
            let handlers = handlersByTarget.get(this);
            if (!handlers) {
              handlers = {};
              handlersByTarget.set(this, handlers);
            }
            handlers[type] = typeof handler2 === "function" ? handler2 : null;
          }
        },
        { enumerable: true }
      );
    }
  }
  __name(defineEventHandlers, "defineEventHandlers");
  function fire(target, event) {
    const handler2 = handlersByTarget.get(target)?.[event.type];
    if (handler2) {
      try {
        handler2.call(target, event);
      } catch (err) {
        reportError(err);
      }
    }
    return target.dispatchEvent(event);
  }
  __name(fire, "fire");
  function getState(event) {
    const state = states.get(event);
    if (!state) {
      throw new TypeError(`Invalid event: '${event}', not an Event.`);
    }
    return state;
  }
  __name(getState, "getState");
  function setMembers(event, init, defaults) {
    const state = getState(event);
    for (const [name, value] of Object.entries(defaults)) {
      const given = init?.[name];
      state[name] = given === void 0 ? Array.isArray(value) ? [] : value : typeof value === "string" ? String(given) : typeof value === "number" ? Number(given) : typeof value === "boolean" ? !!given : given;
    }
  }
  __name(setMembers, "setMembers");
  function getOptions(options) {
    if (options && typeof options === "object") {
      const { capture, once, passive, signal } = options;
      return { capture: !!capture, once: !!once, passive: !!passive, signal };
    } else {
      return { capture: !!options, once: false, passive: false, signal: null };
    }
  }
  __name(getOptions, "getOptions");
  function find(listeners, type, callback, capture) {
    return listeners?.find(
      (listener) => listener.type === type && listener.capture === capture && Object.is(listener.callback, callback)
    );
  }
  __name(find, "find");
  function remove(target, listener) {
    listener.removed = true;
    const listeners = listenersByTarget.get(target);
    const index = listeners?.indexOf(listener) ?? -1;
    if (index >= 0) listeners.splice(index, 1);
  }
  __name(remove, "remove");

  // src/abort.js
  var abort;
  var AbortSignal = class _AbortSignal extends EventTarget {
    static {
      __name(this, "AbortSignal");
    }
    #aborted = false;
    #reason;
    constructor(key) {
      checkInternal(
        key,
        "AbortSignal",
        "AbortController, AbortSignal.abort() and AbortSignal.timeout() make them"
      );
      super();
    }
    static {
      abort = /* @__PURE__ */ __name((signal, reason) => {
        if (!signal.#aborted) {
          signal.#aborted = true;
          signal.#reason = reason === void 0 ? new DOMException(
            "signal is aborted without reason",
            "AbortError"
          ) : reason;
          fire(signal, new Event("abort"));
        }
      }, "abort");
    }
    get aborted() {
      return this.#aborted;
    }
    get reason() {
      return this.#reason;
    }
    throwIfAborted() {
      if (this.#aborted) throw this.#reason;
    }
    static abort(reason) {
      const signal = new _AbortSignal(internal);
      abort(signal, reason);
      return signal;
    }
    static timeout(milliseconds) {
      const signal = new _AbortSignal(internal);
      setTimeout(
        () => abort(signal, new DOMException("signal timed out", "TimeoutError")),
        milliseconds
      );
      return signal;
    }
    // A signal that aborts when the first of `signals` does.
    static any(signals) {
      const signal = new _AbortSignal(internal);
      for (const from of signals) {
        if (from.aborted) {
          abort(signal, from.reason);
          break;
        }
        from.addEventListener("abort", () => abort(signal, from.reason), {
          once: true
        });
      }
      return signal;
    }
  };
  var AbortController = class {
    static {
      __name(this, "AbortController");
    }
    #signal = new AbortSignal(internal);
    get signal() {
      return this.#signal;
    }
    abort(reason) {
      abort(this.#signal, reason);
    }
  };
  defineEventHandlers(AbortSignal.prototype, "abort");
  setToStringTag(AbortSignal, AbortController);

  // src/blob.js
  var blob_exports = {};
  __export(blob_exports, {
    Blob: () => Blob,
    File: () => File,
    getBufferBytes: () => getBufferBytes,
    getBytes: () => getBytes,
    readBytes: () => readBytes
  });
  var bytesByBlob = /* @__PURE__ */ new WeakMap();
  var readBytes = /* @__PURE__ */ Symbol("readBytes");
  var Blob = class _Blob {
    static {
      __name(this, "Blob");
    }
    constructor(parts2 = [], { type = "" } = {}) {
      bytesByBlob.set(this, { bytes: getPartBytes(parts2), type: getType(type) });
    }
    get size() {
      return getBytes(this).length;
    }
    get type() {
      return bytesByBlob.get(this).type;
    }
    slice(start = 0, end, type = "") {
      const bytes = getBytes(this);
      return new _Blob([bytes.slice(start, end ?? bytes.length)], { type });
    }
    async text() {
      return new TextDecoder().decode(getBytes(this));
    }
    async arrayBuffer() {
      return getBytes(this).slice().buffer;
    }
    async bytes() {
      return getBytes(this).slice();
    }
  };
  var File = class extends Blob {
    static {
      __name(this, "File");
    }
    #name;
    #lastModified;
    constructor(parts2, name, options = {}) {
      checkArguments(arguments, 2, "File");
      super(parts2, options);
      this.#name = String(name);
      this.#lastModified = Math.trunc(Number(options.lastModified ?? Date.now()));
    }
    get name() {
      return this.#name;
    }
    get lastModified() {
      return this.#lastModified;
    }
    get webkitRelativePath() {
      return "";
    }
  };
  setToStringTag(Blob, File);
  function getBytes(blob) {
    return blob[readBytes]?.() ?? bytesByBlob.get(blob).bytes;
  }
  __name(getBytes, "getBytes");
  function getBufferBytes(value) {
    return value instanceof ArrayBuffer ? new Uint8Array(value) : ArrayBuffer.isView(value) ? new Uint8Array(value.buffer, value.byteOffset, value.byteLength) : null;
  }
  __name(getBufferBytes, "getBufferBytes");
  function getPartBytes(parts2) {
    const encoder = new TextEncoder();
    const list = Array.from(
      parts2,
      (part) => getBufferBytes(part) ?? (part instanceof Blob ? getBytes(part) : encoder.encode(String(part)))
    );
    const bytes = new Uint8Array(
      list.reduce((size, part) => size + part.length, 0)
    );
    let offset = 0;
    for (const part of list) {
      bytes.set(part, offset);
      offset += part.length;
    }
    return bytes;
  }
  __name(getPartBytes, "getPartBytes");
  function getType(type) {
    type = String(type);
    return /^[\x20-\x7e]*$/.test(type) ? type.toLowerCase() : "";
  }
  __name(getType, "getType");

  // src/canvas.js
  var canvas_exports = {};
  __export(canvas_exports, {
    CanvasGradient: () => CanvasGradient,
    CanvasPattern: () => CanvasPattern,
    ImageBitmap: () => ImageBitmap,
    ImageData: () => ImageData,
    OffscreenCanvas: () => OffscreenCanvas,
    OffscreenCanvasRenderingContext2D: () => OffscreenCanvasRenderingContext2D,
    Path2D: () => Path2D,
    createImageBitmap: () => createImageBitmap
  });
  var { canvas } = natives;
  var getCanvasSurface;
  var getBitmapSurface;
  var getPath;
  var getGradientPaint;
  var getPatternPaint;
  var registry = globalThis.FinalizationRegistry && new FinalizationRegistry(([kind, slot]) => canvas.release(kind, slot));
  var contextTypes = ["2d", "bitmaprenderer", "webgl", "webgl2", "webgpu"];
  var OffscreenCanvas = class {
    static {
      __name(this, "OffscreenCanvas");
    }
    #surface;
    #width;
    #height;
    #context = null;
    // Transparent black, with a context's defaults. A canvas larger than its
    // pixels may be keeps its size, and has none, as a browser's that couldn't
    // be made.
    constructor(width, height) {
      checkArguments(arguments, 2, "OffscreenCanvas");
      this.#width = getSize(width);
      this.#height = getSize(height);
      this.#surface = canvas.create(this.#width, this.#height);
      registry?.register(this, ["surface", this.#surface]);
    }
    static {
      getCanvasSurface = /* @__PURE__ */ __name((canvas2) => canvas2.#surface, "getCanvasSurface");
    }
    // The same 2D context each time, `null` for the other kinds.
    getContext(type, options) {
      checkArguments(arguments, 1, "getContext");
      type = String(type);
      if (!contextTypes.includes(type)) {
        throw new TypeError(`Invalid context type: '${type}'.`);
      }
      return type === "2d" ? this.#context ??= new OffscreenCanvasRenderingContext2D(
        internal,
        this,
        this.#surface
      ) : null;
    }
    get width() {
      return this.#width;
    }
    // Cleared to the new size, its state reset, as in browsers:
    set width(width) {
      this.#width = getSize(width);
      canvas.resize.call(this.#surface, this.#width, this.#height);
    }
    get height() {
      return this.#height;
    }
    set height(height) {
      this.#height = getSize(height);
      canvas.resize.call(this.#surface, this.#width, this.#height);
    }
    async convertToBlob() {
      const png = canvas.toPNG.call(this.#surface);
      return new globalThis.Blob([png], { type: "image/png" });
    }
  };
  var getContextSurface;
  var OffscreenCanvasRenderingContext2D = class {
    static {
      __name(this, "OffscreenCanvasRenderingContext2D");
    }
    #canvas;
    #surface;
    constructor(key, offscreen, surface) {
      checkInternal(
        key,
        "OffscreenCanvasRenderingContext2D",
        "getContext() makes them"
      );
      this.#canvas = offscreen;
      this.#surface = surface;
    }
    static {
      getContextSurface = /* @__PURE__ */ __name((context) => context.#surface, "getContextSurface");
    }
    get canvas() {
      return this.#canvas;
    }
    // Transparent black, of a size or of another `ImageData`'s.
    createImageData(widthOrImage, height) {
      checkArguments(arguments, 1, "createImageData");
      if (widthOrImage instanceof ImageData) {
        return new ImageData(widthOrImage.width, widthOrImage.height);
      } else {
        checkArguments(arguments, 2, "createImageData");
        return new ImageData(widthOrImage, height);
      }
    }
    createLinearGradient(x0, y0, x1, y1) {
      checkArguments(arguments, 4, "createLinearGradient");
      return createGradient("linear", x0, y0, x1, y1);
    }
    createRadialGradient(x0, y0, r0, x1, y1, r1) {
      checkArguments(arguments, 6, "createRadialGradient");
      return createGradient("radial", x0, y0, r0, x1, y1, r1);
    }
    createConicGradient(angle, x, y) {
      checkArguments(arguments, 3, "createConicGradient");
      return createGradient("conic", angle, x, y);
    }
    createPattern(image, repetition) {
      checkArguments(arguments, 2, "createPattern");
      return new CanvasPattern(
        internal,
        canvas.createPattern(getSurface(image), repetition)
      );
    }
    drawImage(image, ...args) {
      checkOverloads(arguments, [3, 5, 9], "drawImage");
      canvas.methods.drawImage.call(this.#surface, getSurface(image), ...args);
    }
    // The path first, `null` for the current one:
    fill(...args) {
      return callWithPath(this.#surface, "fill", args, 0);
    }
    stroke(...args) {
      return callWithPath(this.#surface, "stroke", args, 0);
    }
    clip(...args) {
      return callWithPath(this.#surface, "clip", args, 0);
    }
    isPointInPath(...args) {
      return callWithPath(this.#surface, "isPointInPath", args, 2);
    }
    isPointInStroke(...args) {
      return callWithPath(this.#surface, "isPointInStroke", args, 2);
    }
  };
  var contextMethods = {
    getImageData: 4,
    putImageData: [3, 7],
    measureText: 1,
    beginPath: 0,
    closePath: 0,
    moveTo: 2,
    lineTo: 2,
    bezierCurveTo: 6,
    quadraticCurveTo: 4,
    arc: 5,
    arcTo: 5,
    ellipse: 7,
    rect: 4,
    roundRect: 4,
    fillRect: 4,
    strokeRect: 4,
    clearRect: 4,
    fillText: 3,
    strokeText: 3,
    translate: 2,
    rotate: 1,
    scale: 2,
    transform: 6,
    setTransform: [0, 1, 6],
    resetTransform: 0,
    getTransform: 0,
    save: 0,
    restore: 0,
    reset: 0,
    setLineDash: 1,
    getLineDash: 0
  };
  for (const [name, counts] of Object.entries(contextMethods)) {
    inject(OffscreenCanvasRenderingContext2D.prototype, {
      [name](...args) {
        checkOverloads(args, counts, name);
        return canvas.methods[name].call(getContextSurface(this), ...args);
      }
    });
  }
  for (const name of canvas.properties) {
    inject(
      OffscreenCanvasRenderingContext2D.prototype,
      {
        get [name]() {
          return canvas.get.call(getContextSurface(this), name);
        },
        set [name](value) {
          canvas.set.call(getContextSurface(this), name, value, getPaint(value));
        }
      },
      { enumerable: true }
    );
  }
  var ImageData = class {
    static {
      __name(this, "ImageData");
    }
    #data;
    #width;
    #height;
    // `new ImageData(width, height)`, transparent black, or
    // `new ImageData(data, width[, height])` of RGBA `data`, not premultiplied.
    constructor(dataOrWidth, width, height) {
      checkArguments(arguments, 2, "ImageData");
      if (dataOrWidth instanceof Uint8ClampedArray) {
        const data = dataOrWidth;
        width = getSide(width);
        height = height === void 0 ? data.length / 4 / width : getSide(height);
        if (data.length !== width * height * 4) {
          throw new DOMException(
            "Invalid ImageData: the data is not width \xD7 height \xD7 4 bytes.",
            "IndexSizeError"
          );
        }
        this.#data = data;
      } else {
        height = getSide(width);
        width = getSide(dataOrWidth);
        this.#data = new Uint8ClampedArray(width * height * 4);
      }
      this.#width = width;
      this.#height = height;
    }
    get width() {
      return this.#width;
    }
    get height() {
      return this.#height;
    }
    get data() {
      return this.#data;
    }
    get colorSpace() {
      return "srgb";
    }
  };
  var Path2D = class _Path2D {
    static {
      __name(this, "Path2D");
    }
    #path;
    constructor(path) {
      this.#path = canvas.createPath(
        path instanceof _Path2D ? path.#path : path === void 0 ? null : String(path)
      );
      registry?.register(this, ["path", this.#path]);
    }
    static {
      getPath = /* @__PURE__ */ __name((path) => path instanceof _Path2D ? path.#path : void 0, "getPath");
    }
    addPath(path, transform) {
      checkArguments(arguments, 1, "addPath");
      canvas.pathMethods.addPath.call(this.#path, getPath(path), transform);
    }
  };
  for (const name of [
    "closePath",
    "moveTo",
    "lineTo",
    "bezierCurveTo",
    "quadraticCurveTo",
    "arc",
    "arcTo",
    "ellipse",
    "rect",
    "roundRect"
  ]) {
    const counts = contextMethods[name];
    inject(Path2D.prototype, {
      [name](...args) {
        checkOverloads(args, counts, name);
        return canvas.pathMethods[name].call(getPath(this), ...args);
      }
    });
  }
  var ImageBitmap = class {
    static {
      __name(this, "ImageBitmap");
    }
    #surface;
    constructor(key, surface) {
      checkInternal(key, "ImageBitmap", "createImageBitmap() makes them");
      this.#surface = surface;
      registry?.register(this, ["surface", surface], this);
    }
    static {
      getBitmapSurface = /* @__PURE__ */ __name((bitmap) => {
        if (bitmap.#surface === null) {
          throw new DOMException(
            "the image bitmap is closed",
            "InvalidStateError"
          );
        }
        return bitmap.#surface;
      }, "getBitmapSurface");
    }
    // None once it's closed:
    get width() {
      return this.#surface === null ? 0 : canvas.width.call(this.#surface);
    }
    get height() {
      return this.#surface === null ? 0 : canvas.height.call(this.#surface);
    }
    close() {
      if (this.#surface !== null) {
        registry?.unregister(this);
        canvas.release("surface", this.#surface);
        this.#surface = null;
      }
    }
  };
  var CanvasGradient = class {
    static {
      __name(this, "CanvasGradient");
    }
    #paint;
    constructor(key, paint) {
      checkInternal(key, "CanvasGradient", "the context makes them");
      this.#paint = paint;
      registry?.register(this, ["paint", paint]);
    }
    static {
      getGradientPaint = /* @__PURE__ */ __name((gradient) => gradient.#paint, "getGradientPaint");
    }
    addColorStop(offset, color) {
      checkArguments(arguments, 2, "addColorStop");
      canvas.addColorStop.call(this.#paint, offset, color);
    }
  };
  var CanvasPattern = class {
    static {
      __name(this, "CanvasPattern");
    }
    #paint;
    constructor(key, paint) {
      checkInternal(key, "CanvasPattern", "the context makes them");
      this.#paint = paint;
      registry?.register(this, ["paint", paint]);
    }
    static {
      getPatternPaint = /* @__PURE__ */ __name((pattern) => pattern.#paint, "getPatternPaint");
    }
  };
  setToStringTag(
    OffscreenCanvas,
    OffscreenCanvasRenderingContext2D,
    ImageData,
    Path2D,
    ImageBitmap,
    CanvasGradient,
    CanvasPattern
  );
  async function createImageBitmap(image, ...args) {
    const source = image instanceof globalThis.Blob ? new Uint8Array(await image.arrayBuffer()) : image instanceof ImageData ? image : getSurface(image);
    return new ImageBitmap(internal, canvas.createImageBitmap(source, ...args));
  }
  __name(createImageBitmap, "createImageBitmap");
  function getSurface(image) {
    if (image instanceof OffscreenCanvas) {
      return getCanvasSurface(image);
    } else if (image instanceof ImageBitmap) {
      return getBitmapSurface(image);
    } else {
      throw new TypeError(`Invalid image: '${image}', not a canvas or bitmap.`);
    }
  }
  __name(getSurface, "getSurface");
  function getPaint(style) {
    return style instanceof CanvasGradient ? getGradientPaint(style) : style instanceof CanvasPattern ? getPatternPaint(style) : void 0;
  }
  __name(getPaint, "getPaint");
  function createGradient(kind, ...args) {
    return new CanvasGradient(internal, canvas.createGradient(kind, ...args));
  }
  __name(createGradient, "createGradient");
  function callWithPath(surface, name, args, count) {
    const path = args[0] instanceof Path2D ? getPath(args.shift()) : null;
    checkArguments(args, count, name);
    return canvas.methods[name].call(surface, path, ...args);
  }
  __name(callWithPath, "callWithPath");
  function checkOverloads(args, counts, name) {
    if (typeof counts === "number") {
      checkArguments(args, counts, name);
    } else {
      const count = Math.min(args.length, Math.max(...counts));
      if (!counts.includes(count) || count === 1 && !isObject(args[0])) {
        throw new TypeError(`Invalid ${name}(): ${args.length} arguments.`);
      }
    }
  }
  __name(checkOverloads, "checkOverloads");
  function isObject(value) {
    return value !== null && typeof value === "object";
  }
  __name(isObject, "isObject");
  function getSize(value) {
    const size = Math.trunc(Number(value));
    if (!(size >= 0 && size <= Number.MAX_SAFE_INTEGER)) {
      throw new TypeError(`Invalid canvas size: '${value}'.`);
    }
    return size;
  }
  __name(getSize, "getSize");
  function getSide(value) {
    const side = Math.trunc(Number(value));
    if (!(side > 0)) {
      throw new DOMException(
        `Invalid ImageData size: '${value}'.`,
        "IndexSizeError"
      );
    }
    return side;
  }
  __name(getSide, "getSide");

  // src/dom.js
  var view;
  var dom = {
    get window() {
      return view ??= natives.dom();
    },
    get document() {
      return (view ??= natives.dom()).document;
    }
  };

  // src/clipboard.js
  var Clipboard = class extends EventTarget {
    static {
      __name(this, "Clipboard");
    }
    async readText() {
      return ask({ type: "clipboardRead" })?.text ?? "";
    }
    async writeText(text) {
      checkArguments(arguments, 1, "writeText");
      ask({ type: "clipboardWrite", text: String(text) });
    }
  };
  setToStringTag(Clipboard);
  function ask(question) {
    try {
      return natives.ask(question);
    } catch (err) {
      throw new DOMException(
        `Unable to use the clipboard: ${err.message}`,
        "NotAllowedError"
      );
    }
  }
  __name(ask, "ask");

  // src/console.js
  var maxDepth = 2;
  var maxItems = 100;
  function createConsole(print2) {
    let indent = 0;
    const counts = /* @__PURE__ */ new Map();
    const timers = /* @__PURE__ */ new Map();
    const write = /* @__PURE__ */ __name((text) => print2(
      text.split("\n").map((line) => "  ".repeat(indent) + line).join("\n")
    ), "write");
    const log = /* @__PURE__ */ __name((...args) => write(format(args)), "log");
    const now = /* @__PURE__ */ __name(() => globalThis.performance?.now() ?? Date.now(), "now");
    const timeLog = /* @__PURE__ */ __name((end, label = "default", ...data) => {
      label = String(label);
      if (timers.has(label)) {
        let line = `${label}: ${(now() - timers.get(label)).toFixed(3)} ms`;
        if (end) timers.delete(label);
        else if (data.length) line += ` ${format(data)}`;
        write(line);
      } else {
        write(`Timer '${label}' does not exist`);
      }
    }, "timeLog");
    const group = /* @__PURE__ */ __name((...args) => {
      if (args.length) log(...args);
      indent++;
    }, "group");
    return {
      log,
      info: log,
      warn: log,
      error: log,
      debug: log,
      dirxml: log,
      // The value as an object, strings quoted.
      dir(value) {
        write(inspect(value, 1));
      },
      // The line, then where it was called from.
      trace(...args) {
        const line = format(args);
        const stack = (new Error().stack ?? "").split("\n").slice(1).map((line2) => line2.trim()).join("\n");
        write((line ? `Trace: ${line}` : "Trace") + (stack ? `
${stack}` : ""));
      },
      assert(condition, ...data) {
        if (!condition) {
          write(
            data.length ? `Assertion failed: ${format(data)}` : "Assertion failed"
          );
        }
      },
      count(label = "default") {
        label = String(label);
        const count = (counts.get(label) ?? 0) + 1;
        counts.set(label, count);
        write(`${label}: ${count}`);
      },
      countReset(label = "default") {
        label = String(label);
        if (counts.has(label)) {
          counts.set(label, 0);
        } else {
          write(`Count for '${label}' does not exist`);
        }
      },
      time(label = "default") {
        label = String(label);
        if (timers.has(label)) {
          write(`Timer '${label}' already exists`);
        } else {
          timers.set(label, now());
        }
      },
      timeLog(label, ...data) {
        timeLog(false, label, ...data);
      },
      timeEnd(label) {
        timeLog(true, label);
      },
      group,
      groupCollapsed: group,
      groupEnd() {
        indent = Math.max(0, indent - 1);
      },
      // An array's or object's items as rows, their properties as columns.
      table(data, columns) {
        if (data && typeof data === "object") {
          write(formatTable(data, columns));
        } else {
          log(data);
        }
      },
      clear() {
      }
    };
  }
  __name(createConsole, "createConsole");
  function format(args) {
    const parts2 = [];
    let rest = args;
    const [first, ...after] = args;
    if (typeof first === "string" && first.includes("%")) {
      rest = after;
      parts2.push(
        first.replace(/%([sdifoOc%])/g, (match, directive) => {
          if (directive === "%") return "%";
          if (!rest.length) return match;
          const value = rest.shift();
          switch (directive) {
            case "s":
              return inspect(value, 0);
            case "d":
            case "i": {
              const number = Number(value);
              return String(Number.isFinite(number) ? Math.trunc(number) : number);
            }
            case "f":
              return String(Number(value));
            case "o":
            case "O":
              return inspect(value, 1);
            default:
              return "";
          }
        })
      );
    }
    for (const value of rest) parts2.push(inspect(value, 0));
    return parts2.join(" ");
  }
  __name(format, "format");
  function inspect(value, depth) {
    if (typeof value === "string") {
      return depth === 0 ? value : `'${value}'`;
    }
    if (value === null || typeof value !== "object" && typeof value !== "function") {
      return String(value);
    }
    const tag = Object.prototype.toString.call(value);
    if (tag === "[object Array]") {
      return depth > maxDepth ? "[Array]" : `[${getItems(
        Array.from({ length: Math.min(value.length, maxItems) }, (_, i) => [
          i,
          value[i]
        ]),
        value.length,
        (_, item) => inspect(item, depth + 1)
      )}]`;
    } else if (/^\[object (Async|Generator)?Function\]$/.test(tag)) {
      return `\u0192 ${value.name}()`;
    } else if (tag === "[object Error]") {
      return formatError(value);
    } else {
      const written = String(value);
      if (!written.startsWith("[object ")) {
        return written;
      } else if (depth > maxDepth) {
        return "{\u2026}";
      } else {
        const name = value.constructor?.name;
        const prefix = name && name !== "Object" ? `${name} ` : "";
        const keys = Object.keys(value);
        const members = getItems(
          keys.slice(0, maxItems).map((key) => [key, value[key]]),
          keys.length,
          (key, member) => `${key}: ${inspect(member, depth + 1)}`
        );
        return `${prefix}{${members}}`;
      }
    }
  }
  __name(inspect, "inspect");
  function getItems(entries, total, write) {
    const written = entries.map(([key, value]) => write(key, value));
    if (total > entries.length) {
      written.push(`\u2026 ${total - entries.length} more`);
    }
    return written.join(", ");
  }
  __name(getItems, "getItems");
  function formatTable(data, chosen) {
    let columns = [];
    const rows = Object.keys(data).slice(0, maxItems).map((key) => {
      const row = data[key];
      if (row !== null && typeof row === "object") {
        const cells = {};
        for (const column of Object.keys(row).slice(0, maxItems)) {
          cells[column] = inspect(row[column], 1);
          if (!columns.includes(column)) columns.push(column);
        }
        return { key, cells };
      } else {
        return { key, cells: {}, value: inspect(row, 1) };
      }
    });
    if (chosen && typeof chosen === "object") {
      columns = Array.from(chosen, String);
    }
    const hasValues = rows.some((row) => "value" in row);
    const grid = [
      ["(index)", ...columns, ...hasValues ? ["Value"] : []],
      ...rows.map(({ key, cells, value }) => [
        key,
        ...columns.map((column) => cells[column] ?? ""),
        ...hasValues ? [value ?? ""] : []
      ])
    ];
    const widths = grid[0].map(
      (_, i) => Math.max(...grid.map((row) => [...row[i]].length))
    );
    return grid.map(
      (row) => `\u2502 ${row.map((cell, i) => cell + " ".repeat(widths[i] - [...cell].length)).join(" \u2502 ")} \u2502`
    ).join("\n");
  }
  __name(formatTable, "formatTable");

  // src/crypto.js
  var integerArrays = [
    Int8Array,
    Uint8Array,
    Uint8ClampedArray,
    Int16Array,
    Uint16Array,
    Int32Array,
    Uint32Array,
    BigInt64Array,
    BigUint64Array
  ];
  var Crypto = class {
    static {
      __name(this, "Crypto");
    }
    // At most 65536 bytes, as in browsers.
    getRandomValues(array) {
      if (!integerArrays.some((Class) => array instanceof Class)) {
        throw new DOMException(
          `Invalid array: '${array}', not an integer typed array.`,
          "TypeMismatchError"
        );
      }
      if (array.byteLength > 65536) {
        throw new DOMException(
          "Invalid array: more than 65536 bytes.",
          "QuotaExceededError"
        );
      }
      new Uint8Array(array.buffer, array.byteOffset, array.byteLength).set(
        natives.random(array.byteLength)
      );
      return array;
    }
    // A random version 4 UUID.
    randomUUID() {
      const bytes = natives.random(16);
      bytes[6] = bytes[6] & 15 | 64;
      bytes[8] = bytes[8] & 63 | 128;
      const hex = Array.from(
        bytes,
        (byte) => byte.toString(16).padStart(2, "0")
      ).join("");
      return [
        hex.slice(0, 8),
        hex.slice(8, 12),
        hex.slice(12, 16),
        hex.slice(16, 20),
        hex.slice(20)
      ].join("-");
    }
  };
  setToStringTag(Crypto);

  // src/dialog.js
  var defaultTitle = "Scriptographer";
  function setupDialog(Dialog) {
    Dialog.alert ??= function(...args) {
      natives.ask({ type: "alert", ...getTexts(args) });
    };
    Dialog.confirm ??= function(...args) {
      return natives.ask({ type: "confirm", ...getTexts(args) }) === true;
    };
    Object.assign(Dialog, {
      // The values by name, put into `values` when given, or `null` when the
      // user cancels.
      prompt(title = defaultTitle, components, values) {
        if (!components || typeof components !== "object") {
          throw new TypeError("Invalid components: they must be an object.");
        }
        const list = getComponents(components, values);
        return then(
          natives.ask({ type: "prompt", title: String(title), components: list }),
          (answer) => getValues(list, answer, values)
        );
      },
      // A prompt with one colour: the colour picked, or `null`.
      chooseColor(color) {
        const component = { type: "color" };
        if (color != null) component.value = color;
        return then(
          Dialog.prompt(defaultTitle, { color: component }),
          (values) => values && values.color
        );
      },
      // Each gives a `LocalFile`, or `null` when the user cancels.
      async openFile(options) {
        const handles = await pick(
          showOpenFilePicker,
          getPickerOptions("openFile", options)
        );
        return handles && LocalFile.from(handles[0]);
      },
      async openFiles(options) {
        const handles = await pick(showOpenFilePicker, {
          ...getPickerOptions("openFile", options),
          multiple: true
        });
        return handles && Promise.all(handles.map((handle) => LocalFile.from(handle)));
      },
      async saveFile(options) {
        const handle = await pick(
          showSaveFilePicker,
          getPickerOptions("saveFile", options)
        );
        return handle && LocalFile.from(handle);
      },
      async chooseDirectory(options) {
        const handle = await pick(
          showDirectoryPicker,
          getPickerOptions("chooseDirectory", options)
        );
        return handle && LocalFile.from(handle);
      }
    });
  }
  __name(setupDialog, "setupDialog");
  function getComponents(components, values) {
    return Object.keys(components).map((name) => {
      const definition = components[name];
      const component = { name, ...toJSON(definition) };
      delete component.value;
      let value = values?.[name] !== void 0 ? values[name] : definition.value;
      if (typeof value?.toCSS === "function") {
        value = value.toCSS(true);
      }
      component.value = toJSON(value);
      component.type ??= // As Scriptographer read it from the definition:
      "options" in component ? "list" : typeof definition.onClick === "function" ? "button" : typeof definition.onSelect === "function" ? "menu-entry" : typeof component.value === "number" ? "number" : typeof component.value === "boolean" ? "boolean" : "string";
      if (component.type === "number" && typeof component.units === "string" && component.units !== "none") {
        component.steppers ??= true;
      }
      return component;
    });
  }
  __name(getComponents, "getComponents");
  function getValue(component, value) {
    return component.type === "color" && typeof value === "string" ? new paper.Color(value) : value;
  }
  __name(getValue, "getValue");
  function getValues(list, answer, values) {
    if (answer && typeof answer === "object") {
      const result = values ?? {};
      for (const component of list) {
        if (component.name in answer) {
          result[component.name] = getValue(component, answer[component.name]);
        }
      }
      return result;
    } else {
      return null;
    }
  }
  __name(getValues, "getValues");
  function getTexts(args) {
    const [title, message] = args.length > 1 ? args : [defaultTitle, args[0] ?? ""];
    return { title: String(title), message: String(message) };
  }
  __name(getTexts, "getTexts");
  function toJSON(value) {
    return JSON.parse(JSON.stringify(value ?? null) ?? "null");
  }
  __name(toJSON, "toJSON");
  function then(value, f) {
    return value instanceof Promise ? value.then(f) : f(value);
  }
  __name(then, "then");
  async function pick(picker, options) {
    try {
      return await picker(options);
    } catch (err) {
      if (err.name === "AbortError") return null;
      throw err;
    }
  }
  __name(pick, "pick");
  function getPickerOptions(kind, { types: types2, path } = {}) {
    const options = {};
    if (types2 && kind !== "chooseDirectory") {
      const accepted = [];
      let acceptsAll = false;
      for (const [description, extensions] of getTypes(types2)) {
        if (extensions.length) {
          accepted.push({
            description,
            accept: {
              "application/octet-stream": extensions.map((ext) => `.${ext}`)
            }
          });
        } else {
          acceptsAll = true;
        }
      }
      options.types = accepted;
      options.excludeAcceptAllOption = !acceptsAll;
    }
    if (path != null) {
      const file = path instanceof LocalFile ? path : getLocalFile(path);
      if (file?.path) {
        options.startIn = file.isDirectory ? file : file.parent;
        if (kind === "saveFile" && !file.isDirectory) {
          options.suggestedName = file.name;
        }
      } else if (kind === "saveFile") {
        options.suggestedName = String(path).split(/[\\/]/).pop();
      }
    }
    return options;
  }
  __name(getPickerOptions, "getPickerOptions");
  function getTypes(types2) {
    const entries = Array.isArray(types2) ? Array.from({ length: Math.ceil(types2.length / 2) }, (_, i) => [
      types2[i * 2],
      types2[i * 2 + 1] ?? types2[i * 2]
    ]) : Object.entries(types2);
    return entries.map(([label, patterns]) => [
      String(label),
      (Array.isArray(patterns) ? patterns : String(patterns).split(";")).map(
        (pattern) => String(pattern).trim().replace(/^\*?\.?/, "")
      ).filter((ext) => ext && ext !== "*")
    ]);
  }
  __name(getTypes, "getTypes");
  function getLocalFile(path) {
    try {
      return new LocalFile(String(path));
    } catch {
      return null;
    }
  }
  __name(getLocalFile, "getLocalFile");

  // src/dom-exception.js
  var constants = [
    ["IndexSizeError", "INDEX_SIZE_ERR"],
    ["DOMStringSizeError", "DOMSTRING_SIZE_ERR"],
    ["HierarchyRequestError", "HIERARCHY_REQUEST_ERR"],
    ["WrongDocumentError", "WRONG_DOCUMENT_ERR"],
    ["InvalidCharacterError", "INVALID_CHARACTER_ERR"],
    ["NoDataAllowedError", "NO_DATA_ALLOWED_ERR"],
    ["NoModificationAllowedError", "NO_MODIFICATION_ALLOWED_ERR"],
    ["NotFoundError", "NOT_FOUND_ERR"],
    ["NotSupportedError", "NOT_SUPPORTED_ERR"],
    ["InUseAttributeError", "INUSE_ATTRIBUTE_ERR"],
    ["InvalidStateError", "INVALID_STATE_ERR"],
    ["SyntaxError", "SYNTAX_ERR"],
    ["InvalidModificationError", "INVALID_MODIFICATION_ERR"],
    ["NamespaceError", "NAMESPACE_ERR"],
    ["InvalidAccessError", "INVALID_ACCESS_ERR"],
    ["ValidationError", "VALIDATION_ERR"],
    ["TypeMismatchError", "TYPE_MISMATCH_ERR"],
    ["SecurityError", "SECURITY_ERR"],
    ["NetworkError", "NETWORK_ERR"],
    ["AbortError", "ABORT_ERR"],
    ["URLMismatchError", "URL_MISMATCH_ERR"],
    ["QuotaExceededError", "QUOTA_EXCEEDED_ERR"],
    ["TimeoutError", "TIMEOUT_ERR"],
    ["InvalidNodeTypeError", "INVALID_NODE_TYPE_ERR"],
    ["DataCloneError", "DATA_CLONE_ERR"]
  ];
  var codes = new Map(constants.map(([name], index) => [name, index + 1]));
  var DOMException2 = class extends Error {
    static {
      __name(this, "DOMException");
    }
    #name;
    constructor(message = "", name = "Error") {
      super(message);
      this.#name = String(name);
    }
    get name() {
      return this.#name;
    }
    get code() {
      return codes.get(this.#name) ?? 0;
    }
  };
  defineConstants(
    DOMException2,
    Object.fromEntries(
      constants.map(([, constant], index) => [constant, index + 1])
    )
  );
  setToStringTag(DOMException2);

  // src/encoding.js
  var TextEncoder2 = class {
    static {
      __name(this, "TextEncoder");
    }
    encode(text = "") {
      return natives.encoding.encode(String(text));
    }
    // As much of `text` as fits in `bytes`, whole characters only.
    encodeInto(text, bytes) {
      text = String(text);
      let read = 0;
      let written = 0;
      for (const char of text) {
        const point = char.codePointAt(0);
        const length = point < 128 ? 1 : point < 2048 ? 2 : point < 65536 ? 3 : 4;
        if (written + length > bytes.length) break;
        read += char.length;
        written += length;
      }
      bytes.set(natives.encoding.encode(text.slice(0, read)));
      return { read, written };
    }
  };
  var labels = ["utf-8", "utf8", "unicode-1-1-utf-8", "unicode11utf8"];
  var TextDecoder2 = class {
    static {
      __name(this, "TextDecoder");
    }
    #fatal;
    #ignoreBOM;
    // The bytes of a character a streamed chunk ended in the middle of, and
    // whether the stream is yet to start:
    #pending = new Uint8Array();
    #isStart = true;
    constructor(label = "utf-8", { fatal = false, ignoreBOM = false } = {}) {
      label = String(label).trim().toLowerCase();
      if (!labels.includes(label)) {
        throw new RangeError(`Unsupported encoding: '${label}', only UTF-8.`);
      }
      this.#fatal = !!fatal;
      this.#ignoreBOM = !!ignoreBOM;
    }
    get fatal() {
      return this.#fatal;
    }
    get ignoreBOM() {
      return this.#ignoreBOM;
    }
    // A buffer's or a view's bytes as text: invalid sequences replaced, or a
    // `TypeError` when the decoder is `fatal`.
    decode(input = new Uint8Array(), { stream = false } = {}) {
      const given = getBufferBytes(input) ?? new Uint8Array(input);
      let bytes = given;
      if (this.#pending.length) {
        bytes = new Uint8Array(this.#pending.length + given.length);
        bytes.set(this.#pending);
        bytes.set(given, this.#pending.length);
      }
      const end = stream ? bytes.length - getIncomplete(bytes) : bytes.length;
      this.#pending = bytes.slice(end);
      let text = natives.encoding.decode(bytes.subarray(0, end), this.#fatal);
      if (text === null) {
        throw new TypeError("Invalid UTF-8: the data is not valid UTF-8.");
      }
      if (this.#isStart && !this.#ignoreBOM && text.startsWith("\uFEFF")) {
        text = text.slice(1);
      }
      this.#isStart = !stream || this.#isStart && !text;
      return text;
    }
  };
  for (const target of [TextEncoder2.prototype, TextDecoder2.prototype]) {
    inject(
      target,
      {
        get encoding() {
          return "utf-8";
        }
      },
      { enumerable: true }
    );
  }
  setToStringTag(TextEncoder2, TextDecoder2);
  function getIncomplete(bytes) {
    for (let back = 1; back <= Math.min(3, bytes.length); back++) {
      const byte = bytes[bytes.length - back];
      if (byte >= 192) {
        const length = byte >= 240 ? 4 : byte >= 224 ? 3 : 2;
        return length > back ? back : 0;
      } else if (byte < 128) {
        return 0;
      }
    }
    return 0;
  }
  __name(getIncomplete, "getIncomplete");

  // src/fetch.js
  var fetch_exports = {};
  __export(fetch_exports, {
    FormData: () => FormData,
    Headers: () => Headers,
    Request: () => Request,
    Response: () => Response,
    fetch: () => fetch,
    getBody: () => getBody
  });

  // src/pairs.js
  var pairIterable = {
    forEach(callback, thisArg) {
      for (const [key, value] of this.entries()) {
        callback.call(thisArg, value, key, this);
      }
    },
    *keys() {
      for (const [key] of this.entries()) yield key;
    },
    *values() {
      for (const [, value] of this.entries()) yield value;
    },
    [Symbol.iterator]() {
      return this.entries();
    }
  };
  function getPairs(init) {
    return typeof init[Symbol.iterator] === "function" ? Array.from(init, (pair) => {
      pair = [...pair];
      if (pair.length !== 2) {
        throw new TypeError(`Invalid pair: '${pair}'.`);
      }
      return pair;
    }) : Object.entries(init);
  }
  __name(getPairs, "getPairs");
  function setFirst(list, entry) {
    const index = list.findIndex(([name]) => name === entry[0]);
    const rest = list.filter(([name]) => name !== entry[0]);
    rest.splice(index < 0 ? rest.length : index, 0, entry);
    return rest;
  }
  __name(setFirst, "setFirst");

  // src/fetch.js
  var Headers = class _Headers {
    static {
      __name(this, "Headers");
    }
    // The headers as they came, names in lower case:
    #list = [];
    constructor(init) {
      if (init instanceof _Headers) {
        this.#list = [...init.#list];
      } else if (init != null) {
        for (const [name, value] of getPairs(init)) this.append(name, value);
      }
    }
    get(name) {
      const values = this.#getValues(name);
      return values.length ? values.join(", ") : null;
    }
    getSetCookie() {
      return this.#getValues("set-cookie");
    }
    has(name) {
      return this.#getValues(name).length > 0;
    }
    set(name, value) {
      this.#list = setFirst(this.#list, getHeader(name, value));
    }
    append(name, value) {
      this.#list.push(getHeader(name, value));
    }
    delete(name) {
      name = getName(name);
      this.#list = this.#list.filter((header) => header[0] !== name);
    }
    // As scripts see them: sorted by name, the values of a name joined.
    *entries() {
      const names2 = [...new Set(this.#list.map(([name]) => name))].sort();
      for (const name of names2) yield [name, this.get(name)];
    }
    #getValues(name) {
      name = getName(name);
      return this.#list.filter((header) => header[0] === name).map(([, value]) => value);
    }
  };
  var FormData = class {
    static {
      __name(this, "FormData");
    }
    // `[name, value]`, a value a string or a `File`:
    #entries = [];
    append(name, value, filename) {
      checkArguments(arguments, 2, "append");
      this.#entries.push(getEntry(...arguments));
    }
    set(name, value, filename) {
      checkArguments(arguments, 2, "set");
      this.#entries = setFirst(this.#entries, getEntry(...arguments));
    }
    get(name) {
      return this.#entries.find((entry) => entry[0] === String(name))?.[1] ?? null;
    }
    getAll(name) {
      return this.#entries.filter((entry) => entry[0] === String(name)).map(([, value]) => value);
    }
    has(name) {
      return this.#entries.some((entry) => entry[0] === String(name));
    }
    delete(name) {
      this.#entries = this.#entries.filter((entry) => entry[0] !== String(name));
    }
    *entries() {
      for (const [name, value] of this.#entries) yield [name, value];
    }
  };
  for (const Class of [Headers, FormData]) {
    inject(Class.prototype, pairIterable);
  }
  var Body = class {
    static {
      __name(this, "Body");
    }
    #bytes;
    #isUsed = false;
    // The body's bytes, its type in `headers` unless they have one.
    constructor(body, headers) {
      if (body != null) {
        const { bytes, type } = getBody(body);
        if (type && !headers.has("content-type")) {
          headers.set("content-type", type);
        }
        this.#bytes = bytes;
      }
    }
    get bodyUsed() {
      return this.#isUsed;
    }
    async text() {
      return new TextDecoder().decode(this.#take());
    }
    async json() {
      return JSON.parse(await this.text());
    }
    async arrayBuffer() {
      return this.#take().slice().buffer;
    }
    async bytes() {
      return this.#take().slice();
    }
    async blob() {
      const type = this.headers.get("content-type") ?? "";
      return new Blob([this.#take()], { type });
    }
    #take() {
      if (this.#isUsed) {
        throw new TypeError("Unable to read the body: it was read already.");
      }
      this.#isUsed = true;
      return this.#bytes ?? new Uint8Array();
    }
    // The body, unread, for a copy.
    static getBytes(body) {
      if (body.#isUsed) {
        throw new TypeError("Unable to copy the body: it was read already.");
      }
      return body.#bytes ?? null;
    }
  };
  var Request = class _Request extends Body {
    static {
      __name(this, "Request");
    }
    #url;
    #method;
    #headers;
    #signal;
    #redirect;
    constructor(input, init = {}) {
      const from = input instanceof _Request ? input : null;
      const method = String(init.method ?? from?.method ?? "GET").toUpperCase();
      const headers = new Headers(init.headers ?? from?.headers);
      const body = init.body ?? (from && Body.getBytes(from));
      if (body != null && (method === "GET" || method === "HEAD")) {
        throw new TypeError(`Invalid Request: a ${method} request has no body.`);
      }
      super(body, headers);
      this.#url = from ? from.url : String(input);
      this.#method = method;
      this.#headers = headers;
      this.#signal = init.signal ?? from?.signal ?? null;
      this.#redirect = String(init.redirect ?? from?.redirect ?? "follow");
    }
    get url() {
      return this.#url;
    }
    get method() {
      return this.#method;
    }
    get headers() {
      return this.#headers;
    }
    get signal() {
      return this.#signal;
    }
    get redirect() {
      return this.#redirect;
    }
    clone() {
      return new _Request(this);
    }
  };
  var setResponse;
  var Response = class _Response extends Body {
    static {
      __name(this, "Response");
    }
    #status;
    #statusText;
    #headers;
    #url = "";
    #redirected = false;
    #type = "default";
    constructor(body = null, { status = 200, statusText = "", headers } = {}) {
      status = Number(status);
      if (!(status >= 200 && status <= 599)) {
        throw new RangeError(`Invalid Response status: '${status}'.`);
      }
      headers = new Headers(headers);
      super(body, headers);
      this.#status = status;
      this.#statusText = String(statusText);
      this.#headers = headers;
    }
    static {
      setResponse = /* @__PURE__ */ __name((response, { url, redirected, type }) => {
        response.#url = url;
        response.#redirected = redirected;
        response.#type = type;
        return response;
      }, "setResponse");
    }
    get status() {
      return this.#status;
    }
    get statusText() {
      return this.#statusText;
    }
    get ok() {
      return this.#status >= 200 && this.#status <= 299;
    }
    get headers() {
      return this.#headers;
    }
    get url() {
      return this.#url;
    }
    get redirected() {
      return this.#redirected;
    }
    get type() {
      return this.#type;
    }
    clone() {
      const { status, statusText, headers } = this;
      return setResponse(
        new _Response(Body.getBytes(this), { status, statusText, headers }),
        this
      );
    }
  };
  setToStringTag(Headers, FormData, Request, Response);
  async function fetch(input, init) {
    const request = new Request(input, init);
    const { method, url, headers, signal, redirect } = request;
    signal?.throwIfAborted();
    const body = Body.getBytes(request);
    return new Promise((resolve, reject) => {
      const abort2 = /* @__PURE__ */ __name(() => reject(signal.reason), "abort");
      signal?.addEventListener("abort", abort2);
      natives.request(method, url, [...headers], body, redirect === "follow").then(({ status, statusText, url: url2, redirected, headers: headers2, body: body2 }) => {
        const response = new Response(body2, { status, statusText, headers: headers2 });
        resolve(setResponse(response, { url: url2, redirected, type: "basic" }));
      }, reject).finally(() => signal?.removeEventListener("abort", abort2));
    });
  }
  __name(fetch, "fetch");
  function getBody(body) {
    const bytes = getBufferBytes(body);
    return bytes ? { bytes, type: null } : body instanceof Blob ? { bytes: getBytes(body), type: body.type || null } : body instanceof FormData ? getMultipart(body) : {
      bytes: new TextEncoder().encode(String(body)),
      type: body instanceof URLSearchParams ? "application/x-www-form-urlencoded;charset=UTF-8" : "text/plain;charset=UTF-8"
    };
  }
  __name(getBody, "getBody");
  function getHeader(name, value) {
    return [getName(name), String(value).replace(/^[\t\n\r ]+|[\t\n\r ]+$/g, "")];
  }
  __name(getHeader, "getHeader");
  function getName(name) {
    return String(name).trim().toLowerCase();
  }
  __name(getName, "getName");
  function getEntry(name, value, filename) {
    return [
      String(name),
      value instanceof Blob ? value instanceof File && filename === void 0 ? value : new File([value], filename ?? value.name ?? "blob", {
        type: value.type
      }) : String(value)
    ];
  }
  __name(getEntry, "getEntry");
  function getMultipart(form) {
    const boundary = `----Scriptographer${Array.from(
      { length: 24 },
      () => Math.floor(Math.random() * 16).toString(16)
    ).join("")}`;
    const quote = /* @__PURE__ */ __name((text) => text.replace(/\r/g, "%0D").replace(/\n/g, "%0A").replace(/"/g, "%22"), "quote");
    const parts2 = [];
    for (const [name, value] of form) {
      const disposition = `--${boundary}\r
Content-Disposition: form-data; name="${quote(name)}"`;
      if (value instanceof File) {
        const type = value.type || "application/octet-stream";
        parts2.push(
          `${disposition}; filename="${quote(value.name)}"\r
Content-Type: ${type}\r
\r
`,
          value,
          "\r\n"
        );
      } else {
        parts2.push(`${disposition}\r
\r
${value}\r
`);
      }
    }
    parts2.push(`--${boundary}--\r
`);
    return {
      bytes: getBytes(new Blob(parts2)),
      type: `multipart/form-data; boundary=${boundary}`
    };
  }
  __name(getMultipart, "getMultipart");

  // src/file-system.js
  var file_system_exports = {};
  __export(file_system_exports, {
    FileSystemDirectoryHandle: () => FileSystemDirectoryHandle,
    FileSystemFileHandle: () => FileSystemFileHandle,
    FileSystemHandle: () => FileSystemHandle,
    FileSystemWritableFileStream: () => FileSystemWritableFileStream,
    getHandlePath: () => getHandlePath,
    showDirectoryPicker: () => showDirectoryPicker2,
    showOpenFilePicker: () => showOpenFilePicker2,
    showSaveFilePicker: () => showSaveFilePicker2
  });

  // src/local-file.js
  var types = {
    txt: "text/plain",
    md: "text/markdown",
    csv: "text/csv",
    html: "text/html",
    css: "text/css",
    js: "text/javascript",
    json: "application/json",
    xml: "application/xml",
    pdf: "application/pdf",
    svg: "image/svg+xml",
    png: "image/png",
    jpg: "image/jpeg",
    jpeg: "image/jpeg",
    gif: "image/gif",
    webp: "image/webp",
    bmp: "image/bmp",
    tif: "image/tiff",
    tiff: "image/tiff"
  };
  var LocalFile2 = class _LocalFile extends File {
    static {
      __name(this, "LocalFile");
    }
    #path;
    constructor(...parts2) {
      if (!parts2.length) {
        throw new TypeError("Invalid LocalFile: its path is missing.");
      }
      const path = natives.fs.path(...parts2.map(String));
      super([], natives.fs.info(path).name);
      this.#path = path;
    }
    // The `LocalFile` of a file or directory handle.
    static async from(handle) {
      const path = getHandlePath(handle);
      if (!path) {
        throw new TypeError(
          `Invalid handle: '${handle}', not a FileSystemHandle.`
        );
      }
      return new _LocalFile(path);
    }
    get path() {
      return this.#path;
    }
    get name() {
      return this.#info.name;
    }
    get extension() {
      return this.#info.extension;
    }
    get type() {
      return getType2(this.#path);
    }
    get parent() {
      const { parent } = this.#info;
      return parent ? new _LocalFile(parent) : null;
    }
    get isDirectory() {
      return this.#info.isDirectory;
    }
    // On desktop, every file can be written where it is.
    get writable() {
      return true;
    }
    get size() {
      return this.#info.size;
    }
    get lastModified() {
      return this.#info.lastModified;
    }
    // What it holds now, as its readers read it:
    [readBytes]() {
      return natives.fs.read(this.#path);
    }
    // Text, a `Blob`, a buffer or a view.
    async write(data = "") {
      natives.fs.write(this.#path, getBody(data).bytes, false);
    }
    async append(data = "") {
      natives.fs.write(this.#path, getBody(data).bytes, true);
    }
    async exists() {
      return this.#info.exists;
    }
    // The files and directories in the directory, by name.
    async list() {
      return natives.fs.list(this.#path).map((path) => new _LocalFile(path));
    }
    // The file or directory `name` in this directory.
    async file(name) {
      return new _LocalFile(this.#path, String(name));
    }
    async makeDirectory() {
      natives.fs.makeDirectory(this.#path, true);
      return this;
    }
    // A name renames it where it is, a path moves it; the `LocalFile` follows.
    async rename(nameOrPath) {
      this.#path = natives.fs.rename(this.#path, String(nameOrPath));
      return this;
    }
    async copyTo(fileOrPath) {
      const copy = new _LocalFile(fileOrPath);
      natives.fs.copy(this.#path, copy.path);
      return copy;
    }
    // A directory with what's in it only when `recursive`.
    async remove({ recursive = false } = {}) {
      natives.fs.remove(this.#path, recursive);
    }
    toString() {
      return this.#path;
    }
    get #info() {
      return natives.fs.info(this.#path);
    }
  };
  setToStringTag(LocalFile2);
  function getType2(path) {
    return types[natives.fs.info(path).extension.toLowerCase()] ?? "";
  }
  __name(getType2, "getType");

  // src/file-system.js
  var pathsByHandle = /* @__PURE__ */ new WeakMap();
  var most = 512 * 1024 * 1024;
  var FileSystemHandle = class {
    static {
      __name(this, "FileSystemHandle");
    }
    constructor(key, path) {
      checkInternal(key, "FileSystemHandle", "the pickers give them");
      pathsByHandle.set(this, path);
    }
    get kind() {
      return this instanceof FileSystemDirectoryHandle ? "directory" : "file";
    }
    get name() {
      return natives.fs.info(getPath2(this)).name;
    }
    async isSameEntry(other) {
      return pathsByHandle.get(other) === getPath2(this);
    }
    // Granted: scripts on desktop may change what the user's files.
    async queryPermission() {
      return "granted";
    }
    async requestPermission() {
      return "granted";
    }
  };
  var FileSystemFileHandle = class extends FileSystemHandle {
    static {
      __name(this, "FileSystemFileHandle");
    }
    async getFile() {
      const path = getPath2(this);
      const { name, lastModified } = natives.fs.info(path);
      return new File([natives.fs.read(path)], name, {
        type: getType2(path),
        lastModified
      });
    }
    // A stream that writes the file on `close()`, starting empty, or with what
    // the file holds.
    async createWritable({ keepExistingData = false } = {}) {
      const path = getPath2(this);
      return new FileSystemWritableFileStream(
        internal,
        path,
        keepExistingData ? natives.fs.read(path) : new Uint8Array()
      );
    }
  };
  var FileSystemDirectoryHandle = class extends FileSystemHandle {
    static {
      __name(this, "FileSystemDirectoryHandle");
    }
    async getFileHandle(name, { create = false } = {}) {
      return getEntry2(this, name, false, create);
    }
    async getDirectoryHandle(name, { create = false } = {}) {
      return getEntry2(this, name, true, create);
    }
    async removeEntry(name, { recursive = false } = {}) {
      natives.fs.remove(getEntryPath(this, name), recursive);
    }
    // The names from the directory down to `handle`, or `null` outside it.
    async resolve(handle) {
      const path = pathsByHandle.get(handle);
      return path ? natives.fs.relative(getPath2(this), path) : null;
    }
    // What's in the directory, by name:
    async *entries() {
      for (const path of natives.fs.list(getPath2(this))) {
        yield [natives.fs.info(path).name, createHandle(path)];
      }
    }
    async *keys() {
      for await (const [name] of this.entries()) yield name;
    }
    async *values() {
      for await (const [, handle] of this.entries()) yield handle;
    }
    [Symbol.asyncIterator]() {
      return this.entries();
    }
  };
  var FileSystemWritableFileStream = class {
    static {
      __name(this, "FileSystemWritableFileStream");
    }
    #path;
    #bytes;
    #position = 0;
    #isClosed = false;
    constructor(key, path, bytes) {
      checkInternal(
        key,
        "FileSystemWritableFileStream",
        "createWritable() gives them"
      );
      this.#path = path;
      this.#bytes = bytes;
    }
    // Text, a `Blob`, a buffer or a view at the position, or
    // `{ type: 'write' | 'seek' | 'truncate', data, position, size }`.
    async write(data) {
      const isCommand = data && typeof data === "object" && typeof data.type === "string" && !(data instanceof Blob) && !getBufferBytes(data);
      const { type, position, size } = isCommand ? data : { type: "write" };
      if (type === "seek") {
        return this.seek(position);
      } else if (type === "truncate") {
        return this.truncate(size);
      } else if (type === "write") {
        if (isCommand) data = data.data;
        if (data == null) {
          throw new DOMException(
            "Invalid write(): the data is missing.",
            "SyntaxError"
          );
        }
        this.#check();
        const { bytes } = getBody(data);
        const start = getCount(position ?? this.#position);
        const end = getCount(start + bytes.length);
        if (this.#bytes.length < end) this.#resize(end);
        this.#bytes.set(bytes, start);
        this.#position = end;
      } else {
        throw new TypeError(`Invalid write() command: '${type}'.`);
      }
    }
    // Where the stream writes next.
    async seek(position) {
      if (position == null) {
        throw new DOMException(
          "Invalid seek(): the position is missing.",
          "SyntaxError"
        );
      }
      this.#check();
      this.#position = getCount(position);
    }
    // Its bytes cut, or filled with zeros, to `size`.
    async truncate(size) {
      this.#check();
      this.#resize(getCount(size));
      this.#position = Math.min(this.#position, this.#bytes.length);
    }
    // Write the file, all at once.
    async close() {
      this.#check();
      this.#isClosed = true;
      natives.fs.write(this.#path, this.#bytes, false);
    }
    async abort() {
      this.#check();
      this.#isClosed = true;
    }
    #check() {
      if (this.#isClosed) {
        throw new TypeError("Invalid stream: it is closed.");
      }
    }
    #resize(size) {
      const bytes = new Uint8Array(size);
      bytes.set(this.#bytes.subarray(0, size));
      this.#bytes = bytes;
    }
  };
  setToStringTag(
    FileSystemHandle,
    FileSystemFileHandle,
    FileSystemDirectoryHandle,
    FileSystemWritableFileStream
  );
  async function showOpenFilePicker2(options = {}) {
    const paths = pick2(options.multiple ? "openFiles" : "openFile", options);
    return paths.map((path) => createHandle(path, false));
  }
  __name(showOpenFilePicker2, "showOpenFilePicker");
  async function showSaveFilePicker2(options = {}) {
    const start = getStart(options);
    const { suggestedName } = options;
    const path = suggestedName == null ? start : start ? natives.fs.path(start, String(suggestedName)) : String(suggestedName);
    const [picked] = pick2("saveFile", options, path);
    if (!natives.fs.info(picked).exists) {
      natives.fs.write(picked, new Uint8Array(), false);
    }
    return createHandle(picked, false);
  }
  __name(showSaveFilePicker2, "showSaveFilePicker");
  async function showDirectoryPicker2(options = {}) {
    const [picked] = pick2("chooseDirectory", { startIn: options.startIn });
    return createHandle(picked, true);
  }
  __name(showDirectoryPicker2, "showDirectoryPicker");
  function getHandlePath(handle) {
    return pathsByHandle.get(handle) ?? null;
  }
  __name(getHandlePath, "getHandlePath");
  function getPath2(handle) {
    const path = pathsByHandle.get(handle);
    if (!path) {
      throw new TypeError(`Invalid handle: '${handle}', not a FileSystemHandle.`);
    }
    return path;
  }
  __name(getPath2, "getPath");
  function createHandle(path, isDirectory = natives.fs.info(path).isDirectory) {
    const Handle = isDirectory ? FileSystemDirectoryHandle : FileSystemFileHandle;
    return new Handle(internal, path);
  }
  __name(createHandle, "createHandle");
  function getEntryPath(directory, name) {
    name = String(name);
    if (!name || name === "." || name === ".." || /[/\\]/.test(name)) {
      throw new TypeError(`Invalid name: '${name}'.`);
    }
    return natives.fs.path(getPath2(directory), name);
  }
  __name(getEntryPath, "getEntryPath");
  function getEntry2(directory, name, isDirectory, create) {
    const path = getEntryPath(directory, name);
    const info = natives.fs.info(path);
    if (info.exists) {
      if (info.isDirectory !== isDirectory) {
        throw new DOMException(
          `Invalid entry: '${path}' is a ${info.isDirectory ? "directory" : "file"}.`,
          "TypeMismatchError"
        );
      }
    } else if (create) {
      if (isDirectory) {
        natives.fs.makeDirectory(path, false);
      } else {
        natives.fs.write(path, new Uint8Array(), false);
      }
    } else {
      throw new DOMException(`Unable to find '${path}'.`, "NotFoundError");
    }
    return createHandle(path, isDirectory);
  }
  __name(getEntry2, "getEntry");
  function getCount(value) {
    const count = Number(value);
    if (!(count >= 0 && Number.isFinite(count))) {
      throw new TypeError(`Invalid size: '${value}'.`);
    }
    if (count > most) {
      throw new DOMException(
        `Unable to write ${count} bytes: more than a file may have here.`,
        "QuotaExceededError"
      );
    }
    return Math.trunc(count);
  }
  __name(getCount, "getCount");
  function pick2(type, options, path = getStart(options)) {
    const answer = natives.ask({ type, path, filters: getFilters(options) });
    const paths = (Array.isArray(answer) ? answer : [answer]).filter(
      (path2) => typeof path2 === "string"
    );
    if (!paths.length) {
      throw new DOMException("The user aborted a request.", "AbortError");
    }
    return paths;
  }
  __name(pick2, "pick");
  function getStart({ startIn }) {
    if (startIn == null) {
      return null;
    } else if (typeof startIn === "object") {
      const path = getHandlePath(startIn) ?? startIn.path;
      if (typeof path !== "string") {
        throw new TypeError(
          `Invalid startIn: '${startIn}', not a handle or a LocalFile.`
        );
      }
      const info = natives.fs.info(path);
      return info.isDirectory ? path : info.parent;
    } else {
      return natives.fs.userDirectory(String(startIn));
    }
  }
  __name(getStart, "getStart");
  function getFilters({ types: types2, excludeAcceptAllOption }) {
    const filters = Array.from(types2 ?? [], ({ description, accept }) => {
      if (!accept) {
        throw new TypeError("Invalid file type: its accept is missing.");
      }
      const extensions = Object.values(accept).flat().map((extension) => {
        extension = String(extension);
        if (!/^\.[^.]/.test(extension)) {
          throw new TypeError(
            `Invalid extension: '${extension}', which starts with '.'.`
          );
        }
        return extension.slice(1);
      });
      return [
        description || extensions.map((extension) => `.${extension}`).join(", "),
        extensions
      ];
    });
    if (filters.length && !excludeAcceptAllOption) {
      filters.push(["All Files", []]);
    }
    return filters;
  }
  __name(getFilters, "getFilters");

  // src/geometry.js
  var geometry_exports = {};
  __export(geometry_exports, {
    DOMMatrix: () => DOMMatrix,
    DOMMatrixReadOnly: () => DOMMatrixReadOnly,
    DOMPoint: () => DOMPoint,
    DOMPointReadOnly: () => DOMPointReadOnly,
    DOMQuad: () => DOMQuad,
    DOMRect: () => DOMRect,
    DOMRectReadOnly: () => DOMRectReadOnly
  });

  // src/css-transform.js
  var functions = {
    matrix: Array(6).fill("number"),
    matrix3d: Array(16).fill("number"),
    translate: ["length", "length?"],
    translatex: ["length"],
    translatey: ["length"],
    translatez: ["length"],
    translate3d: ["length", "length", "length"],
    scale: ["scale", "scale?"],
    scalex: ["scale"],
    scaley: ["scale"],
    scalez: ["scale"],
    scale3d: ["scale", "scale", "scale"],
    rotate: ["angle"],
    rotatex: ["angle"],
    rotatey: ["angle"],
    rotatez: ["angle"],
    rotate3d: ["number", "number", "number", "angle"],
    skew: ["angle", "angle?"],
    skewx: ["angle"],
    skewy: ["angle"],
    perspective: ["length"]
  };
  var units = {
    "": ["number", 1],
    "%": ["percentage", 1],
    "px": ["length", 1],
    "cm": ["length", 96 / 2.54],
    "mm": ["length", 96 / 25.4],
    "q": ["length", 96 / 101.6],
    "in": ["length", 96],
    "pc": ["length", 16],
    "pt": ["length", 4 / 3],
    "deg": ["angle", 1],
    "rad": ["angle", 180 / Math.PI],
    "grad": ["angle", 0.9],
    "turn": ["angle", 360]
  };
  function parseTransformList(text) {
    let rest = String(text).replace(/\/\*[^]*?\*\//g, "").trim();
    if (/^none$/i.test(rest)) return [];
    const list = [];
    do {
      const match = /^([a-z0-9]+)\(/i.exec(rest);
      const name = match?.[1].toLowerCase();
      const types2 = functions[name];
      if (!types2) throw syntaxError(text);
      const end = getClosing(rest, match[0].length, text);
      const args = split(rest.slice(match[0].length, end)).map(
        (arg) => parseValue(arg, text)
      );
      list.push({ name, args: getArguments(args, types2, text) });
      rest = rest.slice(end + 1).trimStart();
    } while (rest);
    return list;
  }
  __name(parseTransformList, "parseTransformList");
  function getArguments(args, types2, text) {
    const needed = types2.filter((type) => !type.endsWith("?")).length;
    if (args.length < needed || args.length > types2.length) {
      throw syntaxError(text);
    }
    return args.map(({ type, value }, index) => {
      const wanted = types2[index].replace("?", "");
      if (type === wanted || // A length or an angle may be a plain 0:
      type === "number" && value === 0 && wanted !== "number") {
        return value;
      } else if (wanted === "scale" && (type === "number" || type === "percentage")) {
        return type === "percentage" ? value / 100 : value;
      } else {
        throw syntaxError(text);
      }
    });
  }
  __name(getArguments, "getArguments");
  function parseValue(source, text) {
    const tokens = tokenize(source.trim(), text);
    let at = 0;
    const peek = /* @__PURE__ */ __name(() => tokens[at], "peek");
    const next = /* @__PURE__ */ __name(() => tokens[at++], "next");
    const sum = /* @__PURE__ */ __name(() => {
      let left = product();
      while (peek() === "+" || peek() === "-") {
        const sign = next() === "+" ? 1 : -1;
        const right = product();
        if (left.type !== right.type) throw syntaxError(text);
        left = { type: left.type, value: left.value + sign * right.value };
      }
      return left;
    }, "sum");
    const product = /* @__PURE__ */ __name(() => {
      let left = term();
      while (peek() === "*" || peek() === "/") {
        const operator = next();
        const right = term();
        if (operator === "*" && left.type === "number") {
          left = { type: right.type, value: left.value * right.value };
        } else if (right.type === "number") {
          left = {
            type: left.type,
            value: operator === "*" ? left.value * right.value : left.value / right.value
          };
        } else {
          throw syntaxError(text);
        }
      }
      return left;
    }, "product");
    const term = /* @__PURE__ */ __name(() => {
      const token = next();
      if (token === "(" || token === "calc(") {
        const value2 = sum();
        if (next() !== ")") throw syntaxError(text);
        return value2;
      } else if (token?.type) {
        return token;
      } else {
        throw syntaxError(text);
      }
    }, "term");
    const value = tokens[0] === "calc(" ? term() : next();
    if (!value?.type || at !== tokens.length) throw syntaxError(text);
    return value;
  }
  __name(parseValue, "parseValue");
  function tokenize(source, text) {
    const tokens = [];
    const pattern = /\s*(?:([+-]?(?:\d+\.?\d*|\.\d+)(?:e[+-]?\d+)?)([a-z%]*)|(calc\()|([()*/])|(\s[+-]\s))/iy;
    while (pattern.lastIndex < source.length) {
      const match = pattern.exec(source);
      if (!match) throw syntaxError(text);
      const [, number, unit, calc, symbol, operator] = match;
      if (number !== void 0) {
        const known = units[unit.toLowerCase()];
        if (!known) throw syntaxError(text);
        const [type, factor] = known;
        tokens.push({ type, value: Number(number) * factor });
      } else {
        tokens.push(calc?.toLowerCase() ?? symbol ?? operator.trim());
      }
    }
    return tokens;
  }
  __name(tokenize, "tokenize");
  function getClosing(source, start, text) {
    let depth = 1;
    for (let index = start; index < source.length; index++) {
      if (source[index] === "(") depth++;
      else if (source[index] === ")" && --depth === 0) return index;
    }
    throw syntaxError(text);
  }
  __name(getClosing, "getClosing");
  function split(source) {
    const args = [];
    let depth = 0;
    let start = 0;
    for (let index = 0; index < source.length; index++) {
      const char = source[index];
      if (char === "(") depth++;
      else if (char === ")") depth--;
      else if (char === "," && depth === 0) {
        args.push(source.slice(start, index));
        start = index + 1;
      }
    }
    args.push(source.slice(start));
    return args;
  }
  __name(split, "split");
  function syntaxError(text) {
    return new DOMException(
      `Invalid transform list: '${text}'.`,
      "SyntaxError"
    );
  }
  __name(syntaxError, "syntaxError");

  // src/geometry.js
  var identity = [1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1];
  var aliases = [
    ["a", "m11", 0],
    ["b", "m12", 1],
    ["c", "m21", 4],
    ["d", "m22", 5],
    ["e", "m41", 12],
    ["f", "m42", 13]
  ];
  var threeD = [
    ["m13", 2, 0],
    ["m14", 3, 0],
    ["m23", 6, 0],
    ["m24", 7, 0],
    ["m31", 8, 0],
    ["m32", 9, 0],
    ["m33", 10, 1],
    ["m34", 11, 0],
    ["m43", 14, 0],
    ["m44", 15, 1]
  ];
  var names = [
    "m11",
    "m12",
    "m13",
    "m14",
    "m21",
    "m22",
    "m23",
    "m24",
    "m31",
    "m32",
    "m33",
    "m34",
    "m41",
    "m42",
    "m43",
    "m44"
  ];
  var getPoint;
  var getRect;
  var getMatrix;
  var setMatrix;
  var DOMPointReadOnly = class {
    static {
      __name(this, "DOMPointReadOnly");
    }
    #values;
    constructor(x = 0, y = 0, z = 0, w = 1) {
      this.#values = [+x, +y, +z, +w];
    }
    static {
      getPoint = /* @__PURE__ */ __name((point) => point.#values, "getPoint");
    }
    matrixTransform(matrix = {}) {
      return DOMMatrixReadOnly.fromMatrix(matrix).transformPoint(this);
    }
    toJSON() {
      const { x, y, z, w } = this;
      return { x, y, z, w };
    }
    static fromPoint(point) {
      const { x, y, z, w } = getPointInit(point);
      return new this(x, y, z, w);
    }
  };
  var DOMPoint = class extends DOMPointReadOnly {
    static {
      __name(this, "DOMPoint");
    }
  };
  var DOMRectReadOnly = class {
    static {
      __name(this, "DOMRectReadOnly");
    }
    #values;
    constructor(x = 0, y = 0, width = 0, height = 0) {
      this.#values = [+x, +y, +width, +height];
    }
    static {
      getRect = /* @__PURE__ */ __name((rect) => rect.#values, "getRect");
    }
    // `Math.min()` and `max()` are NaN when either value is, as the sides are:
    get top() {
      return Math.min(this.y, this.y + this.height);
    }
    get right() {
      return Math.max(this.x, this.x + this.width);
    }
    get bottom() {
      return Math.max(this.y, this.y + this.height);
    }
    get left() {
      return Math.min(this.x, this.x + this.width);
    }
    toJSON() {
      const { x, y, width, height, top, right, bottom, left } = this;
      return { x, y, width, height, top, right, bottom, left };
    }
    static fromRect(rect) {
      const { x, y, width, height } = getRectInit(rect);
      return new this(x, y, width, height);
    }
  };
  var DOMRect = class extends DOMRectReadOnly {
    static {
      __name(this, "DOMRect");
    }
  };
  var DOMQuad = class {
    static {
      __name(this, "DOMQuad");
    }
    #points;
    constructor(p1, p2, p3, p4) {
      this.#points = [p1, p2, p3, p4].map((point) => DOMPoint.fromPoint(point));
    }
    get p1() {
      return this.#points[0];
    }
    get p2() {
      return this.#points[1];
    }
    get p3() {
      return this.#points[2];
    }
    get p4() {
      return this.#points[3];
    }
    getBounds() {
      const xs = this.#points.map((point) => point.x);
      const ys = this.#points.map((point) => point.y);
      const left = Math.min(...xs);
      const top = Math.min(...ys);
      return new DOMRect(left, top, Math.max(...xs) - left, Math.max(...ys) - top);
    }
    toJSON() {
      const { p1, p2, p3, p4 } = this;
      return { p1, p2, p3, p4 };
    }
    static fromRect(rect) {
      const { x, y, width, height } = getRectInit(rect);
      return new this(
        { x, y },
        { x: x + width, y },
        { x: x + width, y: y + height },
        { x, y: y + height }
      );
    }
    static fromQuad(quad) {
      const { p1, p2, p3, p4 } = getDictionary(quad, "DOMQuadInit") ?? {};
      return new this(p1, p2, p3, p4);
    }
  };
  var DOMMatrixReadOnly = class {
    static {
      __name(this, "DOMMatrixReadOnly");
    }
    #m;
    #is2D;
    // From a list of 6 numbers (2D) or 16, or else, as WebIDL converts it, from
    // text: CSS's transform functions (another matrix as it writes itself).
    constructor(init) {
      if (init === void 0) {
        this.#m = [...identity];
        this.#is2D = true;
      } else if (typeof init?.[Symbol.iterator] === "function" && typeof init !== "string") {
        ;
        [this.#m, this.#is2D] = getMatrixFromList(init);
      } else {
        ;
        [this.#m, this.#is2D] = getMatrixFromCSS(String(init));
      }
    }
    static {
      getMatrix = /* @__PURE__ */ __name((matrix) => [matrix.#m, matrix.#is2D], "getMatrix");
      setMatrix = /* @__PURE__ */ __name((matrix, [m, is2D]) => {
        matrix.#m = m;
        matrix.#is2D = is2D;
        return matrix;
      }, "setMatrix");
    }
    get is2D() {
      return this.#is2D;
    }
    get isIdentity() {
      return this.#m.every((value, index) => value === identity[index]);
    }
    // The operations on the read-only class give a new matrix, the writable
    // class's change it (`translateSelf()`):
    translate(tx = 0, ty = 0, tz = 0) {
      return operate(this, translation(+tx, +ty, +tz));
    }
    scale(sx = 1, sy = sx, sz = 1, ox = 0, oy = 0, oz = 0) {
      return operate(this, scalingAround(+sx, +sy, +sz, +ox, +oy, +oz));
    }
    scaleNonUniform(sx = 1, sy = 1) {
      return operate(this, scaling(+sx, +sy, 1));
    }
    scale3d(scale = 1, ox = 0, oy = 0, oz = 0) {
      return operate(this, scalingAround(+scale, +scale, +scale, +ox, +oy, +oz));
    }
    rotate(rx = 0, ry, rz) {
      return operate(this, rotationXYZ(rx, ry, rz));
    }
    rotateFromVector(x = 0, y = 0) {
      return operate(this, rotationFromVector(+x, +y));
    }
    rotateAxisAngle(x = 0, y = 0, z = 0, angle = 0) {
      return operate(this, rotation(+x, +y, +z, +angle));
    }
    skewX(sx = 0) {
      return operate(this, get2D([1, 0, Math.tan(toRadians(+sx)), 1, 0, 0]));
    }
    skewY(sy = 0) {
      return operate(this, get2D([1, Math.tan(toRadians(+sy)), 0, 1, 0, 0]));
    }
    multiply(other = {}) {
      return operate(this, getMatrixInit(other));
    }
    flipX() {
      return operate(this, get2D([-1, 0, 0, 1, 0, 0]));
    }
    flipY() {
      return operate(this, get2D([1, 0, 0, -1, 0, 0]));
    }
    inverse() {
      return setMatrix(new DOMMatrix(), invert(getMatrix(this)));
    }
    transformPoint(point) {
      const { x, y, z, w } = getPointInit(point);
      const m = this.#m;
      const row = /* @__PURE__ */ __name((r) => m[r] * x + m[4 + r] * y + m[8 + r] * z + m[12 + r] * w, "row");
      return new DOMPoint(row(0), row(1), row(2), row(3));
    }
    toFloat32Array() {
      return new Float32Array(this.#m);
    }
    toFloat64Array() {
      return new Float64Array(this.#m);
    }
    toJSON() {
      const json = {};
      for (const [name] of aliases) json[name] = this[name];
      for (const name of names) json[name] = this[name];
      json.is2D = this.is2D;
      json.isIdentity = this.isIdentity;
      return json;
    }
    // `matrix(a, b, c, d, e, f)` or `matrix3d(...)`, its numbers as JavaScript
    // writes them.
    toString() {
      const m = this.#m;
      if (!m.every(Number.isFinite)) {
        throw new DOMException(
          "Unable to write a matrix that is not finite.",
          "InvalidStateError"
        );
      }
      return this.#is2D ? `matrix(${aliases.map(([, , index]) => m[index]).join(", ")})` : `matrix3d(${m.join(", ")})`;
    }
    static fromMatrix(other) {
      return setMatrix(new this(), getMatrixInit(other));
    }
    static fromFloat32Array(array) {
      return fromArray(this, array);
    }
    static fromFloat64Array(array) {
      return fromArray(this, array);
    }
  };
  var DOMMatrix = class extends DOMMatrixReadOnly {
    static {
      __name(this, "DOMMatrix");
    }
    translateSelf(tx = 0, ty = 0, tz = 0) {
      return change(this, this.translate(tx, ty, tz));
    }
    scaleSelf(sx = 1, sy = sx, sz = 1, ox = 0, oy = 0, oz = 0) {
      return change(this, this.scale(sx, sy, sz, ox, oy, oz));
    }
    scale3dSelf(scale = 1, ox = 0, oy = 0, oz = 0) {
      return change(this, this.scale3d(scale, ox, oy, oz));
    }
    rotateSelf(rx = 0, ry, rz) {
      return change(this, this.rotate(rx, ry, rz));
    }
    rotateFromVectorSelf(x = 0, y = 0) {
      return change(this, this.rotateFromVector(x, y));
    }
    rotateAxisAngleSelf(x = 0, y = 0, z = 0, angle = 0) {
      return change(this, this.rotateAxisAngle(x, y, z, angle));
    }
    skewXSelf(sx = 0) {
      return change(this, this.skewX(sx));
    }
    skewYSelf(sy = 0) {
      return change(this, this.skewY(sy));
    }
    multiplySelf(other = {}) {
      return change(this, this.multiply(other));
    }
    preMultiplySelf(other = {}) {
      return setMatrix(this, multiply(getMatrixInit(other), getMatrix(this)));
    }
    invertSelf() {
      return setMatrix(this, invert(getMatrix(this)));
    }
    setMatrixValue(transformList) {
      return setMatrix(this, getMatrixFromCSS(String(transformList)));
    }
  };
  defineAttributes(
    DOMPointReadOnly,
    DOMPoint,
    ["x", "y", "z", "w"].map((name, index) => [name, index]),
    (point) => getPoint(point)
  );
  defineAttributes(
    DOMRectReadOnly,
    DOMRect,
    ["x", "y", "width", "height"].map((name, index) => [name, index]),
    (rect) => getRect(rect)
  );
  defineAttributes(
    DOMMatrixReadOnly,
    DOMMatrix,
    [
      ...aliases.map(([name, , index]) => [name, index]),
      ...names.map((name, index) => [name, index])
    ],
    (matrix) => getMatrix(matrix)[0],
    // A matrix that gets a 3D value isn't 2D anymore:
    (matrix, index, value) => {
      if (threeD.some(([, i, value2D]) => i === index && value !== value2D)) {
        setMatrix(matrix, [getMatrix(matrix)[0], false]);
      }
    }
  );
  setToStringTag(
    DOMPointReadOnly,
    DOMPoint,
    DOMRectReadOnly,
    DOMRect,
    DOMQuad,
    DOMMatrixReadOnly,
    DOMMatrix
  );
  function defineAttributes(ReadOnly, Writable, entries, getValues2, changed) {
    for (const [name, index] of entries) {
      inject(
        ReadOnly.prototype,
        {
          get [name]() {
            return getValues2(this)[index];
          }
        },
        { enumerable: true }
      );
      inject(
        Writable.prototype,
        {
          get [name]() {
            return getValues2(this)[index];
          },
          set [name](value) {
            getValues2(this)[index] = +value;
            changed?.(this, index, +value);
          }
        },
        { enumerable: true }
      );
    }
  }
  __name(defineAttributes, "defineAttributes");
  function change(matrix, result) {
    return setMatrix(matrix, getMatrix(result));
  }
  __name(change, "change");
  function operate(matrix, other) {
    return setMatrix(new DOMMatrix(), multiply(getMatrix(matrix), other));
  }
  __name(operate, "operate");
  function multiply([a, is2DA], [b, is2DB]) {
    const m = [];
    for (let i = 0; i < 16; i++) {
      const column = Math.floor(i / 4);
      const row = i % 4;
      let value = 0;
      for (let k = 0; k < 4; k++) value += a[k * 4 + row] * b[column * 4 + k];
      m.push(value);
    }
    return [m, is2DA && is2DB];
  }
  __name(multiply, "multiply");
  function getMatrixFromCSS(text) {
    return (text === "" ? [] : parseTransformList(text)).map(({ name, args }) => getTransform(name, args)).reduce(multiply, [[...identity], true]);
  }
  __name(getMatrixFromCSS, "getMatrixFromCSS");
  function getTransform(name, args) {
    const [a, b, c, d] = args;
    const tan = /* @__PURE__ */ __name((degrees) => Math.tan(toRadians(degrees)), "tan");
    const [m, is2D] = {
      matrix: /* @__PURE__ */ __name(() => get2D(args), "matrix"),
      matrix3d: /* @__PURE__ */ __name(() => [args, false], "matrix3d"),
      translate: /* @__PURE__ */ __name(() => translation(a, b ?? 0, 0), "translate"),
      translatex: /* @__PURE__ */ __name(() => translation(a, 0, 0), "translatex"),
      translatey: /* @__PURE__ */ __name(() => translation(0, a, 0), "translatey"),
      translatez: /* @__PURE__ */ __name(() => translation(0, 0, a), "translatez"),
      translate3d: /* @__PURE__ */ __name(() => translation(a, b, c), "translate3d"),
      scale: /* @__PURE__ */ __name(() => scaling(a, b ?? a, 1), "scale"),
      scalex: /* @__PURE__ */ __name(() => scaling(a, 1, 1), "scalex"),
      scaley: /* @__PURE__ */ __name(() => scaling(1, a, 1), "scaley"),
      scalez: /* @__PURE__ */ __name(() => scaling(1, 1, a), "scalez"),
      scale3d: /* @__PURE__ */ __name(() => scaling(a, b, c), "scale3d"),
      rotate: /* @__PURE__ */ __name(() => rotation(0, 0, 1, a), "rotate"),
      rotatex: /* @__PURE__ */ __name(() => rotation(1, 0, 0, a), "rotatex"),
      rotatey: /* @__PURE__ */ __name(() => rotation(0, 1, 0, a), "rotatey"),
      rotatez: /* @__PURE__ */ __name(() => rotation(0, 0, 1, a), "rotatez"),
      rotate3d: /* @__PURE__ */ __name(() => rotation(a, b, c, d), "rotate3d"),
      skew: /* @__PURE__ */ __name(() => get2D([1, tan(b ?? 0), tan(a), 1, 0, 0]), "skew"),
      skewx: /* @__PURE__ */ __name(() => get2D([1, 0, tan(a), 1, 0, 0]), "skewx"),
      skewy: /* @__PURE__ */ __name(() => get2D([1, tan(a), 0, 1, 0, 0]), "skewy"),
      // `m34`, of a distance of 1 at least:
      perspective: /* @__PURE__ */ __name(() => {
        const m2 = [...identity];
        m2[11] = -1 / Math.max(a, 1);
        return [m2, false];
      }, "perspective")
    }[name]();
    return [m, is2D && !/3d$|z$|^rotate[xy]$|^perspective$/.test(name)];
  }
  __name(getTransform, "getTransform");
  function get2D([a, b, c, d, e, f]) {
    const m = [...identity];
    [m[0], m[1], m[4], m[5], m[12], m[13]] = [a, b, c, d, e, f];
    return [m, true];
  }
  __name(get2D, "get2D");
  function translation(x, y, z) {
    const m = [...identity];
    [m[12], m[13], m[14]] = [x, y, z];
    return [m, z === 0];
  }
  __name(translation, "translation");
  function scaling(x, y, z) {
    const m = [...identity];
    [m[0], m[5], m[10]] = [x, y, z];
    return [m, z === 1];
  }
  __name(scaling, "scaling");
  function scalingAround(sx, sy, sz, ox, oy, oz) {
    return multiply(
      multiply(translation(ox, oy, oz), scaling(sx, sy, sz)),
      translation(-ox, -oy, -oz)
    );
  }
  __name(scalingAround, "scalingAround");
  function rotationXYZ(rx, ry, rz) {
    const [x, y, z] = ry === void 0 && rz === void 0 ? [0, 0, +rx] : [+rx, +(ry ?? 0), +(rz ?? 0)];
    let result = rotation(0, 0, 1, z);
    if (y !== 0) result = multiply(result, rotation(0, 1, 0, y));
    if (x !== 0) result = multiply(result, rotation(1, 0, 0, x));
    return result;
  }
  __name(rotationXYZ, "rotationXYZ");
  function rotationFromVector(x, y) {
    return rotation(
      0,
      0,
      1,
      x === 0 && y === 0 ? 0 : Math.atan2(y, x) * 180 / Math.PI
    );
  }
  __name(rotationFromVector, "rotationFromVector");
  function rotation(x, y, z, degrees) {
    const is2D = x === 0 && y === 0;
    const length = Math.hypot(x, y, z);
    if (length === 0 || !Number.isFinite(length)) return [[...identity], is2D];
    [x, y, z] = [x / length, y / length, z / length];
    const half = toRadians(degrees) / 2;
    const sc = Math.sin(half) * Math.cos(half);
    const sq = Math.sin(half) ** 2;
    const m = [
      1 - 2 * (y * y + z * z) * sq,
      2 * (x * y * sq + z * sc),
      2 * (x * z * sq - y * sc),
      0,
      2 * (x * y * sq - z * sc),
      1 - 2 * (x * x + z * z) * sq,
      2 * (y * z * sq + x * sc),
      0,
      2 * (x * z * sq + y * sc),
      2 * (y * z * sq - x * sc),
      1 - 2 * (x * x + y * y) * sq,
      0,
      0,
      0,
      0,
      1
    ];
    return [m, is2D];
  }
  __name(rotation, "rotation");
  function invert([m, is2D]) {
    const singular = [new Array(16).fill(NaN), false];
    if (!m.every(Number.isFinite)) return singular;
    if (is2D) {
      const [a, b, c, d, e, f] = aliases.map(([, , index]) => m[index]);
      const det2 = a * d - b * c;
      return det2 === 0 ? singular : get2D([
        d / det2,
        -b / det2,
        -c / det2,
        a / det2,
        (c * f - d * e) / det2,
        (b * e - a * f) / det2
      ]);
    }
    const i = [
      m[5] * m[10] * m[15] - m[5] * m[11] * m[14] - m[9] * m[6] * m[15] + m[9] * m[7] * m[14] + m[13] * m[6] * m[11] - m[13] * m[7] * m[10],
      -m[1] * m[10] * m[15] + m[1] * m[11] * m[14] + m[9] * m[2] * m[15] - m[9] * m[3] * m[14] - m[13] * m[2] * m[11] + m[13] * m[3] * m[10],
      m[1] * m[6] * m[15] - m[1] * m[7] * m[14] - m[5] * m[2] * m[15] + m[5] * m[3] * m[14] + m[13] * m[2] * m[7] - m[13] * m[3] * m[6],
      -m[1] * m[6] * m[11] + m[1] * m[7] * m[10] + m[5] * m[2] * m[11] - m[5] * m[3] * m[10] - m[9] * m[2] * m[7] + m[9] * m[3] * m[6],
      -m[4] * m[10] * m[15] + m[4] * m[11] * m[14] + m[8] * m[6] * m[15] - m[8] * m[7] * m[14] - m[12] * m[6] * m[11] + m[12] * m[7] * m[10],
      m[0] * m[10] * m[15] - m[0] * m[11] * m[14] - m[8] * m[2] * m[15] + m[8] * m[3] * m[14] + m[12] * m[2] * m[11] - m[12] * m[3] * m[10],
      -m[0] * m[6] * m[15] + m[0] * m[7] * m[14] + m[4] * m[2] * m[15] - m[4] * m[3] * m[14] - m[12] * m[2] * m[7] + m[12] * m[3] * m[6],
      m[0] * m[6] * m[11] - m[0] * m[7] * m[10] - m[4] * m[2] * m[11] + m[4] * m[3] * m[10] + m[8] * m[2] * m[7] - m[8] * m[3] * m[6],
      m[4] * m[9] * m[15] - m[4] * m[11] * m[13] - m[8] * m[5] * m[15] + m[8] * m[7] * m[13] + m[12] * m[5] * m[11] - m[12] * m[7] * m[9],
      -m[0] * m[9] * m[15] + m[0] * m[11] * m[13] + m[8] * m[1] * m[15] - m[8] * m[3] * m[13] - m[12] * m[1] * m[11] + m[12] * m[3] * m[9],
      m[0] * m[5] * m[15] - m[0] * m[7] * m[13] - m[4] * m[1] * m[15] + m[4] * m[3] * m[13] + m[12] * m[1] * m[7] - m[12] * m[3] * m[5],
      -m[0] * m[5] * m[11] + m[0] * m[7] * m[9] + m[4] * m[1] * m[11] - m[4] * m[3] * m[9] - m[8] * m[1] * m[7] + m[8] * m[3] * m[5],
      -m[4] * m[9] * m[14] + m[4] * m[10] * m[13] + m[8] * m[5] * m[14] - m[8] * m[6] * m[13] - m[12] * m[5] * m[10] + m[12] * m[6] * m[9],
      m[0] * m[9] * m[14] - m[0] * m[10] * m[13] - m[8] * m[1] * m[14] + m[8] * m[2] * m[13] + m[12] * m[1] * m[10] - m[12] * m[2] * m[9],
      -m[0] * m[5] * m[14] + m[0] * m[6] * m[13] + m[4] * m[1] * m[14] - m[4] * m[2] * m[13] - m[12] * m[1] * m[6] + m[12] * m[2] * m[5],
      m[0] * m[5] * m[10] - m[0] * m[6] * m[9] - m[4] * m[1] * m[10] + m[4] * m[2] * m[9] + m[8] * m[1] * m[6] - m[8] * m[2] * m[5]
    ];
    const det = m[0] * i[0] + m[1] * i[4] + m[2] * i[8] + m[3] * i[12];
    return det !== 0 && Number.isFinite(det) ? [i.map((value) => value / det), false] : singular;
  }
  __name(invert, "invert");
  function toRadians(degrees) {
    return degrees * Math.PI / 180;
  }
  __name(toRadians, "toRadians");
  function getDictionary(value, name) {
    if (value == null) return null;
    if (typeof value !== "object" && typeof value !== "function") {
      throw new TypeError(`Invalid ${name}: '${value}'.`);
    }
    return value;
  }
  __name(getDictionary, "getDictionary");
  function getPointInit(point) {
    const {
      x = 0,
      y = 0,
      z = 0,
      w = 1
    } = getDictionary(point, "DOMPointInit") ?? {};
    return { x: +x, y: +y, z: +z, w: +w };
  }
  __name(getPointInit, "getPointInit");
  function getRectInit(rect) {
    const {
      x = 0,
      y = 0,
      width = 0,
      height = 0
    } = getDictionary(rect, "DOMRectInit") ?? {};
    return { x: +x, y: +y, width: +width, height: +height };
  }
  __name(getRectInit, "getRectInit");
  function getMatrixInit(init) {
    init = getDictionary(init, "DOMMatrixInit") ?? {};
    const m = [...identity];
    for (const [alias, name, index] of aliases) {
      const aliasValue = init[alias] === void 0 ? void 0 : +init[alias];
      const value = init[name] === void 0 ? void 0 : +init[name];
      if (aliasValue !== void 0 && value !== void 0 && // Agreeing, as 0 and -0 do, and NaN and NaN:
      aliasValue !== value && !Object.is(aliasValue, value)) {
        throw new TypeError(`Invalid matrix: its ${name} and ${alias} disagree.`);
      }
      m[index] = value ?? aliasValue ?? m[index];
    }
    let hasThreeD = false;
    for (const [name, index, default2D] of threeD) {
      if (init[name] !== void 0) {
        m[index] = +init[name];
        hasThreeD ||= m[index] !== default2D;
      }
    }
    const is2D = init.is2D === void 0 ? void 0 : !!init.is2D;
    if (is2D && hasThreeD) {
      throw new TypeError("Invalid matrix: a 2D matrix has 3D values.");
    }
    return [m, is2D ?? !hasThreeD];
  }
  __name(getMatrixInit, "getMatrixInit");
  function getMatrixFromList(list) {
    const values = Array.from(list ?? [], Number);
    if (values.length === 6) return get2D(values);
    if (values.length === 16) return [values, false];
    throw new TypeError("Invalid matrix: it takes 6 or 16 numbers.");
  }
  __name(getMatrixFromList, "getMatrixFromList");
  function fromArray(Class, array) {
    if (!ArrayBuffer.isView(array)) {
      throw new TypeError(`Invalid array: '${array}', not a typed array.`);
    }
    return setMatrix(new Class(), getMatrixFromList(array));
  }
  __name(fromArray, "fromArray");

  // src/palette.js
  var palettes = /* @__PURE__ */ new Map();
  var lastId = 0;
  var Palette = class {
    static {
      __name(this, "Palette");
    }
    constructor(title = "", components, values) {
      if (!components || typeof components !== "object") {
        throw new TypeError("Invalid components: they must be an object.");
      }
      this.title = String(title);
      this.components = components;
      this.values = values && typeof values === "object" ? values : {};
      for (const component of getComponents(components, this.values)) {
        if (this.values[component.name] === void 0) {
          this.values[component.name] = getValue(component, component.value);
        }
      }
      palettes.set(++lastId, {
        palette: this,
        defaults: getComponents(components),
        reported: ""
      });
    }
    // Every component's value back to the one it was defined with.
    reset() {
      const { defaults } = [...palettes.values()].find(
        (entry) => entry.palette === this
      );
      for (const component of defaults) {
        const value = getValue(component, component.value);
        this.values[component.name] = value;
        this.components[component.name].value = value;
      }
    }
  };
  setToStringTag(Palette);
  function takePalettes() {
    const changed = [];
    for (const [id, entry] of palettes) {
      const state = getState2(id, entry.palette);
      const text = JSON.stringify(state);
      if (entry.reported !== text) {
        entry.reported = text;
        changed.push(state);
      }
    }
    return changed;
  }
  __name(takePalettes, "takePalettes");
  function hasPalettes() {
    return palettes.size > 0;
  }
  __name(hasPalettes, "hasPalettes");
  function handlePaletteEvent(id, type, name, value) {
    const entry = palettes.get(id);
    if (entry) {
      const { palette } = entry;
      const { components, values } = palette;
      const definition = components[name];
      if (type === "change") {
        const component = getComponents(components, values).find(
          (component2) => component2.name === name
        );
        if (component) {
          value = getValue(component, value);
          values[name] = value;
          entry.reported = JSON.stringify(getState2(id, palette));
          definition.name = name;
          definition.value = value;
          call(definition, "onChange", value);
          call(palette, "onChange", definition);
        }
      } else if (type === "click") {
        call(definition, "onClick");
        call(definition, "onSelect");
      } else if (type === "close") {
        palettes.delete(id);
        call(palette, "onClose");
      }
    }
  }
  __name(handlePaletteEvent, "handlePaletteEvent");
  function getState2(id, palette) {
    const { title, components, values } = palette;
    return { id, title, components: getComponents(components, values) };
  }
  __name(getState2, "getState");
  function call(owner, name, ...args) {
    if (typeof owner?.[name] === "function") owner[name](...args);
  }
  __name(call, "call");

  // src/storage.js
  var sessions = /* @__PURE__ */ new WeakMap();
  var Storage = class {
    static {
      __name(this, "Storage");
    }
    constructor(key) {
      checkInternal(key, "Storage", "use localStorage or sessionStorage");
    }
    get length() {
      return getKeys(this).length;
    }
    key(index) {
      return getKeys(this)[index >>> 0] ?? null;
    }
    getItem(key) {
      checkArguments(arguments, 1, "getItem");
      return natives.storage.getItem(getSession(this), String(key));
    }
    // Throws a `QuotaExceededError` when that's more than it holds.
    setItem(key, value) {
      checkArguments(arguments, 2, "setItem");
      natives.storage.setItem(getSession(this), String(key), String(value));
    }
    removeItem(key) {
      checkArguments(arguments, 1, "removeItem");
      natives.storage.removeItem(getSession(this), String(key));
    }
    clear() {
      natives.storage.clear(getSession(this));
    }
  };
  setToStringTag(Storage);
  var handler = {
    get(target, key, receiver) {
      return isItemKey(target, key) ? target.getItem(key) ?? void 0 : Reflect.get(target, key, receiver);
    },
    set(target, key, value, receiver) {
      if (typeof key === "string") {
        target.setItem(key, value);
        return true;
      } else {
        return Reflect.set(target, key, value, receiver);
      }
    },
    has(target, key) {
      return typeof key === "string" && target.getItem(key) !== null || Reflect.has(target, key);
    },
    deleteProperty(target, key) {
      if (isItemKey(target, key)) {
        target.removeItem(key);
        return true;
      } else {
        return Reflect.deleteProperty(target, key);
      }
    },
    // The items' keys, then the storage's own, as `Object.keys()` lists them.
    ownKeys(target) {
      return [.../* @__PURE__ */ new Set([...getKeys(target), ...Reflect.ownKeys(target)])];
    },
    getOwnPropertyDescriptor(target, key) {
      const value = isItemKey(target, key) ? target.getItem(key) : null;
      return value !== null ? { value, writable: true, enumerable: true, configurable: true } : Reflect.getOwnPropertyDescriptor(target, key);
    }
  };
  function createStorage(isSession) {
    const target = new Storage(internal);
    const storage = new Proxy(target, handler);
    sessions.set(target, isSession);
    sessions.set(storage, isSession);
    return storage;
  }
  __name(createStorage, "createStorage");
  function getSession(storage) {
    const isSession = sessions.get(storage);
    if (isSession === void 0) {
      throw new TypeError(`Invalid storage: '${storage}', not a Storage.`);
    }
    return isSession;
  }
  __name(getSession, "getSession");
  function getKeys(storage) {
    return natives.storage.keys(getSession(storage));
  }
  __name(getKeys, "getKeys");
  function isItemKey(target, key) {
    return typeof key === "string" && !(key in target);
  }
  __name(isItemKey, "isItemKey");

  // src/url.js
  var parts = [
    "protocol",
    "username",
    "password",
    "host",
    "hostname",
    "port",
    "pathname",
    "search",
    "hash"
  ];
  var setPairs;
  var setSearch;
  var URL2 = class _URL {
    static {
      __name(this, "URL");
    }
    // The parts as the runtime parsed them:
    #parts;
    #searchParams = null;
    constructor(url, base) {
      this.#parts = parse(url, base);
      if (!this.#parts) {
        throw new TypeError(`Invalid URL: '${url}'.`);
      }
    }
    static {
      setSearch = /* @__PURE__ */ __name((url, query) => {
        url.#parts = natives.url.set(url.#parts.href, "search", query);
      }, "setSearch");
    }
    static canParse(url, base) {
      return !!parse(url, base);
    }
    static parse(url, base) {
      return parse(url, base) ? new _URL(url, base) : null;
    }
    get href() {
      return this.#parts.href;
    }
    set href(href) {
      this.#set("href", href);
    }
    get origin() {
      return this.#parts.origin;
    }
    get searchParams() {
      if (!this.#searchParams) {
        this.#searchParams = new URLSearchParams2(this.#parts.search);
        linksByParams.set(this.#searchParams, this);
      }
      return this.#searchParams;
    }
    toString() {
      return this.#parts.href;
    }
    toJSON() {
      return this.#parts.href;
    }
    // Set a part, and the search params follow.
    #set(part, value) {
      this.#parts = natives.url.set(this.#parts.href, part, String(value));
      if (this.#searchParams) {
        setPairs(this.#searchParams, parseQuery(this.#parts.search));
      }
    }
    static {
      for (const part of parts) {
        inject(
          _URL.prototype,
          {
            get [part]() {
              return this.#parts[part];
            },
            set [part](value) {
              this.#set(part, value);
            }
          },
          { enumerable: true }
        );
      }
    }
  };
  var linksByParams = /* @__PURE__ */ new WeakMap();
  var URLSearchParams2 = class _URLSearchParams {
    static {
      __name(this, "URLSearchParams");
    }
    #pairs = [];
    // A query string (with or without `?`), `[[name, value], …]`,
    // `{ name: value }` or another `URLSearchParams`.
    constructor(init = "") {
      if (init instanceof _URLSearchParams) {
        this.#pairs = init.#pairs.map((pair) => [...pair]);
      } else if (init !== null && typeof init === "object") {
        this.#pairs = getPairs(init).map((pair) => pair.map(String));
      } else {
        this.#pairs = parseQuery(String(init));
      }
    }
    static {
      setPairs = /* @__PURE__ */ __name((params, pairs) => {
        params.#pairs = pairs;
      }, "setPairs");
    }
    get size() {
      return this.#pairs.length;
    }
    append(name, value) {
      this.#pairs.push([String(name), String(value)]);
      this.#update();
    }
    delete(name, value) {
      name = String(name);
      this.#pairs = this.#pairs.filter(
        (pair) => pair[0] !== name || value !== void 0 && pair[1] !== String(value)
      );
      this.#update();
    }
    get(name) {
      return this.#pairs.find((pair) => pair[0] === String(name))?.[1] ?? null;
    }
    getAll(name) {
      return this.#pairs.filter((pair) => pair[0] === String(name)).map(([, value]) => value);
    }
    has(name, value) {
      return this.#pairs.some(
        (pair) => pair[0] === String(name) && (value === void 0 || pair[1] === String(value))
      );
    }
    set(name, value) {
      this.#pairs = setFirst(this.#pairs, [String(name), String(value)]);
      this.#update();
    }
    // By name, in UTF-16 code units, as strings compare: stable.
    sort() {
      this.#pairs.sort(([a], [b]) => a < b ? -1 : a > b ? 1 : 0);
      this.#update();
    }
    *entries() {
      for (const [name, value] of this.#pairs) yield [name, value];
    }
    toString() {
      return natives.url.serializeQuery(this.#pairs);
    }
    // The URL they belong to follows.
    #update() {
      const url = linksByParams.get(this);
      if (url) setSearch(url, this.toString());
    }
  };
  inject(URLSearchParams2.prototype, pairIterable);
  setToStringTag(URL2, URLSearchParams2);
  function parse(url, base) {
    return base === void 0 ? natives.url.parse(String(url)) : natives.url.parse(String(url), String(base));
  }
  __name(parse, "parse");
  function parseQuery(query) {
    return natives.url.parseQuery(query.replace(/^\?/, ""));
  }
  __name(parseQuery, "parseQuery");

  // src/websocket.js
  var states2 = { CONNECTING: 0, OPEN: 1, CLOSING: 2, CLOSED: 3 };
  var schemes = {
    "ws:": "ws:",
    "http:": "ws:",
    "wss:": "wss:",
    "https:": "wss:"
  };
  var WebSocket = class extends EventTarget {
    static {
      __name(this, "WebSocket");
    }
    #url;
    #number;
    #readyState = states2.CONNECTING;
    #protocol = "";
    #binaryType = "blob";
    constructor(url, protocols = []) {
      super();
      url = String(url);
      const parsed = URL.canParse(url) ? new URL(url) : null;
      const scheme = schemes[parsed?.protocol];
      if (!scheme || parsed.hash) {
        throw new DOMException(`Invalid WebSocket URL: '${url}'.`, "SyntaxError");
      }
      parsed.protocol = scheme;
      this.#url = parsed.href;
      this.#number = natives.socket.open(
        this.#url,
        typeof protocols === "string" ? [protocols] : Array.from(protocols, String),
        (incoming) => this.#receive(incoming)
      );
    }
    get url() {
      return this.#url;
    }
    get readyState() {
      return this.#readyState;
    }
    get protocol() {
      return this.#protocol;
    }
    get extensions() {
      return "";
    }
    get bufferedAmount() {
      return 0;
    }
    get binaryType() {
      return this.#binaryType;
    }
    set binaryType(type) {
      if (type === "blob" || type === "arraybuffer") this.#binaryType = type;
    }
    // Text, or the bytes of a `Blob`, a buffer or a view. A closing or closed
    // socket sends nothing, as in browsers.
    send(data) {
      checkArguments(arguments, 1, "send");
      if (this.#readyState === states2.CONNECTING) {
        throw new DOMException(
          "Unable to send: the WebSocket is not open yet.",
          "InvalidStateError"
        );
      }
      if (this.#readyState === states2.OPEN) {
        natives.socket.send(
          this.#number,
          getBufferBytes(data) ?? (data instanceof Blob ? getBytes(data) : String(data))
        );
      }
    }
    // A code of 1000 or 3000 to 4999, a reason of 123 bytes at most.
    close(code, reason = "") {
      if (code !== void 0 && (code = Number(code)) !== 1e3 && !(code >= 3e3 && code <= 4999)) {
        throw new DOMException(
          `Invalid WebSocket close code: '${code}'.`,
          "InvalidAccessError"
        );
      }
      reason = String(reason);
      if (new TextEncoder().encode(reason).length > 123) {
        throw new DOMException(
          "Invalid WebSocket close reason: it is longer than 123 bytes.",
          "SyntaxError"
        );
      }
      if (this.#readyState < states2.CLOSING) {
        this.#readyState = states2.CLOSING;
        natives.socket.close(this.#number, code ?? 1e3, reason);
      }
    }
    // What the socket's thread read: `{ type, … }`.
    #receive({ type, protocol, data, message, code, reason, wasClean }) {
      switch (type) {
        case "open":
          this.#readyState = states2.OPEN;
          this.#protocol = protocol;
          fire(this, new Event("open"));
          break;
        case "message":
          fire(
            this,
            new MessageEvent("message", {
              data: typeof data === "string" ? data : this.#binaryType === "arraybuffer" ? data.buffer : new Blob([data]),
              origin: new URL(this.#url).origin
            })
          );
          break;
        case "error":
          console.error(`WebSocket ${this.#url}: ${message}`);
          fire(this, new Event("error"));
          break;
        case "close":
          this.#readyState = states2.CLOSED;
          fire(this, new CloseEvent("close", { code, reason, wasClean }));
          break;
      }
    }
  };
  defineConstants(WebSocket, states2);
  defineEventHandlers(WebSocket.prototype, "open", "message", "error", "close");
  setToStringTag(WebSocket);

  // src/worker.js
  var Worker = class extends EventTarget {
    static {
      __name(this, "Worker");
    }
    #number;
    constructor(url, { type = "classic", name = "" } = {}) {
      super();
      if (type === "module") {
        throw new DOMException(
          "Unsupported Worker: module workers are not supported.",
          "NotSupportedError"
        );
      }
      this.#number = natives.worker.start(
        String(url),
        String(name),
        (sent) => this.#receive(sent)
      );
    }
    postMessage(data) {
      natives.worker.post(this.#number, data);
    }
    terminate() {
      natives.worker.terminate(this.#number);
    }
    // What the worker sent: `{ type, data }` or `{ type: 'error', message }`.
    #receive({ type, data, message }) {
      if (type === "error") {
        const event = new ErrorEvent("error", { message, cancelable: true });
        if (fire(this, event)) {
          console.error(`Uncaught (in worker) ${message}`);
        }
      } else {
        fire(this, new MessageEvent(type, { data }));
      }
    }
  };
  defineEventHandlers(Worker.prototype, "message", "messageerror", "error");
  setToStringTag(Worker);
  function setupWorker(name) {
    const { addEventListener, removeEventListener, dispatchEvent } = EventTarget.prototype;
    const { postMessage, close, importScripts } = natives.worker.self;
    inject(globalThis, {
      name,
      postMessage,
      close,
      importScripts,
      addEventListener,
      removeEventListener,
      dispatchEvent
    });
    defineEventHandlers(globalThis, "message", "messageerror");
    return ({ type, data }) => fire(globalThis, new MessageEvent(type, { data }));
  }
  __name(setupWorker, "setupWorker");

  // src/index.js
  delete globalThis.__scriptographer;
  var { print } = natives;
  var console2 = createConsole(print);
  inject(globalThis, { console: console2, print: console2.log });
  inject(
    globalThis,
    Object.fromEntries(
      Object.entries(global_scope_exports).filter(([name]) => !(name in globalThis))
    )
  );
  if (globalThis.navigator && !navigator.clipboard) {
    inject(globalThis, { Clipboard });
    inject(navigator, { clipboard: new Clipboard() }, { writable: false });
  }
  if (!globalThis.TextEncoder) {
    inject(globalThis, { TextEncoder: TextEncoder2, TextDecoder: TextDecoder2 });
  }
  if (!globalThis.crypto) {
    inject(globalThis, { Crypto, crypto: new Crypto() });
  }
  if (!globalThis.DOMException) {
    inject(globalThis, { DOMException: DOMException2 });
  }
  if (!globalThis.EventTarget) {
    inject(globalThis, { ...events_exports });
  }
  if (!globalThis.fetch) {
    const { Blob: Blob2, File: File2, getBytes: getBytes2 } = blob_exports;
    const { getBody: getBody2, ...classes } = fetch_exports;
    inject(globalThis, { ...abort_exports, Blob: Blob2, File: File2, ...classes });
    Object.assign(natives, { getBytes: getBytes2 });
  }
  if (!globalThis.localStorage) {
    inject(globalThis, {
      Storage,
      localStorage: createStorage(false),
      sessionStorage: createStorage(true)
    });
  }
  if (!globalThis.URL) {
    inject(globalThis, { URL: URL2, URLSearchParams: URLSearchParams2 });
  }
  if (!globalThis.WebSocket) {
    inject(globalThis, { WebSocket });
  }
  if (!globalThis.Worker) {
    inject(globalThis, { Worker });
    Object.assign(natives, { setupWorker });
  }
  if (natives.dom && !globalThis.document) {
    inject(globalThis, dom, { configurable: true });
  }
  if (natives.fs) {
    const { getHandlePath: getHandlePath2, ...classes } = file_system_exports;
    inject(globalThis, { LocalFile: LocalFile2, ...classes });
  }
  if (!globalThis.OffscreenCanvas) {
    inject(globalThis, { ...canvas_exports });
  }
  if (!globalThis.DOMMatrix) {
    inject(globalThis, { ...geometry_exports, WebKitCSSMatrix: DOMMatrix });
  }
  setupDialog(globalThis.Dialog ??= {});
  inject(globalThis, { Palette });
  Object.assign(natives, { takePalettes, hasPalettes, handlePaletteEvent });
})();
