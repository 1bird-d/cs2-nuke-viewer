//! Winit events to egui input, for the browser.
//!
//! `egui-winit` would normally do this, but version 0.36.1 does not compile for
//! `wasm32` at all: its `NativeFile` implements `DroppedFile::bytes`, and on
//! wasm that trait wants `bytes_async` instead. The dropped-file module was
//! never cfg-gated, so the whole crate fails to build for the web. It is the
//! newest release, so there is nothing to upgrade to.
//!
//! Rather than fork it or drop the panel on the web, this translates the events
//! the panel actually needs. That is pointer movement, clicks and the wheel —
//! there is no text field anywhere in the UI, so no keyboard translation and no
//! IME. Keys are handled by the app before egui ever sees them.
//!
//! `egui-wgpu`, which does the drawing, is unaffected and used unchanged.

use egui::{Event, Modifiers, PointerButton, Pos2, RawInput, Rect, Vec2};
use winit::event::{ElementState, MouseScrollDelta, WindowEvent};

/// Accumulates events between frames.
pub struct WebInput {
    events: Vec<Event>,
    modifiers: Modifiers,
    /// Last known pointer position, in points.
    pointer: Pos2,
    start: web_time::Instant,
    /// Whether egui claimed the pointer last frame. Used to decide if a click
    /// belongs to the panel or the camera; a frame of lag is imperceptible and
    /// is what the winit integration effectively does too.
    pub wants_pointer: bool,
}

impl WebInput {
    #[must_use]
    pub fn new() -> Self {
        Self {
            events: Vec::new(),
            modifiers: Modifiers::default(),
            pointer: Pos2::ZERO,
            start: web_time::Instant::now(),
            wants_pointer: false,
        }
    }

    /// Record an event. Returns true if the panel should get it and the camera
    /// should not.
    pub fn on_window_event(&mut self, event: &WindowEvent, pixels_per_point: f32) -> bool {
        let scale = pixels_per_point.max(0.01);
        match event {
            WindowEvent::CursorMoved { position, .. } => {
                #[allow(clippy::cast_possible_truncation)]
                {
                    self.pointer = Pos2::new(
                        position.x as f32 / scale,
                        position.y as f32 / scale,
                    );
                }
                self.events.push(Event::PointerMoved(self.pointer));
                self.wants_pointer
            }
            WindowEvent::CursorLeft { .. } => {
                self.events.push(Event::PointerGone);
                false
            }
            WindowEvent::MouseInput { state, button, .. } => {
                let Some(button) = translate_button(*button) else {
                    return false;
                };
                self.events.push(Event::PointerButton {
                    pos: self.pointer,
                    button,
                    pressed: *state == ElementState::Pressed,
                    modifiers: self.modifiers,
                });
                self.wants_pointer
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let (unit, delta) = match delta {
                    MouseScrollDelta::LineDelta(x, y) => {
                        (egui::MouseWheelUnit::Line, Vec2::new(*x, *y))
                    }
                    MouseScrollDelta::PixelDelta(p) => {
                        #[allow(clippy::cast_possible_truncation)]
                        (
                            egui::MouseWheelUnit::Point,
                            Vec2::new(p.x as f32 / scale, p.y as f32 / scale),
                        )
                    }
                };
                self.events.push(Event::MouseWheel {
                    unit,
                    delta,
                    // Winit gives no trackpad phase on the web, and egui's own
                    // documentation says to use `Move` when it is unknown.
                    phase: egui::TouchPhase::Move,
                    modifiers: self.modifiers,
                });
                self.wants_pointer
            }
            WindowEvent::ModifiersChanged(state) => {
                let s = state.state();
                self.modifiers = Modifiers {
                    alt: s.alt_key(),
                    ctrl: s.control_key(),
                    shift: s.shift_key(),
                    // No Cmd key reaches a canvas on the platforms this runs on,
                    // and nothing in the panel is bound to one.
                    mac_cmd: false,
                    command: s.control_key(),
                };
                false
            }
            _ => false,
        }
    }

    /// Drain the events into a frame of input.
    pub fn take(&mut self, size: (u32, u32), pixels_per_point: f32) -> RawInput {
        let scale = pixels_per_point.max(0.01);
        #[allow(clippy::cast_precision_loss)]
        let screen = Rect::from_min_size(
            Pos2::ZERO,
            Vec2::new(size.0 as f32 / scale, size.1 as f32 / scale),
        );
        // Modifiers are not a field on `RawInput`; they ride on each event, and
        // every event pushed above already carries the current state.
        RawInput {
            screen_rect: Some(screen),
            time: Some(self.start.elapsed().as_secs_f64()),
            events: std::mem::take(&mut self.events),
            focused: true,
            ..Default::default()
        }
    }

    /// Act on what egui asked the platform to do.
    ///
    /// Only one command matters here: the reference links in the panel are the
    /// point of the panel, so they have to actually open. Everything else —
    /// clipboard, IME, cursor shapes — the browser handles or nothing needs.
    pub fn handle_output(&mut self, output: &egui::PlatformOutput) {
        for command in &output.commands {
            if let egui::OutputCommand::OpenUrl(open) = command {
                if let Some(window) = web_sys::window() {
                    // A new tab, not this one: navigating away would drop the
                    // scene the visitor just waited to download.
                    let _ = window.open_with_url_and_target(&open.url, "_blank");
                }
            }
        }
    }
}

impl Default for WebInput {
    fn default() -> Self {
        Self::new()
    }
}

fn translate_button(button: winit::event::MouseButton) -> Option<PointerButton> {
    Some(match button {
        winit::event::MouseButton::Left => PointerButton::Primary,
        winit::event::MouseButton::Right => PointerButton::Secondary,
        winit::event::MouseButton::Middle => PointerButton::Middle,
        _ => return None,
    })
}
