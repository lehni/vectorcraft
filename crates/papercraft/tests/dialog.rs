//! Scriptographer's `Dialog` and `Palette`: the script asks, the host answers, the script goes on
//! with it.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::cell::RefCell;
use std::rc::Rc;

use serde_json::{Value, json};
use vectorcraft_papercraft::{Engine, PaletteEvent, Runtime};

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
            json!({"type": "alert", "title": "PaperCraft", "message": "Done."}),
            json!({"type": "confirm", "title": "Circles", "message": "Draw them?"})
        ]
    );
    assert_eq!(output, ["true"]);
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
        palette.onChange = function(component) { console.log('changed', component.label) }";
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
    assert_eq!(runtime.take_output(), ["radius 20 20", "changed Radius", "draw 20"]);
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

macro_rules! engine_tests {
    ($module:ident, $engine:ty) => {
        mod $module {
            #[test]
            fn alert_and_confirm_ask_and_wait() {
                super::alert_and_confirm_ask_and_wait::<$engine>();
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
engine_tests!(v8, vectorcraft_papercraft::V8);
#[cfg(feature = "boa")]
engine_tests!(boa, vectorcraft_papercraft::Boa);
