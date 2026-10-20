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
            "a=1&b=2 v,w false true",
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
             canvas.convertToBlob().then(blob => blob.bytes()).then(png => console.log(png[1], png[2], png[3]))",
        )
        .unwrap();
    assert_eq!(
        runtime.take_output(),
        ["48 0,0,0,0 255,0,0,128 true", "0,255,0,255 0,0,0,0 0,0,0,0", "2 3 0,0,0,0", "#0000ff 0", "80 78 71"],
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
engine_tests!(boa, vectorcraft_papercraft::Boa);
#[cfg(all(feature = "v8", not(target_arch = "wasm32")))]
engine_tests!(v8, vectorcraft_papercraft::V8);
