//! The renderer.
//!
//! The whole map is one vertex buffer and one index buffer. An instance is a
//! slice of the index buffer plus a transform, and it is drawn by setting
//! `first_instance` to the instance's id — the shader reads its record out of a
//! storage buffer with `@builtin(instance_index)`. Nothing is uploaded per
//! instance per frame and no vertex data is duplicated between placements.
//!
//! Two passes. The solid pass fills colour and depth. The ghost pass then draws
//! everything that is *not* the subject as a fresnel shell, depth-tested against
//! what the solid pass already wrote, so the plant reads in front of the
//! building rather than through it. See `shaders/ghost.wgsl`.
//!
//! Changing what is visible costs a rebuild of two `Vec<Draw>` and one 550 KB
//! buffer upload — sub-millisecond for 8,625 instances, so a category toggle is
//! instant and needs no reload.

#[cfg(not(target_arch = "wasm32"))]
use std::path::Path;

use anyhow::{anyhow, Context, Result};
use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec3};
use nkp_format::Scene;

use crate::category::{Classification, Mode};

/// Reverse-Z: the near plane is 1.0, so the buffer clears to 0.0 and the test
/// keeps the *greater* value.
const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
const DEPTH_CLEAR: f32 = 0.0;

/// What offscreen frames render into. sRGB so a capture matches the window.
const OFFSCREEN_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

/// `copy_texture_to_buffer` needs each row aligned to 256 bytes.
#[cfg(not(target_arch = "wasm32"))]
const COPY_ALIGN: u32 = 256;

/// Ghost shading knobs, live-adjustable.
#[derive(Debug, Clone, Copy)]
pub struct GhostParams {
    /// Overall brightness of the shell.
    pub gain: f32,
    /// Fresnel exponent. Higher is a thinner, sharper rim.
    pub fresnel: f32,
    /// Metres at which the shell has fallen to 1/e.
    pub fade: f32,
}

impl Default for GhostParams {
    fn default() -> Self {
        // Tuned against de_nuke's reactor building, which is the densest
        // interior in the map and the case that has to stay readable. The fade
        // is 200 m rather than the 90 m that reads well from inside, because
        // the map is 320 m across and a shorter fade leaves the establishing
        // shot with nothing around the plant at all.
        Self {
            gain: 0.6,
            fresnel: 2.6,
            fade: 200.0,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct FrameUniform {
    view_proj: [[f32; 4]; 4],
    camera_pos: [f32; 4],
    light_dir: [f32; 4],
    ghost: [f32; 4],
}

/// The per-instance record the shader reads: a 3x4 row-major matrix and a
/// colour. 64 bytes.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct InstanceGpu {
    rows: [[f32; 4]; 3],
    colour: [f32; 4],
}

/// One drawable slice of the shared index buffer.
#[derive(Clone, Copy)]
struct Draw {
    first_index: u32,
    index_count: u32,
    base_vertex: i32,
    instance: u32,
}

/// Where a frame ends up.
enum Target {
    Window {
        surface: wgpu::Surface<'static>,
        config: wgpu::SurfaceConfiguration,
    },
    Offscreen {
        // Only the readback path reads this back out, and that is desktop-only.
        #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
        texture: wgpu::Texture,
        view: wgpu::TextureView,
        width: u32,
        height: u32,
    },
}

impl Target {
    fn size(&self) -> (u32, u32) {
        match self {
            Self::Window { config, .. } => (config.width, config.height),
            Self::Offscreen { width, height, .. } => (*width, *height),
        }
    }

    fn format(&self) -> wgpu::TextureFormat {
        match self {
            Self::Window { config, .. } => config.format,
            Self::Offscreen { .. } => OFFSCREEN_FORMAT,
        }
    }
}

/// What is on screen right now, for the panel and the title bar.
#[derive(Debug, Clone, Copy, Default)]
pub struct DrawStats {
    /// Instances in the solid pass.
    pub solid: usize,
    /// Instances in the ghost pass.
    pub ghost: usize,
    /// Triangles across both passes.
    pub triangles: u64,
}

pub struct Renderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    target: Target,

    solid_pipeline: wgpu::RenderPipeline,
    ghost_pipeline: wgpu::RenderPipeline,
    frame_buffer: wgpu::Buffer,
    frame_bind_group: wgpu::BindGroup,
    instance_buffer: wgpu::Buffer,
    instance_bind_group: wgpu::BindGroup,
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    depth: wgpu::TextureView,

    /// Every instance's transform and colour, kept CPU-side so a colour change
    /// is a memcpy and one upload rather than a rebuild.
    records: Vec<InstanceGpu>,
    solid_draws: Vec<Draw>,
    ghost_draws: Vec<Draw>,
    stats: DrawStats,

    /// Set by `F12`; the next presented frame is written here. There is nowhere
    /// to write a file in a browser, so the whole readback path is desktop-only.
    #[cfg(not(target_arch = "wasm32"))]
    pending_capture: Option<std::path::PathBuf>,
}

impl Renderer {
    /// Bring up a device that draws into `window`.
    ///
    /// # Errors
    ///
    /// Fails if no adapter supports the surface.
    pub async fn windowed(
        window: std::sync::Arc<winit::window::Window>,
        scene: &Scene,
    ) -> Result<Self> {
        let size = window.inner_size();
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let surface = instance
            .create_surface(window)
            .context("creating the window surface")?;
        let adapter = request_adapter(&instance, Some(&surface)).await?;
        let (device, queue) = request_device(&adapter).await?;

        let mut config = surface
            .get_default_config(&adapter, size.width.max(1), size.height.max(1))
            .context("the adapter cannot configure this surface")?;
        // An sRGB target means the shader writes linear colour and the hardware
        // encodes it, which is what the tonemap assumes.
        let caps = surface.get_capabilities(&adapter);
        if let Some(srgb) = caps.formats.iter().copied().find(wgpu::TextureFormat::is_srgb) {
            config.format = srgb;
        }
        config.present_mode = wgpu::PresentMode::AutoNoVsync;
        // COPY_SRC lets F12 grab exactly what is on screen, panel included.
        // Not every platform allows it on a swapchain image, so ask first.
        if caps.usages.contains(wgpu::TextureUsages::COPY_SRC) {
            config.usage |= wgpu::TextureUsages::COPY_SRC;
        }
        surface.configure(&device, &config);

        Self::build(device, queue, Target::Window { surface, config }, scene)
    }

    /// Bring up a device with no window at all, for frame capture.
    ///
    /// # Errors
    ///
    /// Fails if no adapter is available.
    pub async fn offscreen(scene: &Scene, width: u32, height: u32) -> Result<Self> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter = request_adapter(&instance, None).await?;
        let (device, queue) = request_device(&adapter).await?;
        let target = create_offscreen(&device, width.max(1), height.max(1));
        Self::build(device, queue, target, scene)
    }

    fn build(
        device: wgpu::Device,
        queue: wgpu::Queue,
        target: Target,
        scene: &Scene,
    ) -> Result<Self> {
        let records = build_records(scene);

        let vertices = create_buffer(
            &device,
            &queue,
            "vertices",
            scene.vertex_bytes(),
            wgpu::BufferUsages::VERTEX,
        );
        let indices = create_buffer(
            &device,
            &queue,
            "indices",
            scene.index_bytes(),
            wgpu::BufferUsages::INDEX,
        );
        let instance_buffer = create_buffer(
            &device,
            &queue,
            "instances",
            bytemuck::cast_slice(&records),
            wgpu::BufferUsages::STORAGE,
        );

        let frame_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("frame"),
            size: std::mem::size_of::<FrameUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let frame_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("frame layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let instance_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("instance layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        let frame_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("frame"),
            layout: &frame_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: frame_buffer.as_entire_binding(),
            }],
        });
        let instance_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("instances"),
            layout: &instance_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: instance_buffer.as_entire_binding(),
            }],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("scene"),
            bind_group_layouts: &[Some(&frame_layout), Some(&instance_layout)],
            immediate_size: 0,
        });

        let format = target.format();
        let solid_pipeline = make_pipeline(
            &device,
            &pipeline_layout,
            format,
            "solid",
            include_str!("shaders/scene.wgsl"),
            PassKind::Solid,
        );
        let ghost_pipeline = make_pipeline(
            &device,
            &pipeline_layout,
            format,
            "ghost",
            include_str!("shaders/ghost.wgsl"),
            PassKind::Ghost,
        );

        let (width, height) = target.size();
        let depth = create_depth(&device, width, height);

        Ok(Self {
            device,
            queue,
            target,
            solid_pipeline,
            ghost_pipeline,
            frame_buffer,
            frame_bind_group,
            instance_buffer,
            instance_bind_group,
            vertices,
            indices,
            depth,
            records,
            solid_draws: Vec::new(),
            ghost_draws: Vec::new(),
            stats: DrawStats::default(),
            #[cfg(not(target_arch = "wasm32"))]
            pending_capture: None,
        })
    }

    /// Write the next presented frame — panel and all — to `path`.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn request_capture(&mut self, path: std::path::PathBuf) {
        self.pending_capture = Some(path);
    }

    /// The size the target is actually configured at, in physical pixels.
    ///
    /// The authority on whether a resize is needed. A canvas reports its CSS
    /// size before layout has run, so the surface can end up configured 1x1
    /// while the element is full-window; comparing against a cached size would
    /// never notice, and every frame would fail validation because the depth
    /// attachment did not match the colour attachment.
    #[must_use]
    pub fn size(&self) -> (u32, u32) {
        self.target.size()
    }

    /// The colour format frames are drawn into. The panel needs it to build a
    /// matching pipeline.
    #[must_use]
    pub fn format(&self) -> wgpu::TextureFormat {
        self.target.format()
    }

    /// The device, for anything that needs to build its own GPU resources.
    #[must_use]
    pub fn device(&self) -> &wgpu::Device {
        &self.device
    }

    /// Rebuild the draw lists and instance colours from the current
    /// classification. Call after any change to a category's mode or colour.
    ///
    /// `override_colour` lets a caller paint specific instances a different
    /// colour than their category — that is how leg groups will work.
    pub fn apply(
        &mut self,
        scene: &Scene,
        classification: &Classification,
        override_colour: &dyn Fn(usize) -> Option<[f32; 3]>,
    ) {
        self.solid_draws.clear();
        self.ghost_draws.clear();
        let mut triangles = 0u64;

        for (id, instance) in scene.instances().iter().enumerate() {
            if instance.index_count == 0 {
                continue;
            }
            let category = classification.category_of(id);
            if category.mode == Mode::Hidden {
                continue;
            }

            let colour = override_colour(id).unwrap_or(category.colour);
            self.records[id].colour = [colour[0], colour[1], colour[2], 1.0];

            #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
            let draw = Draw {
                first_index: instance.first_index,
                index_count: instance.index_count,
                base_vertex: instance.base_vertex as i32,
                instance: id as u32,
            };
            triangles += u64::from(instance.index_count) / 3;
            match category.mode {
                Mode::Solid => self.solid_draws.push(draw),
                Mode::Ghost => self.ghost_draws.push(draw),
                Mode::Hidden => unreachable!("filtered above"),
            }
        }

        self.queue
            .write_buffer(&self.instance_buffer, 0, bytemuck::cast_slice(&self.records));
        self.stats = DrawStats {
            solid: self.solid_draws.len(),
            ghost: self.ghost_draws.len(),
            triangles,
        };
    }

    /// What is currently on screen.
    #[must_use]
    pub fn stats(&self) -> DrawStats {
        self.stats
    }

    /// The target aspect ratio.
    #[must_use]
    pub fn aspect(&self) -> f32 {
        let (w, h) = self.target.size();
        #[allow(clippy::cast_precision_loss)]
        {
            w as f32 / h.max(1) as f32
        }
    }

    /// Resize the target and its depth buffer.
    pub fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        match &mut self.target {
            Target::Window { surface, config } => {
                config.width = width;
                config.height = height;
                surface.configure(&self.device, config);
            }
            Target::Offscreen { .. } => {
                self.target = create_offscreen(&self.device, width, height);
            }
        }
        self.depth = create_depth(&self.device, width, height);
    }

    /// Draw one frame and present it.
    ///
    /// `overlay` is handed the encoder and the target view after the scene has
    /// been drawn, so a UI layer can add itself without this module knowing
    /// anything about it.
    ///
    /// # Errors
    ///
    /// Fails only if the surface rejects the frame.
    pub fn render(
        &mut self,
        view_proj: Mat4,
        camera_pos: Vec3,
        ghost: GhostParams,
        overlay: &mut dyn FnMut(&wgpu::Device, &wgpu::Queue, &mut wgpu::CommandEncoder, &wgpu::TextureView, (u32, u32)),
    ) -> Result<()> {
        let size = self.target.size();
        match &self.target {
            Target::Window { surface, config } => {
                use wgpu::CurrentSurfaceTexture as Current;
                let frame = match surface.get_current_texture() {
                    Current::Success(frame) | Current::Suboptimal(frame) => frame,
                    Current::Outdated | Current::Lost => {
                        surface.configure(&self.device, config);
                        return Ok(());
                    }
                    // Occluded means the window is hidden: nothing to draw, and
                    // nothing wrong. Timeout is the compositor being slow.
                    Current::Timeout | Current::Occluded => return Ok(()),
                    Current::Validation => return Err(anyhow!("surface rejected the frame")),
                };
                let view = frame
                    .texture
                    .create_view(&wgpu::TextureViewDescriptor::default());
                let mut encoder = self.encode(&view, view_proj, camera_pos, ghost);
                overlay(&self.device, &self.queue, &mut encoder, &view, size);

                // Grab before present, so the capture is the frame the user is
                // about to see rather than a re-render that might differ.
                #[cfg(not(target_arch = "wasm32"))]
                let grab = self.pending_capture.take().map(|path| {
                    let readback = begin_readback(
                        &self.device,
                        &mut encoder,
                        &frame.texture,
                        size.0,
                        size.1,
                    );
                    (path, readback)
                });

                self.queue.submit(Some(encoder.finish()));
                self.queue.present(frame);

                #[cfg(not(target_arch = "wasm32"))]
                if let Some((path, readback)) = grab {
                    let bgra = needs_swizzle(config.format);
                    match finish_readback(&self.device, &readback, size.0, size.1, bgra, &path) {
                        Ok(()) => println!("wrote {}", path.display()),
                        Err(e) => eprintln!("capture failed: {e:#}"),
                    }
                }
            }
            Target::Offscreen { view, .. } => {
                let view = view.clone();
                let mut encoder = self.encode(&view, view_proj, camera_pos, ghost);
                overlay(&self.device, &self.queue, &mut encoder, &view, size);
                self.queue.submit(Some(encoder.finish()));
            }
        }
        Ok(())
    }

    fn encode(
        &self,
        view: &wgpu::TextureView,
        view_proj: Mat4,
        camera_pos: Vec3,
        ghost: GhostParams,
    ) -> wgpu::CommandEncoder {
        // Late afternoon sun from the south-west, steep enough to separate roofs
        // from walls without flattening the vertical pipe runs.
        let light = Vec3::new(0.35, -0.82, 0.45).normalize();
        self.queue.write_buffer(
            &self.frame_buffer,
            0,
            bytemuck::bytes_of(&FrameUniform {
                view_proj: view_proj.to_cols_array_2d(),
                camera_pos: [camera_pos.x, camera_pos.y, camera_pos.z, 0.0],
                light_dir: [light.x, light.y, light.z, 0.0],
                ghost: [ghost.gain, ghost.fresnel, ghost.fade, 0.0],
            }),
        );

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("frame") });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("scene"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.021,
                            g: 0.024,
                            b: 0.031,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(DEPTH_CLEAR),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_bind_group(0, &self.frame_bind_group, &[]);
            pass.set_bind_group(1, &self.instance_bind_group, &[]);
            pass.set_vertex_buffer(0, self.vertices.slice(..));
            pass.set_index_buffer(self.indices.slice(..), wgpu::IndexFormat::Uint32);

            // Solid first so the ghost pass has real depth to test against.
            pass.set_pipeline(&self.solid_pipeline);
            for draw in &self.solid_draws {
                issue(&mut pass, draw);
            }
            pass.set_pipeline(&self.ghost_pipeline);
            for draw in &self.ghost_draws {
                issue(&mut pass, draw);
            }
        }
        encoder
    }

    #[cfg(not(target_arch = "wasm32"))]
    /// Render one frame offscreen and write it to `path` as a PNG.
    ///
    /// # Errors
    ///
    /// Fails if the renderer is windowed, or if the file cannot be written.
    pub fn capture(
        &mut self,
        view_proj: Mat4,
        camera_pos: Vec3,
        ghost: GhostParams,
        path: &Path,
    ) -> Result<()> {
        let Target::Offscreen {
            texture,
            width,
            height,
            ..
        } = &self.target
        else {
            return Err(anyhow!("capture needs an offscreen renderer"));
        };
        let (width, height) = (*width, *height);
        let texture = texture.clone();

        self.render(view_proj, camera_pos, ghost, &mut |_, _, _, _, _| {})?;

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("readback") });
        let readback = begin_readback(&self.device, &mut encoder, &texture, width, height);
        self.queue.submit(Some(encoder.finish()));
        // The offscreen target is always Rgba8UnormSrgb, so no swizzle.
        finish_readback(&self.device, &readback, width, height, false, path)
    }
}

#[cfg(not(target_arch = "wasm32"))]
/// Queue a copy of `texture` into a mappable buffer.
fn begin_readback(
    device: &wgpu::Device,
    encoder: &mut wgpu::CommandEncoder,
    texture: &wgpu::Texture,
    width: u32,
    height: u32,
) -> wgpu::Buffer {
    let padded = (width * 4).div_ceil(COPY_ALIGN) * COPY_ALIGN;
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("readback"),
        size: u64::from(padded) * u64::from(height),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded),
                rows_per_image: Some(height),
            },
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
    readback
}

#[cfg(not(target_arch = "wasm32"))]
/// Wait for the copy, strip the row padding, and write a PNG.
fn finish_readback(
    device: &wgpu::Device,
    readback: &wgpu::Buffer,
    width: u32,
    height: u32,
    bgra: bool,
    path: &Path,
) -> Result<()> {
    let row_bytes = width * 4;
    let padded = row_bytes.div_ceil(COPY_ALIGN) * COPY_ALIGN;

    let slice = readback.slice(..);
    slice.map_async(wgpu::MapMode::Read, |_| {});
    device.poll(wgpu::PollType::Wait {
        submission_index: None,
        timeout: None,
    })?;

    let mapped = slice
        .get_mapped_range()
        .map_err(|e| anyhow!("mapping the readback buffer: {e}"))?;
    let mut pixels = Vec::with_capacity((row_bytes * height) as usize);
    for row in 0..height {
        let start = (row * padded) as usize;
        pixels.extend_from_slice(&mapped[start..start + row_bytes as usize]);
    }
    drop(mapped);
    readback.unmap();

    // A BGRA swapchain has to be swizzled; PNG is RGBA either way.
    if bgra {
        for pixel in pixels.chunks_exact_mut(4) {
            pixel.swap(0, 2);
        }
    }

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let file = std::fs::File::create(path).with_context(|| format!("creating {}", path.display()))?;
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder
        .write_header()?
        .write_image_data(&pixels)
        .with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

/// Whether a format stores blue in the first byte, so a readback needs a swap.
#[cfg(not(target_arch = "wasm32"))]
fn needs_swizzle(format: wgpu::TextureFormat) -> bool {
    matches!(
        format,
        wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb
    )
}

fn issue(pass: &mut wgpu::RenderPass<'_>, draw: &Draw) {
    pass.draw_indexed(
        draw.first_index..draw.first_index + draw.index_count,
        draw.base_vertex,
        // Non-zero `first_instance` is how the shader learns which instance it
        // is; nothing per-instance touches the vertex buffer.
        draw.instance..draw.instance + 1,
    );
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PassKind {
    Solid,
    Ghost,
}

fn make_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    format: wgpu::TextureFormat,
    label: &str,
    source: &str,
    kind: PassKind,
) -> wgpu::RenderPipeline {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(label),
        source: wgpu::ShaderSource::Wgsl(source.into()),
    });

    let ghost = kind == PassKind::Ghost;
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            buffers: &[Some(wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<nkp_format::Vertex>() as u64,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &wgpu::vertex_attr_array![
                    0 => Float32x3, 1 => Float32x3, 2 => Float32x2
                ],
            })],
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                // Additive is commutative, so the ghost pass never needs
                // sorting and cannot flicker as the camera moves.
                blend: ghost.then_some(wgpu::BlendState {
                    color: wgpu::BlendComponent {
                        src_factor: wgpu::BlendFactor::One,
                        dst_factor: wgpu::BlendFactor::One,
                        operation: wgpu::BlendOperation::Add,
                    },
                    alpha: wgpu::BlendComponent {
                        src_factor: wgpu::BlendFactor::Zero,
                        dst_factor: wgpu::BlendFactor::One,
                        operation: wgpu::BlendOperation::Add,
                    },
                }),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            // Source geometry is inconsistently wound and the interiors are full
            // of one-sided walls you fly through. Culling costs more in missing
            // surfaces than it saves at 3.6 M triangles.
            cull_mode: None,
            ..wgpu::PrimitiveState::default()
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            // The ghost pass must not occlude itself, or the near face of every
            // room would hide the far one and the x-ray would only be one layer
            // deep.
            depth_write_enabled: Some(!ghost),
            depth_compare: Some(wgpu::CompareFunction::Greater),
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        }),
        multisample: wgpu::MultisampleState::default(),
        multiview_mask: None,
        cache: None,
    })
}

fn build_records(scene: &Scene) -> Vec<InstanceGpu> {
    scene
        .instances()
        .iter()
        .map(|instance| {
            let t = instance.transform;
            InstanceGpu {
                rows: [
                    [t[0], t[1], t[2], t[3]],
                    [t[4], t[5], t[6], t[7]],
                    [t[8], t[9], t[10], t[11]],
                ],
                colour: [1.0, 0.0, 1.0, 1.0],
            }
        })
        .collect()
}

async fn request_adapter(
    instance: &wgpu::Instance,
    surface: Option<&wgpu::Surface<'static>>,
) -> Result<wgpu::Adapter> {
    let adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: surface,
            ..wgpu::RequestAdapterOptions::default()
        })
        .await
        .context("no GPU adapter is available")?;
    let info = adapter.get_info();
    println!("adapter: {} ({:?})", info.name, info.backend);
    Ok(adapter)
}

async fn request_device(adapter: &wgpu::Adapter) -> Result<(wgpu::Device, wgpu::Queue)> {
    // The full de_nuke bake wants a 99 MB vertex buffer, so ask for headroom
    // above the 256 MB default — but ask for more than the adapter actually
    // has and device creation is *rejected*, which on the web means a visitor
    // gets an error instead of a viewer. A required limit is a hard floor, not
    // a wish. Take whichever is smaller, so the request can never be refused.
    //
    // The published web subset needs 60 MB of vertices and 19 MB of indices
    // and fits inside the default on any conformant implementation, so this
    // only ever matters to the desktop build reading the whole map.
    let max_buffer_size = adapter.limits().max_buffer_size.min(512 << 20);
    adapter
        .request_device(&wgpu::DeviceDescriptor {
            label: Some("nukeplant"),
            required_limits: wgpu::Limits {
                max_buffer_size,
                ..wgpu::Limits::default()
            },
            memory_hints: wgpu::MemoryHints::Performance,
            ..wgpu::DeviceDescriptor::default()
        })
        .await
        .context("requesting a device")
        .map_err(Into::into)
}

fn create_offscreen(device: &wgpu::Device, width: u32, height: u32) -> Target {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("offscreen"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: OFFSCREEN_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    Target::Offscreen {
        texture,
        view,
        width,
        height,
    }
}

fn create_buffer(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    label: &str,
    data: &[u8],
    usage: wgpu::BufferUsages,
) -> wgpu::Buffer {
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: data.len().max(4) as u64,
        usage: usage | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    queue.write_buffer(&buffer, 0, data);
    buffer
}

fn create_depth(device: &wgpu::Device, width: u32, height: u32) -> wgpu::TextureView {
    device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some("depth"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: DEPTH_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        })
        .create_view(&wgpu::TextureViewDescriptor::default())
}
