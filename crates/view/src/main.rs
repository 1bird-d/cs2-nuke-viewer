//! The nukeplant viewer.
//!
//! Loads a baked `.nkp` scene, lets you fly it, and lets you strip it down to
//! the plant while you are flying — every category can be turned solid, ghosted
//! or hidden without leaving the window.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use clap::Parser;
use glam::Vec3;
use winit::application::ApplicationHandler;
use winit::event::{DeviceEvent, DeviceId, ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{Window, WindowId};

use plant::camera::Camera;
use plant::category::{Classification, Mode};
use plant::gpu::{GhostParams, Renderer};
use plant::panel::{self, Panel};
use plant::{
    build_labels, cursor_ray, export, identify, identify_instance, pick, Input, Instant, ViewKey,
};

#[derive(Parser)]
#[command(name = "nukeplant", about = "Fly de_nuke as a power plant")]
struct Args {
    /// The baked scene to open.
    #[arg(default_value = "scenes/de_nuke.nkp")]
    scene: PathBuf,

    /// Open the window without taking focus. Agent-driven runs use this so a
    /// launch does not interrupt whatever the user is doing.
    #[arg(long)]
    unfocused: bool,

    /// Which view to start in: `plant`, `pipes` or `everything`.
    #[arg(long, default_value = "plant")]
    preset: String,

    /// Start with one category solid and everything else hidden.
    #[arg(long)]
    solo: Option<String>,

    /// Comma-separated category ids to keep solid, hiding everything else —
    /// `--keep pipe,vessel,instrument`. Applied after `--preset`.
    #[arg(long)]
    keep: Option<String>,

    /// Render one frame to this PNG and exit, without opening a window.
    #[arg(long)]
    screenshot: Option<PathBuf>,

    /// Camera position for `--screenshot`, as `x,y,z` in world metres.
    #[arg(long, allow_hyphen_values = true)]
    eye: Option<String>,

    /// Camera yaw in degrees for `--screenshot`.
    #[arg(long, allow_hyphen_values = true)]
    yaw: Option<f32>,

    /// Camera pitch in degrees for `--screenshot`.
    #[arg(long, allow_hyphen_values = true)]
    pitch: Option<f32>,

    /// Frame size for `--screenshot`.
    #[arg(long, default_value_t = 1920)]
    width: u32,

    /// Frame size for `--screenshot`.
    #[arg(long, default_value_t = 1080)]
    height: u32,

    /// Print what the map is made of, by category, and exit.
    #[arg(long)]
    stats: bool,

    /// Write a smaller `.nkp` holding only the categories that are not hidden,
    /// then exit. This is what the web viewer is published with — the plant on
    /// its own, at a fraction of the size of the whole map.
    #[arg(long)]
    export_web: Option<PathBuf>,

    /// Open the window, let it settle, capture the real frame — panel and all —
    /// to this PNG, then quit. This is how a change gets checked without
    /// screen-grabbing, which would capture whatever is in front of the window.
    #[arg(long)]
    selftest: Option<PathBuf>,
}

fn triple(text: &str, flag: &str) -> Result<[f32; 3]> {
    let parts: Vec<f32> = text
        .split(',')
        .map(|p| p.trim().parse::<f32>())
        .collect::<Result<_, _>>()
        .with_context(|| format!("{flag} {text} is not three numbers"))?;
    let [a, b, c] = parts[..] else {
        anyhow::bail!("{flag} wants three values but got {}", parts.len());
    };
    Ok([a, b, c])
}

fn main() -> Result<()> {
    let args = Args::parse();
    let scene = nkp_format::Scene::open(&args.scene)
        .with_context(|| format!("opening {}", args.scene.display()))?;

    let header = *scene.header();
    println!(
        "scene:  {} instances, {} vertices, {} triangles, {} materials",
        header.instance_count,
        header.vertex_count,
        header.index_count / 3,
        header.material_count
    );

    let mut classification = Classification::new(&scene)?;

    if args.stats {
        print_stats(&classification);
        return Ok(());
    }

    match args.preset.as_str() {
        "plant" => classification.preset_plant(),
        "pipes" => classification.preset_pipes_only(),
        "everything" | "all" => classification.preset_everything(),
        other => anyhow::bail!("--preset wants plant, pipes or everything, not {other}"),
    }
    if let Some(id) = &args.solo {
        let index = classification
            .index_of(id)
            .with_context(|| format!("--solo {id} is not a category"))?;
        classification.set_all(Mode::Hidden);
        classification.categories[index].mode = Mode::Solid;
    }
    if let Some(list) = &args.keep {
        let wanted: Vec<usize> = list
            .split(',')
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .map(|id| {
                classification
                    .index_of(id)
                    .with_context(|| format!("--keep {id} is not a category"))
            })
            .collect::<Result<_>>()?;
        classification.set_all(Mode::Hidden);
        for index in wanted {
            classification.categories[index].mode = Mode::Solid;
        }
    }

    if let Some(path) = &args.export_web {
        let summary = export::write_subset(&scene, &classification, path)?;
        #[allow(clippy::cast_precision_loss)]
        let mb = |bytes: usize| bytes as f64 / (1024.0 * 1024.0);
        println!(
            "wrote {}\n  instances {} -> {}\n  vertices  {} -> {} ({} triangles)\n  size      \
             {:.1} MB -> {:.1} MB",
            path.display(),
            summary.instances_in,
            summary.instances_out,
            summary.vertices_in,
            summary.vertices_out,
            summary.triangles_out,
            mb(summary.bytes_in),
            mb(summary.bytes_out),
        );
        return Ok(());
    }

    if let Some(path) = &args.screenshot {
        return screenshot(&scene, &classification, &args, path);
    }

    print_legend(&classification);

    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::Poll);
    let mut app = App::new(scene, classification, args.unfocused);
    // The same camera flags work on the window as on `--screenshot`, so a shot
    // can be lined up offscreen and then flown from.
    apply_camera_overrides(&mut app.camera, &args)?;
    app.selftest = args.selftest.clone();
    event_loop.run_app(&mut app)?;
    app.result
}

/// Apply `--eye`, `--yaw` and `--pitch` on top of whatever framing was chosen.
fn apply_camera_overrides(camera: &mut Camera, args: &Args) -> Result<()> {
    if let Some(eye) = &args.eye {
        camera.position = Vec3::from(triple(eye, "--eye")?);
    }
    if let Some(yaw) = args.yaw {
        camera.yaw = yaw.to_radians();
    }
    if let Some(pitch) = args.pitch {
        camera.pitch = pitch.to_radians();
    }
    Ok(())
}

fn print_stats(classification: &Classification) {
    println!(
        "\n{:<16} {:>9} {:>12}  {}",
        "category", "instances", "triangles", "default"
    );
    for category in &classification.categories {
        println!(
            "{:<16} {:>9} {:>12}  {}",
            category.label,
            category.instances,
            category.triangles,
            category.mode.tag()
        );
    }
}

/// The controls, printed on start so they are discoverable without the panel.
fn print_legend(classification: &Classification) {
    println!("\n  fly     WASD, Q/E down/up, hold RIGHT MOUSE to look");
    println!("          Shift sprint, Ctrl creep, scroll speed, F frame what is visible");
    println!("\n  views   P plant   I pipes+instruments only   M whole map");
    println!("          G ghost everything hidden   X hide everything ghosted");
    println!("          [ ] ghost brightness   H hide panel   F12 screenshot   Esc quit");
    let identifier = identify::Identifier::new();
    println!(
        "\n  names   hover anything to see what it most likely represents ({} props identified)",
        identifier.as_ref().map_or(0, identify::Identifier::len)
    );
    println!("          L toggles the pinned equipment names. Sourced from docs/ABWR-review.md.");
    println!(
        "\n  real    LEFT CLICK to hold a prop in the panel, with {} links to photographs of the",
        identifier
            .as_ref()
            .map_or(0, identify::Identifier::reference_count)
    );
    println!("          real equipment it represents. Only solid geometry is clickable.");
    println!("\n  toggle a category (solid -> ghost -> hidden):");
    for (i, category) in classification.categories.iter().enumerate().take(10) {
        let key = if i == 9 { 0 } else { i + 1 };
        println!(
            "          {key}  {:<16} {:>6} inst  {}",
            category.label,
            category.instances,
            category.mode.tag()
        );
    }
    println!();
}

fn screenshot(
    scene: &nkp_format::Scene,
    classification: &Classification,
    args: &Args,
    path: &PathBuf,
) -> Result<()> {
    let (min, max) = classification.visible_bounds(scene);
    let mut camera = Camera::default();
    #[allow(clippy::cast_precision_loss)]
    let aspect = args.width as f32 / args.height.max(1) as f32;
    camera.frame(Vec3::from(min), Vec3::from(max), aspect);
    apply_camera_overrides(&mut camera, args)?;

    let mut renderer = pollster::block_on(Renderer::offscreen(scene, args.width, args.height))?;
    renderer.apply(scene, classification, &|_| None);
    let stats = renderer.stats();
    println!(
        "drawing {} solid + {} ghost instances, {} triangles",
        stats.solid, stats.ghost, stats.triangles
    );
    let view_proj = camera.view_projection(renderer.aspect());
    renderer.capture(view_proj, camera.position, GhostParams::default(), path)?;
    let p = camera.position;
    println!(
        "wrote {} from [{:.1}, {:.1}, {:.1}] m, yaw {:.0} deg, pitch {:.0} deg",
        path.display(),
        p.x,
        p.y,
        p.z,
        camera.yaw.to_degrees(),
        camera.pitch.to_degrees()
    );
    Ok(())
}

struct App {
    scene: nkp_format::Scene,
    classification: Classification,
    unfocused: bool,
    window: Option<Arc<Window>>,
    renderer: Option<Renderer>,
    panel: Option<Panel>,
    camera: Camera,
    input: Input,
    ghost: GhostParams,
    /// Last picked instance, drawn in white so you can see what you hit, and
    /// held in the panel so its reference links stay clickable while the cursor
    /// moves off it.
    selected: Option<usize>,
    /// The identification table, from the ABWR review.
    identifier: identify::Identifier,
    /// Instance under the cursor, refreshed whenever the mouse moves.
    hovered: Option<usize>,
    /// The last prop the cursor named, so its reference links stay reachable
    /// after the pointer moves off the geometry towards them.
    sticky: Option<&'static identify::Entry>,
    /// Whether pinned names are drawn over each identified asset.
    show_labels: bool,
    /// Whether the key legend is painted in the corner of the viewport.
    show_keys: bool,
    /// Screen position of the last mouse move, in physical pixels.
    cursor: (f32, f32),
    /// Surface size in physical pixels, for turning the cursor into a ray.
    viewport: (u32, u32),
    /// Set whenever a category changes, so the draw lists rebuild next frame.
    dirty: bool,
    last_frame: Instant,
    last_title: Instant,
    frames: u32,
    fps: f32,
    /// `--selftest`: capture the window after it has settled, then quit.
    selftest: Option<PathBuf>,
    /// Frames drawn since the window opened, for the self-test countdown.
    drawn: u32,
    result: Result<()>,
}

impl App {
    fn new(
        scene: nkp_format::Scene,
        classification: Classification,
        unfocused: bool,
    ) -> Self {
        let (min, max) = classification.visible_bounds(&scene);
        let mut camera = Camera::default();
        // The window opens 1600x900; `F` re-frames against the real aspect once
        // the surface exists.
        camera.frame(Vec3::from(min), Vec3::from(max), 16.0 / 9.0);
        Self {
            scene,
            classification,
            selected: None,
            identifier: identify::Identifier::new().expect("identification table compiles"),
            hovered: None,
            show_labels: true,
            show_keys: true,
            sticky: None,
            cursor: (0.0, 0.0),
            viewport: (1600, 900),
            unfocused,
            window: None,
            renderer: None,
            panel: None,
            camera,
            input: Input::default(),
            ghost: GhostParams::default(),
            dirty: true,
            last_frame: Instant::now(),
            last_title: Instant::now(),
            frames: 0,
            fps: 0.0,
            selftest: None,
            drawn: 0,
            result: Ok(()),
        }
    }

    fn fail(&mut self, event_loop: &ActiveEventLoop, error: anyhow::Error) {
        self.result = Err(error);
        event_loop.exit();
    }

    /// Re-frame the camera on whatever is currently visible.
    fn frame_visible(&mut self) {
        let (min, max) = self.classification.visible_bounds(&self.scene);
        let aspect = self.renderer.as_ref().map_or(16.0 / 9.0, Renderer::aspect);
        self.camera.frame(Vec3::from(min), Vec3::from(max), aspect);
    }

    /// The ray through the current cursor position, if the surface has a size.
    fn cursor_ray(&self) -> Option<pick::Ray> {
        let aspect = self.renderer.as_ref()?.aspect();
        cursor_ray(self.cursor, self.viewport, &self.camera, aspect)
    }

    /// Refresh what the cursor is over. Only runs on mouse movement, never per
    /// frame, because an exact ray cast over every solid instance is cheap but
    /// not free.
    fn update_hover(&mut self) {
        if self.input.looking {
            // Mouse-look moves the cursor constantly and the pointer is hidden;
            // a tooltip chasing it would be noise.
            self.hovered = None;
            return;
        }
        self.hovered = self
            .cursor_ray()
            .and_then(|ray| pick::nearest(&self.scene, &self.classification, &ray))
            .map(|hit| hit.instance);
        if let Some(entry) = self.hovered.and_then(|id| self.identified(id)) {
            self.sticky = Some(entry);
        }
    }

    /// The review's identification of an instance, if it has one.
    fn identified(&self, instance: usize) -> Option<&'static identify::Entry> {
        identify_instance(&self.scene, &self.identifier, instance)
    }

    /// Cast a ray through the cursor and hold whatever it hits in the panel.
    ///
    /// Clicking used to paint pipes into hand-tagged runs. That went: the map's
    /// pipework is decorative kit, so the runs were a judgement recorded against
    /// a process flow that does not exist, and the panel space is worth more as
    /// the equipment identification. A click now just latches the selection so
    /// the reference links stay clickable once the cursor leaves the prop.
    fn select(&mut self) {
        let Some(ray) = self.cursor_ray() else {
            return;
        };
        let Some(hit) = pick::nearest(&self.scene, &self.classification, &ray) else {
            self.selected = None;
            self.dirty = true;
            return;
        };

        let instance = &self.scene.instances()[hit.instance];
        let name = self.scene.instance_name(instance).to_string();
        let material = self
            .scene
            .materials()
            .get(instance.material as usize)
            .map_or("", |m| self.scene.material_name(m))
            .to_string();

        // The hit point is printed because it is the coordinate you want when
        // deciding where a proxy — the turbine stack — should sit.
        println!(
            "#{} at [{:.1}, {:.1}, {:.1}] m   {name}   {material}",
            hit.instance, hit.point.x, hit.point.y, hit.point.z
        );
        if let Some(entry) = self.identified(hit.instance) {
            println!(
                "        {}: {} → {}",
                entry.confidence.tag(),
                entry.what,
                entry.real
            );
            for reference in entry.refs {
                println!("        {}  {}", reference.label, reference.url);
            }
        }
        self.selected = Some(hit.instance);
        self.dirty = true;
    }

    /// Report what changed, so the console keeps up with the window.
    fn announce(&self, what: &str) {
        let (instances, triangles) = self.classification.drawn();
        println!("{what}  ->  {instances} instances, {triangles} triangles");
    }

    fn handle_key(&mut self, code: KeyCode) {
        if let Some(key) = view_key(code) {
            let effect =
                plant::apply_view_key(key, &mut self.classification, &mut self.ghost);
            if effect.dirty {
                self.dirty = true;
            }
            if effect.toggle_labels {
                self.show_labels = !self.show_labels;
                println!(
                    "equipment names {}",
                    if self.show_labels { "shown" } else { "hidden" }
                );
            }
            if effect.toggle_keys {
                self.show_keys = !self.show_keys;
            }
            if let Some(what) = effect.announce {
                if effect.dirty {
                    self.announce(&what);
                } else {
                    println!("{what}");
                }
            }
            return;
        }
        match code {
            KeyCode::KeyF => self.frame_visible(),
            KeyCode::F12 => {
                if let Some(renderer) = &mut self.renderer {
                    let stamp = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map_or(0, |d| d.as_secs());
                    let path = PathBuf::from(format!("captures/nukeplant_{stamp}.png"));
                    renderer.request_capture(path);
                }
            }
            _ => {}
        }
    }
}

/// Map a physical key to what it means to the viewer, for the keys that behave
/// the same in the browser. Everything else — `F`, `H`, `F12`, `Esc` — is
/// desktop-specific and stays in [`App::handle_key`].
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

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attributes = Window::default_attributes()
            .with_title("nukeplant — de_nuke")
            .with_inner_size(winit::dpi::LogicalSize::new(1600, 900))
            // A window that steals focus every rebuild is the single most
            // annoying thing a tool like this can do while you are working.
            .with_active(!self.unfocused);

        let window = match event_loop.create_window(attributes) {
            Ok(window) => Arc::new(window),
            Err(e) => return self.fail(event_loop, e.into()),
        };
        match pollster::block_on(Renderer::windowed(window.clone(), &self.scene)) {
            Ok(renderer) => {
                self.panel = Some(Panel::new(renderer.device(), &window, renderer.format()));
                self.renderer = Some(renderer);
            }
            Err(e) => return self.fail(event_loop, e),
        }
        self.window = Some(window);
        self.dirty = true;
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        // Give the panel first refusal. A click on a button, or typing in a
        // field, must not also fly the camera.
        let consumed = match (self.panel.as_mut(), self.window.as_ref()) {
            (Some(panel), Some(window)) => panel.on_window_event(window, &event),
            _ => false,
        };
        if consumed
            && !plant::is_release(&event)
            && !matches!(
                event,
                WindowEvent::CloseRequested
                    | WindowEvent::Resized(_)
                    | WindowEvent::RedrawRequested
                    | WindowEvent::Focused(_)
            )
        {
            return;
        }

        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            // Losing focus means the release events are going somewhere else.
            // Alt-tab away mid-flight and the key-up never arrives, so without
            // this the camera is still travelling when you come back.
            WindowEvent::Focused(false) => {
                self.input = Input::default();
                if let Some(window) = &self.window {
                    window.set_cursor_visible(true);
                }
            }
            WindowEvent::Resized(size) => {
                self.viewport = (size.width, size.height);
                if let Some(renderer) = &mut self.renderer {
                    renderer.resize(size.width, size.height);
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
                    // Right drag looks around; a right *click* that did not
                    // move the camera untags. Tracking that properly needs a
                    // drag threshold, so untagging lives on Alt+left instead —
                    // less clever, and it never fights the camera.
                    MouseButton::Right => {
                        self.input.looking = pressed;
                        if let Some(window) = &self.window {
                            window.set_cursor_visible(!pressed);
                        }
                    }
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
                    KeyCode::Escape if down => event_loop.exit(),
                    KeyCode::KeyH if down && !event.repeat => {
                        if let Some(panel) = &mut self.panel {
                            panel.visible = !panel.visible;
                            println!(
                                "panel {}",
                                if panel.visible { "shown" } else { "hidden" }
                            );
                        }
                    }
                    // Everything else acts on press only, not on release.
                    _ if down && !event.repeat => self.handle_key(code),
                    _ => {}
                }
            }
            WindowEvent::RedrawRequested => {
                let now = Instant::now();
                let dt = (now - self.last_frame).as_secs_f32().min(0.1);
                self.last_frame = now;

                // Destructured so the renderer and the panel can be borrowed at
                // the same time — they are disjoint fields, but the borrow
                // checker only sees that if `self` is taken apart first.
                let App {
                    scene,
                    classification,
                    selected,
                    identifier,
                    hovered,
                    sticky,
                    show_labels,
                    show_keys,
                    cursor,
                    window,
                    renderer,
                    panel,
                    camera,
                    input,
                    ghost,
                    dirty,
                    fps,
                    ..
                } = self;
                let (Some(renderer), Some(panel), Some(window)) =
                    (renderer.as_mut(), panel.as_mut(), window.as_ref())
                else {
                    return;
                };

                let (forward, right, up) = input.axes();
                camera.fly(forward, right, up, dt, input.boost());

                if *dirty {
                    // The picked instance draws white, because you have to be
                    // able to see what you just clicked.
                    let picked = *selected;
                    renderer.apply(scene, classification, &|id| {
                        (picked == Some(id)).then_some([1.0, 1.0, 1.0])
                    });
                    *dirty = false;
                }

                let view_proj = camera.view_projection(renderer.aspect());
                let stats = renderer.stats();
                let shown = *fps;
                let mut panel_response = panel::Response::default();
                let selected_name = selected
                    .and_then(|id| scene.instances().get(id))
                    .map_or(String::new(), |i| scene.instance_name(i).to_string());
                let selection = panel::Selection {
                    instance: *selected,
                    name: &selected_name,
                    entry: selected.and_then(|id| {
                        let inst = scene.instances().get(id)?;
                        let material = scene
                            .materials()
                            .get(inst.material as usize)
                            .map_or("", |m| scene.material_name(m));
                        identifier.lookup(material, scene.instance_name(inst))
                    }),
                    sticky: *sticky,
                };

                let labels = build_labels(
                    scene,
                    classification,
                    identifier,
                    *hovered,
                    *show_labels,
                    camera.position,
                    view_proj,
                    *cursor,
                );

                let outcome = renderer.render(
                    view_proj,
                    camera.position,
                    *ghost,
                    &mut |device, queue, encoder, view, size| {
                        panel_response = panel.draw(
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
                            *show_keys,
                            stats,
                            shown,
                        );
                    },
                );
                if let Err(e) = outcome {
                    return self.fail(event_loop, e);
                }

                if panel_response.dirty {
                    self.dirty = true;
                }
                if panel_response.frame_visible {
                    self.frame_visible();
                }

                // Self-test: let the window settle so egui has laid out and the
                // swapchain has cycled, then grab one real frame and quit.
                self.drawn += 1;
                if let Some(path) = self.selftest.clone() {
                    if self.drawn == 25 {
                        // Park the cursor at screen centre so the capture
                        // exercises the hover card, not just the pinned labels.
                        #[allow(clippy::cast_precision_loss)]
                        {
                            self.cursor =
                                (self.viewport.0 as f32 / 2.0, self.viewport.1 as f32 / 2.0);
                        }
                        self.update_hover();
                    }
                    if self.drawn == 30 {
                        if let Some(renderer) = &mut self.renderer {
                            renderer.request_capture(path);
                        }
                    } else if self.drawn > 31 {
                        event_loop.exit();
                    }
                }

                self.frames += 1;
                let elapsed = (now - self.last_title).as_secs_f32();
                if elapsed >= 0.5 {
                    #[allow(clippy::cast_precision_loss)]
                    {
                        self.fps = self.frames as f32 / elapsed;
                    }
                    if let Some(window) = &self.window {
                        let p = self.camera.position;
                        window.set_title(&format!(
                            "nukeplant — {:.0} fps — {} solid + {} ghost — {} tris — \
                             [{:.0}, {:.0}, {:.0}] m",
                            self.fps, stats.solid, stats.ghost, stats.triangles, p.x, p.y, p.z
                        ));
                    }
                    self.frames = 0;
                    self.last_title = now;
                }
            }
            _ => {}
        }
    }

    fn device_event(&mut self, _: &ActiveEventLoop, _: DeviceId, event: DeviceEvent) {
        if let DeviceEvent::MouseMotion { delta } = event {
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
