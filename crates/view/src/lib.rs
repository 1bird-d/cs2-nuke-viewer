//! The nukeplant viewer, as a library.
//!
//! Everything that is not tied to how the program was started lives here, so
//! the desktop binary and the browser build render the same scene through the
//! same code. The two entry points differ only in how they get a window, how
//! they get the scene bytes, and what they are allowed to write to disk:
//!
//! | | desktop | browser |
//! |---|---|---|
//! | window | `winit` on the OS | a `<canvas>` |
//! | scene | memory-mapped file | fetched, or dropped on the page |
//! | screenshots | `F12` writes a PNG | not available |
//! | arguments | `clap` | URL query string |
//!
//! Nothing below knows which of those it is running under.

pub mod camera;
pub mod category;
pub mod gpu;
pub mod identify;
pub mod panel;
pub mod pick;
// Not gated to wasm: the gesture logic is pure arithmetic over positions and
// timestamps, so it is unit-tested on the desktop where tests actually run.
pub mod touch;

use glam::Vec3;

use category::{Classification, Mode};
use gpu::GhostParams;

/// The wall clock.
///
/// `std::time::Instant` compiles on wasm and then panics the first time it is
/// read, because there is no monotonic clock behind it. `web-time` is the same
/// type everywhere else and a `performance.now()` shim in the browser.
pub use web_time::Instant;

/// Which movement keys are down this frame.
#[derive(Default)]
pub struct Input {
    pub forward: bool,
    pub back: bool,
    pub left: bool,
    pub right: bool,
    pub up: bool,
    pub down: bool,
    pub fast: bool,
    pub slow: bool,
    pub looking: bool,
}

impl Input {
    /// Forward, right and up, each in −1..1.
    #[must_use]
    pub fn axes(&self) -> (f32, f32, f32) {
        let axis = |a: bool, b: bool| f32::from(a) - f32::from(b);
        (
            axis(self.forward, self.back),
            axis(self.right, self.left),
            axis(self.up, self.down),
        )
    }

    /// Speed multiplier from the modifier keys.
    #[must_use]
    pub fn boost(&self) -> f32 {
        if self.fast {
            5.0
        } else if self.slow {
            0.2
        } else {
            1.0
        }
    }
}

/// Whether an event is a key or mouse-button *release*.
///
/// Releases must reach the camera even when the panel says it consumed the
/// event. egui claims keyboard and pointer input whenever it has focus, and if
/// it swallows the release of a key that is currently held, nothing ever clears
/// that key: the camera flies in one direction forever, as though the key were
/// still down. A press can safely be eaten by the UI. A release cannot, because
/// it is the only thing that ends a state.
#[must_use]
pub fn is_release(event: &winit::event::WindowEvent) -> bool {
    use winit::event::{ElementState, WindowEvent};
    match event {
        WindowEvent::KeyboardInput { event, .. } => event.state == ElementState::Released,
        WindowEvent::MouseInput { state, .. } => *state == ElementState::Released,
        _ => false,
    }
}

/// What a view key did, so the caller can report it and rebuild if needed.
pub struct KeyEffect {
    /// Human-readable description of the change, for the console or the title.
    pub announce: Option<String>,
    /// The set of drawn instances changed; draw lists need rebuilding.
    pub dirty: bool,
    /// `L` was pressed.
    pub toggle_labels: bool,
    /// `K` was pressed.
    pub toggle_keys: bool,
}

/// Apply the keys that mean the same thing on both platforms: the category
/// digits, the three presets, the two bulk ghost operations and the brightness
/// pair.
///
/// Kept here rather than in either entry point so the desktop and the browser
/// cannot drift into having different controls, which would make the on-screen
/// legend wrong in one of them.
#[must_use]
pub fn apply_view_key(
    key: ViewKey,
    classification: &mut Classification,
    ghost: &mut GhostParams,
) -> KeyEffect {
    let mut effect = KeyEffect {
        announce: None,
        dirty: false,
        toggle_labels: false,
        toggle_keys: false,
    };
    match key {
        ViewKey::Category(index) => {
            if let Some(category) = classification.categories.get_mut(index) {
                category.mode = category.mode.next();
                effect.announce = Some(format!("{} {}", category.label, category.mode.tag()));
                effect.dirty = true;
            }
        }
        ViewKey::PresetPlant => {
            classification.preset_plant();
            effect.announce = Some("preset: process plant solid, structure ghosted".into());
            effect.dirty = true;
        }
        ViewKey::PresetPipes => {
            classification.preset_pipes_only();
            effect.announce = Some("preset: pipework and instrumentation only".into());
            effect.dirty = true;
        }
        ViewKey::PresetEverything => {
            classification.preset_everything();
            effect.announce = Some("preset: whole map".into());
            effect.dirty = true;
        }
        ViewKey::GhostHidden => {
            for category in &mut classification.categories {
                if category.mode == Mode::Hidden {
                    category.mode = Mode::Ghost;
                }
            }
            effect.announce = Some("ghosted everything that was hidden".into());
            effect.dirty = true;
        }
        ViewKey::HideGhosted => {
            for category in &mut classification.categories {
                if category.mode == Mode::Ghost {
                    category.mode = Mode::Hidden;
                }
            }
            effect.announce = Some("hid everything that was ghosted".into());
            effect.dirty = true;
        }
        ViewKey::Dimmer | ViewKey::Brighter => {
            let step = if matches!(key, ViewKey::Brighter) { 1.25 } else { 0.8 };
            ghost.gain = (ghost.gain * step).clamp(0.02, 4.0);
            effect.announce = Some(format!("ghost brightness {:.2}", ghost.gain));
        }
        ViewKey::ToggleLabels => effect.toggle_labels = true,
        ViewKey::ToggleKeys => effect.toggle_keys = true,
    }
    effect
}

/// The platform-independent meaning of a key press.
#[derive(Clone, Copy)]
pub enum ViewKey {
    /// Cycle one category: solid, ghost, hidden.
    Category(usize),
    PresetPlant,
    PresetPipes,
    PresetEverything,
    GhostHidden,
    HideGhosted,
    Dimmer,
    Brighter,
    ToggleLabels,
    /// Show or hide the key legend in the corner of the viewport.
    ToggleKeys,
}

/// Collect the names to draw over the scene this frame.
///
/// One pinned label per *distinct* identified asset, placed on whichever of its
/// instances is nearest the camera. Labelling every instance would put 1,927
/// tags on the pipework alone; one per asset is the readable version and still
/// names everything the review could identify.
#[must_use]
pub fn build_labels<'a>(
    scene: &nkp_format::Scene,
    classification: &Classification,
    identifier: &identify::Identifier,
    hovered: Option<usize>,
    show_labels: bool,
    eye: Vec3,
    view_proj: glam::Mat4,
    cursor_px: (f32, f32),
) -> panel::Labels<'a> {
    let naming = |instance: usize| -> Option<panel::Naming<'a>> {
        let inst = scene.instances().get(instance)?;
        let material = scene
            .materials()
            .get(inst.material as usize)
            .map_or("", |m| scene.material_name(m));
        let entry = identifier.lookup(material, scene.instance_name(inst))?;
        let world = (Vec3::from(inst.aabb_min) + Vec3::from(inst.aabb_max)) * 0.5;
        Some(panel::Naming {
            entry,
            world,
            distance: (world - eye).length(),
        })
    };

    let mut pinned: Vec<(String, panel::Naming<'a>)> = Vec::new();
    if show_labels {
        // Keep the nearest instance of each asset. Keyed by the entry's own
        // address, which is stable because the table is `'static`.
        let mut best: std::collections::HashMap<usize, (usize, f32)> =
            std::collections::HashMap::new();
        for (id, _) in scene.instances().iter().enumerate() {
            if classification.category_of(id).mode != Mode::Solid {
                continue;
            }
            let Some(named) = naming(id) else { continue };
            let key = std::ptr::from_ref(named.entry) as usize;
            let entry = best.entry(key).or_insert((id, f32::INFINITY));
            if named.distance < entry.1 {
                *entry = (id, named.distance);
            }
        }
        for (id, _) in best.into_values() {
            if let Some(named) = naming(id) {
                // Beyond this the label is smaller than the thing it names and
                // only adds clutter to a wide shot.
                if named.distance <= 320.0 {
                    pinned.push((named.entry.real.to_string(), named));
                }
            }
        }
        // Nearest first: the closest label wins contested screen space when the
        // panel declutters them.
        pinned.sort_by(|a, b| a.1.distance.total_cmp(&b.1.distance));
    }

    panel::Labels {
        hovered: hovered.and_then(naming),
        pinned,
        pinned_on: show_labels,
        view_proj,
        cursor_px,
    }
}

/// The review's identification of one instance, if it has one.
#[must_use]
pub fn identify_instance(
    scene: &nkp_format::Scene,
    identifier: &identify::Identifier,
    instance: usize,
) -> Option<&'static identify::Entry> {
    let inst = scene.instances().get(instance)?;
    let material = scene
        .materials()
        .get(inst.material as usize)
        .map_or("", |m| scene.material_name(m));
    identifier.lookup(material, scene.instance_name(inst))
}

/// Turn a cursor position in physical pixels into a picking ray.
#[must_use]
pub fn cursor_ray(
    cursor: (f32, f32),
    viewport: (u32, u32),
    camera: &camera::Camera,
    aspect: f32,
) -> Option<pick::Ray> {
    let (width, height) = viewport;
    if width == 0 || height == 0 {
        return None;
    }
    #[allow(clippy::cast_precision_loss)]
    let ndc = (
        (cursor.0 / width as f32).mul_add(2.0, -1.0),
        // Screen y grows downward, clip space y grows up.
        1.0 - (cursor.1 / height as f32) * 2.0,
    );
    let view_proj = camera.view_projection(aspect);
    Some(pick::Ray::through(
        view_proj.inverse(),
        ndc,
        camera.position,
    ))
}

#[cfg(not(target_arch = "wasm32"))]
pub mod export;

#[cfg(target_arch = "wasm32")]
pub mod web;
#[cfg(target_arch = "wasm32")]
pub mod webinput;

#[cfg(test)]
mod tests {
    use super::*;
    use winit::event::{DeviceId, ElementState, MouseButton, WindowEvent};

    /// The stuck-key bug: hold a movement key, let the panel take focus, and
    /// the key-up is consumed before the camera sees it — so the camera keeps
    /// flying with nothing holding the key down. Releases must be exempt from
    /// the consumed-by-egui early return.
    #[test]
    fn releases_are_recognised_so_they_can_never_be_swallowed() {
        let press = WindowEvent::MouseInput {
            device_id: DeviceId::dummy(),
            state: ElementState::Pressed,
            button: MouseButton::Right,
        };
        let release = WindowEvent::MouseInput {
            device_id: DeviceId::dummy(),
            state: ElementState::Released,
            button: MouseButton::Right,
        };
        assert!(!is_release(&press));
        assert!(is_release(&release));
        // Anything that is not a press or release is neither.
        assert!(!is_release(&WindowEvent::RedrawRequested));
    }
}
