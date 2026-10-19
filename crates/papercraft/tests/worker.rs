//! Scripts on threads of their own: requests go to the script's thread, replies come back.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
#![cfg(not(target_arch = "wasm32"))]

use std::time::Duration;

use serde_json::json;
use vectorcraft_engine::Session;
use vectorcraft_papercraft::worker::{Reply, Request, Work, Worker};

const WAIT: Duration = Duration::from_secs(10);

fn session() -> Session {
    let mut s = Session::new();
    s.execute("file.new", &json!({})).unwrap();
    s.execute("shape.rectangle", &json!({"x": 10, "y": 10, "width": 30, "height": 20})).unwrap();
    s
}

fn spawn(s: &Session) -> Worker {
    let st = s.active().unwrap();
    Worker::spawn((400.0, 300.0), None, serde_json::json!({}), st.doc.clone(), st.selection.clone()).unwrap()
}

fn send(s: &Session, worker: &Worker, work: Work) {
    let st = s.active().unwrap();
    let style = serde_json::json!({ "strokeColor": [0, 0, 0], "strokeWidth": 1 });
    worker.send(Request::Run { doc: st.doc.clone(), selection: st.selection.clone(), style, work });
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
