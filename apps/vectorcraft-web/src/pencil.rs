//! Pen and touch input (an iPad with an Apple Pencil): the shell reads Pointer Events itself, in
//! the capture phase, before eframe's own touch and pointer handlers, which can't tell a pen from
//! a finger and see each touch twice (as a pointer and as a touch event).
//!
//! - A pen drives egui's pointer and is a touch contact with the pen's pressure as its force (the
//!   canvas reads it), so holding it still opens the context menu (egui's long touch). It hovers
//!   where the pen can (Apple Pencil hover).
//! - Fingers are touch contacts: two pinch and pan the canvas, and tap to undo or redo. The first
//!   finger also drives the pointer, so it taps the panels and menus; on the canvas it navigates
//!   only once a pen has been seen ([`touch::set_source`]), for this page session only.
//! - Palm rejection: while a pen is down, fingers are ignored; a pen landing cancels the fingers
//!   already down (a palm resting first).
//!
//! Mouse and trackpad events (a Magic Keyboard) are left to eframe. Until the first pen
//! contact, fingers and their DOM touch events also stay with eframe (including its text agent).
//! Their times, which egui leaves out, are noted for the app ([`Pointers::take_times`]).

#[cfg(target_arch = "wasm32")]
use std::cell::RefCell;
#[cfg(target_arch = "wasm32")]
use std::rc::Rc;

use egui::{Event, PointerButton, Pos2, TouchDeviceId, TouchId, TouchPhase};
use vectorcraft_ui_egui::canvas::PointerTimes;
use vectorcraft_ui_egui::touch::{self, Source};
#[cfg(target_arch = "wasm32")]
use wasm_bindgen::JsCast as _;

/// The pen's touch device (fingers use another, so egui doesn't pinch a pen with a finger).
const PEN_DEVICE: TouchDeviceId = TouchDeviceId(1);
const FINGER_DEVICE: TouchDeviceId = TouchDeviceId(2);

/// The pen and finger contacts followed, and the egui events they made since the last frame.
#[derive(Default)]
pub struct Pointers {
    events: Vec<Event>,
    /// The pen pressing, by pointer id.
    pen: Option<i32>,
    /// The fingers down, by pointer id.
    fingers: Vec<i32>,
    /// The contact driving egui's pointer.
    primary: Option<i32>,
    button: Option<PointerButton>,
    /// The times of the mouse events eframe handles, since the last frame.
    times: PointerTimes,
}

/// The most times noted of a kind between two frames: a frame that doesn't come (the tab hidden)
/// leaves no list growing.
const MOST_TIMES: usize = 256;

#[cfg(target_arch = "wasm32")]
pub type SharedPointers = Rc<RefCell<Pointers>>;

/// Browser-independent snapshot, also used by the routing regression tests.
struct Input<'a> {
    pointer_type: &'a str,
    id: i32,
    button: i16,
    pressure: f32,
    modifiers: egui::Modifiers,
}

// Match eframe's button_from_mouse_event, including barrel/right and auxiliary buttons.
fn button(number: i16) -> Option<PointerButton> {
    match number {
        0 => Some(PointerButton::Primary),
        1 => Some(PointerButton::Middle),
        2 => Some(PointerButton::Secondary),
        3 => Some(PointerButton::Extra1),
        4 => Some(PointerButton::Extra2),
        _ => None,
    }
}

impl Pointers {
    /// The events since the last frame, for the frame's raw input.
    pub fn take(&mut self) -> Vec<Event> {
        std::mem::take(&mut self.events)
    }

    /// The times of the mouse events since the last frame, for the app.
    pub fn take_times(&mut self) -> PointerTimes {
        std::mem::take(&mut self.times)
    }

    /// Notes the time of a mouse event that eframe handles: its DOM type `kind` and `timeStamp`
    /// (milliseconds by `performance.now()`, whose seconds are eframe's clock). Moves, and presses
    /// and releases of the primary button.
    fn note_time(&mut self, kind: &str, pointer_type: &str, button: i16, time_stamp: f64) {
        let list = match kind {
            "mousemove" => &mut self.times.moves,
            "pointerdown" if pointer_type == "mouse" && button == 0 => &mut self.times.downs,
            "pointerup" if pointer_type == "mouse" && button == 0 => &mut self.times.ups,
            _ => return,
        };
        if list.len() < MOST_TIMES {
            list.push(time_stamp / 1000.0);
        }
    }

    fn touch(&mut self, device: TouchDeviceId, id: i32, phase: TouchPhase, pos: Pos2, force: Option<f32>) {
        self.events.push(Event::Touch { device_id: device, id: TouchId::from(id as u64), phase, pos, force });
    }

    fn press(&mut self, id: i32, pos: Pos2, button: PointerButton, modifiers: egui::Modifiers) {
        self.primary = Some(id);
        self.button = Some(button);
        self.events.push(Event::ModifiersChanged(modifiers));
        self.events.push(Event::PointerMoved(pos));
        self.events.push(Event::PointerButton { pos, button, pressed: true, modifiers });
    }

    fn release(&mut self, pos: Pos2, modifiers: egui::Modifiers) {
        self.primary = None;
        if let Some(button) = self.button.take() {
            self.events.push(Event::ModifiersChanged(modifiers));
            self.events.push(Event::PointerButton { pos, button, pressed: false, modifiers });
        }
    }

    /// Cancel every finger down (a palm under a pen landing).
    fn cancel_fingers(&mut self, pos: Pos2, modifiers: egui::Modifiers) {
        for id in std::mem::take(&mut self.fingers) {
            self.touch(FINGER_DEVICE, id, TouchPhase::Cancel, pos, None);
            if self.primary == Some(id) {
                self.release(pos, modifiers);
                self.events.push(Event::PointerGone);
            }
        }
    }

    /// The paired DOM touch events must follow the same routing as pointer events.
    fn captures_touch(&self, on_canvas: bool) -> bool {
        touch::pen_seen() && (on_canvas || self.pen.is_some() || !self.fingers.is_empty())
    }

    /// Follow one pointer event; `true` if it was the pen's or a finger's (taken from eframe).
    fn feed(&mut self, kind: &str, e: &Input<'_>, pos: Pos2, coalesced: &[(Pos2, f32)], on_canvas: bool) -> bool {
        let id = e.id;
        match e.pointer_type {
            "pen" => {
                let force = Some(e.pressure.clamp(0.0, 1.0));
                match kind {
                    "pointerdown" => {
                        if !on_canvas && self.pen.is_none() {
                            return false;
                        }
                        let Some(button) = button(e.button) else { return false };
                        touch::set_source(Source::Pen);
                        self.cancel_fingers(pos, e.modifiers);
                        self.pen = Some(id);
                        self.touch(PEN_DEVICE, id, TouchPhase::Start, pos, force);
                        self.press(id, pos, button, e.modifiers);
                    }
                    "pointermove" if self.pen == Some(id) => {
                        // The positions the pen reported between two events (up to 240 a second
                        // on an iPad), each with its pressure, so fast strokes keep their curve.
                        for c in coalesced.iter().take(coalesced.len().saturating_sub(1)) {
                            self.touch(PEN_DEVICE, id, TouchPhase::Move, c.0, Some(c.1));
                            self.events.push(Event::PointerMoved(c.0));
                        }
                        self.touch(PEN_DEVICE, id, TouchPhase::Move, pos, force);
                        self.events.push(Event::PointerMoved(pos));
                    }
                    // Hovering (Apple Pencil hover): the pointer follows without pressing.
                    "pointermove" => {
                        if self.primary.is_some() {
                            return true;
                        }
                        if !on_canvas {
                            return false;
                        }
                        self.events.push(Event::PointerMoved(pos));
                    }
                    "pointerup" | "pointercancel" if self.pen == Some(id) => {
                        let phase = if kind == "pointerup" { TouchPhase::End } else { TouchPhase::Cancel };
                        self.pen = None;
                        self.touch(PEN_DEVICE, id, phase, pos, None);
                        self.release(pos, e.modifiers);
                    }
                    // A pen leaving the hover range, or an event of a press we didn't take.
                    _ => {
                        if kind == "pointerleave" && self.primary.is_none() && on_canvas {
                            self.events.push(Event::PointerGone);
                        }
                        return self.pen.is_some();
                    }
                }
                true
            }
            // Keep eframe's touch/IME path unchanged on touch-only devices. Reloading the
            // page resets this policy; an old localStorage marker is deliberately ignored.
            "touch" if !touch::pen_seen() => false,
            "touch" => {
                match kind {
                    "pointerdown" => {
                        // Palm rejection: nothing presses while the pen does.
                        if self.pen.is_some() {
                            return true;
                        }
                        if !on_canvas && self.fingers.is_empty() {
                            return false;
                        }
                        self.fingers.push(id);
                        self.touch(FINGER_DEVICE, id, TouchPhase::Start, pos, None);
                        if self.primary.is_none() && self.fingers.len() == 1 {
                            touch::set_source(Source::Finger);
                            self.press(id, pos, PointerButton::Primary, e.modifiers);
                        }
                    }
                    "pointermove" if self.fingers.contains(&id) => {
                        self.touch(FINGER_DEVICE, id, TouchPhase::Move, pos, None);
                        if self.primary == Some(id) {
                            self.events.push(Event::PointerMoved(pos));
                        }
                    }
                    "pointerup" | "pointercancel" if self.fingers.contains(&id) => {
                        let phase = if kind == "pointerup" { TouchPhase::End } else { TouchPhase::Cancel };
                        self.fingers.retain(|f| *f != id);
                        self.touch(FINGER_DEVICE, id, phase, pos, None);
                        if self.primary == Some(id) {
                            self.release(pos, e.modifiers);
                            self.events.push(Event::PointerGone);
                        }
                    }
                    // A rejected palm's later events, or one we never took.
                    _ => return self.pen.is_some() || !self.fingers.is_empty() || on_canvas,
                }
                true
            }
            "mouse" => {
                if kind == "pointerdown" {
                    touch::set_source(Source::Mouse);
                }
                false
            }
            _ => false,
        }
    }
}

/// Take pen and finger Pointer Events on the page from eframe into `pointers` (and the touch
/// events that follow them), for the runner's frames. Mouse events stay eframe's.
#[cfg(target_arch = "wasm32")]
pub fn install(runner: &eframe::WebRunner, canvas: &web_sys::HtmlCanvasElement, pointers: &SharedPointers) -> Result<(), wasm_bindgen::JsValue> {
    let window: web_sys::EventTarget = web_sys::window().ok_or("no window")?.into();
    let options = web_sys::AddEventListenerOptions::new();
    options.set_capture(true);
    options.set_passive(false);
    for kind in ["pointerdown", "pointermove", "pointerup", "pointercancel", "pointerleave"] {
        let (pointers, canvas) = (pointers.clone(), canvas.clone());
        runner.add_event_listener_ex(&window, kind, &options, move |e: web_sys::PointerEvent, runner, _| {
            let on_canvas = e.target().is_some_and(|t| &t == canvas.unchecked_ref::<web_sys::EventTarget>());
            let rect = canvas.get_bounding_client_rect();
            let z = runner.egui_ctx().zoom_factor();
            let pos = egui::pos2((e.client_x() as f32 - rect.left() as f32) / z, (e.client_y() as f32 - rect.top() as f32) / z);
            let at = |x: i32, y: i32| egui::pos2((x as f32 - rect.left() as f32) / z, (y as f32 - rect.top() as f32) / z);
            let coalesced: Vec<(Pos2, f32)> = if kind == "pointermove"
                && e.pointer_type() == "pen"
                // Safari before 17 hasn't it, and calling a missing method would abort the app.
                && js_sys::Reflect::has(&e, &"getCoalescedEvents".into()).unwrap_or(false)
            {
                e.get_coalesced_events()
                    .iter()
                    .filter_map(|c| c.dyn_into::<web_sys::PointerEvent>().ok())
                    .map(|c| (at(c.client_x(), c.client_y()), c.pressure().clamp(0.0, 1.0)))
                    .collect()
            } else {
                vec![]
            };
            let pointer_type = e.pointer_type();
            pointers.borrow_mut().note_time(kind, &pointer_type, e.button(), e.time_stamp());
            let input = Input {
                pointer_type: &pointer_type,
                id: e.pointer_id(),
                button: e.button(),
                pressure: e.pressure(),
                // Same semantics as eframe's modifiers_from_mouse_event.
                modifiers: egui::Modifiers {
                    alt: e.alt_key(),
                    ctrl: e.ctrl_key(),
                    shift: e.shift_key(),
                    mac_cmd: e.meta_key(),
                    command: e.ctrl_key() || e.meta_key(),
                },
            };
            if !pointers.borrow_mut().feed(kind, &input, pos, &coalesced, on_canvas) {
                return;
            }
            e.stop_immediate_propagation();
            // No compatibility mouse events, no text selection or magnifier.
            e.prevent_default();
            if kind == "pointerup" {
                // Keys typed next (a Magic Keyboard) go to the app.
                canvas.focus().ok();
                // Safari lets a page open the share sheet, a download or the on-screen keyboard
                // only during the user's gesture: handle the release now.
                runner.logic();
            }
            runner.egui_ctx().request_repaint();
        })?;
    }
    // eframe takes the mouse's moves from its mouse events.
    {
        let pointers = pointers.clone();
        runner.add_event_listener_ex(&window, "mousemove", &options, move |e: web_sys::MouseEvent, _runner, _| {
            pointers.borrow_mut().note_time("mousemove", "mouse", e.button(), e.time_stamp());
        })?;
    }
    // The touch events behind those pointer events: eframe would see each touch a second time,
    // and Safari would scroll, zoom or select.
    for kind in ["touchstart", "touchmove", "touchend", "touchcancel"] {
        let (pointers, canvas) = (pointers.clone(), canvas.clone());
        runner.add_event_listener_ex(&window, kind, &options, move |e: web_sys::TouchEvent, _runner, _| {
            let on_canvas = e.target().is_some_and(|t| &t == canvas.unchecked_ref::<web_sys::EventTarget>());
            let p = pointers.borrow();
            if p.captures_touch(on_canvas) {
                e.stop_immediate_propagation();
                if e.cancelable() {
                    e.prevent_default();
                }
            }
        })?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(pointer_type: &str, button: i16) -> Input<'_> {
        Input { pointer_type, id: 7, button, pressure: 0.6, modifiers: egui::Modifiers::default() }
    }

    fn reset() {
        touch::set_pen_seen(false);
        touch::set_source(Source::Mouse);
    }

    #[test]
    fn pen_buttons_and_modifiers_survive_press_release_and_cancel() {
        for (number, expected) in [
            (0, PointerButton::Primary),
            (1, PointerButton::Middle),
            (2, PointerButton::Secondary),
            (3, PointerButton::Extra1),
            (4, PointerButton::Extra2),
        ] {
            for end in ["pointerup", "pointercancel"] {
                reset();
                let mut p = Pointers::default();
                let mut e = input("pen", number);
                e.modifiers = egui::Modifiers { shift: true, ctrl: true, alt: true, mac_cmd: true, command: true };
                let pos = Pos2::new(30.0, 40.0);
                assert!(p.feed("pointerdown", &e, pos, &[], true));
                assert!(p.feed(end, &e, pos, &[], false));
                let buttons: Vec<_> = p
                    .take()
                    .into_iter()
                    .filter_map(|e| match e {
                        Event::PointerButton { button, pressed, modifiers, .. } => Some((button, pressed, modifiers)),
                        _ => None,
                    })
                    .collect();
                assert_eq!(buttons, [(expected, true, e.modifiers), (expected, false, e.modifiers)]);
                assert!(p.pen.is_none() && p.primary.is_none());
            }
        }
        reset();
    }

    #[test]
    fn mouse_times_are_noted_in_seconds() {
        let mut p = Pointers::default();
        p.note_time("pointerdown", "mouse", 0, 1000.0);
        p.note_time("mousemove", "mouse", 0, 1008.5);
        p.note_time("pointerup", "mouse", 0, 1016.0);
        // A pen's events are its own, and other buttons aren't the tool's.
        p.note_time("pointerdown", "pen", 0, 1020.0);
        p.note_time("pointerdown", "mouse", 2, 1030.0);
        assert_eq!(p.take_times(), PointerTimes { moves: vec![1.0085], downs: vec![1.0], ups: vec![1.016] });
        assert_eq!(p.take_times(), PointerTimes::default(), "a frame's times only");
        for i in 0..1000 {
            p.note_time("mousemove", "mouse", 0, f64::from(i));
        }
        assert_eq!(p.take_times().moves.len(), MOST_TIMES, "a hidden tab's times stay bounded");
    }

    #[test]
    fn touch_only_and_mouse_input_stay_with_eframe_and_session_can_reset() {
        reset();
        let mut p = Pointers::default();
        let pos = Pos2::new(10.0, 20.0);
        for pointer_type in ["touch", "mouse"] {
            for kind in ["pointerdown", "pointermove", "pointerup", "pointercancel"] {
                assert!(!p.feed(kind, &input(pointer_type, 0), pos, &[], true));
            }
        }
        assert!(p.take().is_empty());
        assert!(!p.captures_touch(true), "eframe must also receive DOM touch events before a pen");
        assert!(p.feed("pointerdown", &input("pen", 0), pos, &[], true));
        assert!(p.feed("pointerup", &input("pen", 0), pos, &[], true));
        assert!(p.captures_touch(true));
        assert!(p.feed("pointerdown", &input("touch", 0), pos, &[], true));
        assert!(p.feed("pointerup", &input("touch", 0), pos, &[], true));
        // A new page starts from false, without reading any old persistent marker.
        reset();
        let mut next_page = Pointers::default();
        assert!(!next_page.feed("pointerdown", &input("touch", 0), pos, &[], true));
        assert!(next_page.take().is_empty());
        assert!(!next_page.captures_touch(true));
    }

    #[test]
    fn finger_modifiers_and_palm_cancellation_are_preserved() {
        reset();
        touch::set_source(Source::Pen);
        let mut p = Pointers::default();
        let pos = Pos2::new(10.0, 20.0);
        let mut finger = input("touch", 0);
        finger.id = 10;
        finger.modifiers.shift = true;
        assert!(p.feed("pointerdown", &finger, pos, &[], true));
        assert!(p.take().iter().any(|e| matches!(e, Event::PointerButton { pressed: true, modifiers, .. } if modifiers.shift)));
        let pen = input("pen", 0);
        assert!(p.feed("pointerdown", &pen, pos, &[], true));
        assert!(p.take().iter().any(|e| matches!(e, Event::Touch { phase: TouchPhase::Cancel, .. })));
        assert!(p.fingers.is_empty());
        assert!(p.feed("pointermove", &finger, pos, &[], true));
        assert!(p.take().is_empty(), "a resting palm cannot move the pen");
        assert!(p.feed("pointerup", &pen, pos, &[], true));
        assert!(p.feed("pointerdown", &finger, pos, &[], true));
        assert!(p.feed("pointerup", &finger, pos, &[], true));
        assert!(p.take().iter().any(|e| matches!(e, Event::PointerButton { pressed: false, modifiers, .. } if modifiers.shift)));
        reset();
    }

    #[test]
    fn unknown_pen_buttons_are_not_reinterpreted_as_primary() {
        reset();
        let mut p = Pointers::default();
        assert!(!p.feed("pointerdown", &input("pen", 5), Pos2::ZERO, &[], true));
        assert!(p.take().is_empty());
        assert!(!touch::pen_seen());
    }
}
