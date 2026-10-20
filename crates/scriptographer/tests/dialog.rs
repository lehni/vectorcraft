//! Scriptographer's `Dialog` and `Palette`: the script asks, the host answers, the script goes on
//! with it.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::cell::RefCell;
use std::rc::Rc;

use serde_json::{Value, json};
use vectorcraft_scriptographer::{Engine, PaletteEvent, Runtime};

/// Runs `script` with dialogs answered by `answers`, in turn. Returns the questions and the output.
fn run<E: Engine>(script: &str, answers: Vec<Value>) -> (Vec<Value>, Vec<String>) {
    let mut runtime = Runtime::<E>::new().unwrap();
    let scope = runtime.create_scope(400.0, 300.0).unwrap();
    let questions = Rc::new(RefCell::new(Vec::new()));
    let (asked, mut answers) = (questions.clone(), answers.into_iter());
    runtime.set_ask(Box::new(move |question| {
        asked.borrow_mut().push(question);
        Ok(answers.next().unwrap_or(Value::Null))
    }));
    runtime.execute(&scope, script).unwrap();
    let questions = questions.borrow().clone();
    (questions, runtime.take_output())
}

fn alert_and_confirm_ask_and_wait<E: Engine>() {
    let (questions, output) = run::<E>("Dialog.alert('Done.')\nconsole.log(Dialog.confirm('Circles', 'Draw them?'))", vec![json!(true), json!(true)]);
    assert_eq!(
        questions,
        [
            json!({"type": "alert", "title": "Scriptographer", "message": "Done."}),
            json!({"type": "confirm", "title": "Circles", "message": "Draw them?"})
        ]
    );
    assert_eq!(output, ["true"]);
}

fn file_dialogs_return_local_files<E: Engine>() {
    let script = "(async () => {\n\
        const file = await Dialog.openFile({ path: '/tmp/in.svg' })\n\
        console.log(file instanceof LocalFile, file.path)\n\
        console.log(await Dialog.saveFile())\n\
        console.log((await Dialog.chooseDirectory({ path: new LocalFile('/tmp') })).path)\n\
    })()";
    let (questions, output) = run::<E>(script, vec![json!("/tmp/b.svg"), Value::Null, json!("/tmp/out")]);
    assert_eq!(
        questions,
        [
            json!({"type": "openFile", "path": "/tmp", "filters": []}),
            json!({"type": "saveFile", "path": null, "filters": []}),
            json!({"type": "chooseDirectory", "path": "/tmp", "filters": []})
        ]
    );
    assert_eq!(output, ["true /tmp/b.svg", "null", "/tmp/out"], "a cancelled dialog gives null");
    assert_eq!(questions[0]["path"], "/tmp", "open dialogs start in the file's directory");
}

fn file_dialogs_take_file_types<E: Engine>() {
    let script = "Dialog.openFile({ types: { Text: ['txt', '*.text'], SVG: 'svg', 'All Files': '*' } })\n\
        Dialog.saveFile({ types: ['Text Files (*.txt)', '*.txt;*.text', 'All Files', '*.*', '*.svg'] })\n\
        Dialog.openFile({ types: { SVG: 'svg' } })";
    let (questions, _) = run::<E>(script, vec![Value::Null, Value::Null, Value::Null]);
    assert_eq!(questions[2]["filters"], json!([["SVG", ["svg"]]]), "every file only when the script says");
    assert_eq!(questions[0]["filters"], json!([["Text", ["txt", "text"]], ["SVG", ["svg"]], ["All Files", []]]), "in the order written");
    assert_eq!(
        questions[1]["filters"],
        json!([["Text Files (*.txt)", ["txt", "text"]], ["*.svg", ["svg"]], ["All Files", []]]),
        "Scriptographer's pairs, a lone pattern its own label, every file last"
    );
}

fn pickers_give_handles<E: Engine>(name: &str) {
    let folder = std::env::temp_dir().join(format!("scriptographer-pickers-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&folder);
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("in.txt"), "inside").unwrap();
    let path = |name: &str| folder.join(name).to_string_lossy().into_owned();
    let script = "(async () => { try {\n\
        const [file] = await showOpenFilePicker({ types: [{ description: 'Text', accept: { 'text/plain': ['.txt', '.text'] } }] })\n\
        const got = await file.getFile()\n\
        console.log(file instanceof FileSystemFileHandle, file instanceof FileSystemHandle, file.kind, file.name, got instanceof File, got.type, await got.text())\n\
        console.log((await showOpenFilePicker({ multiple: true })).map(h => h.name).join())\n\
        try { await showOpenFilePicker() } catch (e) { console.log(e.name) }\n\
        const directory = await showDirectoryPicker({ startIn: file })\n\
        console.log(directory.kind, await directory.queryPermission(), await file.isSameEntry(await directory.getFileHandle('in.txt')))\n\
        const saved = await showSaveFilePicker({ suggestedName: 'out.txt', startIn: directory, excludeAcceptAllOption: true, types: [{ accept: { 'text/plain': '.txt' } }] })\n\
        const stream = await saved.createWritable()\n\
        await stream.write('hello')\n\
        console.log(JSON.stringify(await (await saved.getFile()).text()))\n\
        await stream.seek(0)\n\
        await stream.write(new Blob(['J']))\n\
        await stream.write({ type: 'write', position: 5, data: new TextEncoder().encode('!?') })\n\
        await stream.truncate(6)\n\
        await stream.close()\n\
        try { await stream.write('more') } catch (e) { console.log(e.name) }\n\
        console.log(await (await saved.getFile()).text())\n\
        const sub = await directory.getDirectoryHandle('sub', { create: true })\n\
        const inner = await sub.getFileHandle('inner.txt', { create: true })\n\
        console.log((await directory.resolve(inner)).join('/'), await sub.resolve(file))\n\
        const names = []\n\
        for await (const [name, handle] of directory) names.push(name + ':' + handle.kind)\n\
        console.log(names.join(), (await directory.keys().next()).value)\n\
        for (const [method, name] of [['getFileHandle', 'missing'], ['getFileHandle', 'sub'], ['getDirectoryHandle', '../x']])\n\
            try { await directory[method](name) } catch (e) { console.log(e.name) }\n\
        try { await directory.removeEntry('sub') } catch (e) { console.log(e.name) }\n\
        await directory.removeEntry('sub', { recursive: true })\n\
        console.log(await LocalFile.from(saved).text(), LocalFile.from(directory).isDirectory)\n\
        try { new FileSystemFileHandle() } catch (e) { console.log(e.name) }\n\
    } catch (e) { console.log('error', e.name, e.message) } })()";
    let answers = vec![json!(path("in.txt")), json!([path("in.txt"), path("out.txt")]), Value::Null, json!(path("")), json!(path("out.txt"))];
    let (questions, output) = run::<E>(script, answers);
    assert_eq!(
        questions[0],
        json!({"type": "openFile", "path": null, "filters": [["Text", ["txt", "text"]], ["All Files", []]]}),
        "the file types, and every file unless left out"
    );
    assert_eq!(questions[1]["type"], "openFiles");
    assert_eq!(questions[3]["path"], json!(path("").trim_end_matches('/')), "startIn a file: its directory");
    assert_eq!(questions[4], json!({"type": "saveFile", "path": path("out.txt"), "filters": [[".txt", ["txt"]]]}), "suggestedName in startIn");
    assert_eq!(
        output,
        [
            "true true file in.txt true text/plain inside",
            "in.txt,out.txt",
            "AbortError",
            "directory granted true",
            "\"\"",
            "TypeError",
            "Jello!",
            "sub/inner.txt null",
            "in.txt:file,out.txt:file,sub:directory in.txt",
            "NotFoundError",
            "TypeMismatchError",
            "TypeError",
            "InvalidModificationError",
            "Jello! true",
            "TypeError"
        ],
        "the stream writes on close"
    );
    std::fs::remove_dir_all(&folder).unwrap();
}

fn choose_color_is_a_prompt_with_one_color<E: Engine>() {
    let script = "console.log(Dialog.chooseColor(new Color('red')).green, Dialog.chooseColor())";
    let (questions, output) = run::<E>(script, vec![json!({"color": "#00ff00"}), Value::Null]);
    assert_eq!(
        questions[0],
        json!({"type": "prompt", "title": "Scriptographer", "components": [{"name": "color", "type": "color", "value": "#ff0000"}]})
    );
    assert_eq!(output, ["1 null"]);
}

fn prompt_returns_the_values<E: Engine>() {
    let script = "var values = { radius: 10 }\n\
        var result = Dialog.prompt('Circles', {\n\
            radius: { type: 'number', label: 'Radius', value: 5 },\n\
            name: { label: 'Name', value: 'circle' },\n\
            fill: { type: 'color', label: 'Fill', value: new Color('red') },\n\
            mode: { label: 'Mode', options: ['one', 'many'], value: 'one' }\n\
        }, values)\n\
        console.log(result === values, values.radius, values.name, values.fill.green, values.mode)";
    let answer = json!({"radius": 20, "name": "dot", "fill": "#00ff00", "mode": "many"});
    let (questions, output) = run::<E>(script, vec![answer]);
    assert_eq!(
        questions[0],
        json!({"type": "prompt", "title": "Circles", "components": [
            {"name": "radius", "type": "number", "label": "Radius", "value": 10},
            {"name": "name", "type": "string", "label": "Name", "value": "circle"},
            {"name": "fill", "type": "color", "label": "Fill", "value": "#ff0000"},
            {"name": "mode", "type": "list", "label": "Mode", "options": ["one", "many"], "value": "one"}
        ]})
    );
    assert_eq!(output, ["true 20 dot 1 many"]);
}

fn a_cancelled_prompt_returns_null<E: Engine>() {
    let (_, output) = run::<E>("console.log(Dialog.prompt('Circles', { radius: { value: 5 } }))", vec![Value::Null]);
    assert_eq!(output, ["null"]);
}

fn without_a_host_dialogs_throw<E: Engine>() {
    let mut runtime = Runtime::<E>::new().unwrap();
    let scope = runtime.create_scope(400.0, 300.0).unwrap();
    let error = runtime.execute(&scope, "Dialog.alert('Hi')").unwrap_err();
    assert!(error.to_string().contains("Dialogs need the desktop app"), "{error}");
}

fn palettes_report_and_take_what_the_user_does<E: Engine>() {
    let mut runtime = Runtime::<E>::new().unwrap();
    let scope = runtime.create_scope(400.0, 300.0).unwrap();
    let script = "var values = { radius: 10 }\n\
        var palette = new Palette('Circles', {\n\
            radius: { type: 'number', label: 'Radius', onChange: function(value) { console.log('radius', value, values.radius) } },\n\
            draw: { type: 'button', value: 'Draw', onClick: function() { console.log('draw', values.radius) } }\n\
        }, values)\n\
        palette.onChange = function(component) { console.log('changed', component.name, component.value, component.label) }";
    runtime.execute(&scope, script).unwrap();
    let palettes = runtime.take_palettes().unwrap();
    assert_eq!(
        palettes,
        [json!({"id": 1, "title": "Circles", "components": [
            {"name": "radius", "type": "number", "label": "Radius", "value": 10},
            {"name": "draw", "type": "button", "value": "Draw"}
        ]})]
    );
    assert!(runtime.take_palettes().unwrap().is_empty(), "reported once, until it changes");
    runtime.palette_event(1, PaletteEvent::Change("radius".into(), json!(20))).unwrap();
    runtime.palette_event(1, PaletteEvent::Click("draw".into())).unwrap();
    assert_eq!(runtime.take_output(), ["radius 20 20", "changed radius 20 Radius", "draw 20"]);
    runtime.execute(&scope, "project.activeLayer.data.x = 1").unwrap();
    assert!(runtime.take_palettes().unwrap().is_empty(), "the change came from the panel");
    assert!(runtime.has_palettes());
    runtime.palette_event(1, PaletteEvent::Close).unwrap();
    assert!(!runtime.has_palettes());
}

fn what_the_script_changes_shows_in_the_palette<E: Engine>() {
    let mut runtime = Runtime::<E>::new().unwrap();
    let scope = runtime.create_scope(400.0, 300.0).unwrap();
    runtime.execute(&scope, "var values = {}\nnew Palette('P', { size: { value: 1 } }, values)\nfunction onFrame() { values.size = 2 }").unwrap();
    assert_eq!(runtime.take_palettes().unwrap()[0]["components"][0]["value"], 1);
    runtime.run_timers(20.0).unwrap();
    assert_eq!(runtime.take_palettes().unwrap()[0]["components"][0]["value"], 2);
}

fn what_the_script_changes_in_a_component_shows_too<E: Engine>() {
    let mut runtime = Runtime::<E>::new().unwrap();
    let scope = runtime.create_scope(400.0, 300.0).unwrap();
    let script = "var palette = new Palette('P', {\n\
            mode: { options: ['a', 'b'], value: 'a' },\n\
            lock: { type: 'button', value: 'Lock', onClick: function() {\n\
                palette.components.mode.enabled = false\n\
                palette.components.mode.options = ['a', 'b', 'c']\n\
            } }\n\
        })";
    runtime.execute(&scope, script).unwrap();
    runtime.take_palettes().unwrap();
    runtime.palette_event(1, PaletteEvent::Click("lock".into())).unwrap();
    let mode = &runtime.take_palettes().unwrap()[0]["components"][0];
    assert_eq!(mode["enabled"], false);
    assert_eq!(mode["options"], json!(["a", "b", "c"]));
}

fn palette_menus_and_reset<E: Engine>() {
    let mut runtime = Runtime::<E>::new().unwrap();
    let scope = runtime.create_scope(400.0, 300.0).unwrap();
    let script = "var values = { size: 5 }\n\
        var palette = new Palette('P', {\n\
            size: { type: 'number', label: 'Size', value: 1, units: 'point' },\n\
            line: { type: 'menu-separator' },\n\
            reset: { label: 'Reset', onSelect: function() { palette.reset(); console.log('reset', values.size, palette.components.size.value) } }\n\
        }, values)";
    runtime.execute(&scope, script).unwrap();
    let components = &runtime.take_palettes().unwrap()[0]["components"];
    assert_eq!(components[0]["value"], 5, "the values given win over the definition's");
    assert_eq!(components[0]["steppers"], true, "a number with units has steppers");
    assert_eq!(components[2]["type"], "menu-entry", "onSelect makes a menu entry");
    runtime.palette_event(1, PaletteEvent::Click("reset".into())).unwrap();
    assert_eq!(runtime.take_output(), ["reset 1 1"], "back to the value it was defined with");
    assert_eq!(runtime.take_palettes().unwrap()[0]["components"][0]["value"], 1, "and the palette shows it");
}

macro_rules! engine_tests {
    ($module:ident, $engine:ty) => {
        mod $module {
            #[test]
            fn palette_menus_and_reset() {
                super::palette_menus_and_reset::<$engine>();
            }

            #[test]
            fn alert_and_confirm_ask_and_wait() {
                super::alert_and_confirm_ask_and_wait::<$engine>();
            }

            #[test]
            fn file_dialogs_return_local_files() {
                super::file_dialogs_return_local_files::<$engine>();
            }

            #[test]
            fn pickers_give_handles() {
                super::pickers_give_handles::<$engine>(stringify!($module));
            }

            #[test]
            fn file_dialogs_take_file_types() {
                super::file_dialogs_take_file_types::<$engine>();
            }

            #[test]
            fn choose_color_is_a_prompt_with_one_color() {
                super::choose_color_is_a_prompt_with_one_color::<$engine>();
            }

            #[test]
            fn prompt_returns_the_values() {
                super::prompt_returns_the_values::<$engine>();
            }

            #[test]
            fn a_cancelled_prompt_returns_null() {
                super::a_cancelled_prompt_returns_null::<$engine>();
            }

            #[test]
            fn palettes_report_and_take_what_the_user_does() {
                super::palettes_report_and_take_what_the_user_does::<$engine>();
            }

            #[test]
            fn what_the_script_changes_shows_in_the_palette() {
                super::what_the_script_changes_shows_in_the_palette::<$engine>();
            }

            #[test]
            fn what_the_script_changes_in_a_component_shows_too() {
                super::what_the_script_changes_in_a_component_shows_too::<$engine>();
            }

            #[test]
            fn without_a_host_dialogs_throw() {
                super::without_a_host_dialogs_throw::<$engine>();
            }
        }
    };
}

#[cfg(all(feature = "v8", not(target_arch = "wasm32")))]
engine_tests!(v8, vectorcraft_scriptographer::V8);
#[cfg(feature = "boa")]
engine_tests!(boa, vectorcraft_scriptographer::Boa);
