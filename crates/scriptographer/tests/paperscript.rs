#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! The same tests for every engine, through [`Runtime`].

use serde_json::Value;
use vectorcraft_scriptographer::{Engine, Runtime, Setup};

fn runs_paperscript_with_console_and_paper_globals<E: Engine>() {
    let mut runtime = Runtime::<E>::new().unwrap();
    let scope = runtime.create_scope(800.0, 600.0).unwrap();
    runtime
        .execute(
            &scope,
            "console.log('Hello World')\n\
             new Path.Circle({ center: view.bounds.center, radius: 100, fillColor: 'red' })",
        )
        .unwrap();
    assert_eq!(runtime.take_output(), ["Hello World"]);

    let project: Value = serde_json::from_str(&runtime.export_json(&scope).unwrap()).unwrap();
    assert_eq!(project[0][0], "Layer");
    let path = &project[0][1]["children"][0];
    assert_eq!(path[0], "Path");
    let path = &path[1];
    assert_eq!(path["fillColor"], serde_json::json!([1, 0, 0]));
    assert_eq!(path["closed"], true);

    // A circle of radius 100 around the view's centre (400, 300), drawn with four segments at the
    // left, top, right and bottom, with handles of 100 · 4(√2 − 1)/3 along the tangents. Each
    // segment is `[[x, y], [inX, inY], [outX, outY]]`.
    let k = 100.0 * 4.0 * (2f64.sqrt() - 1.0) / 3.0;
    let expected = [([300.0, 300.0], [0.0, k]), ([400.0, 200.0], [-k, 0.0]), ([500.0, 300.0], [0.0, -k]), ([400.0, 400.0], [k, 0.0])];
    let segments = path["segments"].as_array().unwrap();
    assert_eq!(segments.len(), 4);
    let number = |v: &Value| v.as_f64().unwrap();
    for (segment, (point, handle_in)) in segments.iter().zip(expected) {
        assert!((number(&segment[0][0]) - point[0]).abs() < 1e-9, "{segment}");
        assert!((number(&segment[0][1]) - point[1]).abs() < 1e-9, "{segment}");
        assert!((number(&segment[1][0]) - handle_in[0]).abs() < 1e-5, "{segment}");
        assert!((number(&segment[1][1]) - handle_in[1]).abs() < 1e-5, "{segment}");
        assert!((number(&segment[2][0]) + handle_in[0]).abs() < 1e-5, "{segment}");
        assert!((number(&segment[2][1]) + handle_in[1]).abs() < 1e-5, "{segment}");
    }
}

fn paperscript_operators_work_on_points<E: Engine>() {
    let mut runtime = Runtime::<E>::new().unwrap();
    let scope = runtime.create_scope(800.0, 600.0).unwrap();
    runtime.execute(&scope, "console.log(new Point(10, 20) + [1, 2], new Point(10, 20) * 2)").unwrap();
    assert_eq!(runtime.take_output(), ["{ x: 11, y: 22 } { x: 20, y: 40 }"]);
}

fn script_errors_are_returned<E: Engine>() {
    let mut runtime = Runtime::<E>::new().unwrap();
    let scope = runtime.create_scope(800.0, 600.0).unwrap();
    let error = runtime.execute(&scope, "undefinedFunction()").unwrap_err();
    assert!(error.to_string().contains("undefinedFunction"), "{error}");
}

fn host_function_errors_are_thrown_in_scripts<E: Engine>() {
    let mut runtime = Runtime::<E>::new().unwrap();
    let scope = runtime.create_scope(800.0, 600.0).unwrap();
    runtime.execute(&scope, "try { setInterval(42) } catch (e) { console.log(e.message) }").unwrap();
    assert_eq!(runtime.take_output(), ["setInterval: the callback is not a function"]);
}

fn intervals_run_when_due<E: Engine>() {
    let mut runtime = Runtime::<E>::new().unwrap();
    let scope = runtime.create_scope(800.0, 600.0).unwrap();
    runtime
        .execute(
            &scope,
            "var count = 0\n\
             var id = setInterval(function() { console.log(++count); if (count == 3) clearInterval(id) }, 10)",
        )
        .unwrap();
    for now in [5.0, 10.0, 15.0, 20.0, 45.0, 50.0, 100.0] {
        runtime.run_timers(now).unwrap();
    }
    // Due at 10, 20 and 30; the one missed until 45 runs once, then the interval is cleared.
    assert_eq!(runtime.take_output(), ["1", "2", "3"]);
}

fn timeouts_run_once<E: Engine>() {
    let mut runtime = Runtime::<E>::new().unwrap();
    let scope = runtime.create_scope(800.0, 600.0).unwrap();
    runtime
        .execute(
            &scope,
            "setTimeout(function() { print('soon') }, 10)\n\
             setTimeout(function() { console.warn('later') }, 100)\n\
             var cleared = setTimeout(function() { print('never') }, 20)\n\
             clearTimeout(cleared)",
        )
        .unwrap();
    for now in [5.0, 10.0, 15.0, 50.0] {
        runtime.run_timers(now).unwrap();
    }
    assert_eq!(runtime.take_output(), ["soon"], "the later one waits, even past its own delay from now");
    assert!(runtime.is_running());
    runtime.run_timers(100.0).unwrap();
    runtime.run_timers(200.0).unwrap();
    assert_eq!(runtime.take_output(), ["later"]);
    assert!(!runtime.is_running(), "nothing is left once the timeouts ran");
}

fn animation_frames_run_once_a_frame<E: Engine>() {
    let mut runtime = Runtime::<E>::new().unwrap();
    let scope = runtime.create_scope(800.0, 600.0).unwrap();
    runtime
        .execute(
            &scope,
            "let frames = 0\n\
             function frame(time) { print('frame', time); if (++frames < 2) requestAnimationFrame(frame) }\n\
             requestAnimationFrame(frame)\n\
             cancelAnimationFrame(requestAnimationFrame(() => print('never')))",
        )
        .unwrap();
    assert!(runtime.is_running(), "a frame is waiting");
    runtime.run_timers(16.0).unwrap();
    assert_eq!(runtime.take_output(), ["frame 16"], "the one it asks for runs on the next frame");
    runtime.run_timers(32.0).unwrap();
    assert_eq!(runtime.take_output(), ["frame 32"]);
    assert!(!runtime.is_running());
}

fn local_files_read_write_and_list<E: Engine>(name: &str) {
    let folder = std::env::temp_dir().join(format!("scriptographer-files-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&folder);
    std::fs::create_dir_all(&folder).unwrap();
    let mut runtime = Runtime::<E>::new().unwrap();
    runtime.set_script(&Setup { file: Some(folder.join("script.js").to_string_lossy().into_owned()), ..Setup::default() }).unwrap();
    let scope = runtime.create_scope(800.0, 600.0).unwrap();
    let script = "const notes = new LocalFile('notes.txt')\n\
         console.log(notes instanceof File, notes instanceof Blob, await notes.exists(), notes.name, notes.extension, notes.type, notes.parent.path == script.directory.path)\n\
         try { await notes.text() } catch (e) { console.log(e.name) }\n\
         await notes.write('one\\n')\n\
         await notes.append(new Blob(['two']))\n\
         console.log(await notes.exists(), notes.size, await notes.text(), (await notes.bytes()).length, (await notes.arrayBuffer()).byteLength, await notes.slice(-3).text())\n\
         console.log(await new Response(notes).text() == await notes.text(), (await new Response(new FormData()).text()).length >= 0)\n\
         const sub = await script.directory.file('sub')\n\
         await sub.makeDirectory()\n\
         const copy = await notes.copyTo(new LocalFile(script.directory, 'sub', 'copy.txt'))\n\
         console.log(sub.isDirectory, copy.path == sub.path + '/copy.txt', (await script.directory.list()).map(f => f.name).join())\n\
         await copy.rename('moved.txt')\n\
         console.log(copy.name, (await sub.list()).map(f => f.name).join())\n\
         try { await sub.remove() } catch (e) { console.log(e.name) }\n\
         await sub.remove({ recursive: true })\n\
         console.log(await sub.exists(), String(notes) == notes.path, script.file.name, Object.prototype.toString.call(notes))";
    runtime.execute(&scope, &format!("(async () => {{ try {{ {script} }} catch (e) {{ console.log('error', e.name, e.message) }} }})()")).unwrap();
    settle(&mut runtime);
    assert_eq!(
        runtime.take_output(),
        [
            "true true false notes.txt txt text/plain true",
            "NotFoundError",
            "true 7 one\ntwo 7 7 two",
            "true true",
            "true true notes.txt,sub",
            "moved.txt moved.txt",
            "InvalidModificationError",
            "false true script.js [object LocalFile]"
        ]
    );
    std::fs::remove_dir_all(&folder).unwrap();
}

fn scripts_await_at_the_top_level<E: Engine>() {
    let mut runtime = Runtime::<E>::new().unwrap();
    let scope = runtime.create_scope(800.0, 600.0).unwrap();
    runtime.execute(&scope, "const two = await Promise.resolve(2)\nconsole.log(two)\nfunction onStop() { console.log('stopped') }").unwrap();
    runtime.run_timers(0.0).unwrap();
    assert!(runtime.call_handler("onStop").unwrap(), "its handlers, once it has finished");
    assert_eq!(runtime.take_output(), ["2", "stopped"]);
    let failure = runtime.execute(&scope, "await null\nthrow new Error('now')").unwrap_err();
    assert!(failure.to_string().contains("now"), "a failure before it returns is its error: {failure}");
    runtime.execute(&scope, "await new Promise(resolve => setTimeout(resolve, 0))\nthrow new Error('later')").unwrap();
    let failure = runtime.run_timers(1.0).unwrap_err();
    assert!(failure.to_string().contains("later"), "a failure in a frame is the frame's error: {failure}");
}

/// V8's `WebAssembly`, its Promises too (Boa has none; browsers their own).
#[cfg(all(feature = "v8", not(target_arch = "wasm32")))]
#[test]
fn scripts_run_webassembly() {
    let mut runtime = Runtime::<vectorcraft_scriptographer::V8>::new().unwrap();
    let scope = runtime.create_scope(800.0, 600.0).unwrap();
    // A module exporting `answer()`, which returns 42.
    let script = "const bytes = new Uint8Array([0, 97, 115, 109, 1, 0, 0, 0, 1, 5, 1, 96, 0, 1, 127, 3, 2, 1, 0, 7, 10, 1, 6, 97, 110, 115, 119, 101, 114, 0, 0, 10, 6, 1, 4, 0, 65, 42, 11])\n\
         console.log(typeof WebAssembly)\n\
         const module = new WebAssembly.Module(bytes)\n\
         console.log(new WebAssembly.Instance(module).exports.answer())\n\
         const { instance } = await WebAssembly.instantiate(bytes)\n\
         console.log(instance.exports.answer() + 1)";
    runtime.execute(&scope, &format!("(async () => {{ try {{ {script} }} catch (e) {{ console.log('error', e.name, e.message) }} }})()")).unwrap();
    settle(&mut runtime);
    assert_eq!(runtime.take_output(), ["object", "42", "43"]);
}

fn host_classes_can_be_subclassed<E: Engine>() {
    let mut runtime = Runtime::<E>::new().unwrap();
    let scope = runtime.create_scope(800.0, 600.0).unwrap();
    let script = "const classes = [[Blob, [['a']]], [File, [['a'], 'a.txt']], [LocalFile, ['a.txt']], [Headers, []], [Request, ['https://example.com/']],\n\
         [Response, ['x']], [FormData, []], [AbortController, []], [OffscreenCanvas, [2, 2]], [ImageData, [1, 1]], [Path2D, []], [DOMMatrix, []],\n\
         [DOMPoint, [1, 2]], [DOMRect, [0, 0, 1, 1]], [DOMQuad, []], [URL, ['https://example.com/a']], [URLSearchParams, ['a=1']],\n\
         [TextEncoder, []], [TextDecoder, []], [DOMException, ['m', 'NotFoundError']], [EventTarget, []], [Event, ['e']]]\n\
         const failed = classes.filter(([Base, args]) => {\n\
             class Sub extends Base { mine() { return 'mine' } }\n\
             const made = new Sub(...args)\n\
             return !(made instanceof Sub && made instanceof Base && made.mine() == 'mine')\n\
         }).map(([Base]) => Base.name)\n\
         console.log(failed.join() || 'all')\n\
         class Text extends Blob { get length() { return this.size } }\n\
         class Address extends URL { get file() { return this.pathname.split('/').pop() } }\n\
         class Spot extends DOMPoint { get sum() { return this.x + this.y } }\n\
         class Missing extends DOMException {}\n\
         console.log(new Text(['abc']).length, new Address('https://a.b/c/d.txt').file, new Spot(1, 2).sum, new Missing('gone', 'NotFoundError').code)\n\
         try { Blob(['a']) } catch (e) { console.log(e.name) }";
    runtime.execute(&scope, script).unwrap();
    assert_eq!(runtime.take_output(), ["all", "3 d.txt 3 8", "TypeError"]);
}

fn web_imports_are_kept_for_the_next_run<E: Engine>(name: &str) {
    let address = serve(2);
    let cache = std::env::temp_dir().join(format!("scriptographer-cache-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&cache);
    let run = || {
        let mut runtime = Runtime::<E>::new().unwrap();
        runtime.set_script(&Setup { cache: Some(cache.clone()), ..Setup::default() }).unwrap();
        let scope = runtime.create_scope(800.0, 600.0).unwrap();
        runtime.execute(&scope, &format!("import {{ kept }} from '{address}/kept.js'\nconsole.log(kept)")).unwrap();
        runtime.take_output().into_iter().filter(|line| !line.starts_with("import ")).collect::<Vec<_>>()
    };
    assert_eq!(run(), ["once"], "downloaded, and kept");
    assert_eq!(run(), ["once"], "the copy, which the server says is still the one");
    assert_eq!(run(), ["once"], "the copy, with the server gone");
    std::fs::remove_dir_all(&cache).unwrap();
}

fn files_are_blobs_with_a_name<E: Engine>() {
    let mut runtime = Runtime::<E>::new().unwrap();
    let scope = runtime.create_scope(800.0, 600.0).unwrap();
    let script = "const file = new File(['<svg/>'], 'dot.svg', { type: 'Image/SVG+XML', lastModified: 42 })\n\
         console.log(file instanceof Blob, File.prototype instanceof Blob, file.name, file.type, file.size, file.lastModified, await file.text())\n\
         try { File(['a'], 'a.txt') } catch (e) { console.log(e.name) }\n\
         try { new File(['a']) } catch (e) { console.log(e.name) }";
    runtime.execute(&scope, &format!("(async () => {{ try {{ {script} }} catch (e) {{ console.log('error', e.name, e.message) }} }})()")).unwrap();
    settle(&mut runtime);
    assert_eq!(runtime.take_output(), ["true true dot.svg image/svg+xml 6 42 <svg/>", "TypeError", "TypeError"]);
}

fn scripts_have_a_dom<E: Engine>() {
    let mut runtime = Runtime::<E>::new().unwrap();
    let scope = runtime.create_scope(800.0, 600.0).unwrap();
    runtime
        .execute(
            &scope,
            "var svg = document.createElementNS('http://www.w3.org/2000/svg', 'svg')\n\
             document.body.appendChild(svg)\n\
             svg.setAttribute('width', '20')\n\
             var clicks = 0\n\
             svg.addEventListener('click', function() { clicks++ })\n\
             svg.dispatchEvent(new window.Event('click'))\n\
             console.log(document.querySelector('svg') === svg, svg.namespaceURI, clicks, document.body.innerHTML)\n\
             console.log(atob(btoa('hé')), atob('aGk'))",
        )
        .unwrap();
    assert_eq!(runtime.take_output(), ["true http://www.w3.org/2000/svg 1 <svg width=\"20\" />", "hé hi"]);
}

fn scripts_use_todays_javascript<E: Engine>() {
    let mut runtime = Runtime::<E>::new().unwrap();
    let scope = runtime.create_scope(800.0, 600.0).unwrap();
    runtime
        .execute(
            &scope,
            "const add = (a, b) => a + b\n\
             class Dot { constructor(at) { this.at = at } }\n\
             let [x, y] = [1, 2]\n\
             const dot = new Dot(new Point(x, y) * 2)\n\
             console.log(`${add(1, 2)}`, dot.at + [x, y], dot?.missing?.value ?? 'none')",
        )
        .unwrap();
    assert_eq!(runtime.take_output(), ["3 { x: 3, y: 6 } none"]);
}

fn promises_resolve<E: Engine>() {
    let mut runtime = Runtime::<E>::new().unwrap();
    let scope = runtime.create_scope(800.0, 600.0).unwrap();
    runtime.execute(&scope, "Promise.resolve(2).then(n => console.log('then', n))\nconsole.log('first')").unwrap();
    assert_eq!(runtime.take_output(), ["first", "then 2"], "after the script, before the host goes on");
}

fn scripts_have_the_browsers_everyday_apis<E: Engine>() {
    let mut runtime = Runtime::<E>::new().unwrap();
    let scope = runtime.create_scope(800.0, 600.0).unwrap();
    runtime
        .execute(
            &scope,
            "const bytes = new TextEncoder().encode('hé')\n\
             console.log(bytes.length, new TextDecoder().decode(bytes), new TextDecoder('utf-8').decode(bytes.buffer))\n\
             const random = crypto.getRandomValues(new Uint32Array(4))\n\
             console.log(random.length, /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/.test(crypto.randomUUID()))\n\
             const t = performance.now()\n\
             console.log(t >= 0 && performance.now() >= t)\n\
             queueMicrotask(() => console.log('microtask'))\n\
             const kept = ['EventTarget', 'OffscreenCanvas', 'URL'].map(name => Object.getOwnPropertyDescriptor(globalThis, name))\n\
             console.log(kept.every(global => global.writable && !global.enumerable), Object.prototype.toString.call(new ImageData(1, 1)))\n\
             console.log('script')",
        )
        .unwrap();
    assert_eq!(
        runtime.take_output(),
        ["3 hé hé", "4 true", "true", "true [object ImageData]", "script", "microtask"],
        "the globals are the realm's kind: values scripts can set, not listed"
    );
}

/// Encoding as the Encoding standard has it, and base64 as browsers read it.
fn text_encodes_as_the_standard_has_it<E: Engine>() {
    let mut runtime = Runtime::<E>::new().unwrap();
    let scope = runtime.create_scope(800.0, 600.0).unwrap();
    runtime
        .execute(
            &scope,
            "const thrown = f => { try { return f() } catch (e) { return e.name } }\n\
             const streamed = new TextDecoder()\n\
             console.log(streamed.decode(new Uint8Array([0xef, 0xbb, 0xbf, 0x68, 0xc3]), { stream: true }) + streamed.decode(new Uint8Array([0xa9])))\n\
             console.log(new TextDecoder('utf-8', { ignoreBOM: true }).decode(new Uint8Array([0xef, 0xbb, 0xbf])).length, new TextDecoder().decode(new Uint8Array([0xff])))\n\
             console.log(thrown(() => new TextDecoder('utf-8', { fatal: true }).decode(new Uint8Array([0xff]))), thrown(() => new TextDecoder('latin1')))\n\
             const into = new Uint8Array(3)\n\
             console.log(JSON.stringify(new TextEncoder().encodeInto('aé€', into)), into.join())\n\
             console.log(atob(' aG k= '), btoa('ab'), thrown(() => atob('a')), thrown(() => btoa('€')))\n\
             console.log(thrown(() => crypto.getRandomValues(new Float32Array(1))))",
        )
        .unwrap();
    assert_eq!(
        runtime.take_output(),
        [
            "hé",
            "1 \u{fffd}",
            "TypeError RangeError",
            "{\"read\":2,\"written\":3} 97,195,169",
            "hi YWI= InvalidCharacterError InvalidCharacterError",
            "TypeMismatchError"
        ]
    );
}

fn scripts_import_modules<E: Engine>(name: &str) {
    let folder = std::env::temp_dir().join(format!("scriptographer-imports-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&folder);
    std::fs::create_dir_all(folder.join("lib")).unwrap();
    let write = |file: &str, code: &str| std::fs::write(folder.join(file), code).unwrap();
    write(
        "shapes.js",
        "console.log('shapes runs')\nexport const size = 10\nexport function twice(point) { return point * 2 }\nexport default 'shapes'",
    );
    write("lib/util.js", "import { size, twice } from '../shapes.js'\nexport const big = twice(new Point(size, 1))");
    write("a.js", "import { b } from './b.js'\nexport const a = 1");
    write("b.js", "import { a } from './a.js'\nexport const b = 2");
    let mut runtime = Runtime::<E>::new().unwrap();
    runtime.set_script(&Setup { file: Some(folder.join("script.js").to_string_lossy().into_owned()), ..Setup::default() }).unwrap();
    let scope = runtime.create_scope(800.0, 600.0).unwrap();
    runtime
        .execute(
            &scope,
            "import shapes, { size } from './shapes.js'\n\
             import * as util from './lib/util.js'\n\
             console.log(shapes, size, util.big)",
        )
        .unwrap();
    assert_eq!(runtime.take_output(), ["shapes runs", "shapes 10 { x: 20, y: 2 }"], "shapes.js ran once, for both importers");
    runtime.execute(&scope, "import('./shapes.js').then(m => console.log('dynamic', m.default, m.size))").unwrap();
    assert_eq!(runtime.take_output(), ["dynamic shapes 10"], "the same module, already loaded");
    runtime.execute(&scope, "import('./missing.js').catch(e => console.log('rejected'))").unwrap();
    assert_eq!(runtime.take_output(), ["rejected"]);
    let error = runtime.execute(&scope, "import { a } from './a.js'").unwrap_err();
    assert!(error.to_string().contains("imports itself"), "{error}");
    let error = runtime.execute(&scope, "import x from './missing.js'").unwrap_err();
    assert!(error.to_string().contains("missing.js"), "{error}");
    std::fs::remove_dir_all(&folder).unwrap();
}

/// A local HTTP server for `count` requests: `/data.json` answers JSON, `/echo` answers the
/// request's method, content type and body. Returns its address.
fn serve(count: usize) -> String {
    use std::io::{BufRead, BufReader, Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = format!("http://{}", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for stream in listener.incoming().take(count) {
            let mut stream = stream.unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            let (method, path) = {
                let mut parts = line.split_whitespace();
                (parts.next().unwrap().to_owned(), parts.next().unwrap().to_owned())
            };
            let (mut length, mut kind, mut tag) = (0, String::new(), String::new());
            loop {
                let mut header = String::new();
                reader.read_line(&mut header).unwrap();
                if header.trim().is_empty() {
                    break;
                }
                let (name, value) = header.split_once(':').unwrap();
                match name.to_ascii_lowercase().as_str() {
                    "content-length" => length = value.trim().parse().unwrap(),
                    "content-type" => kind = value.trim().to_owned(),
                    "if-none-match" => tag = value.trim().to_owned(),
                    _ => {}
                }
            }
            let mut body = vec![0; length];
            reader.read_exact(&mut body).unwrap();
            let (status, content_type, answer) = match path.as_str() {
                "/data.json" => ("200 OK", "application/json", br#"{"a": 1, "list": [1, 2]}"#.to_vec()),
                "/lib/mod.js" => ("200 OK", "text/javascript", b"import { b } from '../dep.js'\nexport const a = b + new Point(1, 1)".to_vec()),
                "/dep.js" => ("200 OK", "text/javascript", b"export const b = new Point(40, 41)".to_vec()),
                "/echo" => ("201 Created", "text/plain", [format!("{method} {kind}\n").into_bytes(), body].concat()),
                // Not modified when asked for the copy it gave, whose `ETag` the head names.
                "/kept.js" if tag == "\"one\"" => ("304 Not Modified", "text/javascript", Vec::new()),
                "/kept.js" => ("200 OK", "text/javascript", b"export const kept = 'once'".to_vec()),
                _ => ("404 Not Found", "text/plain", b"missing".to_vec()),
            };
            let head = format!(
                "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nX-Test: one\r\nETag: \"one\"\r\nConnection: close\r\n\r\n",
                answer.len()
            );
            stream.write_all(head.as_bytes()).unwrap();
            stream.write_all(&answer).unwrap();
        }
    });
    address
}

/// Runs the runtime's timers until nothing is left, as frames would, for at most two seconds.
fn settle<E: Engine>(runtime: &mut Runtime<E>) {
    let start = std::time::Instant::now();
    while runtime.is_running() && start.elapsed() < std::time::Duration::from_secs(2) {
        std::thread::sleep(std::time::Duration::from_millis(5));
        runtime.run_timers(start.elapsed().as_secs_f64() * 1000.0).unwrap();
    }
}

fn scripts_fetch<E: Engine>() {
    let address = serve(4);
    let mut runtime = Runtime::<E>::new().unwrap();
    let scope = runtime.create_scope(800.0, 600.0).unwrap();
    let script = format!(
        "const base = '{address}'\n\
         const response = await fetch(base + '/data.json')\n\
         const data = await response.json()\n\
         console.log(response.status, response.ok, response.headers.get('content-type'), response.headers.get('x-test'), data.a, data.list.length)\n\
         const missing = await fetch(base + '/nothing')\n\
         console.log(missing.status, missing.ok, await missing.text())\n\
         const form = new FormData()\n\
         form.append('name', 'dot')\n\
         form.append('file', new Blob(['<svg/>'], {{ type: 'image/svg+xml' }}), 'dot.svg')\n\
         const echo = await (await fetch(base + '/echo', {{ method: 'POST', body: form }})).text()\n\
         console.log(echo.startsWith('POST multipart/form-data; boundary='), echo.includes('name=\"name\"\\r\\n\\r\\ndot'), echo.includes('filename=\"dot.svg\"\\r\\nContent-Type: image/svg+xml\\r\\n\\r\\n<svg/>'))\n\
         const sent = await fetch(new Request(base + '/echo', {{ method: 'PUT', body: JSON.stringify({{ x: 1 }}), headers: {{ 'Content-Type': 'application/json' }} }}))\n\
         console.log(sent.status, await sent.text())"
    );
    runtime.execute(&scope, &format!("(async () => {{ try {{ {script} }} catch (e) {{ console.log('error', e.name, e.message) }} }})()")).unwrap();
    settle(&mut runtime);
    assert_eq!(
        runtime.take_output(),
        ["200 true application/json one 1 2", "404 false missing", "true true true", "201 PUT application/json\n{\"x\":1}"]
    );
}

fn scripts_import_from_the_web<E: Engine>() {
    let address = serve(2);
    let mut runtime = Runtime::<E>::new().unwrap();
    let scope = runtime.create_scope(800.0, 600.0).unwrap();
    runtime.execute(&scope, &format!("import {{ a }} from '{address}/lib/mod.js'\nconsole.log(a)")).unwrap();
    assert_eq!(runtime.take_output(), [format!("import {address}/lib/mod.js"), format!("import {address}/dep.js"), "{ x: 41, y: 42 }".to_owned()]);
}

fn scripts_have_urls<E: Engine>() {
    let mut runtime = Runtime::<E>::new().unwrap();
    let scope = runtime.create_scope(800.0, 600.0).unwrap();
    runtime
        .execute(
            &scope,
            "const url = new URL('../b/c?x=1&y=two#top', 'https://user:pw@example.com:8080/a/index.html')\n\
             console.log(url.href, url.origin, url.host, url.pathname, url.search, url.hash, url.username)\n\
             url.searchParams.append('z', 'a b')\n\
             url.hash = ''\n\
             console.log(url.toString(), url.searchParams.get('y'), url.searchParams.size)\n\
             url.search = '?q=1'\n\
             console.log(url.searchParams.get('q'), url.searchParams.has('x'), JSON.stringify({ url }))\n\
             const params = new URLSearchParams({ b: 2, a: 1 })\n\
             params.sort()\n\
             console.log(String(params), [...new URLSearchParams('?k=v&k=w').getAll('k')], URL.canParse('nope'), URL.canParse('/x', 'http://h'))\n\
             try { new URL('nope') } catch (e) { console.log('invalid') }",
        )
        .unwrap();
    assert_eq!(
        runtime.take_output(),
        [
            "https://user:pw@example.com:8080/b/c?x=1&y=two#top https://example.com:8080 example.com:8080 /b/c ?x=1&y=two #top user",
            "https://user:pw@example.com:8080/b/c?x=1&y=two&z=a+b two 3",
            "1 false {\"url\":\"https://user:pw@example.com:8080/b/c?q=1\"}",
            "a=1&b=2 ['v', 'w'] false true",
            "invalid"
        ]
    );
}

fn scripts_measure_text<E: Engine>() {
    let mut runtime = Runtime::<E>::new().unwrap();
    let scope = runtime.create_scope(800.0, 600.0).unwrap();
    runtime
        .execute(
            &scope,
            "const ctx = new OffscreenCanvas(1, 1).getContext('2d')\n\
             console.log(ctx.font)\n\
             ctx.font = '20px \"Source Sans 3\"'\n\
             const small = ctx.measureText('Hello')\n\
             ctx.font = 'bold 40px \"Source Sans 3\"'\n\
             const big = ctx.measureText('Hello')\n\
             console.log(small.width > 30, big.width > 2 * small.width, small.fontBoundingBoxAscent > small.actualBoundingBoxAscent, small.actualBoundingBoxAscent > 10)\n\
             console.log(ctx.measureText('Hello ').width > ctx.measureText('Hello').width, ctx.measureText('').width)\n\
             const text = new PointText({ point: [0, 0], content: 'Hello', fontFamily: 'Source Sans 3', fontSize: 20 })\n\
             console.log(Math.abs(text.bounds.width - small.width) < 0.01)",
        )
        .unwrap();
    assert_eq!(runtime.take_output(), ["10px sans-serif", "true true true true", "true 0", "true"], "bold is wider than twice the size alone");
}

fn canvases_have_pixels<E: Engine>() {
    let mut runtime = Runtime::<E>::new().unwrap();
    let scope = runtime.create_scope(800.0, 600.0).unwrap();
    runtime
        .execute(
            &scope,
            "const canvas = new OffscreenCanvas(4, 3)\n\
             const ctx = canvas.getContext('2d')\n\
             ctx.fillStyle = 'rgba(255, 0, 0, 0.5)'\n\
             ctx.fillRect(1, 1, 2, 5)\n\
             const data = ctx.getImageData(0, 0, 4, 3).data\n\
             console.log(data.length, [...data.slice(0, 4)], [...data.slice(20, 24)], data instanceof Uint8ClampedArray)\n\
             const image = new ImageData(new Uint8ClampedArray([0, 255, 0, 255]), 1)\n\
             ctx.putImageData(image, 3, 0)\n\
             ctx.clearRect(1, 1, 1, 1)\n\
             console.log([...ctx.getImageData(3, 0, 1, 1).data], [...ctx.getImageData(1, 1, 1, 1).data], [...ctx.getImageData(-1, -1, 1, 1).data])\n\
             canvas.width = 2\n\
             console.log(canvas.width, canvas.height, [...ctx.getImageData(1, 1, 1, 1).data])\n\
             const raster = new Raster(new Size(3, 2))\n\
             raster.setPixel(1, 1, new Color(0, 0, 1))\n\
             console.log(raster.getPixel(1, 1).toCSS(true), raster.getPixel(0, 0).alpha)\n\
             console.log(Object.keys(canvas).length, Object.keys(new Path2D()).length, ctx.fillStyle)\n\
             canvas.convertToBlob().then(blob => blob.bytes()).then(png => console.log(png[1], png[2], png[3]))",
        )
        .unwrap();
    assert_eq!(
        runtime.take_output(),
        [
            "48 [0, 0, 0, 0] [255, 0, 0, 128] true",
            "[0, 255, 0, 255] [0, 0, 0, 0] [0, 0, 0, 0]",
            "2 3 [0, 0, 0, 0]",
            "#0000ff 0",
            "0 0 #000000",
            "80 78 71"
        ],
        "a half red stays half red, cleared and outside is transparent, resizing clears, PNG's signature"
    );
}

fn canvases_draw<E: Engine>() {
    let mut runtime = Runtime::<E>::new().unwrap();
    let scope = runtime.create_scope(800.0, 600.0).unwrap();
    runtime
        .execute(
            &scope,
            "const canvas = new OffscreenCanvas(40, 40)\n\
             const ctx = canvas.getContext('2d')\n\
             const at = (x, y) => [...ctx.getImageData(x, y, 1, 1).data].join()\n\
             ctx.save()\n\
             ctx.translate(10, 10)\n\
             ctx.scale(2, 2)\n\
             ctx.fillStyle = 'red'\n\
             ctx.beginPath(); ctx.moveTo(0, 0); ctx.lineTo(5, 0); ctx.lineTo(5, 5); ctx.lineTo(0, 5); ctx.closePath(); ctx.fill()\n\
             ctx.lineWidth = 2\n\
             ctx.strokeStyle = 'blue'\n\
             ctx.beginPath(); ctx.moveTo(0, 10); ctx.lineTo(10, 10); ctx.stroke()\n\
             ctx.restore()\n\
             console.log(at(15, 15), at(25, 25), at(15, 29), at(15, 32), ctx.fillStyle, ctx.getTransform().a)\n\
             ctx.clearRect(0, 0, 40, 40)\n\
             ctx.beginPath(); ctx.arc(20, 20, 10, 0, Math.PI * 2); ctx.fillStyle = '#00ff00'; ctx.fill()\n\
             console.log(at(20, 20), at(2, 2), ctx.isPointInPath(20, 20), ctx.isPointInPath(1, 1))\n\
             ctx.save(); ctx.beginPath(); ctx.rect(0, 0, 20, 40); ctx.clip()\n\
             ctx.fillStyle = 'blue'; ctx.fillRect(0, 0, 40, 40); ctx.restore()\n\
             console.log(at(5, 5), at(35, 5))\n\
             const gradient = ctx.createLinearGradient(0, 0, 40, 0)\n\
             gradient.addColorStop(0, 'red'); gradient.addColorStop(1, 'blue')\n\
             ctx.fillStyle = gradient; ctx.fillRect(0, 0, 40, 40)\n\
             const [left, right] = [ctx.getImageData(1, 1, 1, 1).data, ctx.getImageData(38, 1, 1, 1).data]\n\
             console.log(left[0] > 200 && left[2] < 50, right[2] > 200 && right[0] < 50, ctx.fillStyle === gradient)\n\
             const small = new OffscreenCanvas(2, 2)\n\
             small.getContext('2d').fillRect(0, 0, 1, 1)\n\
             ctx.imageSmoothingEnabled = false\n\
             ctx.drawImage(small, 0, 0, 20, 20)\n\
             console.log(at(5, 5), at(15, 15) === at(15, 5))\n\
             ctx.globalCompositeOperation = 'destination-out'\n\
             ctx.fillRect(0, 0, 10, 10)\n\
             ctx.globalCompositeOperation = 'source-over'\n\
             console.log(at(5, 5).endsWith(',0'))\n\
             ctx.clearRect(0, 0, 40, 40)\n\
             ctx.font = '30px \"Source Sans 3\"'; ctx.fillStyle = 'black'; ctx.fillText('H', 5, 35)\n\
             const ink = ctx.getImageData(0, 0, 40, 40).data.filter((v, i) => i % 4 === 3 && v > 128).length\n\
             console.log(ink > 50)\n\
             const raster = new Path.Circle({ center: [10, 10], radius: 10, fillColor: 'red', insert: false }).rasterize({ resolution: 72, insert: false })\n\
             console.log(raster.width, raster.getPixel(10, 10).toCSS(true), raster.getPixel(0, 0).alpha)",
        )
        .unwrap();
    assert_eq!(
        runtime.take_output(),
        [
            "255,0,0,255 0,0,0,0 0,0,255,255 0,0,0,0 #000000 1",
            "0,255,0,255 0,0,0,0 true false",
            "0,0,255,255 0,0,0,0",
            "true true true",
            "0,0,0,255 true",
            "true",
            "true",
            "20 #ff0000 0"
        ]
    );
}

fn fetch_classes_work_alone<E: Engine>() {
    let mut runtime = Runtime::<E>::new().unwrap();
    let scope = runtime.create_scope(800.0, 600.0).unwrap();
    runtime
        .execute(
            &scope,
            "const headers = new Headers({ 'Content-Type': 'text/plain', 'X-B': '1' })\n\
             headers.append('x-b', '2')\n\
             console.log([...headers].map(([n, v]) => n + '=' + v).join(' '), headers.has('content-type'))\n\
             const blob = new Blob(['ab', new Uint8Array([99]), new Blob(['d'])], { type: 'Text/Plain' })\n\
             blob.slice(1, -1).text().then(t => console.log(blob.size, blob.type, t))\n\
             const form = new FormData()\n\
             form.append('a', 1)\n\
             form.set('a', 2)\n\
             console.log([...form.entries()].join(), form.has('a'))\n\
             new Response('hi', { status: 202 }).text().then(t => console.log('response', t))\n\
             const controller = new AbortController()\n\
             controller.signal.addEventListener('abort', () => console.log('aborted', controller.signal.reason.name))\n\
             fetch('http://127.0.0.1:1/', { signal: controller.signal }).catch(e => console.log('rejected', e.name))\n\
             controller.abort()",
        )
        .unwrap();
    settle(&mut runtime);
    assert_eq!(
        runtime.take_output(),
        ["content-type=text/plain x-b=1, 2 true", "a,2 true", "aborted AbortError", "4 text/plain bc", "response hi", "rejected AbortError"]
    );
}

fn css_colors_resolve_without_a_canvas<E: Engine>() {
    let mut runtime = Runtime::<E>::new().unwrap();
    let scope = runtime.create_scope(800.0, 600.0).unwrap();
    runtime
        .execute(
            &scope,
            "['cornflowerblue', '#f80', 'rgb(0, 128, 255)', 'hsl(120, 100%, 50%)', 'nocolor'].forEach(function(css) {\n\
                 console.log(new Color(css).toCSS(true))\n\
             })",
        )
        .unwrap();
    // cornflowerblue is rgb(100, 149, 237) in CSS; an unknown name is black, as in browsers.
    assert_eq!(runtime.take_output(), ["#6495ed", "#ff8800", "#0080ff", "#00ff00", "#000000"]);
}

fn canvases_draw_shadows_and_filters<E: Engine>() {
    let mut runtime = Runtime::<E>::new().unwrap();
    let scope = runtime.create_scope(800.0, 600.0).unwrap();
    runtime
        .execute(
            &scope,
            "const canvas = new OffscreenCanvas(40, 40)\n\
             const ctx = canvas.getContext('2d')\n\
             const at = (x, y) => [...ctx.getImageData(x, y, 1, 1).data].join()\n\
             const alpha = (x, y) => ctx.getImageData(x, y, 1, 1).data[3]\n\
             ctx.save()\n\
             ctx.scale(2, 2)\n\
             ctx.shadowColor = 'blue'; ctx.shadowOffsetX = 15; ctx.shadowColor = 'nope'\n\
             ctx.fillStyle = 'red'; ctx.fillRect(2.5, 2.5, 5, 5)\n\
             console.log(at(10, 10), at(20, 10), at(30, 10), ctx.shadowColor)\n\
             ctx.restore()\n\
             console.log(ctx.shadowColor, ctx.shadowOffsetX)\n\
             ctx.clearRect(0, 0, 40, 40)\n\
             ctx.shadowColor = 'black'; ctx.shadowBlur = 8; ctx.fillRect(15, 15, 10, 10)\n\
             console.log(alpha(20, 20), alpha(13, 20) > 0 && alpha(13, 20) < 255, alpha(0, 0))\n\
             ctx.shadowColor = 'transparent'\n\
             ctx.clearRect(0, 0, 40, 40)\n\
             ctx.filter = 'blur(2px)'; ctx.filter = 'bogus(1px)'; ctx.fillRect(10, 10, 20, 20)\n\
             console.log(ctx.filter, alpha(20, 20), alpha(10, 20) > 0 && alpha(10, 20) < 255, alpha(7, 20) > 0)\n\
             ctx.clearRect(0, 0, 40, 40)\n\
             ctx.filter = 'drop-shadow(10px 0 lime) brightness(50%)'; ctx.fillStyle = 'red'; ctx.fillRect(0, 0, 10, 10)\n\
             console.log(at(5, 5), at(15, 5), at(25, 5))\n\
             ctx.filter = 'none'\n\
             ctx.clearRect(0, 0, 40, 40)\n\
             // A clip right after a stroke under a transform is in pixels too.\n\
             ctx.save(); ctx.scale(4, 4); ctx.beginPath(); ctx.moveTo(0, 9); ctx.lineTo(1, 9); ctx.stroke(); ctx.restore()\n\
             ctx.save(); ctx.beginPath(); ctx.rect(0, 0, 20, 30); ctx.clip(); ctx.fillStyle = 'blue'; ctx.fillRect(0, 0, 40, 30); ctx.restore()\n\
             console.log(at(5, 5), at(35, 5))",
        )
        .unwrap();
    assert_eq!(
        runtime.take_output(),
        [
            "255,0,0,255 0,0,255,255 0,0,0,0 #0000ff",
            "rgba(0, 0, 0, 0) 0",
            "255 true 0",
            "blur(2px) 255 true true",
            "255,0,0,255 0,255,0,255 0,0,0,0",
            "0,0,255,255 0,0,0,0",
        ]
    );
}

fn canvases_draw_path2d<E: Engine>() {
    let mut runtime = Runtime::<E>::new().unwrap();
    let scope = runtime.create_scope(800.0, 600.0).unwrap();
    runtime
        .execute(
            &scope,
            "const canvas = new OffscreenCanvas(40, 40)\n\
             const ctx = canvas.getContext('2d')\n\
             const at = (x, y) => [...ctx.getImageData(x, y, 1, 1).data].join()\n\
             const square = new Path2D(); square.rect(0, 0, 10, 10)\n\
             ctx.translate(20, 20); ctx.fillStyle = 'red'; ctx.fill(square)\n\
             console.log(at(25, 25), at(5, 5), ctx.isPointInPath(square, 25, 25), ctx.isPointInPath(square, 5, 5))\n\
             ctx.resetTransform(); ctx.clearRect(0, 0, 40, 40)\n\
             const svg = new Path2D('M0 0 H20 V20 H0 Z m30 0 h10 v10 h-10 z L oops')\n\
             ctx.fill(svg)\n\
             console.log(at(10, 10), at(35, 5), at(25, 5))\n\
             const both = new Path2D(square); both.addPath(square, { e: 30, f: 30 })\n\
             ctx.clearRect(0, 0, 40, 40); ctx.fill(both)\n\
             console.log(at(5, 5), at(35, 35), at(20, 20))\n\
             ctx.clearRect(0, 0, 40, 40); ctx.save(); ctx.clip(square); ctx.fillRect(0, 0, 40, 40); ctx.restore()\n\
             console.log(at(5, 5), at(15, 15))\n\
             const line = new Path2D(); line.moveTo(0, 20); line.lineTo(40, 20)\n\
             ctx.lineWidth = 4\n\
             console.log(ctx.isPointInStroke(line, 20, 21), ctx.isPointInStroke(line, 20, 23))\n\
             ctx.scale(2, 2)\n\
             console.log(ctx.isPointInStroke(line, 20, 43), ctx.isPointInStroke(line, 20, 45))\n\
             ctx.beginPath(); ctx.moveTo(0, 5); ctx.lineTo(10, 5)\n\
             console.log(ctx.isPointInStroke(5, 10), ctx.isPointInStroke(5, 20))",
        )
        .unwrap();
    assert_eq!(
        runtime.take_output(),
        [
            "255,0,0,255 0,0,0,0 true false",
            // The data's valid start draws; the rest is dropped.
            "255,0,0,255 255,0,0,255 0,0,0,0",
            "255,0,0,255 255,0,0,255 0,0,0,0",
            "255,0,0,255 0,0,0,0",
            // The line is in user space, its width scaled with it.
            "true false",
            "true false",
            "true false",
        ]
    );
}

fn canvas_colors_are_css_colors<E: Engine>() {
    let mut runtime = Runtime::<E>::new().unwrap();
    let scope = runtime.create_scope(800.0, 600.0).unwrap();
    runtime
        .execute(
            &scope,
            "const ctx = new OffscreenCanvas(1, 1).getContext('2d')\n\
             const read = css => { ctx.fillStyle = '#123456'; ctx.fillStyle = css; return ctx.fillStyle }\n\
             console.log(read('rgb(255 0 0 / 50%)'), read('hsl( 120 , 100% , 50% )'), read('grey'), read('oklch(0.628 0.258 29.2)'))\n\
             console.log(read('rgba(255,255,255,0.5)'), read('rgb(255, 0 0)'), read('nope'))\n\
             console.log(new Color('grey').toCSS(true))",
        )
        .unwrap();
    assert_eq!(
        runtime.take_output(),
        [
            "rgba(255, 0, 0, 0.5) #00ff00 #808080 #ff0000",
            // Alpha as few decimals as give the same 8 bits back; invalid colours change nothing.
            "rgba(255, 255, 255, 0.5) #123456 #123456",
            "#808080",
        ]
    );
}

fn image_bitmaps_decode_crop_flip_and_resize<E: Engine>() {
    let mut runtime = Runtime::<E>::new().unwrap();
    let scope = runtime.create_scope(800.0, 600.0).unwrap();
    runtime
        .execute(
            &scope,
            "const source = new OffscreenCanvas(4, 2)\n\
             const ctx = source.getContext('2d')\n\
             ctx.fillStyle = 'red'; ctx.fillRect(0, 0, 2, 2); ctx.fillStyle = 'blue'; ctx.fillRect(2, 0, 2, 2)\n\
             const tall = new OffscreenCanvas(1, 2)\n\
             const tctx = tall.getContext('2d')\n\
             tctx.fillStyle = 'red'; tctx.fillRect(0, 0, 1, 1); tctx.fillStyle = 'blue'; tctx.fillRect(0, 1, 1, 1)\n\
             const at = (image, x, y) => {\n\
                 const canvas = new OffscreenCanvas(image.width, image.height)\n\
                 const c = canvas.getContext('2d')\n\
                 c.drawImage(image, 0, 0)\n\
                 return [...c.getImageData(x, y, 1, 1).data].join()\n\
             }\n\
             ;(async () => {\n\
                 const decoded = await createImageBitmap(await source.convertToBlob())\n\
                 console.log(decoded instanceof ImageBitmap, decoded.width, decoded.height, at(decoded, 0, 0), at(decoded, 3, 1))\n\
                 const cropped = await createImageBitmap(source, 2, 0, 2, 2)\n\
                 console.log(cropped.width, at(cropped, 0, 0))\n\
                 console.log(at(await createImageBitmap(tall, { imageOrientation: 'flipY' }), 0, 0))\n\
                 const big = await createImageBitmap(source, { resizeWidth: 8, resizeQuality: 'pixelated' })\n\
                 console.log(big.width, big.height, at(big, 3, 3), at(big, 4, 0))\n\
                 console.log(at(await createImageBitmap(new ImageData(new Uint8ClampedArray([0, 255, 0, 255]), 1, 1)), 0, 0))\n\
                 decoded.close()\n\
                 try { new OffscreenCanvas(1, 1).getContext('2d').drawImage(decoded, 0, 0) } catch (e) { console.log(decoded.width, e instanceof DOMException, e.name, e.message) }\n\
                 try { await createImageBitmap(new Blob(['no image'])) } catch (e) { console.log(e.name) }\n\
             })()",
        )
        .unwrap();
    assert_eq!(
        runtime.take_output(),
        [
            "true 4 2 255,0,0,255 0,0,255,255",
            "2 0,0,255,255",
            "0,0,255,255",
            "8 4 255,0,0,255 0,0,255,255",
            "0,255,0,255",
            "0 true InvalidStateError the image bitmap is closed",
            "InvalidStateError",
        ]
    );
}

fn host_functions_throw_the_browsers_exceptions<E: Engine>() {
    let mut runtime = Runtime::<E>::new().unwrap();
    let scope = runtime.create_scope(800.0, 600.0).unwrap();
    runtime
        .execute(
            &scope,
            "const ctx = new OffscreenCanvas(1, 1).getContext('2d')\n\
             const thrown = f => { try { f() } catch (e) { return e } }\n\
             const range = thrown(() => ctx.arc(0, 0, -1, 0, 1))\n\
             console.log(range instanceof DOMException, range instanceof Error, range.name, range.code, range.code === DOMException.INDEX_SIZE_ERR)\n\
             console.log(String(range), Object.prototype.toString.call(range) !== '')\n\
             console.log(thrown(() => new Path2D().addPath(1)) instanceof TypeError, thrown(() => ctx.roundRect(0, 0, 1, 1, -1)) instanceof RangeError)\n\
             const own = new DOMException('gone', 'NotFoundError')\n\
             console.log(own.name, own.message, own.code, new DOMException().name)\n\
             const controller = new AbortController(); controller.abort()\n\
             console.log(controller.signal.reason instanceof DOMException, controller.signal.reason.name)",
        )
        .unwrap();
    assert_eq!(
        runtime.take_output(),
        [
            "true true IndexSizeError 1 true",
            "IndexSizeError: arc: the radius is negative true",
            "true true",
            "NotFoundError gone 8 Error",
            "true AbortError",
        ]
    );
}

fn canvas_properties_ignore_what_they_dont_take<E: Engine>() {
    let mut runtime = Runtime::<E>::new().unwrap();
    let scope = runtime.create_scope(800.0, 600.0).unwrap();
    runtime
        .execute(
            &scope,
            "const canvas = new OffscreenCanvas(2, 2)\n\
             const ctx = canvas.getContext('2d')\n\
             ctx.lineWidth = 0; ctx.lineWidth = -1; ctx.lineWidth = Infinity; ctx.lineCap = 'flat'; ctx.globalAlpha = 2\n\
             ctx.globalCompositeOperation = 'Xor'; ctx.textAlign = 'middle'; ctx.letterSpacing = '3 px'; ctx.font = 'big'\n\
             console.log(ctx.lineWidth, ctx.lineCap, ctx.globalAlpha, ctx.globalCompositeOperation, ctx.textAlign, ctx.letterSpacing, ctx.font)\n\
             ctx.lineWidth = '1e1'; ctx.lineCap = 'round'; ctx.wordSpacing = '2em'; ctx.imageSmoothingEnabled = 0; ctx.fontKerning = 'none'\n\
             console.log(ctx.lineWidth, ctx.lineCap, ctx.wordSpacing, ctx.imageSmoothingEnabled, ctx.fontKerning)\n\
             ctx.save(); ctx.lineWidth = 3; ctx.restore()\n\
             console.log(ctx.lineWidth)\n\
             canvas.width = 2\n\
             console.log(ctx.lineWidth, ctx.lineCap, ctx.wordSpacing, ctx.fontKerning)",
        )
        .unwrap();
    assert_eq!(
        runtime.take_output(),
        [
            "1 butt 1 source-over start 0px 10px sans-serif",
            "10 round 2em false none",
            "10",
            // Setting the size resets the context, as in browsers.
            "1 butt 0px auto",
        ]
    );
}

fn geometry_classes_are_the_browsers<E: Engine>() {
    let mut runtime = Runtime::<E>::new().unwrap();
    let scope = runtime.create_scope(800.0, 600.0).unwrap();
    runtime
        .execute(
            &scope,
            "const m = new DOMMatrix().translate(10, 20).scale(2)\n\
             console.log(String(m), m.a, m.m41, m.is2D, m.isIdentity, Object.prototype.toString.call(m))\n\
             const p = m.transformPoint(new DOMPoint(1, 1))\n\
             console.log(p instanceof DOMPoint, p.x, p.y, String(m.inverse().multiply(m)), m instanceof DOMMatrixReadOnly)\n\
             const r = new DOMMatrix([1, 0, 0, 1, 0, 0]).rotate(90)\n\
             console.log(Math.round(r.b), Math.round(r.c), new DOMMatrix().rotate(0, 30, 0).is2D)\n\
             m.m13 = 1\n\
             console.log(m.is2D, String(DOMMatrix.fromMatrix({ a: 2, m22: 3 })))\n\
             const thrown = f => { try { f() } catch (e) { return e.name } }\n\
             console.log(thrown(() => DOMMatrix.fromMatrix({ a: 1, m11: 2 })), thrown(() => new DOMMatrix('matrix(1, 0, 0, 1, 0, 0)')), thrown(() => String(new DOMMatrix([NaN, 0, 0, 1, 0, 0]))))\n\
             const rect = new DOMRect(10, 10, -5, 20)\n\
             console.log(rect.left, rect.right, rect.top, rect.bottom, JSON.stringify(new DOMQuad({ x: 1, y: 2 }, { x: 5 }).getBounds()))\n\
             const ctx = new OffscreenCanvas(1, 1).getContext('2d')\n\
             ctx.setTransform(new DOMMatrix().translate(3, 4))\n\
             console.log(ctx.getTransform() instanceof DOMMatrix, ctx.getTransform().e, ctx.getTransform().f, WebKitCSSMatrix === DOMMatrix)",
        )
        .unwrap();
    assert_eq!(
        runtime.take_output(),
        [
            "matrix(2, 0, 0, 2, 10, 20) 2 10 true false [object DOMMatrix]",
            "true 12 22 matrix(1, 0, 0, 1, 0, 0) true",
            "1 -1 false",
            "false matrix(2, 0, 0, 3, 0, 0)",
            // `a` and `m11` disagree; no CSS outside a window; a matrix that isn't finite has no string.
            "TypeError TypeError InvalidStateError",
            "5 10 10 30 {\"x\":0,\"y\":0,\"width\":5,\"height\":2,\"top\":0,\"right\":5,\"bottom\":2,\"left\":0}",
            "true 3 4 true",
        ]
    );
}

fn local_storage_and_preferences_last_and_session_storage_doesnt<E: Engine>(name: &str) {
    let folder = std::env::temp_dir().join(format!("scriptographer-storage-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&folder);
    let (script, storage) = (folder.join("script.js").to_string_lossy().into_owned(), folder.join("storage"));
    let run = |source: &str| {
        let mut runtime = Runtime::<E>::new().unwrap();
        runtime.set_script(&Setup { file: Some(script.clone()), storage: Some(storage.clone()), ..Setup::default() }).unwrap();
        let scope = runtime.create_scope(800.0, 600.0).unwrap();
        runtime.execute(&scope, source).unwrap();
        runtime.keep_preferences().unwrap();
        runtime.take_output()
    };
    let first = run("localStorage.setItem('a', 1); localStorage.b = { toString: () => 'two' }; sessionStorage.c = 'gone'\n\
         console.log(localStorage.length, localStorage.getItem('a'), localStorage.b, localStorage.key(1), localStorage.getItem('x'), localStorage.x)\n\
         console.log(Object.keys(localStorage).join(), 'b' in localStorage, localStorage instanceof Storage, typeof localStorage.getItem)\n\
         localStorage.getItem = 'item'; delete localStorage.a\n\
         console.log(localStorage.getItem('getItem'), localStorage.length)\n\
         const thrown = f => { try { f() } catch (e) { return e.name } }\n\
         console.log(thrown(() => localStorage.setItem('big', 'x'.repeat(6 * 1024 * 1024))))\n\
         script.preferences.point = new Point(1, 2)");
    assert_eq!(
        first,
        [
            "2 1 two b null undefined",
            "a,b true true function",
            // Properties the storage has are its own; others are items.
            "item 2",
            "QuotaExceededError",
        ]
    );
    // The next run of the script has its local items, not its session ones, and its preferences.
    let second = run(
        "console.log(localStorage.length, localStorage.b, localStorage.getItem('getItem'), sessionStorage.length, script.preferences.point instanceof Point)\n\
         localStorage.clear()",
    );
    assert_eq!(second, ["2 two item 0 true"]);
    let third = run("console.log(localStorage.length, script.preferences.point.y)");
    assert_eq!(third, ["0 2"], "clear() leaves the preferences");
    std::fs::remove_dir_all(&folder).unwrap();
}

fn the_console_writes_as_browsers_do<E: Engine>() {
    let mut runtime = Runtime::<E>::new().unwrap();
    let scope = runtime.create_scope(800.0, 600.0).unwrap();
    runtime
        .execute(
            &scope,
            "class Swatch { constructor() { this.name = 'red' } }\n\
             function make() {}\n\
             console.log([1, 'two', [3, { four: 4 }]], { a: 1, b: 'b' }, new Swatch(), make, null, new Point(1, 2))\n\
             console.log('%s is %d years and %f%%, %o', 'Paper.js', 15.7, 0.5, { x: 1 }, 'more')\n\
             console.assert(1 === 1, 'not shown'); console.assert(false, 'shown', 2)\n\
             console.count(); console.count(); console.count('other'); console.countReset(); console.count()\n\
             console.group('outer'); console.info('in'); console.group(); console.warn('deeper'); console.groupEnd(); console.groupEnd(); console.error('out')\n\
             console.time('t'); console.time('t'); console.timeEnd('t'); console.timeEnd('t')\n\
             console.table([{ a: 1, b: 2 }, { a: 3 }])\n\
             print('printed')\n\
             function deep() { console.trace('here') }\n\
             deep()",
        )
        .unwrap();
    let mut output = runtime.take_output();
    // The trace ends the output: the line, then where it came from.
    let trace = output.pop().unwrap();
    assert!(trace.starts_with("Trace: here\n") && trace.contains("deep"), "{trace}");
    let timed = output.iter().position(|line| line.starts_with("t: ")).unwrap();
    assert!(output[timed].ends_with(" ms"), "{}", output[timed]);
    output.remove(timed);
    assert_eq!(
        output,
        [
            "[1, 'two', [3, {four: 4}]] {a: 1, b: 'b'} Swatch {name: 'red'} ƒ make() null { x: 1, y: 2 }",
            "Paper.js is 15 years and 0.5%, {x: 1} more",
            "Assertion failed: shown 2",
            "default: 1",
            "default: 2",
            "other: 1",
            "default: 1",
            "outer",
            "  in",
            "    deeper",
            "out",
            "Timer 't' already exists",
            "Timer 't' does not exist",
            "│ (index) │ a │ b │\n│ 0       │ 1 │ 2 │\n│ 1       │ 3 │   │",
            "printed",
        ]
    );
}

fn structured_clone_copies_as_browsers_do<E: Engine>() {
    let mut runtime = Runtime::<E>::new().unwrap();
    let scope = runtime.create_scope(800.0, 600.0).unwrap();
    runtime
        .execute(
            &scope,
            "const shared = { n: 1 }\n\
             const original = { list: [shared, shared], when: new Date(5), pattern: /a/gi, map: new Map([[1, shared]]), set: new Set(['x']), bytes: new Uint8Array([1, 2]), matrix: new DOMMatrix().translate(3, 4) }\n\
             original.self = original\n\
             const copy = structuredClone(original)\n\
             console.log(copy !== original, copy.self === copy, copy.list[0] === copy.list[1], copy.list[0] !== shared, copy.map.get(1) === copy.list[0])\n\
             console.log(copy.when.getTime(), String(copy.pattern), copy.set.has('x'), copy.bytes.join(), copy.matrix instanceof DOMMatrix, copy.matrix.e)\n\
             const error = structuredClone(new RangeError('far'))\n\
             console.log(error instanceof RangeError, error.message, structuredClone(5), structuredClone('text'))\n\
             const thrown = f => { try { f() } catch (e) { return e.name } }\n\
             console.log(thrown(() => structuredClone(() => 1)), thrown(() => structuredClone({ f: Symbol() })), thrown(() => structuredClone(new OffscreenCanvas(1, 1))))",
        )
        .unwrap();
    assert_eq!(
        runtime.take_output(),
        [
            // Shared and cyclic objects stay so.
            "true true true true true",
            "5 /a/gi true 1,2 true 3",
            "true far 5 text",
            "DataCloneError DataCloneError DataCloneError",
        ]
    );
}

fn navigator_speaks_the_apps_language<E: Engine>() {
    let mut runtime = Runtime::<E>::new().unwrap();
    let scope = runtime.create_scope(800.0, 600.0).unwrap();
    runtime.execute(&scope, "console.log(navigator.language, navigator.languages.join())").unwrap();
    runtime.set_script(&Setup { language: Some("de".into()), ..Setup::default() }).unwrap();
    runtime.execute(&scope, "console.log(navigator.language, navigator.languages.join())").unwrap();
    assert_eq!(runtime.take_output(), ["en-US en-US", "de de"]);
}

fn event_targets_are_the_browsers<E: Engine>() {
    let mut runtime = Runtime::<E>::new().unwrap();
    let scope = runtime.create_scope(800.0, 600.0).unwrap();
    runtime
        .execute(
            &scope,
            "class Emitter extends EventTarget { ping() { return this.dispatchEvent(new CustomEvent('ping', { detail: 42, cancelable: true })) } }\n\
             const emitter = new Emitter()\n\
             const log = []\n\
             const listener = event => log.push('a ' + event.detail + ' ' + (event.target === emitter) + ' ' + (event.currentTarget === emitter))\n\
             emitter.addEventListener('ping', listener)\n\
             emitter.addEventListener('ping', listener)\n\
             emitter.addEventListener('ping', () => log.push('once'), { once: true })\n\
             emitter.addEventListener('ping', { handleEvent(event) { log.push('handler ' + (this !== emitter)); event.preventDefault() } })\n\
             emitter.addEventListener('ping', () => { throw new Error('oops') })\n\
             const controller = new AbortController()\n\
             emitter.addEventListener('ping', () => log.push('until aborted'), { signal: controller.signal })\n\
             console.log(emitter instanceof Emitter, emitter instanceof EventTarget, emitter.ping(), log.join(', '))\n\
             log.length = 0; controller.abort(); emitter.removeEventListener('ping', listener)\n\
             console.log(emitter.ping(), log.join(', '))\n\
             const event = new Event('x', { cancelable: true })\n\
             emitter.addEventListener('x', e => { e.stopImmediatePropagation(); e.preventDefault() }, { passive: true })\n\
             emitter.addEventListener('x', () => log.push('never'))\n\
             console.log(emitter.dispatchEvent(event), event.defaultPrevented, event.eventPhase, Event.AT_TARGET, controller.signal instanceof EventTarget)\n\
             const thrown = f => { try { f() } catch (e) { return e.name } }\n\
             console.log(thrown(() => EventTarget()), thrown(() => emitter.dispatchEvent({})), Object.prototype.toString.call(event))",
        )
        .unwrap();
    // Reports go on with where the error was, as each engine writes it.
    let report = "Uncaught Error: oops";
    let output: Vec<String> = runtime.take_output().into_iter().map(|line| if line.starts_with(report) { report.to_owned() } else { line }).collect();
    assert_eq!(
        output,
        [
            // A listener added twice runs once; one that throws is reported, and the others run.
            "Uncaught Error: oops",
            "true true false a 42 true true, once, handler true, until aborted",
            "Uncaught Error: oops",
            "false handler true",
            // A passive listener can't prevent the default.
            "true false 0 2 true",
            "TypeError TypeError [object Event]",
        ]
    );
}

fn the_clipboard_needs_the_app<E: Engine>() {
    let mut runtime = Runtime::<E>::new().unwrap();
    let scope = runtime.create_scope(800.0, 600.0).unwrap();
    runtime
        .execute(
            &scope,
            "navigator.clipboard.readText().catch(e => console.log(e.name))\n\
             navigator.clipboard.writeText('x').catch(e => console.log(e.name))",
        )
        .unwrap();
    assert_eq!(runtime.take_output(), ["NotAllowedError", "NotAllowedError"]);
}

/// A WebSocket server on a free local port that echoes what it's sent, for one connection; its
/// address.
fn echo_server() -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = format!("ws://{}", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut socket = tungstenite::accept(stream).unwrap();
        while let Ok(message) = socket.read() {
            if message.is_text() || message.is_binary() {
                socket.send(message).unwrap();
            }
        }
    });
    address
}

fn web_sockets_talk<E: Engine>() {
    let mut runtime = Runtime::<E>::new().unwrap();
    let scope = runtime.create_scope(800.0, 600.0).unwrap();
    let source = format!(
        "const socket = new WebSocket('{}')\n\
         socket.binaryType = 'arraybuffer'\n\
         console.log(socket.readyState === WebSocket.CONNECTING, socket instanceof EventTarget)\n\
         socket.onopen = () => {{ console.log('open', socket.readyState); socket.send('hello'); socket.send(new Uint8Array([1, 2, 3])) }}\n\
         let count = 0\n\
         socket.addEventListener('message', event => {{\n\
             console.log('message', typeof event.data === 'string' ? event.data : new Uint8Array(event.data).join(), event instanceof MessageEvent)\n\
             if (++count === 2) socket.close(4001, 'done')\n\
         }})\n\
         socket.onclose = event => console.log('close', event.code, event.reason, event.wasClean, socket.readyState, event instanceof CloseEvent)",
        echo_server()
    );
    runtime.execute(&scope, &source).unwrap();
    let started = std::time::Instant::now();
    while runtime.is_running() && started.elapsed() < std::time::Duration::from_secs(5) {
        runtime.run_timers(started.elapsed().as_secs_f64() * 1000.0).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(runtime.take_output(), ["true true", "open 1", "message hello true", "message 1,2,3 true", "close 4001 done true 3 true"]);
    assert!(!runtime.is_running(), "a closed socket keeps the script running no more");
}

fn web_sockets_that_cant_connect_close<E: Engine>() {
    // A port nothing listens on.
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let mut runtime = Runtime::<E>::new().unwrap();
    let scope = runtime.create_scope(800.0, 600.0).unwrap();
    let source = format!(
        "const socket = new WebSocket('ws://127.0.0.1:{port}')\n\
         socket.onerror = event => console.log('error', event.type)\n\
         socket.onclose = event => console.log('close', event.code, event.wasClean)\n\
         const thrown = f => {{ try {{ f() }} catch (e) {{ return e.name }} }}\n\
         console.log(thrown(() => new WebSocket('ftp://x')), thrown(() => socket.send('early')), thrown(() => socket.close(1001)))"
    );
    runtime.execute(&scope, &source).unwrap();
    let started = std::time::Instant::now();
    while runtime.is_running() && started.elapsed() < std::time::Duration::from_secs(5) {
        runtime.run_timers(started.elapsed().as_secs_f64() * 1000.0).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let output = runtime.take_output();
    assert_eq!(output.first().map(String::as_str), Some("SyntaxError InvalidStateError InvalidAccessError"));
    // The console says why; the events don't.
    assert!(output.get(1).is_some_and(|line| line.starts_with(&format!("WebSocket ws://127.0.0.1:{port}/: "))), "{output:?}");
    assert_eq!(output.get(2..), Some(&["error error".to_owned(), "close 1006 false".to_owned()][..]));
}

/// Runs the runtime's timers until it stops running, for five seconds at most.
fn run_until_done<E: Engine>(runtime: &mut Runtime<E>) {
    let started = std::time::Instant::now();
    while runtime.is_running() && started.elapsed() < std::time::Duration::from_secs(5) {
        runtime.run_timers(started.elapsed().as_secs_f64() * 1000.0).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

fn workers_run_scripts_on_their_threads<E: Engine>(name: &str) {
    let folder = std::env::temp_dir().join(format!("scriptographer-workers-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&folder);
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("helper.js"), "function double(n) { return n * 2 }").unwrap();
    std::fs::write(
        folder.join("echo.js"),
        "importScripts('helper.js')\n\
         self.onmessage = event => {\n\
             postMessage({ doubled: double(event.data.n), shared: event.data.list[0] === event.data.list[1], name: self.name })\n\
             if (event.data.n === 2) close()\n\
         }\n\
         setTimeout(() => { throw new Error('later') }, 0)\n\
         console.log('worker started', self instanceof EventTarget || typeof self.addEventListener)",
    )
    .unwrap();
    let mut runtime = Runtime::<E>::new().unwrap();
    runtime.set_script(&Setup { file: Some(folder.join("script.js").to_string_lossy().into_owned()), ..Setup::default() }).unwrap();
    let scope = runtime.create_scope(800.0, 600.0).unwrap();
    runtime
        .execute(
            &scope,
            "const worker = new Worker('echo.js', { name: 'echo' })\n\
             worker.onmessage = event => console.log('reply', JSON.stringify(event.data))\n\
             worker.onerror = event => { console.log('error', event instanceof ErrorEvent, event.message.includes('later')); event.preventDefault() }\n\
             const list = [{}]\n\
             list.push(list[0])\n\
             worker.postMessage({ n: 1, list })\n\
             worker.postMessage({ n: 2, list })",
        )
        .unwrap();
    run_until_done(&mut runtime);
    assert_eq!(
        runtime.take_output(),
        [
            "worker started function",
            "reply {\"doubled\":2,\"shared\":true,\"name\":\"echo\"}",
            "error true true",
            "reply {\"doubled\":4,\"shared\":true,\"name\":\"echo\"}",
        ]
    );
    assert!(!runtime.is_running(), "a closed worker keeps its parent running no more");
    // A worker that never returns stops when it's terminated.
    std::fs::write(folder.join("forever.js"), "while (true) {}").unwrap();
    runtime.execute(&scope, "const forever = new Worker('forever.js')\nsetTimeout(() => forever.terminate(), 50)").unwrap();
    run_until_done(&mut runtime);
    assert!(!runtime.is_running(), "the terminated worker is gone");
    std::fs::remove_dir_all(&folder).unwrap();
}

fn runtimes_live_side_by_side<E: Engine>() {
    // As the app runs them: a second script starts while the first keeps running, and the first
    // stops first.
    let mut first = Runtime::<E>::new().unwrap();
    let first_scope = first.create_scope(100.0, 100.0).unwrap();
    let mut second = Runtime::<E>::new().unwrap();
    let second_scope = second.create_scope(100.0, 100.0).unwrap();
    first.execute(&first_scope, "console.log('first')").unwrap();
    second.execute(&second_scope, "console.log('second')").unwrap();
    first.execute(&first_scope, "console.log('first again')").unwrap();
    assert_eq!(first.take_output(), ["first", "first again"]);
    assert_eq!(second.take_output(), ["second"]);
    drop(first);
    second.execute(&second_scope, "console.log('second again')").unwrap();
    assert_eq!(second.take_output(), ["second again"]);
}

fn runtimes_share_nothing<E: Engine>() {
    // Each starts as Paper.js left it after loading (in V8, from one snapshot), whatever another
    // did to its own.
    let mut first = Runtime::<E>::new().unwrap();
    let first_scope = first.create_scope(100.0, 100.0).unwrap();
    first.execute(&first_scope, "globalThis.left = 1; Point.prototype.left = 1; new Path.Circle([50, 50], 10)").unwrap();
    let mut second = Runtime::<E>::new().unwrap();
    let second_scope = second.create_scope(100.0, 100.0).unwrap();
    second.execute(&second_scope, "console.log(typeof left, new Point().left, project.activeLayer.children.length)").unwrap();
    assert_eq!(second.take_output(), ["undefined undefined 0"]);
}

macro_rules! engine_tests {
    ($module:ident, $engine:ty) => {
        mod $module {
            #[test]
            fn runs_paperscript_with_console_and_paper_globals() {
                super::runs_paperscript_with_console_and_paper_globals::<$engine>();
            }

            #[test]
            fn paperscript_operators_work_on_points() {
                super::paperscript_operators_work_on_points::<$engine>();
            }

            #[test]
            fn script_errors_are_returned() {
                super::script_errors_are_returned::<$engine>();
            }

            #[test]
            fn host_function_errors_are_thrown_in_scripts() {
                super::host_function_errors_are_thrown_in_scripts::<$engine>();
            }

            #[test]
            fn intervals_run_when_due() {
                super::intervals_run_when_due::<$engine>();
            }

            #[test]
            fn local_files_read_write_and_list() {
                super::local_files_read_write_and_list::<$engine>(stringify!($module));
            }

            #[test]
            fn scripts_await_at_the_top_level() {
                super::scripts_await_at_the_top_level::<$engine>();
            }

            #[test]
            fn host_classes_can_be_subclassed() {
                super::host_classes_can_be_subclassed::<$engine>();
            }

            #[test]
            fn web_imports_are_kept_for_the_next_run() {
                super::web_imports_are_kept_for_the_next_run::<$engine>(stringify!($module));
            }

            #[test]
            fn files_are_blobs_with_a_name() {
                super::files_are_blobs_with_a_name::<$engine>();
            }

            #[test]
            fn scripts_have_a_dom() {
                super::scripts_have_a_dom::<$engine>();
            }

            #[test]
            fn scripts_use_todays_javascript() {
                super::scripts_use_todays_javascript::<$engine>();
            }

            #[test]
            fn promises_resolve() {
                super::promises_resolve::<$engine>();
            }

            #[test]
            fn scripts_have_the_browsers_everyday_apis() {
                super::scripts_have_the_browsers_everyday_apis::<$engine>();
            }

            #[test]
            fn text_encodes_as_the_standard_has_it() {
                super::text_encodes_as_the_standard_has_it::<$engine>();
            }

            #[test]
            fn scripts_import_modules() {
                super::scripts_import_modules::<$engine>(stringify!($module));
            }

            #[test]
            fn scripts_fetch() {
                super::scripts_fetch::<$engine>();
            }

            #[test]
            fn scripts_import_from_the_web() {
                super::scripts_import_from_the_web::<$engine>();
            }

            #[test]
            fn scripts_have_urls() {
                super::scripts_have_urls::<$engine>();
            }

            #[test]
            fn scripts_measure_text() {
                super::scripts_measure_text::<$engine>();
            }

            #[test]
            fn canvases_have_pixels() {
                super::canvases_have_pixels::<$engine>();
            }

            #[test]
            fn canvases_draw() {
                super::canvases_draw::<$engine>();
            }

            #[test]
            fn fetch_classes_work_alone() {
                super::fetch_classes_work_alone::<$engine>();
            }

            #[test]
            fn timeouts_run_once() {
                super::timeouts_run_once::<$engine>();
            }

            #[test]
            fn runtimes_live_side_by_side() {
                super::runtimes_live_side_by_side::<$engine>();
            }

            #[test]
            fn canvases_draw_shadows_and_filters() {
                super::canvases_draw_shadows_and_filters::<$engine>();
            }

            #[test]
            fn canvases_draw_path2d() {
                super::canvases_draw_path2d::<$engine>();
            }

            #[test]
            fn canvas_colors_are_css_colors() {
                super::canvas_colors_are_css_colors::<$engine>();
            }

            #[test]
            fn image_bitmaps_decode_crop_flip_and_resize() {
                super::image_bitmaps_decode_crop_flip_and_resize::<$engine>();
            }

            #[test]
            fn host_functions_throw_the_browsers_exceptions() {
                super::host_functions_throw_the_browsers_exceptions::<$engine>();
            }

            #[test]
            fn canvas_properties_ignore_what_they_dont_take() {
                super::canvas_properties_ignore_what_they_dont_take::<$engine>();
            }

            #[test]
            fn geometry_classes_are_the_browsers() {
                super::geometry_classes_are_the_browsers::<$engine>();
            }

            #[test]
            fn local_storage_and_preferences_last_and_session_storage_doesnt() {
                super::local_storage_and_preferences_last_and_session_storage_doesnt::<$engine>(stringify!($module));
            }

            #[test]
            fn the_console_writes_as_browsers_do() {
                super::the_console_writes_as_browsers_do::<$engine>();
            }

            #[test]
            fn animation_frames_run_once_a_frame() {
                super::animation_frames_run_once_a_frame::<$engine>();
            }

            #[test]
            fn structured_clone_copies_as_browsers_do() {
                super::structured_clone_copies_as_browsers_do::<$engine>();
            }

            #[test]
            fn navigator_speaks_the_apps_language() {
                super::navigator_speaks_the_apps_language::<$engine>();
            }

            #[test]
            fn event_targets_are_the_browsers() {
                super::event_targets_are_the_browsers::<$engine>();
            }

            #[test]
            fn the_clipboard_needs_the_app() {
                super::the_clipboard_needs_the_app::<$engine>();
            }

            #[test]
            fn web_sockets_talk() {
                super::web_sockets_talk::<$engine>();
            }

            #[test]
            fn web_sockets_that_cant_connect_close() {
                super::web_sockets_that_cant_connect_close::<$engine>();
            }

            #[test]
            fn workers_run_scripts_on_their_threads() {
                super::workers_run_scripts_on_their_threads::<$engine>(stringify!($module));
            }

            #[test]
            fn runtimes_share_nothing() {
                super::runtimes_share_nothing::<$engine>();
            }

            #[test]
            fn css_colors_resolve_without_a_canvas() {
                super::css_colors_resolve_without_a_canvas::<$engine>();
            }
        }
    };
}

#[cfg(feature = "boa")]
engine_tests!(boa, vectorcraft_scriptographer::Boa);
#[cfg(all(feature = "v8", not(target_arch = "wasm32")))]
engine_tests!(v8, vectorcraft_scriptographer::V8);
