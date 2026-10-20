#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! The same tests for every engine, through [`Runtime`].

use serde_json::Value;
use vectorcraft_papercraft::{Engine, Runtime};

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

fn files_read_write_and_list<E: Engine>(name: &str) {
    let folder = std::env::temp_dir().join(format!("papercraft-files-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&folder);
    std::fs::create_dir_all(&folder).unwrap();
    let mut runtime = Runtime::<E>::new().unwrap();
    runtime.set_script(Some(&folder.join("script.js").to_string_lossy()), &serde_json::json!({})).unwrap();
    let scope = runtime.create_scope(800.0, 600.0).unwrap();
    runtime
        .execute(
            &scope,
            "var notes = new File('notes.txt')\n\
             console.log(notes.exists(), notes.name, notes.extension, notes.parent.path == script.directory.path)\n\
             notes.open()\n\
             notes.writeln('one')\n\
             notes.write('two')\n\
             notes.close()\n\
             try { notes.write('three') } catch (e) { console.log(e.message) }\n\
             notes.open({ append: true })\n\
             notes.writeln('')\n\
             notes.close()\n\
             console.log(notes.exists(), notes.size, notes.readLines().join('|'))\n\
             var sub = new File(script.directory, 'sub')\n\
             sub.makeDirectory()\n\
             notes.copyTo(new File(sub, 'copy.txt'))\n\
             console.log(script.directory.list(/\\.txt$/).map(function(f) { return f.name }).join())\n\
             console.log(script.directory.list(null, true).length, sub.list(function(f) { return f.isFile() })[0].readAll())\n\
             console.log(sub.remove(), sub.remove(true), sub.exists(), String(notes) == notes.path)",
        )
        .unwrap();
    assert_eq!(
        runtime.take_output(),
        ["false notes.txt txt true", "File: not opened", "true 8 one|two", "notes.txt", "3 one\ntwo\n", "false true false true"]
    );
    std::fs::remove_dir_all(&folder).unwrap();
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
             console.log('script')",
        )
        .unwrap();
    assert_eq!(runtime.take_output(), ["3 hé hé", "4 true", "true", "script", "microtask"]);
}

fn scripts_import_modules<E: Engine>(name: &str) {
    let folder = std::env::temp_dir().join(format!("papercraft-imports-{name}-{}", std::process::id()));
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
    runtime.set_script(Some(&folder.join("script.js").to_string_lossy()), &serde_json::json!({})).unwrap();
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
            let (mut length, mut kind) = (0, String::new());
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
                _ => ("404 Not Found", "text/plain", b"missing".to_vec()),
            };
            let head = format!(
                "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nX-Test: one\r\nConnection: close\r\n\r\n",
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
            fn files_read_write_and_list() {
                super::files_read_write_and_list::<$engine>(stringify!($module));
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
            fn css_colors_resolve_without_a_canvas() {
                super::css_colors_resolve_without_a_canvas::<$engine>();
            }
        }
    };
}

#[cfg(feature = "boa")]
engine_tests!(boa, vectorcraft_papercraft::Boa);
#[cfg(all(feature = "v8", not(target_arch = "wasm32")))]
engine_tests!(v8, vectorcraft_papercraft::V8);
