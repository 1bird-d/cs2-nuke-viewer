//! The in-window control panel.
//!
//! Everything the viewer can do is reachable here without touching the keyboard
//! or restarting: each category can be set solid, ghosted or hidden, recoloured,
//! and the ghost shading tuned, all while the camera keeps flying.
//!
//! The panel owns nothing about the scene. It reads and mutates a
//! [`Classification`] and a [`GhostParams`], reports whether anything changed,
//! and leaves the caller to rebuild the draw lists — which keeps the rebuild
//! out of the UI code and makes it obvious when it happens.

use egui_wgpu::{Renderer as EguiRenderer, RendererOptions, ScreenDescriptor};
use winit::window::Window;

#[cfg(not(target_arch = "wasm32"))]
use egui_winit::State;
#[cfg(target_arch = "wasm32")]
use crate::webinput::WebInput;

use crate::category::{Category, Classification, Mode};
use crate::gpu::{DrawStats, GhostParams};
use crate::identify::Entry;

/// What the panel wants the app to do after a frame.
#[derive(Default)]
pub struct Response {
    /// A category's mode or colour changed; rebuild the draw lists.
    pub dirty: bool,
    /// Re-frame the camera on what is now visible.
    pub frame_visible: bool,
    /// The pointer is over the panel, so the viewport must ignore this click.
    pub wants_pointer: bool,
    /// The panel has keyboard focus, so movement keys must be ignored.
    pub wants_keyboard: bool,
}

/// What the app tells the panel about the thing the user last clicked.
#[derive(Clone, Copy, Default)]
pub struct Selection<'a> {
    /// The picked instance, if any.
    pub instance: Option<usize>,
    /// Its asset name, for the console-style readout.
    pub name: &'a str,
    /// What the review says it is, if it says anything.
    pub entry: Option<&'static Entry>,
    /// The last thing the cursor named, kept after it moves away.
    ///
    /// Without this the panel is unusable: the reference links only appear
    /// while something is hovered, and moving the pointer towards them takes it
    /// off the geometry, so the links disappear before they can be clicked.
    pub sticky: Option<&'static Entry>,
}

/// One thing to name on screen: either under the cursor, or pinned in space.
pub struct Naming<'a> {
    /// What the review says this prop is.
    pub entry: &'a crate::identify::Entry,
    /// Where it is, in world metres — used to place a pinned label.
    pub world: glam::Vec3,
    /// How far the camera is from it.
    pub distance: f32,
}

/// Everything the label layer needs for one frame.
pub struct Labels<'a> {
    /// The prop under the cursor, if the ray hit one that can be named.
    pub hovered: Option<Naming<'a>>,
    /// One pinned label per distinct identified asset that is currently solid.
    pub pinned: Vec<(String, Naming<'a>)>,
    /// Whether pinned labels are switched on.
    pub pinned_on: bool,
    /// Clip-space transform for projecting a world position to the screen.
    pub view_proj: glam::Mat4,
    /// Cursor position in physical pixels — the same one the pick ray used.
    ///
    /// Deliberately not egui's own pointer position: the ray and the card have
    /// to agree about where the cursor is, and egui only knows once it has seen
    /// a real pointer event, which a headless self-test never generates.
    pub cursor_px: (f32, f32),
}

pub struct Panel {
    /// On the desktop `egui-winit` translates input. On the web it does not
    /// build at all, so [`WebInput`] does the same job — see that module.
    #[cfg(not(target_arch = "wasm32"))]
    state: State,
    #[cfg(target_arch = "wasm32")]
    context: egui::Context,
    #[cfg(target_arch = "wasm32")]
    input: WebInput,
    renderer: EguiRenderer,
    /// Hidden with `H` for a clean frame when capturing.
    pub visible: bool,
}

impl Panel {
    /// Attach egui to a window and a surface format.
    pub fn new(
        device: &wgpu::Device,
        #[cfg_attr(target_arch = "wasm32", allow(unused_variables))]
        window: &Window,
        format: wgpu::TextureFormat,
    ) -> Self {
        let context = egui::Context::default();
        context.set_visuals(visuals());
        #[allow(clippy::cast_possible_truncation)]
        let scale = window.scale_factor() as f32;
        context.set_pixels_per_point(scale);

        #[cfg(not(target_arch = "wasm32"))]
        let state = State::new(
            context,
            egui::ViewportId::ROOT,
            window,
            Some(scale),
            None,
            None,
        );

        let renderer = EguiRenderer::new(
            device,
            format,
            RendererOptions {
                // The scene has already resolved into the target by the time the
                // panel draws, so egui needs no depth buffer of its own.
                depth_stencil_format: None,
                ..RendererOptions::default()
            },
        );
        Self {
            #[cfg(not(target_arch = "wasm32"))]
            state,
            #[cfg(target_arch = "wasm32")]
            context,
            #[cfg(target_arch = "wasm32")]
            input: WebInput::new(),
            renderer,
            visible: true,
        }
    }

    /// The egui context, however it is being held.
    fn context(&self) -> egui::Context {
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.state.egui_ctx().clone()
        }
        #[cfg(target_arch = "wasm32")]
        {
            self.context.clone()
        }
    }

    /// Feed a window event to egui. Returns true if egui consumed it.
    ///
    /// When the panel is hidden nothing is interactive, so events go straight
    /// to the camera even though the label layer is still drawing.
    #[allow(unused_variables)]
    pub fn on_window_event(&mut self, window: &Window, event: &winit::event::WindowEvent) -> bool {
        #[cfg(not(target_arch = "wasm32"))]
        let consumed = self.state.on_window_event(window, event).consumed;
        #[cfg(target_arch = "wasm32")]
        let consumed = {
            let scale = self.context.pixels_per_point();
            self.input.on_window_event(event, scale)
        };
        self.visible && consumed
    }

    /// Build and draw the panel into `encoder`.
    ///
    /// Called from inside the renderer's overlay hook, after the scene has been
    /// drawn into `view`.
    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &mut self,
        #[cfg_attr(target_arch = "wasm32", allow(unused_variables))]
        window: &Window,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        size: (u32, u32),
        classification: &mut Classification,
        ghost: &mut GhostParams,
        selection: Selection<'_>,
        labels: &Labels<'_>,
        stats: DrawStats,
        fps: f32,
    ) -> Response {
        let mut response = Response::default();
        let context = self.context();
        #[cfg(not(target_arch = "wasm32"))]
        let input = self.state.take_egui_input(window);
        #[cfg(target_arch = "wasm32")]
        let input = self.input.take(size, context.pixels_per_point());

        // The label layer stays up even when the panel is hidden: `H` is for
        // capturing a clean frame, and the names are the content of that frame,
        // not chrome.
        let show_panel = self.visible;
        let output = context.run_ui(input, |ui| {
            let ctx = ui.ctx();
            // Labels first, so the panel window draws over them rather than
            // being obscured by a label that happens to land underneath it.
            draw_labels(ctx, labels);
            if show_panel {
                build(
                    ctx,
                    classification,
                    ghost,
                    selection,
                    labels,
                    stats,
                    fps,
                    &mut response,
                );
            }
        });

        response.wants_pointer = context.egui_wants_pointer_input();
        response.wants_keyboard = context.egui_wants_keyboard_input();
        #[cfg(not(target_arch = "wasm32"))]
        self.state
            .handle_platform_output(window, output.platform_output);
        #[cfg(target_arch = "wasm32")]
        {
            // Recorded for the next frame's hit test: a click only reaches the
            // panel if egui claimed the pointer, and that is not known until
            // after the UI has run.
            self.input.wants_pointer = response.wants_pointer && self.visible;
            self.input.handle_output(&output.platform_output);
        }

        let pixels_per_point = context.pixels_per_point();
        let jobs = context.tessellate(output.shapes, pixels_per_point);
        let descriptor = ScreenDescriptor {
            size_in_pixels: [size.0, size.1],
            pixels_per_point,
        };

        // A texture can carry several deltas in one frame — a full upload
        // followed by partial patches — so each one has to be applied in order.
        for (id, deltas) in &output.textures_delta.set {
            for delta in deltas {
                self.renderer.update_texture(device, queue, *id, delta);
            }
        }
        self.renderer
            .update_buffers(device, queue, encoder, &jobs, &descriptor);

        {
            let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("panel"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        // Load, not clear: the scene is already in this target.
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            // egui-wgpu wants a 'static pass; the encoder outlives this scope.
            self.renderer
                .render(&mut pass.forget_lifetime(), &jobs, &descriptor);
        }

        for id in &output.textures_delta.free {
            self.renderer.free_texture(id);
        }
        response
    }
}

/// Names over the scene: a pinned tag on each identified asset, and a full card
/// under the cursor.
///
/// Drawn straight onto egui's background layer rather than through a 3D text
/// renderer. Projecting the anchor and calling `painter.text` gives crisp type,
/// correct DPI scaling and no new shaders, which for a page of labels is a
/// better trade than an SDF atlas.
fn draw_labels(ctx: &egui::Context, labels: &Labels<'_>) {
    let screen = ctx.viewport_rect();
    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Background,
        egui::Id::new("nukeplant_labels"),
    ));

    let project = |world: glam::Vec3| -> Option<egui::Pos2> {
        let clip = labels.view_proj * world.extend(1.0);
        // Reverse-Z: w <= 0 is behind the camera.
        if clip.w <= 0.0 {
            return None;
        }
        let ndc = clip.truncate() / clip.w;
        if ndc.x < -1.2 || ndc.x > 1.2 || ndc.y < -1.2 || ndc.y > 1.2 {
            return None;
        }
        Some(egui::pos2(
            screen.left() + (ndc.x * 0.5 + 0.5) * screen.width(),
            screen.top() + (0.5 - ndc.y * 0.5) * screen.height(),
        ))
    };

    if labels.pinned_on {
        // Declutter in screen space. `pinned` arrives nearest-first, so the
        // closest label wins any contested patch of screen and anything that
        // would overlap it is dropped rather than drawn on top. Without this a
        // wide shot stacks twenty names into an unreadable pile.
        let mut placed: Vec<egui::Rect> = Vec::with_capacity(labels.pinned.len());
        for (label, naming) in &labels.pinned {
            let Some(at) = project(naming.world) else {
                continue;
            };
            let [r, g, b] = naming.entry.confidence.colour();
            let colour = egui::Color32::from_rgb(r, g, b);

            let text_at = at + egui::vec2(9.0, -9.0);
            let galley =
                painter.layout_no_wrap(label.clone(), egui::FontId::proportional(12.0), colour);
            let rect =
                egui::Rect::from_min_size(text_at, galley.size()).expand2(egui::vec2(4.0, 2.0));
            // A little breathing room, so labels do not merely touch.
            let claim = rect.expand(3.0);
            if placed.iter().any(|other| other.intersects(claim)) {
                continue;
            }
            placed.push(claim);

            // A short leader from a dot to the text, so a label reads as
            // belonging to a specific object rather than floating near it.
            painter.circle_filled(at, 2.5, colour);
            painter.line_segment([at, text_at], egui::Stroke::new(1.0, colour.gamma_multiply(0.6)));
            painter.rect_filled(rect, 2.0, egui::Color32::from_rgba_unmultiplied(10, 12, 16, 214));
            painter.galley(text_at, galley, colour);
        }
    }

    // The hover card. Placed at the cursor, flipped when it would run off the
    // right or bottom edge.
    let Some(naming) = &labels.hovered else {
        return;
    };
    let scale = ctx.pixels_per_point().max(0.01);
    let cursor = egui::pos2(labels.cursor_px.0 / scale, labels.cursor_px.1 / scale);
    let entry = naming.entry;
    let [r, g, b] = entry.confidence.colour();
    let accent = egui::Color32::from_rgb(r, g, b);

    const WIDTH: f32 = 340.0;
    let mut lines: Vec<std::sync::Arc<egui::Galley>> = Vec::new();
    let mut push = |painter: &egui::Painter, text: String, size: f32, colour: egui::Color32| {
        lines.push(painter.layout(text, egui::FontId::proportional(size), colour, WIDTH));
    };
    push(
        &painter,
        format!("{}   ·   {:.0} m away", entry.confidence.tag(), naming.distance),
        10.5,
        accent,
    );
    push(&painter, entry.what.to_string(), 15.0, egui::Color32::from_rgb(235, 240, 248));
    push(&painter, format!("→ {}", entry.real), 13.0, accent);
    push(&painter, entry.note.to_string(), 11.5, egui::Color32::from_rgb(168, 178, 194));
    // The links themselves live in the panel, where they can be real clickable
    // widgets. Painting them here would put text over the viewport that looks
    // like a link and does nothing, which is worse than a pointer to where the
    // links are.
    if !entry.refs.is_empty() {
        let count = entry.refs.len();
        let plural = if count == 1 { "photo" } else { "photos" };
        push(
            &painter,
            format!("click — {count} {plural} of the real unit, in the panel"),
            11.0,
            accent.gamma_multiply(0.85),
        );
    }

    let pad = 10.0;
    let gap = 5.0;
    #[allow(clippy::cast_precision_loss)]
    let gaps = (lines.len().saturating_sub(1)) as f32 * gap;
    let height = lines.iter().map(|g| g.size().y).sum::<f32>() + gaps + pad * 2.0;
    let width = WIDTH + pad * 2.0;

    let mut origin = cursor + egui::vec2(18.0, 18.0);
    if origin.x + width > screen.right() {
        origin.x = cursor.x - width - 18.0;
    }
    if origin.y + height > screen.bottom() {
        origin.y = (screen.bottom() - height - 8.0).max(screen.top() + 8.0);
    }

    let rect = egui::Rect::from_min_size(origin, egui::vec2(width, height));
    painter.rect_filled(rect, 3.0, egui::Color32::from_rgba_unmultiplied(12, 14, 19, 244));
    painter.rect_stroke(
        rect,
        3.0,
        egui::Stroke::new(1.0, accent.gamma_multiply(0.55)),
        egui::StrokeKind::Inside,
    );

    let mut y = origin.y + pad;
    for galley in lines {
        let size = galley.size();
        painter.galley(egui::pos2(origin.x + pad, y), galley, accent);
        y += size.y + gap;
    }
}

/// A dark panel that sits over a dark viewport without competing with it.
///
/// The viewport behind is near-black, so egui's default dark theme lands at
/// almost no contrast against it. The fill goes opaque and the text goes bright;
/// a translucent panel over a dark scene is unreadable, not tasteful.
fn visuals() -> egui::Visuals {
    let mut visuals = egui::Visuals::dark();
    let fill = egui::Color32::from_rgb(18, 21, 27);
    visuals.window_fill = fill;
    visuals.panel_fill = fill;
    visuals.extreme_bg_color = egui::Color32::from_rgb(10, 12, 16);
    visuals.window_stroke = egui::Stroke::new(1.0, egui::Color32::from_rgb(58, 66, 82));
    visuals.override_text_color = Some(egui::Color32::from_rgb(222, 228, 238));
    visuals.widgets.noninteractive.bg_fill = fill;
    visuals.widgets.inactive.bg_fill = egui::Color32::from_rgb(34, 39, 49);
    visuals.widgets.hovered.bg_fill = egui::Color32::from_rgb(52, 60, 75);
    visuals.widgets.active.bg_fill = egui::Color32::from_rgb(70, 80, 98);
    visuals.selection.bg_fill = egui::Color32::from_rgb(196, 92, 30);
    visuals
}

#[allow(clippy::too_many_arguments)]
fn build(
    ctx: &egui::Context,
    classification: &mut Classification,
    ghost: &mut GhostParams,
    selection: Selection<'_>,
    labels: &Labels<'_>,
    stats: DrawStats,
    fps: f32,
    response: &mut Response,
) {
    egui::Window::new("Plant")
        .default_pos([16.0, 16.0])
        .default_width(320.0)
        .resizable(true)
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(format!("{fps:.0} fps"))
                        .monospace()
                        .color(egui::Color32::from_rgb(150, 160, 178)),
                );
                ui.separator();
                ui.label(
                    egui::RichText::new(format!(
                        "{} solid  {} ghost  {} tris",
                        stats.solid, stats.ghost, stats.triangles
                    ))
                    .monospace()
                    .color(egui::Color32::from_rgb(150, 160, 178)),
                );
            });

            ui.add_space(6.0);
            ui.label(egui::RichText::new("VIEWS").small().monospace());
            ui.horizontal_wrapped(|ui| {
                if ui
                    .button("Process plant")
                    .on_hover_text("Pipework, ducting, vessels and instruments solid; the building ghosted")
                    .clicked()
                {
                    classification.preset_plant();
                    response.dirty = true;
                }
                if ui
                    .button("Pipes only")
                    .on_hover_text("Pipework and instrumentation, nothing else drawn")
                    .clicked()
                {
                    classification.preset_pipes_only();
                    response.dirty = true;
                }
                if ui.button("Whole map").on_hover_text("Everything solid").clicked() {
                    classification.preset_everything();
                    response.dirty = true;
                }
            });
            ui.horizontal_wrapped(|ui| {
                if ui
                    .button("Ghost hidden")
                    .on_hover_text("Bring back everything hidden, as an x-ray shell")
                    .clicked()
                {
                    for category in &mut classification.categories {
                        if category.mode == Mode::Hidden {
                            category.mode = Mode::Ghost;
                        }
                    }
                    response.dirty = true;
                }
                if ui
                    .button("Hide ghosted")
                    .on_hover_text("Drop everything currently ghosted")
                    .clicked()
                {
                    for category in &mut classification.categories {
                        if category.mode == Mode::Ghost {
                            category.mode = Mode::Hidden;
                        }
                    }
                    response.dirty = true;
                }
                if ui.button("Frame visible").clicked() {
                    response.frame_visible = true;
                }
            });

            ui.add_space(10.0);
            ui.label(egui::RichText::new("CATEGORIES").small().monospace());
            ui.add_space(2.0);

            egui::Grid::new("categories")
                .num_columns(4)
                .spacing([8.0, 4.0])
                .striped(true)
                .show(ui, |ui| {
                    for (i, category) in classification.categories.iter_mut().enumerate() {
                        if category_row(ui, i, category) {
                            response.dirty = true;
                        }
                        ui.end_row();
                    }
                });

            ui.add_space(10.0);
            real_world_match(ui, selection, labels);

            ui.add_space(10.0);
            ui.label(egui::RichText::new("GHOST SHADING").small().monospace());
            let mut changed = false;
            changed |= ui
                .add(egui::Slider::new(&mut ghost.gain, 0.02..=2.0).text("brightness"))
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut ghost.fresnel, 0.5..=8.0).text("edge falloff"))
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut ghost.fade, 10.0..=400.0).text("fade metres"))
                .changed();
            // Ghost parameters live in the frame uniform, which is rewritten
            // every frame anyway, so nothing needs rebuilding for these.
            let _ = changed;

            ui.add_space(8.0);
            ui.separator();
            ui.label(
                egui::RichText::new(
                    "WASD fly · right mouse look · Shift sprint · F frame · H hide panel",
                )
                .small()
                .color(egui::Color32::from_rgb(120, 130, 148)),
            );
        });
}

/// What the thing you are pointing at is, and links to photographs of the real
/// unit so the claim can be checked rather than believed.
///
/// The links have to be real egui widgets rather than text painted over the
/// viewport, which is why they live here and the hover card only points at them.
/// `egui-winit` is built with its `links` feature, so a click reaches the system
/// browser through `handle_platform_output`.
///
/// Falls back to whatever is under the cursor when nothing has been clicked, so
/// the section fills in by pointing and stays put once you click.
fn real_world_match(ui: &mut egui::Ui, selection: Selection<'_>, labels: &Labels<'_>) {
    ui.label(egui::RichText::new("REAL-WORLD MATCH").small().monospace());

    let hovered = labels.hovered.as_ref().map(|n| n.entry);
    let Some(entry) = selection.entry.or(hovered).or(selection.sticky) else {
        ui.label(
            egui::RichText::new("point at something to name it")
                .small()
                .color(egui::Color32::from_rgb(130, 140, 158)),
        );
        return;
    };

    let [r, g, b] = entry.confidence.colour();
    let accent = egui::Color32::from_rgb(r, g, b);
    let pinned = selection.entry.is_some();

    ui.horizontal_wrapped(|ui| {
        ui.label(
            egui::RichText::new(entry.confidence.tag())
                .small()
                .monospace()
                .strong()
                .color(accent),
        );
        if let Some(instance) = selection.instance.filter(|_| pinned) {
            ui.label(
                egui::RichText::new(format!("#{instance} {}", selection.name))
                    .small()
                    .monospace()
                    .color(egui::Color32::from_rgb(130, 140, 158)),
            );
        }
    });

    ui.label(egui::RichText::new(entry.what).strong());
    ui.label(egui::RichText::new(format!("→ {}", entry.real)).color(accent));
    ui.label(
        egui::RichText::new(entry.note)
            .small()
            .color(egui::Color32::from_rgb(168, 178, 194)),
    );

    if entry.refs.is_empty() {
        ui.add_space(2.0);
        ui.label(
            egui::RichText::new("no photograph of a real counterpart — see the note")
                .small()
                .italics()
                .color(egui::Color32::from_rgb(130, 140, 158)),
        );
        return;
    }

    ui.add_space(4.0);
    ui.label(
        egui::RichText::new("PICTURES OF THE REAL UNIT")
            .small()
            .monospace()
            .color(egui::Color32::from_rgb(130, 140, 158)),
    );
    for reference in entry.refs {
        ui.hyperlink_to(
            egui::RichText::new(format!("↗ {}", reference.label)).small(),
            reference.url,
        )
        .on_hover_text(reference.url);
    }
}

/// One row: swatch, name, count, and the three mode buttons. Returns true if
/// anything changed.
fn category_row(ui: &mut egui::Ui, index: usize, category: &mut Category) -> bool {
    let mut changed = false;

    let mut rgb = category.colour;
    if ui
        .color_edit_button_rgb(&mut rgb)
        .on_hover_text("Colour for this category")
        .changed()
    {
        category.colour = rgb;
        changed = true;
    }

    let key = if index == 9 { 0 } else { index + 1 };
    let label = if index < 10 {
        format!("{key}  {}", category.label)
    } else {
        format!("   {}", category.label)
    };
    ui.label(label);

    ui.label(
        egui::RichText::new(format!("{}", category.instances))
            .monospace()
            .color(egui::Color32::from_rgb(130, 140, 158)),
    );

    ui.horizontal(|ui| {
        for mode in [Mode::Solid, Mode::Ghost, Mode::Hidden] {
            let tag = match mode {
                Mode::Solid => "S",
                Mode::Ghost => "G",
                Mode::Hidden => "—",
            };
            let selected = category.mode == mode;
            if ui
                .selectable_label(selected, tag)
                .on_hover_text(mode.tag())
                .clicked()
                && !selected
            {
                category.mode = mode;
                changed = true;
            }
        }
    });

    changed
}
