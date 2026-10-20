//! The system's times of the mouse events (`NSEvent.timestamp`), which egui leaves out: an AppKit
//! monitor notes each move, drag, press and release as the app receives it, and before each frame
//! they become the app's [`PointerTimes`], by egui's clock. A Scriptographer tool script measures the
//! mouse's speed by them: the frame's time instead is late by however long the frame took to start.
//!
//! The monitor needs an `unsafe` call: AppKit's handler type can't say that the block must return
//! an event.
#![allow(unsafe_code)]

use std::ptr::NonNull;
use std::sync::Mutex;

use block2::RcBlock;
use objc2_app_kit::{NSEvent, NSEventMask, NSEventType};
use objc2_foundation::NSProcessInfo;
use vectorcraft_ui_egui::canvas::PointerTimes;

/// The events noted since the last frame: their type and time, in seconds since the system started.
static SEEN: Mutex<Vec<(NSEventType, f64)>> = Mutex::new(Vec::new());

/// The most events noted between two frames: a frame that doesn't come (the window hidden) leaves
/// no list growing.
const MOST: usize = 256;

/// Starts noting the mouse events' times.
pub fn install() {
    let block = RcBlock::new(|event: NonNull<NSEvent>| -> *mut NSEvent {
        // SAFETY: AppKit passes the event it is about to send, valid for the call.
        let (kind, time) = unsafe { (event.as_ref().r#type(), event.as_ref().timestamp()) };
        if let Ok(mut seen) = SEEN.lock()
            && seen.len() < MOST
        {
            seen.push((kind, time));
        }
        event.as_ptr()
    });
    let mask = NSEventMask::MouseMoved | NSEventMask::LeftMouseDragged | NSEventMask::LeftMouseDown | NSEventMask::LeftMouseUp;
    // SAFETY: the block returns the event it was given, so AppKit sends it on unchanged.
    let monitor = unsafe { NSEvent::addLocalMonitorForEventsMatchingMask_handler(mask, &block) };
    // The monitor watches for as long as the app runs: it is never removed.
    std::mem::forget(monitor);
}

/// The times of the events since the last call, by egui's clock: `now` is the frame's time in it.
pub fn take(now: f64) -> PointerTimes {
    let seen = SEEN.lock().map(|mut seen| std::mem::take(&mut *seen)).unwrap_or_default();
    let uptime = NSProcessInfo::processInfo().systemUptime();
    let mut times = PointerTimes::default();
    for (kind, time) in seen {
        let list = match kind {
            NSEventType::MouseMoved | NSEventType::LeftMouseDragged => &mut times.moves,
            NSEventType::LeftMouseDown => &mut times.downs,
            NSEventType::LeftMouseUp => &mut times.ups,
            _ => continue,
        };
        list.push(now - (uptime - time));
    }
    times
}
