#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Text in scripts: point type, area type and type on a path are Paper.js's PointText, AreaText
//! and PathText, laid out by VectorCraft's text layout, and what a script changes in them goes
//! back to the document.

use serde_json::json;
use vectorcraft_doc::{Justify, Node, NodeId, NodeKind, TextObject};
use vectorcraft_engine::Session;
use vectorcraft_scriptographer::host::{Columns, flags as host_flags};
use vectorcraft_scriptographer::{Engine, Runtime};

/// A session with one layer holding a rectangle, then point type "Hello world" at 24 pt, and area
/// type that breaks into several lines.
fn document() -> (Session, NodeId, NodeId, NodeId) {
    let mut s = Session::new();
    s.execute("file.new", &json!({"width": 400, "height": 300})).unwrap();
    let id = |v: serde_json::Value| NodeId(v["id"].as_u64().unwrap());
    let rectangle = id(s.execute("shape.rectangle", &json!({"x": 200, "y": 10, "width": 30, "height": 20})).unwrap());
    let point = id(s.execute("text.create", &json!({"x": 10, "y": 50, "text": "Hello world", "size": 24})).unwrap());
    let area = id(s
        .execute(
            "text.create",
            &json!({"x": 10, "y": 100, "text": "Text in an area breaks into lines that fit it.", "size": 12,
                "area": {"width": 80, "height": 150}}),
        )
        .unwrap());
    (s, rectangle, point, area)
}

/// Runs `script` on the session's document and applies what it changed, as the app does.
fn run<E: Engine>(s: &mut Session, script: &str) -> Vec<String> {
    let st = s.active().unwrap();
    let (doc, selection) = (st.doc.clone(), st.selection.clone());
    let mut runtime = Runtime::<E>::new().unwrap();
    let scope = runtime.create_scope(400.0, 300.0).unwrap();
    runtime.bind(&scope, doc, selection).unwrap();
    runtime.execute(&scope, script).unwrap();
    if let Some(patch) = runtime.take_patch(&scope).unwrap() {
        s.execute("document.patch", &patch.params).unwrap();
    }
    runtime.take_output()
}

fn node(s: &Session, id: NodeId) -> Node {
    s.active().unwrap().doc.node(id).unwrap().clone()
}

fn text(s: &Session, id: NodeId) -> TextObject {
    match node(s, id).kind {
        NodeKind::Text(t) => *t,
        _ => panic!("{id} isn't text"),
    }
}

fn reads_text<E: Engine>() {
    let (mut s, _, point, area) = document();
    let output = run::<E>(
        &mut s,
        &format!(
            "var point = project.getItemByKey({}), area = project.getItemByKey({})\n\
             console.log(point.className, point.content, point.fontSize, point.point)\n\
             console.log(area.className, area.textPath.className, area.textPath.bounds.width)",
            point.0, area.0
        ),
    );
    assert_eq!(output, ["PointText Hello world 24 { x: 10, y: 50 }", "AreaText Path 80"]);
}

/// The lines scripts see are those of VectorCraft's layout of the text.
fn lines_come_from_the_documents_layout<E: Engine>() {
    let (mut s, _, _, area) = document();
    let layout = vectorcraft_text::layout(vectorcraft_text::FontDb::global(), &text(&s, area));
    let plain = text(&s, area).plain_text();
    let expected: Vec<String> = layout.lines.iter().map(|line| plain.get(line.start..line.end).unwrap().trim().to_owned()).collect();
    let baselines: Vec<f64> = layout.lines.iter().map(|line| line.baseline).collect();
    assert!(expected.len() > 1, "the area breaks the text into lines");
    let output = run::<E>(
        &mut s,
        &format!(
            "var area = project.getItemByKey({})\n\
             console.log(JSON.stringify(area.lines.map(function(line) {{ return line.content.trim() }})))\n\
             console.log(JSON.stringify(area.lines.map(function(line) {{ return line.baseline.point.y }})))\n\
             console.log(area.visibleRange.end === area.content.length)",
            area.0
        ),
    );
    assert_eq!(output[0], serde_json::to_string(&expected).unwrap(), "the lines of the document's layout");
    assert_eq!(serde_json::from_str::<Vec<f64>>(&output[1]).unwrap(), baselines, "and their baselines");
    assert_eq!(output[2], "true", "all of it visible");
}

/// Text the script didn't change is laid out as the document has it, without its JSON.
fn lays_out_unchanged_text_from_the_document<E: Engine>() {
    let (mut s, _, _, area) = document();
    let lines = vectorcraft_text::layout(vectorcraft_text::FontDb::global(), &text(&s, area)).lines.len();
    let output = run::<E>(&mut s, &format!("console.log(project.host.layOut(null, 0, {}).lines.length)", area.0));
    assert_eq!(output, [lines.to_string()]);
}

/// Threads the session's area type into a larger second frame beside it, after giving it more text
/// than it holds, so the story flows on into the second frame. Returns that frame.
fn threaded(s: &mut Session, area: NodeId) -> NodeId {
    let story = "Text in an area breaks into lines that fit it. ".repeat(5);
    s.execute("text.setText", &json!({"id": area.0, "text": story})).unwrap();
    NodeId(s.execute("text.thread.add", &json!({"from": area.0, "rect": [120, 100, 200, 300]})).unwrap()["id"].as_u64().unwrap())
}

/// The frames of a thread are linked text items, which share the whole story, and each one is laid
/// out from where the frame before it ends, as the document splits it.
fn threads_share_their_story<E: Engine>() {
    let (mut s, _, _, area) = document();
    let next = threaded(&mut s, area);
    let (first, rest) = (text(&s, area).plain_text(), text(&s, next).plain_text());
    assert!(!first.is_empty() && !rest.is_empty(), "the story flows across both frames");
    let story = first.clone() + &rest;
    // Touching the second frame first loads the first one into its story.
    let output = run::<E>(
        &mut s,
        &format!(
            "var next = project.getItemByKey({}), first = project.getItemByKey({})\n\
             console.log(next.isLinked(), first.story == next.story, first.nextTextItem == next, next.content.length)\n\
             console.log(first.visibleRange.start, first.visibleRange.end, next.visibleRange.start, next.visibleRange.end)\n\
             var layout = project.host.layOut(null, {}, {})\n\
             console.log(layout.lines[0].start, layout.end)",
            next.0,
            area.0,
            first.len(),
            next.0
        ),
    );
    assert_eq!(
        output,
        [
            format!("true true true {}", story.len()),
            format!("0 {} {} {}", first.len(), first.len(), story.len()),
            format!("{} {}", first.len(), story.len())
        ]
    );
}

/// A story a script changes flows across the frames of its thread again, whichever frame it was
/// changed through.
fn a_changed_story_flows_across_its_frames<E: Engine>() {
    for through in [0, 1] {
        let (mut s, _, _, area) = document();
        let next = threaded(&mut s, area);
        let story = "A story told anew, word by word. ".repeat(8);
        let key = [area, next][through];
        run::<E>(&mut s, &format!("project.getItemByKey({}).content = {:?}", key.0, story));
        let (first, rest) = (text(&s, area).plain_text(), text(&s, next).plain_text());
        assert!(!first.is_empty() && !rest.is_empty(), "flows across both frames: {first:?} {rest:?}");
        assert_eq!(first + &rest, story, "through frame {through}");
        assert_eq!(s.active().unwrap().doc.text_threads, [vec![area, next]]);
    }
}

fn threads(s: &Session) -> Vec<Vec<NodeId>> {
    s.active().unwrap().doc.text_threads.clone()
}

/// Text items a script links become a thread, its story flowing across them; an item it unlinks
/// leaves the thread, and its text stays with the story.
fn scripts_link_and_unlink_frames<E: Engine>() {
    let (mut s, _, _, area) = document();
    let other = NodeId(
        s.execute("text.create", &json!({"x": 120, "y": 100, "text": "More.", "size": 12, "area": {"width": 200, "height": 300}})).unwrap()["id"]
            .as_u64()
            .unwrap(),
    );
    s.execute("text.setText", &json!({"id": area.0, "text": "Text in an area breaks into lines that fit it. ".repeat(5)})).unwrap();
    let output =
        run::<E>(&mut s, &format!("var a = project.getItemByKey({}); a.link(project.getItemByKey({})); console.log(a.content)", area.0, other.0));
    let story = output[0].clone();
    assert_eq!(threads(&s), [vec![area, other]]);
    let (first, rest) = (text(&s, area).plain_text(), text(&s, other).plain_text());
    assert!(!rest.is_empty(), "the story flows on into the second frame");
    assert_eq!(first + &rest, story);
    run::<E>(&mut s, &format!("project.getItemByKey({}).unlink()", other.0));
    assert!(threads(&s).is_empty());
    assert_eq!((text(&s, area).plain_text(), text(&s, other).plain_text()), (story, String::new()));
}

/// Text along two paths a script links becomes a thread of type on a path: the story runs along
/// the first path and on along the second.
fn scripts_link_text_on_paths<E: Engine>() {
    let (mut s, ..) = document();
    let story = "This text runs along two paths, one after the other.";
    run::<E>(
        &mut s,
        &format!(
            "var first = new PathText(new Path.Line([20, 300], [80, 300]))\n\
             first.content = {story:?}\n\
             first.link(new PathText(new Path.Line([20, 350], [380, 350])))"
        ),
    );
    let threads = threads(&s);
    let [thread] = threads.as_slice() else { panic!("one thread: {threads:?}") };
    let shares: Vec<String> = thread.iter().map(|id| text(&s, *id).plain_text()).collect();
    assert!(thread.iter().all(|id| matches!(text(&s, *id).kind, vectorcraft_doc::TextKind::OnPath { .. })));
    assert_eq!(shares.len(), 2);
    assert!(shares.iter().all(|t| !t.is_empty()), "the story runs on along the second path: {shares:?}");
    assert_eq!(shares.concat(), story);
}

/// A new text item a script links into a thread becomes a frame of it.
fn scripts_link_new_frames<E: Engine>() {
    let (mut s, _, _, area) = document();
    s.execute("text.setText", &json!({"id": area.0, "text": "Text in an area breaks into lines that fit it. ".repeat(5)})).unwrap();
    let story = text(&s, area).plain_text();
    run::<E>(&mut s, &format!("project.getItemByKey({}).link(new AreaText(new Rectangle(120, 100, 200, 300)))", area.0));
    let threads = threads(&s);
    let [thread] = threads.as_slice() else { panic!("one thread: {threads:?}") };
    let [first, new] = thread.as_slice() else { panic!("two frames: {thread:?}") };
    assert_eq!(*first, area);
    assert!(!text(&s, *new).plain_text().is_empty(), "the story flows into the new frame");
    assert_eq!(text(&s, area).plain_text() + &text(&s, *new).plain_text(), story);
}

/// A frame a script changes but not the story of keeps its share, and the story flows into its new
/// shape.
fn a_changed_frame_keeps_the_story<E: Engine>() {
    let (mut s, _, _, area) = document();
    let next = threaded(&mut s, area);
    let story = text(&s, area).plain_text() + &text(&s, next).plain_text();
    let before = text(&s, area).plain_text().len();
    run::<E>(&mut s, &format!("project.getItemByKey({}).textPath.scale(1, 0.5, [0, 100])", area.0));
    let (first, rest) = (text(&s, area).plain_text(), text(&s, next).plain_text());
    assert!(first.len() < before, "the smaller frame holds less: {} of {before}", first.len());
    assert_eq!(first + &rest, story, "and the story stays whole");
}

/// When one frame of a thread changes, scripts reload every frame of it, as each one holds the
/// whole story: also the frames whose objects didn't change.
#[test]
fn a_threads_frames_reload_together() {
    let (mut s, _, _, area) = document();
    let next = threaded(&mut s, area);
    let st = s.active().unwrap();
    let (old, selection) = (st.doc.clone(), st.selection.clone());
    let mut new = (*old).clone();
    new.node_mut(area).unwrap().name = Some("Renamed".into());
    let columns = Columns::of_changes(&old, &selection, &new, &selection);
    let flags = |id: NodeId| columns.keys.iter().position(|k| *k == id.0 as f64).map(|i| columns.flags[i]);
    assert!(flags(next).is_some_and(|f| f & host_flags::CONTENT_CHANGED != 0), "the other frame reloads: {:?}", columns.keys);
}

fn writes_text_back_as_one_undo_step<E: Engine>() {
    let (mut s, _, point, _) = document();
    let before = s.execute("document.json", &json!({})).unwrap();
    let family = text(&s, point).runs[0].style.font_family.clone();
    run::<E>(
        &mut s,
        &format!(
            "var point = project.getItemByKey({})\n\
             point.content = 'Changed'\n\
             point.fontSize = 30\n\
             point.paragraphs[0].justification = 'center'",
            point.0
        ),
    );
    let t = text(&s, point);
    assert_eq!(t.plain_text(), "Changed");
    assert_eq!(t.runs[0].style.size, 30.0);
    assert_eq!(t.para.justify, Justify::Center);
    assert_eq!(t.runs[0].style.font_family, family, "what the script didn't change stays");
    assert!(t.cached_bounds.is_some(), "laid out again");
    s.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(s.execute("document.json", &json!({})).unwrap(), before, "one undo step undoes the script");
}

fn ranges_style_runs<E: Engine>() {
    let (mut s, _, point, _) = document();
    run::<E>(&mut s, &format!("var range = project.getItemByKey({}).range(0, 5)\n range.fontSize = 40\n range.fillColor = 'red'", point.0));
    let runs = text(&s, point).runs;
    let sizes: Vec<(String, f64)> = runs.iter().map(|r| (r.text.clone(), r.style.size)).collect();
    assert_eq!(sizes, [("Hello".to_owned(), 40.0), (" world".to_owned(), 24.0)]);
    assert_eq!(runs[0].style.fill, vectorcraft_color::Paint::solid(vectorcraft_color::Color::rgb(1.0, 0.0, 0.0)), "the range's colour");
}

fn area_options_go_back<E: Engine>() {
    let (mut s, _, _, area) = document();
    run::<E>(&mut s, &format!("var area = project.getItemByKey({})\n area.columns = 2\n area.gutter = 6", area.0));
    let t = text(&s, area);
    assert_eq!((t.area.columns, t.area.gutter), (2, 6.0));
    // Paper.js leaves its defaults out of its JSON, and they still go back:
    run::<E>(&mut s, &format!("project.getItemByKey({}).columns = 1", area.0));
    assert_eq!(text(&s, area).area.columns, 1, "back to one column");
}

fn scripts_make_new_text<E: Engine>() {
    let (mut s, ..) = document();
    run::<E>(&mut s, "new PointText({ point: [20, 200], content: 'New', fontSize: 18 })");
    let layer = s.active().unwrap().doc.layers[0].clone();
    let NodeKind::Text(t) = &layer.children().unwrap().last().unwrap().kind else {
        panic!("the new item is text");
    };
    assert_eq!((t.plain_text(), t.runs[0].style.size), ("New".to_owned(), 18.0));
    assert_eq!(t.xf.translation(), vectorcraft_geom::Vec2::new(20.0, 200.0), "at its point");
    assert!(t.cached_bounds.is_some(), "laid out");
}

/// Text a script makes on a path stays out of threads, as Scriptographer's tutorial makes it.
fn scripts_make_text_on_a_path<E: Engine>() {
    let (mut s, ..) = document();
    run::<E>(&mut s, "var textItem = new PathText(new Path.Circle(new Point(100, 100), 25))\ntextItem.content = 'This text runs along the path.'");
    let layer = s.active().unwrap().doc.layers[0].clone();
    let texts: Vec<String> = layer
        .children()
        .unwrap()
        .iter()
        .filter_map(|n| match &n.kind {
            NodeKind::Text(t) if matches!(t.kind, vectorcraft_doc::TextKind::OnPath { .. }) => Some(t.plain_text()),
            _ => None,
        })
        .collect();
    assert_eq!(texts, ["This text runs along the path."]);
    assert!(threads(&s).is_empty());
}

fn type_on_a_path_is_path_text<E: Engine>() {
    let (mut s, ..) = document();
    let path = NodeId(s.execute("shape.ellipse", &json!({"x": 50, "y": 150, "width": 100, "height": 100})).unwrap()["id"].as_u64().unwrap());
    let on_path =
        NodeId(s.execute("text.createInPath", &json!({"path": path.0, "mode": "onPath", "text": "Around"})).unwrap()["id"].as_u64().unwrap());
    let output = run::<E>(
        &mut s,
        &format!(
            "var text = project.getItemByKey({})\n\
             console.log(text.className, text.content, text.textPath.className, text.lines.length)\n\
             text.content = 'Around and around'",
            on_path.0
        ),
    );
    assert_eq!(output, ["PathText Around Path 1"]);
    assert_eq!(text(&s, on_path).plain_text(), "Around and around");
    assert!(matches!(text(&s, on_path).kind, vectorcraft_doc::TextKind::OnPath { .. }), "still on its path");
}

fn items_wrap_text<E: Engine>() {
    let (mut s, rectangle, ..) = document();
    run::<E>(&mut s, &format!("project.getItemByKey({}).textWrap = {{ offset: 10 }}", rectangle.0));
    let wrap = node(&s, rectangle).extras().wrap.unwrap();
    assert_eq!((wrap.offset, wrap.invert), (10.0, false));
}

macro_rules! engine_tests {
    ($module:ident, $engine:ty) => {
        mod $module {
            #[test]
            fn reads_text() {
                super::reads_text::<$engine>();
            }

            #[test]
            fn lines_come_from_the_documents_layout() {
                super::lines_come_from_the_documents_layout::<$engine>();
            }

            #[test]
            fn lays_out_unchanged_text_from_the_document() {
                super::lays_out_unchanged_text_from_the_document::<$engine>();
            }

            #[test]
            fn threads_share_their_story() {
                super::threads_share_their_story::<$engine>();
            }

            #[test]
            fn a_changed_story_flows_across_its_frames() {
                super::a_changed_story_flows_across_its_frames::<$engine>();
            }

            #[test]
            fn scripts_link_and_unlink_frames() {
                super::scripts_link_and_unlink_frames::<$engine>();
            }

            #[test]
            fn scripts_link_text_on_paths() {
                super::scripts_link_text_on_paths::<$engine>();
            }

            #[test]
            fn scripts_link_new_frames() {
                super::scripts_link_new_frames::<$engine>();
            }

            #[test]
            fn a_changed_frame_keeps_the_story() {
                super::a_changed_frame_keeps_the_story::<$engine>();
            }

            #[test]
            fn writes_text_back_as_one_undo_step() {
                super::writes_text_back_as_one_undo_step::<$engine>();
            }

            #[test]
            fn ranges_style_runs() {
                super::ranges_style_runs::<$engine>();
            }

            #[test]
            fn area_options_go_back() {
                super::area_options_go_back::<$engine>();
            }

            #[test]
            fn scripts_make_new_text() {
                super::scripts_make_new_text::<$engine>();
            }

            #[test]
            fn scripts_make_text_on_a_path() {
                super::scripts_make_text_on_a_path::<$engine>();
            }

            #[test]
            fn type_on_a_path_is_path_text() {
                super::type_on_a_path_is_path_text::<$engine>();
            }

            #[test]
            fn items_wrap_text() {
                super::items_wrap_text::<$engine>();
            }
        }
    };
}

#[cfg(feature = "boa")]
engine_tests!(boa, vectorcraft_scriptographer::Boa);
#[cfg(all(feature = "v8", not(target_arch = "wasm32")))]
engine_tests!(v8, vectorcraft_scriptographer::V8);
