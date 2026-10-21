//! Scripts on threads of their own: requests go to the script's thread, replies come back.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
#![cfg(not(target_arch = "wasm32"))]

use std::time::Duration;

use serde_json::json;
use vectorcraft_engine::Session;
use vectorcraft_scriptographer::Setup;
use vectorcraft_scriptographer::worker::{Reply, Request, Work, Worker};

const WAIT: Duration = Duration::from_secs(10);

fn session() -> Session {
    let mut s = Session::new();
    s.execute("file.new", &json!({})).unwrap();
    s.execute("shape.rectangle", &json!({"x": 10, "y": 10, "width": 30, "height": 20})).unwrap();
    s
}

fn spawn(s: &Session) -> Worker {
    let st = s.active().unwrap();
    Worker::spawn((400.0, 300.0), Setup::default(), st.doc.clone(), st.selection.clone()).unwrap()
}

fn send(s: &Session, worker: &Worker, work: Work) {
    let st = s.active().unwrap();
    // The styles the app gives scripts.
    let paint = &s.paint;
    let style = vectorcraft_scriptographer::host::current_style(&paint.fill, &paint.stroke, paint.stroke_width, &s.new_type_style().fill);
    worker.send(Request::Run { doc: st.doc.clone(), selection: st.selection.clone(), artboard: 0, style, work });
}

/// Applies the replies until `Done`, as the app does, and returns `Done`'s error.
fn settle(s: &mut Session, worker: &Worker) -> Option<String> {
    loop {
        match worker.recv(WAIT).expect("a reply") {
            Reply::Changes { patch, .. } => {
                let result = s.execute("document.patch", &patch.unwrap()).unwrap();
                let st = s.active().unwrap();
                worker.send(Request::Applied { result, doc: st.doc.clone(), selection: st.selection.clone(), behind: false });
            }
            Reply::Done { error, .. } => return error,
            Reply::Ask(question) => panic!("unexpected dialog: {question}"),
        }
    }
}

fn left(s: &Session) -> f64 {
    s.active().unwrap().doc.layers[0].children().unwrap()[0].geometric_bounds().unwrap().x0
}

#[test]
fn a_script_runs_on_its_thread_and_its_changes_come_back() {
    let mut s = session();
    let worker = spawn(&s);
    send(&s, &worker, Work::Execute("var item = project.activeLayer.firstChild\nfunction onFrame() { item.position += [10, 0] }".into()));
    assert_eq!(settle(&mut s, &worker), None);
    for (i, now) in [20.0, 40.0].into_iter().enumerate() {
        send(&s, &worker, Work::Frame(now));
        assert_eq!(settle(&mut s, &worker), None);
        assert_eq!(left(&s), 20.0 + 10.0 * i as f64, "each frame moves it");
    }
}

/// A script that loops forever stops when the app interrupts it, as Stop does.
#[test]
fn an_endless_loop_stops_when_interrupted() {
    let mut s = session();
    let worker = spawn(&s);
    send(&s, &worker, Work::Execute("while (true) {}".into()));
    assert!(worker.recv(Duration::from_millis(200)).is_none(), "it loops, and the app goes on");
    worker.interrupt();
    assert!(settle(&mut s, &worker).is_some(), "it stopped with an error");
}

/// A dialog waits on the script's thread for the app's answer.
#[test]
fn a_dialog_waits_for_the_answer() {
    let s = session();
    let worker = spawn(&s);
    send(&s, &worker, Work::Execute("console.log(Dialog.confirm('Sure?'))".into()));
    let Some(Reply::Ask(question)) = worker.recv(WAIT) else { panic!("a question") };
    assert_eq!(question["message"], "Sure?");
    assert!(worker.recv(Duration::from_millis(100)).is_none(), "the script waits");
    worker.send(Request::Answer(json!(true)));
    let Some(Reply::Done { output, error: None, .. }) = worker.recv(WAIT) else { panic!("done") };
    assert_eq!(output, ["true"]);
}

/// Text on a path that a script moves goes on moving with it after the app changes the text, as
/// when the user selects all and clears the fill.
#[test]
fn text_on_a_path_keeps_its_path_when_the_app_changes_it() {
    let mut s = session();
    let worker = spawn(&s);
    let script = "var path = new Path.Circle([100, 100], 25)\n\
                  var text = new PathText(path)\n\
                  text.content = 'This text runs along the path.'\n\
                  function onFrame() { path.position += [10, 0] }";
    send(&s, &worker, Work::Execute(script.into()));
    assert_eq!(settle(&mut s, &worker), None);
    let path_left = |s: &Session| {
        let st = s.active().unwrap();
        st.doc.layers[0].children().unwrap().iter().find_map(|n| match &n.kind {
            vectorcraft_doc::NodeKind::Text(t) => t.kind.path_span().and(n.geometric_bounds()).map(|b| b.x0),
            _ => None,
        })
    };
    send(&s, &worker, Work::Frame(20.0));
    assert_eq!(settle(&mut s, &worker), None);
    let before = path_left(&s).expect("the text on its path");
    s.execute("select.all", &json!({})).unwrap();
    s.execute("paint.setFill", &json!({"none": true})).unwrap();
    send(&s, &worker, Work::Frame(40.0));
    assert_eq!(settle(&mut s, &worker), None);
    assert!(path_left(&s).unwrap() > before + 5.0, "the text moves on with its path");
}

/// New text takes the style the app's Type tool gives new type, a fill without a stroke, while new
/// paths take the app's current paint, a white fill and a black stroke by default.
#[test]
fn new_text_takes_the_text_style_and_new_paths_the_current_style() {
    let mut s = session();
    let worker = spawn(&s);
    send(&s, &worker, Work::Execute("new Path.Rectangle([0, 0], [10, 10])\nnew PointText({ point: [20, 20], content: 'Hi' })".into()));
    assert_eq!(settle(&mut s, &worker), None);
    let st = s.active().unwrap();
    let nodes = st.doc.layers[0].children().unwrap();
    let text = nodes.iter().find_map(|n| match &n.kind {
        vectorcraft_doc::NodeKind::Text(t) => Some(t.runs[0].style.clone()),
        _ => None,
    });
    let text = text.expect("the text");
    assert_eq!(text.fill, vectorcraft_color::Paint::solid(vectorcraft_color::Color::BLACK), "black, as the fill is white");
    assert!(!text.has_stroke(), "no stroke");
    let path = nodes.iter().rev().find(|n| matches!(n.kind, vectorcraft_doc::NodeKind::Path { .. })).unwrap();
    assert!(!path.appearance.stroke().unwrap().paint.is_none(), "the path's stroke");
}
