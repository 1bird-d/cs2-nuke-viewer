//! The browser entry point.
//!
//! Same renderer, same categories, same identification table as the desktop
//! build — see [`crate`]. What differs is everything around them:
//!
//! * The scene arrives over the network, gzip-compressed, and is inflated by
//!   the browser's own `DecompressionStream` rather than a Rust zlib crate.
//!   That keeps roughly 28 MB off the wire and a decompressor out of the wasm.
//! * Device creation is asynchronous and cannot block, so the app runs with no
//!   renderer for the first few frames and picks it up when it is ready.
//! * There is nowhere to write a PNG, so `F12` does not exist here.
//!
//! # WebGPU is required
//!
//! Per-instance transforms and colours are read from a read-only storage buffer
//! indexed by `@builtin(instance_index)`. WebGL2 has no storage buffers at all,
//! so there is no fallback to give — the page checks for `navigator.gpu` before
//! loading this module and says so plainly if it is missing.

use std::cell::RefCell;
use std::rc::Rc;

use glam::Vec3;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, TouchPhase, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::platform::web::{EventLoopExtWebSys, WindowAttributesExtWebSys};
use winit::window::{Window, WindowId};

use crate::camera::Camera;
use crate::category::Classification;
use crate::gpu::{GhostParams, Renderer};
use crate::panel::{self, Panel};
use crate::touch::{self, Touches};
use crate::{
    build_labels, cursor_ray, identify, identify_instance, pick, Input, Instant, ViewKey,
};

/// Where the published scene lives, relative to the page.
const SCENE_URL: &str = "de_nuke.plant.nkp.gz";

/// `DecompressionStream` bound by hand.
///
/// `web-sys` has it, but only behind `--cfg=web_sys_unstable_apis`, which would
/// mean every build of this project — including CI and anyone who clones it —
/// needing a `RUSTFLAGS` incantation to compile. The API itself is not unstable;
/// it has been in every major browser for years. Fifteen lines of extern is a
/// better trade than a build that fails without an environment variable.
#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_name = DecompressionStream)]
    type DecompressionStream;

    #[wasm_bindgen(constructor, catch)]
    fn new(format: &str) -> Result<DecompressionStream, JsValue>;

    #[wasm_bindgen(method, getter)]
    fn readable(this: &DecompressionStream) -> web_sys::ReadableStream;

    #[wasm_bindgen(method, getter)]
    fn writable(this: &DecompressionStream) -> web_sys::WritableStream;
}

/// Start the viewer on the canvas with this element id.
///
/// # Errors
///
/// Reports failures through the page's own status element rather than a panic,
/// because a wasm panic in a browser is a stack trace in a console nobody has
/// open.
#[wasm_bindgen]
pub fn start(canvas_id: String) {
    console_error_panic_hook::set_once();
    wasm_bindgen_futures::spawn_local(async move {
        if let Err(e) = run(&canvas_id).await {
            status(&format!("{e}"));
        }
    });
}

/// Write a message into the page's status element, if the page has one.
fn status(text: &str) {
    let Some(document) = web_sys::window().and_then(|w| w.document()) else {
        return;
    };
    if let Some(element) = document.get_element_by_id("status") {
        element.set_text_content(Some(text));
    }
}

/// Whether the primary input is a touchscreen — a phone or a tablet.
///
/// The same test `web/index.html` uses in CSS, so the page and the viewer
/// cannot disagree about which one the reader is holding. `hover: none` is the
/// discriminating half: a desktop machine with a touchscreen still reports
/// `hover: hover`, because it also has a mouse, and keeps its panel. Only a
/// device whose *primary* input cannot hover loses it.
///
/// Hover is also the thing the panel is built around — names appear under the
/// cursor before anything is clicked — so a device that cannot hover is exactly
/// a device the panel was never designed for.
fn is_touch_only() -> bool {
    web_sys::window()
        .and_then(|window| window.match_media("(hover: none) and (pointer: coarse)").ok())
        .flatten()
        .is_some_and(|query| query.matches())
}

/// Hide the loading overlay once there is something to look at.
fn hide_overlay() {
    let Some(document) = web_sys::window().and_then(|w| w.document()) else {
        return;
    };
    if let Some(element) = document.get_element_by_id("overlay") {
        let _ = element.set_attribute("hidden", "");
    }
}

/// Fetch the scene and inflate it.
///
/// `DecompressionStream` is a browser built-in, so the gzip payload costs
/// nothing in wasm size. It has been in every major engine for years; if it is
/// somehow missing, the uncompressed bytes still parse, so try that rather than
/// give up.
async fn fetch_scene(url: &str) -> Result<Vec<u8>, String> {
    let window = web_sys::window().ok_or("no window")?;
    let response = wasm_bindgen_futures::JsFuture::from(window.fetch_with_str(url))
        .await
        .map_err(|_| format!("could not fetch {url}"))?
        .dyn_into::<web_sys::Response>()
        .map_err(|_| "fetch did not return a response".to_string())?;
    if !response.ok() {
        return Err(format!(
            "{url} came back {} {}",
            response.status(),
            response.status_text()
        ));
    }

    let buffer = wasm_bindgen_futures::JsFuture::from(
        response
            .array_buffer()
            .map_err(|_| "no body on the response".to_string())?,
    )
    .await
    .map_err(|_| "could not read the response body".to_string())?;
    let bytes = js_sys::Uint8Array::new(&buffer).to_vec();

    // Decide from the first two bytes, not the file extension. Some hosts serve
    // a `.gz` with `Content-Encoding: gzip`, in which case the browser has
    // already inflated it and trying again would fail on plain scene data.
    // 0x1f 0x8b is the gzip magic; `.nkp` starts with "NKPSCEN\0".
    if bytes.starts_with(&[0x1f, 0x8b]) {
        inflate(&bytes).await
    } else {
        Ok(bytes)
    }
}

/// How large the inflated scene will be, from the gzip trailer.
///
/// A gzip member ends with the uncompressed length in the last four bytes,
/// little-endian, modulo 2^32. The scene is 80 MB, so it is exact here.
///
/// Worth reading rather than letting the `Vec` find out by doubling. Growing to
/// 80 MB that way ends with a copy that holds the 64 MB old buffer and the
/// 128 MB new one at once — about 192 MB of wasm linear memory, which is never
/// returned to the system once taken. On a phone, where the browser kills tabs
/// that get greedy, that transient is the largest single allocation in the
/// whole load and it buys nothing.
///
/// A wrong or hostile trailer costs only a bad guess: capacity is a hint, and
/// the read loop appends whatever actually arrives. The clamp is there so a
/// corrupt length cannot ask for a gigabyte before the first byte is read.
fn inflated_size(gzip: &[u8]) -> usize {
    /// Four times the 80 MB the scene actually is, and far below the point
    /// where reserving would itself be the problem.
    const CEILING: usize = 320 << 20;

    let Some(trailer) = gzip.get(gzip.len().wrapping_sub(4)..) else {
        return 0;
    };
    let Ok(bytes) = <[u8; 4]>::try_from(trailer) else {
        return 0;
    };
    (u32::from_le_bytes(bytes) as usize).min(CEILING)
}

/// Gunzip through the browser's own streams implementation.
///
/// The compressed bytes are pushed in without awaiting the write, which ignores
/// backpressure — deliberately, because the read loop below is the only
/// consumer and awaiting the write before starting it would deadlock.
async fn inflate(bytes: &[u8]) -> Result<Vec<u8>, String> {
    let stream =
        DecompressionStream::new("gzip").map_err(|_| "this browser cannot gunzip".to_string())?;

    let writer = stream
        .writable()
        .get_writer()
        .map_err(|_| "no writer on the gunzip stream".to_string())?;
    let chunk = js_sys::Uint8Array::from(bytes);
    let _ = writer.write_with_chunk(&chunk);
    let _ = writer.close();

    // `web_sys::Response` cannot be built from a `ReadableStream`, so the
    // inflated chunks are drained by hand instead of letting `array_buffer()`
    // do it.
    let reader = stream
        .readable()
        .get_reader()
        .dyn_into::<web_sys::ReadableStreamDefaultReader>()
        .map_err(|_| "no reader on the gunzip stream".to_string())?;

    let mut out: Vec<u8> = Vec::with_capacity(inflated_size(bytes));
    loop {
        let result = wasm_bindgen_futures::JsFuture::from(reader.read())
            .await
            .map_err(|_| "the scene did not gunzip".to_string())?;
        let done = js_sys::Reflect::get(&result, &JsValue::from_str("done"))
            .ok()
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        if done {
            break;
        }
        let value = js_sys::Reflect::get(&result, &JsValue::from_str("value"))
            .map_err(|_| "a gunzip chunk had no value".to_string())?;
        out.extend_from_slice(&js_sys::Uint8Array::new(&value).to_vec());
    }
    Ok(out)
}

async fn run(canvas_id: &str) -> Result<(), String> {
    status("Downloading the plant…");
    let bytes = fetch_scene(SCENE_URL).await?;
    #[allow(clippy::cast_precision_loss)]
    let mb = bytes.len() as f64 / (1024.0 * 1024.0);
    status(&format!("Unpacking {mb:.0} MB…"));

    let scene = nkp_format::Scene::from_bytes(bytes, SCENE_URL).map_err(|e| format!("{e:#}"))?;
    let mut classification =
        Classification::new(&scene).map_err(|e| format!("could not classify: {e:#}"))?;
    classification.preset_plant();

    let document = web_sys::window()
        .and_then(|w| w.document())
        .ok_or("no document")?;
    let canvas = document
        .get_element_by_id(canvas_id)
        .ok_or_else(|| format!("no element with id {canvas_id}"))?
        .dyn_into::<web_sys::HtmlCanvasElement>()
        .map_err(|_| format!("#{canvas_id} is not a canvas"))?;

    status("Starting the renderer…");
    let event_loop = EventLoop::new().map_err(|e| format!("{e}"))?;
    event_loop.set_control_flow(ControlFlow::Poll);
    event_loop.spawn_app(App::new(scene, classification, canvas));
    Ok(())
}

struct App {
    scene: Rc<nkp_format::Scene>,
    classification: Classification,
    canvas: web_sys::HtmlCanvasElement,
    window: Option<std::sync::Arc<Window>>,
    /// Filled in by the async device request, which cannot block the event loop.
    renderer: Rc<RefCell<Option<Renderer>>>,
    /// Set once, when the renderer first appears, to build the egui pass.
    panel: Option<Panel>,
    camera: Camera,
    input: Input,
    ghost: GhostParams,
    selected: Option<usize>,
    identifier: identify::Identifier,
    hovered: Option<usize>,
    show_labels: bool,
    /// Whether the key legend is painted in the corner of the viewport.
    show_keys: bool,
    /// The last prop the cursor named, so its links stay reachable.
    sticky: Option<&'static identify::Entry>,
    /// Fingers, on a device that has them.
    touches: Touches,
    /// Whether this is a phone or a tablet, decided once at startup.
    ///
    /// On one of those the viewer is a viewer and nothing else: no panel, no
    /// key legend, no pinned names. All of it is text sized for a screen twenty
    /// times the area, and reading is not what someone holding a phone came to
    /// do. What is left is the plant, and the means to fly around it.
    touch_only: bool,
    cursor: (f32, f32),
    viewport: (u32, u32),
    dirty: bool,
    last_frame: Instant,
    last_fps: Instant,
    frames: u32,
    fps: f32,
}

impl App {
    fn new(
        scene: nkp_format::Scene,
        classification: Classification,
        canvas: web_sys::HtmlCanvasElement,
    ) -> Self {
        let (min, max) = classification.visible_bounds(&scene);
        let mut camera = Camera::default();
        camera.frame(Vec3::from(min), Vec3::from(max), 16.0 / 9.0);
        let touch_only = is_touch_only();
        Self {
            scene: Rc::new(scene),
            classification,
            canvas,
            window: None,
            renderer: Rc::new(RefCell::new(None)),
            panel: None,
            camera,
            input: Input::default(),
            ghost: GhostParams::default(),
            selected: None,
            identifier: identify::Identifier::new().expect("identification table compiles"),
            hovered: None,
            show_labels: !touch_only,
            show_keys: !touch_only,
            sticky: None,
            touches: Touches::new(),
            touch_only,
            cursor: (0.0, 0.0),
            viewport: (1280, 720),
            dirty: true,
            last_frame: Instant::now(),
            last_fps: Instant::now(),
            frames: 0,
            fps: 0.0,
        }
    }

    /// The size the surface should be, in physical pixels.
    ///
    /// Winit's web backend owns the canvas: it watches the element with a
    /// `ResizeObserver` and writes the drawing-buffer size itself. Setting
    /// `canvas.width` here as well produced two writers disagreeing, and the
    /// surface stayed configured at its startup size while the element grew —
    /// a small picture in the corner of a large black canvas. So winit is the
    /// only authority, and this just asks it.
    /// Size the canvas's drawing buffer to its CSS box, and report that size.
    ///
    /// Winit's web backend does **not** do this for a canvas handed to it with
    /// `with_canvas`. Left alone the drawing buffer stays at 1x1 while the
    /// element is laid out full-window, so the entire scene renders into a
    /// single pixel and the browser stretches it over the page: one flat colour,
    /// no panel, nothing to see. It looks like a dead renderer and is really a
    /// one-pixel one.
    ///
    /// So this owns the size — sets it and reports it — and the surface is
    /// configured from the same number. One authority, and the caller's
    /// "has it changed?" test can never oscillate between two of them.
    fn sync_canvas_size(&self) -> (u32, u32) {
        let ratio = web_sys::window().map_or(1.0, |w| w.device_pixel_ratio());
        let rect = self.canvas.get_bounding_client_rect();
        // Round rather than truncate: at 125% and 150% scaling the CSS box is
        // fractional, and rounding keeps the result stable frame to frame.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let width = ((rect.width() * ratio).round() as u32).max(1);
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let height = ((rect.height() * ratio).round() as u32).max(1);
        if self.canvas.width() != width {
            self.canvas.set_width(width);
        }
        if self.canvas.height() != height {
            self.canvas.set_height(height);
        }
        (width, height)
    }

    fn frame_visible(&mut self) {
        let (min, max) = self.classification.visible_bounds(&self.scene);
        #[allow(clippy::cast_precision_loss)]
        let aspect = self.viewport.0 as f32 / self.viewport.1.max(1) as f32;
        self.camera.frame(Vec3::from(min), Vec3::from(max), aspect);
    }

    fn update_hover(&mut self) {
        if self.input.looking {
            self.hovered = None;
            return;
        }
        #[allow(clippy::cast_precision_loss)]
        let aspect = self.viewport.0 as f32 / self.viewport.1.max(1) as f32;
        self.hovered = cursor_ray(self.cursor, self.viewport, &self.camera, aspect)
            .and_then(|ray| pick::nearest(&self.scene, &self.classification, &ray))
            .map(|hit| hit.instance);
        if let Some(entry) = self
            .hovered
            .and_then(|id| identify_instance(&self.scene, &self.identifier, id))
        {
            self.sticky = Some(entry);
        }
    }

    /// Turn one finger's event into camera motion.
    ///
    /// Winit's web backend already separates touch from mouse — it branches on
    /// the pointer type and a touch never arrives as `CursorMoved` or
    /// `MouseInput` — so nothing here can affect a mouse, and a desktop machine
    /// with a touchscreen simply gains a second way to fly.
    ///
    /// Look deltas are computed from successive positions rather than taken
    /// from `DeviceEvent::MouseMotion`. Winit does fire that for touch as well,
    /// but on WebKit its delta comes from `PointerEvent.movementX/Y`, which is
    /// very likely always zero for a touch pointer. Positions are reported
    /// either way, so subtracting them is the version that cannot be wrong.
    fn on_touch(&mut self, finger: winit::event::Touch) {
        let phase = match finger.phase {
            TouchPhase::Started => touch::Phase::Started,
            TouchPhase::Moved => touch::Phase::Moved,
            // A cancel is the browser taking the gesture away — a system edge
            // swipe, a phone call. It ends the finger exactly as a lift does,
            // and treating it as anything else would leave it down forever.
            TouchPhase::Ended | TouchPhase::Cancelled => touch::Phase::Ended,
        };

        // Winit reports physical pixels; the gesture thresholds are in CSS
        // pixels so that they mean the same movement of the hand on a phone at
        // ratio 3 as on a laptop at 1.
        let ratio = web_sys::window().map_or(1.0, |w| w.device_pixel_ratio());
        #[allow(clippy::cast_possible_truncation)]
        let at = (
            (finger.location.x / ratio) as f32,
            (finger.location.y / ratio) as f32,
        );

        match self.touches.on_touch(finger.id, phase, at, Instant::now()) {
            Some(touch::Action::Look { dx, dy }) => {
                // `Camera::look` multiplies by a sensitivity, so hand it the
                // radians already worked out and a factor of one.
                self.camera
                    .look(touch::look_radians(dx), touch::look_radians(dy), 1.0);
            }
            Some(touch::Action::Speed(ratio)) => {
                self.camera.speed = (self.camera.speed * ratio).clamp(0.2, 400.0);
            }
            Some(touch::Action::Reframe) => self.frame_visible(),
            None => {}
        }
    }

    fn select(&mut self) {
        #[allow(clippy::cast_precision_loss)]
        let aspect = self.viewport.0 as f32 / self.viewport.1.max(1) as f32;
        self.selected = cursor_ray(self.cursor, self.viewport, &self.camera, aspect)
            .and_then(|ray| pick::nearest(&self.scene, &self.classification, &ray))
            .map(|hit| hit.instance);
        self.dirty = true;
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attributes = Window::default_attributes()
            .with_title("nukeplant")
            .with_canvas(Some(self.canvas.clone()));
        let Ok(window) = event_loop.create_window(attributes) else {
            status("could not attach a window to the canvas");
            return;
        };
        let window = std::sync::Arc::new(window);
        self.window = Some(window.clone());
        self.viewport = self.sync_canvas_size();

        // Device creation is async and the event loop cannot wait for it, so it
        // lands in the shared slot and the first few frames simply draw nothing.
        let slot = self.renderer.clone();
        let scene = self.scene.clone();
        let (width, height) = self.viewport;
        wasm_bindgen_futures::spawn_local(async move {
            match Renderer::windowed(window, &scene).await {
                Ok(mut renderer) => {
                    // Configuring a WebGPU surface *writes* the canvas drawing
                    // buffer size. `Renderer::windowed` configures it from
                    // winit's `inner_size()`, which on web is 1x1 until winit is
                    // told otherwise — so creating the renderer undoes the
                    // sizing the page just did, and everything afterwards
                    // renders into a single pixel stretched across the window.
                    // One flat colour, no panel. Put the real size back before
                    // the first frame rather than waiting for the render loop
                    // to notice, which never happens in a tab that is not
                    // compositing.
                    renderer.resize(width, height);
                    *slot.borrow_mut() = Some(renderer);
                    hide_overlay();
                }
                Err(e) => status(&format!(
                    "WebGPU did not start: {e:#}. Try Chrome, Edge, or Safari 26 and later."
                )),
            }
        });
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let consumed = match (self.panel.as_mut(), self.window.as_ref()) {
            (Some(panel), Some(window)) => panel.on_window_event(window, &event),
            _ => false,
        };
        if consumed
            && !crate::is_release(&event)
            && !matches!(
                event,
                WindowEvent::RedrawRequested | WindowEvent::Resized(_) | WindowEvent::Focused(_)
            )
        {
            return;
        }

        match event {
            // Switching tab or clicking away sends the key-up somewhere else,
            // and the camera would still be travelling on return. The same is
            // true of a finger: the lift lands on whatever the user switched
            // to, and this page never hears the gesture end.
            WindowEvent::Focused(false) => {
                self.input = Input::default();
                self.touches.clear();
            }
            WindowEvent::Touch(finger) => self.on_touch(finger),
            WindowEvent::Resized(size) => {
                self.viewport = (size.width.max(1), size.height.max(1));
                if let Some(renderer) = self.renderer.borrow_mut().as_mut() {
                    renderer.resize(self.viewport.0, self.viewport.1);
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                #[allow(clippy::cast_possible_truncation)]
                {
                    self.cursor = (position.x as f32, position.y as f32);
                }
                self.update_hover();
            }
            WindowEvent::MouseInput { button, state, .. } => {
                let pressed = state == ElementState::Pressed;
                match button {
                    MouseButton::Right => self.input.looking = pressed,
                    MouseButton::Left if pressed => self.select(),
                    _ => {}
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let ticks = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y,
                    MouseScrollDelta::PixelDelta(p) => {
                        #[allow(clippy::cast_possible_truncation)]
                        {
                            p.y as f32 / 60.0
                        }
                    }
                };
                self.camera.speed = (self.camera.speed * 1.15f32.powf(ticks)).clamp(0.2, 400.0);
            }
            WindowEvent::KeyboardInput { event, .. } => {
                let PhysicalKey::Code(code) = event.physical_key else {
                    return;
                };
                let down = event.state == ElementState::Pressed;
                match code {
                    KeyCode::KeyW => self.input.forward = down,
                    KeyCode::KeyS => self.input.back = down,
                    KeyCode::KeyA => self.input.left = down,
                    KeyCode::KeyD => self.input.right = down,
                    KeyCode::KeyE | KeyCode::Space => self.input.up = down,
                    KeyCode::KeyQ => self.input.down = down,
                    KeyCode::ShiftLeft | KeyCode::ShiftRight => self.input.fast = down,
                    KeyCode::ControlLeft | KeyCode::ControlRight => self.input.slow = down,
                    KeyCode::KeyF if down && !event.repeat => self.frame_visible(),
                    KeyCode::KeyH if down && !event.repeat => {
                        if let Some(panel) = &mut self.panel {
                            panel.visible = !panel.visible;
                        }
                    }
                    _ if down && !event.repeat => {
                        if let Some(key) = view_key(code) {
                            let effect = crate::apply_view_key(
                                key,
                                &mut self.classification,
                                &mut self.ghost,
                            );
                            if effect.dirty {
                                self.dirty = true;
                            }
                            if effect.toggle_labels {
                                self.show_labels = !self.show_labels;
                            }
                            if effect.toggle_keys {
                                self.show_keys = !self.show_keys;
                            }
                        }
                    }
                    _ => {}
                }
            }
            WindowEvent::RedrawRequested => self.draw(),
            WindowEvent::CloseRequested => event_loop.exit(),
            _ => {}
        }
    }

    fn device_event(
        &mut self,
        _: &ActiveEventLoop,
        _: winit::event::DeviceId,
        event: winit::event::DeviceEvent,
    ) {
        if let winit::event::DeviceEvent::MouseMotion { delta } = event {
            if self.input.looking {
                #[allow(clippy::cast_possible_truncation)]
                self.camera.look(delta.0 as f32, delta.1 as f32, 0.0022);
            }
        }
    }

    fn about_to_wait(&mut self, _: &ActiveEventLoop) {
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }
}

impl App {
    fn draw(&mut self) {
        let now = Instant::now();
        let dt = (now - self.last_frame).as_secs_f32().min(0.1);
        self.last_frame = now;

        let (width, height) = self.sync_canvas_size();
        self.viewport = (width, height);

        let mut slot = self.renderer.borrow_mut();
        let Some(renderer) = slot.as_mut() else {
            return;
        };
        // Against the renderer's own size, not a cached one. The surface is
        // first configured from `window.inner_size()`, which on a canvas that
        // has not been laid out yet is 1x1 — so the first real frame must
        // reconfigure even though nothing "changed".
        if renderer.size() != (width, height) {
            renderer.resize(width, height);
        }
        let Some(window) = self.window.as_ref() else {
            return;
        };
        if self.panel.is_none() {
            let mut panel = Panel::new(renderer.device(), window, renderer.format());
            // Built either way, because the same egui pass draws the labels and
            // the renderer expects a callback. Hidden on a phone, where `H` and
            // `K` are unreachable and there is nothing to read it with.
            panel.visible = !self.touch_only;
            self.panel = Some(panel);
        }
        let Some(panel) = self.panel.as_mut() else {
            return;
        };

        // Two fingers win over the keys while they are down. They cannot both
        // be in use on the same device, and checking the touch state first
        // means a stuck key cannot fight the joystick.
        let (forward, right, up, boost) = match self.touches.fly() {
            Some((forward, right, throttle)) => (forward, right, 0.0, throttle),
            None => {
                let (forward, right, up) = self.input.axes();
                (forward, right, up, self.input.boost())
            }
        };
        self.camera.fly(forward, right, up, dt, boost);

        if self.dirty {
            let picked = self.selected;
            renderer.apply(&self.scene, &self.classification, &|id| {
                (picked == Some(id)).then_some([1.0, 1.0, 1.0])
            });
            self.dirty = false;
        }

        let view_proj = self.camera.view_projection(renderer.aspect());
        let stats = renderer.stats();
        let selected_name = self
            .selected
            .and_then(|id| self.scene.instances().get(id))
            .map_or(String::new(), |i| self.scene.instance_name(i).to_string());
        let selection = panel::Selection {
            instance: self.selected,
            name: &selected_name,
            entry: self
                .selected
                .and_then(|id| identify_instance(&self.scene, &self.identifier, id)),
            sticky: self.sticky,
        };
        let labels = build_labels(
            &self.scene,
            &self.classification,
            &self.identifier,
            self.hovered,
            self.show_labels,
            self.camera.position,
            view_proj,
            self.cursor,
        );

        let classification = &mut self.classification;
        let ghost = &mut self.ghost;
        let show_keys = self.show_keys;
        let fps = self.fps;
        let mut response = panel::Response::default();
        let outcome = renderer.render(
            view_proj,
            self.camera.position,
            *ghost,
            &mut |device, queue, encoder, view, size| {
                response = panel.draw(
                    window,
                    device,
                    queue,
                    encoder,
                    view,
                    size,
                    classification,
                    ghost,
                    selection,
                    &labels,
                    show_keys,
                    stats,
                    fps,
                );
            },
        );
        drop(slot);

        if outcome.is_err() {
            return;
        }
        if response.dirty {
            self.dirty = true;
        }
        if response.frame_visible {
            self.frame_visible();
        }

        self.frames += 1;
        let elapsed = (now - self.last_fps).as_secs_f32();
        if elapsed >= 0.5 {
            #[allow(clippy::cast_precision_loss)]
            {
                self.fps = self.frames as f32 / elapsed;
            }
            self.frames = 0;
            self.last_fps = now;
        }
    }
}

/// The keys that mean the same thing here as on the desktop.
fn view_key(code: KeyCode) -> Option<ViewKey> {
    Some(match code {
        KeyCode::Digit1 => ViewKey::Category(0),
        KeyCode::Digit2 => ViewKey::Category(1),
        KeyCode::Digit3 => ViewKey::Category(2),
        KeyCode::Digit4 => ViewKey::Category(3),
        KeyCode::Digit5 => ViewKey::Category(4),
        KeyCode::Digit6 => ViewKey::Category(5),
        KeyCode::Digit7 => ViewKey::Category(6),
        KeyCode::Digit8 => ViewKey::Category(7),
        KeyCode::Digit9 => ViewKey::Category(8),
        KeyCode::Digit0 => ViewKey::Category(9),
        KeyCode::KeyP => ViewKey::PresetPlant,
        KeyCode::KeyI => ViewKey::PresetPipes,
        KeyCode::KeyM => ViewKey::PresetEverything,
        KeyCode::KeyG => ViewKey::GhostHidden,
        KeyCode::KeyX => ViewKey::HideGhosted,
        KeyCode::BracketLeft => ViewKey::Dimmer,
        KeyCode::BracketRight => ViewKey::Brighter,
        KeyCode::KeyL => ViewKey::ToggleLabels,
        KeyCode::KeyK => ViewKey::ToggleKeys,
        _ => return None,
    })
}
