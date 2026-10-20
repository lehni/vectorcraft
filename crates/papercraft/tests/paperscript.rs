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
            "console.log('Hello World', document === project)\n\
             new Path.Circle({ center: view.bounds.center, radius: 100, fillColor: 'red' })",
        )
        .unwrap();
    assert_eq!(runtime.take_output(), ["Hello World true"], "`document` is the project");

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
