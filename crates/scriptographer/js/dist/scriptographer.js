(() => {
  var __defProp = Object.defineProperty;
  var __export = (target, all) => {
    for (var name in all)
      __defProp(target, name, { get: all[name], enumerable: true });
  };

  // src/format-error.js
  function formatError(err) {
    const text = String(err);
    const stack = err?.stack;
    return typeof stack === "string" && stack.startsWith(text) ? stack : text;
  }

  // src/console.js
  var maxDepth = 2;
  var maxItems = 100;
  function createConsole(print2) {
    let indent = 0;
    const counts = /* @__PURE__ */ new Map();
    const timers = /* @__PURE__ */ new Map();
    const write = (text) => print2(
      text.split("\n").map((line) => "  ".repeat(indent) + line).join("\n")
    );
    const log = (...args) => write(format(args));
    const now = () => globalThis.performance?.now() ?? Date.now();
    const timeLog = (end, label = "default", ...data) => {
      label = String(label);
      if (timers.has(label)) {
        let line = `${label}: ${(now() - timers.get(label)).toFixed(3)} ms`;
        if (end) timers.delete(label);
        else if (data.length) line += ` ${format(data)}`;
        write(line);
      } else {
        write(`Timer '${label}' does not exist`);
      }
    };
    const group = (...args) => {
      if (args.length) log(...args);
      indent++;
    };
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
  function format(args) {
    const parts = [];
    let rest = args;
    const [first, ...after] = args;
    if (typeof first === "string" && first.includes("%")) {
      rest = after;
      parts.push(
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
    for (const value of rest) parts.push(inspect(value, 0));
    return parts.join(" ");
  }
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
  function getItems(entries, total, write) {
    const written = entries.map(([key, value]) => write(key, value));
    if (total > entries.length) {
      written.push(`\u2026 ${total - entries.length} more`);
    }
    return written.join(", ");
  }
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

  // src/dialog.js
  function setupFileDialogs(Dialog) {
    Object.assign(Dialog, {
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
  async function pick(picker, options) {
    try {
      return await picker(options);
    } catch (err) {
      if (err.name === "AbortError") return null;
      throw err;
    }
  }
  function getPickerOptions(kind, { types, path } = {}) {
    const options = {};
    if (types && kind !== "chooseDirectory") {
      const accepted = [];
      let acceptsAll = false;
      for (const [description, extensions] of getTypes(types)) {
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
  function getTypes(types) {
    const entries = Array.isArray(types) ? Array.from({ length: Math.ceil(types.length / 2) }, (_, i) => [
      types[i * 2],
      types[i * 2 + 1] ?? types[i * 2]
    ]) : Object.entries(types);
    return entries.map(([label, patterns]) => [
      String(label),
      (Array.isArray(patterns) ? patterns : String(patterns).split(";")).map(
        (pattern) => String(pattern).trim().replace(/^\*?\.?/, "")
      ).filter((ext) => ext && ext !== "*")
    ]);
  }
  function getLocalFile(path) {
    try {
      return new LocalFile(String(path));
    } catch {
      return null;
    }
  }

  // src/events.js
  var events_exports = {};
  __export(events_exports, {
    CloseEvent: () => CloseEvent,
    CustomEvent: () => CustomEvent,
    ErrorEvent: () => ErrorEvent,
    Event: () => Event,
    EventTarget: () => EventTarget,
    MessageEvent: () => MessageEvent
  });
  var phases = { NONE: 0, CAPTURING_PHASE: 1, AT_TARGET: 2, BUBBLING_PHASE: 3 };
  var listenersByTarget = /* @__PURE__ */ new WeakMap();
  var states = /* @__PURE__ */ new WeakMap();
  var EventTarget = class {
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
            console.error(`Uncaught ${formatError(err)}`);
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
    constructor(type, init) {
      super(...arguments);
      setMembers(this, init, { wasClean: false, code: 0, reason: "" });
    }
  };
  var ErrorEvent = class extends Event {
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
      Object.defineProperty(Class.prototype, name, {
        get() {
          return getState(this)[name];
        },
        configurable: true,
        enumerable: true
      });
    }
  }
  for (const [name, value] of Object.entries(phases)) {
    for (const target of [Event, Event.prototype]) {
      Object.defineProperty(target, name, { value, enumerable: true });
    }
  }
  for (const Class of [
    EventTarget,
    Event,
    CustomEvent,
    MessageEvent,
    CloseEvent,
    ErrorEvent
  ]) {
    Object.defineProperty(Class.prototype, Symbol.toStringTag, {
      value: Class.name,
      configurable: true
    });
  }
  function getState(event) {
    const state = states.get(event);
    if (!state) {
      throw new TypeError(`Invalid event: '${event}', not an Event.`);
    }
    return state;
  }
  function setMembers(event, init, defaults) {
    const state = getState(event);
    for (const [name, value] of Object.entries(defaults)) {
      const given = init?.[name];
      state[name] = given === void 0 ? Array.isArray(value) ? [] : value : typeof value === "string" ? String(given) : typeof value === "number" ? Number(given) : typeof value === "boolean" ? !!given : given;
    }
  }
  function getOptions(options) {
    if (options && typeof options === "object") {
      const { capture, once, passive, signal } = options;
      return { capture: !!capture, once: !!once, passive: !!passive, signal };
    } else {
      return { capture: !!options, once: false, passive: false, signal: null };
    }
  }
  function find(listeners, type, callback, capture) {
    return listeners?.find(
      (listener) => listener.type === type && listener.capture === capture && Object.is(listener.callback, callback)
    );
  }
  function remove(target, listener) {
    listener.removed = true;
    const listeners = listenersByTarget.get(target);
    const index = listeners?.indexOf(listener) ?? -1;
    if (index >= 0) listeners.splice(index, 1);
  }

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
    #values;
    constructor(x = 0, y = 0, z = 0, w = 1) {
      this.#values = [+x, +y, +z, +w];
    }
    static {
      getPoint = (point) => point.#values;
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
  };
  var DOMRectReadOnly = class {
    #values;
    constructor(x = 0, y = 0, width = 0, height = 0) {
      this.#values = [+x, +y, +width, +height];
    }
    static {
      getRect = (rect) => rect.#values;
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
  };
  var DOMQuad = class {
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
    #m;
    #is2D;
    // From 6 numbers (2D) or 16. As in a worker, there's no CSS to parse.
    constructor(init) {
      if (init === void 0) {
        this.#m = [...identity];
        this.#is2D = true;
      } else if (typeof init === "string") {
        throw new TypeError("Unable to parse a matrix from CSS outside a window.");
      } else {
        ;
        [this.#m, this.#is2D] = getMatrixFromList(init);
      }
    }
    static {
      getMatrix = (matrix) => [matrix.#m, matrix.#is2D];
      setMatrix = (matrix, [m, is2D]) => {
        matrix.#m = m;
        matrix.#is2D = is2D;
        return matrix;
      };
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
      const row = (r) => m[r] * x + m[4 + r] * y + m[8 + r] * z + m[12 + r] * w;
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
    setMatrixValue() {
      throw new TypeError("Unable to parse a matrix from CSS outside a window.");
    }
  };
  defineValues(
    DOMPointReadOnly,
    DOMPoint,
    ["x", "y", "z", "w"].map((name, index) => [name, index]),
    (point) => getPoint(point)
  );
  defineValues(
    DOMRectReadOnly,
    DOMRect,
    ["x", "y", "width", "height"].map((name, index) => [name, index]),
    (rect) => getRect(rect)
  );
  defineValues(
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
  for (const Class of [
    DOMPointReadOnly,
    DOMPoint,
    DOMRectReadOnly,
    DOMRect,
    DOMQuad,
    DOMMatrixReadOnly,
    DOMMatrix
  ]) {
    Object.defineProperty(Class.prototype, Symbol.toStringTag, {
      value: Class.name,
      configurable: true
    });
  }
  function defineValues(ReadOnly, Writable, entries, getValues, changed) {
    for (const [name, index] of entries) {
      const get = function() {
        return getValues(this)[index];
      };
      Object.defineProperty(ReadOnly.prototype, name, {
        get,
        configurable: true,
        enumerable: true
      });
      Object.defineProperty(Writable.prototype, name, {
        get,
        set(value) {
          getValues(this)[index] = +value;
          changed?.(this, index, +value);
        },
        configurable: true,
        enumerable: true
      });
    }
  }
  function change(matrix, result) {
    return setMatrix(matrix, getMatrix(result));
  }
  function operate(matrix, other) {
    return setMatrix(new DOMMatrix(), multiply(getMatrix(matrix), other));
  }
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
  function get2D([a, b, c, d, e, f]) {
    const m = [...identity];
    [m[0], m[1], m[4], m[5], m[12], m[13]] = [a, b, c, d, e, f];
    return [m, true];
  }
  function translation(x, y, z) {
    const m = [...identity];
    [m[12], m[13], m[14]] = [x, y, z];
    return [m, z === 0];
  }
  function scaling(x, y, z) {
    const m = [...identity];
    [m[0], m[5], m[10]] = [x, y, z];
    return [m, z === 1];
  }
  function scalingAround(sx, sy, sz, ox, oy, oz) {
    return multiply(
      multiply(translation(ox, oy, oz), scaling(sx, sy, sz)),
      translation(-ox, -oy, -oz)
    );
  }
  function rotationXYZ(rx, ry, rz) {
    const [x, y, z] = ry === void 0 && rz === void 0 ? [0, 0, +rx] : [+rx, +(ry ?? 0), +(rz ?? 0)];
    let result = rotation(0, 0, 1, z);
    if (y !== 0) result = multiply(result, rotation(0, 1, 0, y));
    if (x !== 0) result = multiply(result, rotation(1, 0, 0, x));
    return result;
  }
  function rotationFromVector(x, y) {
    return rotation(
      0,
      0,
      1,
      x === 0 && y === 0 ? 0 : Math.atan2(y, x) * 180 / Math.PI
    );
  }
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
  function toRadians(degrees) {
    return degrees * Math.PI / 180;
  }
  function getDictionary(value, name) {
    if (value == null) return null;
    if (typeof value !== "object" && typeof value !== "function") {
      throw new TypeError(`Invalid ${name}: '${value}'.`);
    }
    return value;
  }
  function getPointInit(point) {
    const {
      x = 0,
      y = 0,
      z = 0,
      w = 1
    } = getDictionary(point, "DOMPointInit") ?? {};
    return { x: +x, y: +y, z: +z, w: +w };
  }
  function getRectInit(rect) {
    const {
      x = 0,
      y = 0,
      width = 0,
      height = 0
    } = getDictionary(rect, "DOMRectInit") ?? {};
    return { x: +x, y: +y, width: +width, height: +height };
  }
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
  function getMatrixFromList(list) {
    const values = Array.from(list ?? [], Number);
    if (values.length === 6) return get2D(values);
    if (values.length === 16) return [values, false];
    throw new TypeError("Invalid matrix: it takes 6 or 16 numbers.");
  }
  function fromArray(Class, array) {
    if (!ArrayBuffer.isView(array)) {
      throw new TypeError(`Invalid array: '${array}', not a typed array.`);
    }
    return setMatrix(new Class(), getMatrixFromList(array));
  }

  // src/index.js
  var { print } = globalThis.__scriptographer;
  delete globalThis.__scriptographer;
  var console2 = createConsole(print);
  setGlobals({ console: console2, print: console2.log });
  if (!globalThis.EventTarget) {
    setGlobals(events_exports);
  }
  if (!globalThis.DOMMatrix) {
    setGlobals({ ...geometry_exports, WebKitCSSMatrix: DOMMatrix });
  }
  setupFileDialogs(globalThis.Dialog ??= {});
  function setGlobals(values) {
    for (const [name, value] of Object.entries(values)) {
      Object.defineProperty(globalThis, name, {
        value,
        writable: true,
        configurable: true
      });
    }
  }
})();
