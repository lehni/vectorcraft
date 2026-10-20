#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! The web-platform-tests of `OffscreenCanvas` that run in a worker (`html/canvas/offscreen`),
//! against PaperCraft's canvas in V8, when `cargo xtask corpus --wpt` has fetched them; skipped
//! when it hasn't. The tests that fail are listed in `tests/wpt-expected-failures.txt`: this test
//! fails when another one fails, or when a listed one passes, so the list stays the truth.
//! `WPT_UPDATE=1` writes the list anew. The pass rate by folder prints with `--nocapture`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use vectorcraft_papercraft::{Runtime, V8};

/// Reports each test's result, and the harness's, as JSON lines on the console.
const REPORTER: &str = "add_completion_callback(function (tests, status) {\n\
    for (const test of tests) console.log(JSON.stringify(['test', test.name, test.status, test.message]));\n\
    console.log(JSON.stringify(['done', status.status, status.message]));\n\
});";

/// testharness.js's test statuses.
const STATUSES: [&str; 5] = ["PASS", "FAIL", "TIMEOUT", "NOTRUN", "PRECONDITION_FAILED"];

const LIST_HEADER: &str = "# The web-platform-tests (https://github.com/web-platform-tests/wpt, BSD-3-Clause) of\n\
# html/canvas/offscreen that PaperCraft fails, by file and title: see tests/wpt.rs.\n";

/// How long a test file may take, its async tests included.
const TIMEOUT: Duration = Duration::from_secs(5);

struct Harness {
    testharness: String,
    canvas_tests: String,
    /// Where the corpus is served, for what the tests fetch from the test server's root.
    server: String,
}

#[test]
fn offscreen_canvas() {
    let corpus = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus/wpt");
    if !corpus.join("LICENSE.md").exists() {
        eprintln!("skipped: no WPT corpus (cargo xtask corpus --wpt)");
        return;
    }
    let testharness = std::fs::read_to_string(corpus.join("resources/testharness.js")).unwrap();
    // Scripts have a DOM of their own (linkedom), which would make testharness.js wait for a
    // window's load event. These are worker tests: they run as in a shell.
    let window_check = "if ('document' in global_scope) {";
    assert!(testharness.contains(window_check), "testharness.js no longer checks for a window this way");
    let harness = Harness {
        testharness: testharness.replacen(window_check, "if (false) {", 1),
        canvas_tests: std::fs::read_to_string(corpus.join("html/canvas/resources/canvas-tests.js")).unwrap(),
        server: serve(corpus.clone()),
    };
    let tests = corpus.join("html/canvas/offscreen");
    let mut files = Vec::new();
    collect(&tests, &mut files);
    files.sort();

    let mut results = BTreeMap::new();
    for file in &files {
        let name = file.strip_prefix(&tests).unwrap().to_string_lossy().replace('\\', "/");
        for (test, status, message) in run(&harness, file) {
            results.insert(format!("{name}: {test}"), (status, message));
        }
    }

    // The pass rate by folder.
    let mut folders: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    for (key, (status, _)) in &results {
        let file = key.split_once(": ").map_or(key.as_str(), |(file, _)| file);
        let folder = file.split_once('/').map_or("(top)", |(folder, _)| folder).to_owned();
        let counts = folders.entry(folder).or_default();
        counts.1 += 1;
        if status == "PASS" {
            counts.0 += 1;
        }
    }
    let passed: usize = folders.values().map(|c| c.0).sum();
    println!("WPT html/canvas/offscreen: {passed} of {} pass", results.len());
    for (folder, (pass, total)) in &folders {
        println!("  {folder:<32} {pass:>4} / {total:<4}");
    }

    let failing: BTreeSet<&String> = results.iter().filter(|(_, (status, _))| status != "PASS").map(|(key, _)| key).collect();
    let list = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/wpt-expected-failures.txt");
    if std::env::var_os("WPT_UPDATE").is_some() {
        let mut text = String::from(LIST_HEADER);
        text.extend(failing.iter().map(|key| format!("{key}\n")));
        std::fs::write(&list, text).unwrap();
        return;
    }
    let expected: BTreeSet<String> =
        std::fs::read_to_string(&list).unwrap_or_default().lines().filter(|line| !line.starts_with('#')).map(str::to_owned).collect();
    let newly_failing: Vec<String> =
        failing.iter().filter(|key| !expected.contains(**key)).map(|key| format!("{key}: {} {}", results[*key].0, results[*key].1)).collect();
    let newly_passing: Vec<&String> = expected.iter().filter(|key| results.get(*key).is_some_and(|(status, _)| status == "PASS")).collect();
    assert!(
        newly_failing.is_empty() && newly_passing.is_empty(),
        "failing, not expected to:\n  {}\npassing, expected to fail (take them off the list, or run with WPT_UPDATE=1):\n  {}",
        newly_failing.join("\n  "),
        newly_passing.iter().map(|k| k.as_str()).collect::<Vec<_>>().join("\n  ")
    );
}

fn collect(folder: &Path, files: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(folder).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            // Manual tests need a person to look.
            if path.file_name().is_some_and(|n| n != "manual") {
                collect(&path, files);
            }
        } else if path.to_string_lossy().ends_with(".worker.js") {
            files.push(path);
        }
    }
}

/// Each test of the worker test `file`, with its status and message; the file's own name when the
/// harness reported nothing.
fn run(harness: &Harness, file: &Path) -> Vec<(String, String, String)> {
    let source = std::fs::read_to_string(file).unwrap();
    // The harness is loaded already; nothing else is imported. What's fetched comes from the
    // corpus, as from the test server.
    let source: String = source.lines().filter(|line| !line.trim_start().starts_with("importScripts(")).map(|line| format!("{line}\n")).collect();
    let source = source.replace("fetch('/", &format!("fetch('{}/", harness.server));
    let mut runtime = Runtime::<V8>::new().unwrap();
    let interrupt = runtime.interrupt().unwrap();
    let (stop, stopped) = std::sync::mpsc::channel::<()>();
    let watchdog = std::thread::spawn(move || {
        if stopped.recv_timeout(TIMEOUT).is_err() {
            interrupt();
        }
    });

    let mut results = Vec::new();
    let mut error = None;
    let started = Instant::now();
    let loaded = runtime.eval(&harness.testharness).and_then(|_| runtime.eval(&harness.canvas_tests)).and_then(|_| runtime.eval(REPORTER));
    if let Err(e) = loaded.and_then(|_| runtime.eval(&source)) {
        error = Some(e.to_string());
    }
    let mut done = false;
    while !done && error.is_none() && started.elapsed() < TIMEOUT {
        if let Err(e) = runtime.run_timers(started.elapsed().as_secs_f64() * 1000.0) {
            error = Some(e.to_string());
        }
        for line in runtime.take_output() {
            let Ok(serde_json::Value::Array(report)) = serde_json::from_str::<serde_json::Value>(&line) else {
                continue;
            };
            match report.first().and_then(|kind| kind.as_str()) {
                Some("test") => {
                    let name = report.get(1).and_then(|n| n.as_str()).unwrap_or_default().to_owned();
                    let status = report.get(2).and_then(|s| s.as_u64()).and_then(|s| STATUSES.get(s as usize)).unwrap_or(&"FAIL");
                    let message = report.get(3).and_then(|m| m.as_str()).unwrap_or_default().replace('\n', " ");
                    results.push((name, (*status).to_owned(), message));
                }
                Some("done") => done = true,
                _ => {}
            }
        }
        if !done {
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    let _ = stop.send(());
    watchdog.join().unwrap();
    if results.is_empty() {
        let message = error.unwrap_or_else(|| "the harness reported nothing".into());
        let name = file.file_name().unwrap().to_string_lossy().into_owned();
        results.push((name, if done { "FAIL" } else { "TIMEOUT" }.to_owned(), message));
    }
    results
}

/// Serves the files in `root` over HTTP on a free local port, for as long as the test runs, and
/// gives back its address.
fn serve(root: PathBuf) -> String {
    use std::io::{BufRead, BufReader, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = format!("http://{}", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut line = String::new();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            if reader.read_line(&mut line).is_err() {
                continue;
            }
            // The headers say nothing the answer depends on.
            let mut header = String::new();
            while reader.read_line(&mut header).is_ok_and(|n| n > 0) && !header.trim().is_empty() {
                header.clear();
            }
            let path = line.split_whitespace().nth(1).unwrap_or("/").trim_start_matches('/').to_owned();
            let file = root.join(&path);
            let answer = match std::fs::read(&file) {
                Ok(body) if file.starts_with(&root) && !path.contains("..") => {
                    let kind = match file.extension().and_then(|e| e.to_str()) {
                        Some("png") => "image/png",
                        Some("ttf") => "font/ttf",
                        Some("svg") => "image/svg+xml",
                        _ => "application/octet-stream",
                    };
                    let mut answer =
                        format!("HTTP/1.1 200 OK\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len())
                            .into_bytes();
                    answer.extend(body);
                    answer
                }
                _ => b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec(),
            };
            let _ = stream.write_all(&answer);
        }
    });
    address
}
