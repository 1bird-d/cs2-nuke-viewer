//! Turning placements plus models into one concatenated scene.
//!
//! Every distinct model is decoded once. Its vertex buffers are appended to a
//! single global vertex buffer and its draw calls to a single global index
//! buffer, so 1,482 pipe placements cost 1,482 instance records and one copy of
//! the pipe. That layout is also exactly what the renderer wants: one buffer
//! binding for the whole map, and a draw per instance that differs only in the
//! index range and the transform.

use std::collections::HashMap;

use anyhow::{Context, Result};
use nkp_format::{Header, Instance, Material, Vertex, MAGIC, SOURCE_UNIT_METRES, VERSION};
use source2::{Model, Resource};

use crate::world::{Placement, Transform};
use crate::Resolver;

/// One draw call of one model, already resident in the global buffers.
#[derive(Debug, Clone, Copy)]
struct BakedDrawCall {
    first_index: u32,
    index_count: u32,
    base_vertex: u32,
    material: u32,
    /// Local-space AABB in Source units, for deriving the world AABB per
    /// placement without re-walking the vertices.
    local_min: [f32; 3],
    local_max: [f32; 3],
}

/// A model's draw calls, flattened across its meshes in file order. This is the
/// list `m_nDrawCallIndex` addresses.
type BakedModel = Vec<BakedDrawCall>;

/// Everything the writer needs, assembled in memory.
pub struct BakedScene {
    /// The interleaved vertex buffer.
    pub vertices: Vec<Vertex>,
    /// The shared index buffer.
    pub indices: Vec<u32>,
    /// One record per placement.
    pub instances: Vec<Instance>,
    /// The material table.
    pub materials: Vec<Material>,
    /// The string blob instances and materials point into.
    pub strings: Vec<u8>,
    /// World AABB of everything, in metres.
    pub scene_min: [f32; 3],
    /// World AABB of everything, in metres.
    pub scene_max: [f32; 3],
    /// Identifies the source data, so annotations can detect a re-bake.
    pub bake_hash: u64,
    /// Models that were placed but could not be read, for reporting.
    pub failed_models: Vec<String>,
}

/// Interns strings so 8,000 instances sharing 350 model names cost 350 copies.
#[derive(Default)]
struct Strings {
    blob: Vec<u8>,
    seen: HashMap<String, (u32, u32)>,
}

impl Strings {
    fn intern(&mut self, s: &str) -> (u32, u32) {
        if let Some(&at) = self.seen.get(s) {
            return at;
        }
        #[allow(clippy::cast_possible_truncation)]
        let at = (self.blob.len() as u32, s.len() as u32);
        self.blob.extend_from_slice(s.as_bytes());
        self.seen.insert(s.to_string(), at);
        at
    }
}

/// Build the scene.
///
/// # Errors
///
/// Fails only on an unrecoverable problem. A model that will not decode is
/// recorded in [`BakedScene::failed_models`] and skipped, because one broken
/// prop should not cost the whole map.
pub fn bake(resolver: &Resolver, placements: &[Placement]) -> Result<BakedScene> {
    let mut scene = BakedScene {
        vertices: Vec::new(),
        indices: Vec::new(),
        instances: Vec::with_capacity(placements.len()),
        materials: Vec::new(),
        strings: Vec::new(),
        scene_min: [f32::INFINITY; 3],
        scene_max: [f32::NEG_INFINITY; 3],
        bake_hash: 0,
        failed_models: Vec::new(),
    };
    let mut strings = Strings::default();
    let mut material_ids: HashMap<String, u32> = HashMap::new();
    let mut models: HashMap<String, Option<BakedModel>> = HashMap::new();
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;

    for placement in placements {
        let baked = match models.get(&placement.model) {
            Some(cached) => cached,
            None => {
                let baked = load_model(
                    resolver,
                    &placement.model,
                    &mut scene,
                    &mut strings,
                    &mut material_ids,
                )
                .unwrap_or_else(|e| {
                    scene
                        .failed_models
                        .push(format!("{}: {e:#}", placement.model));
                    None
                });
                models.entry(placement.model.clone()).or_insert(baked)
            }
        };
        let Some(baked) = baked else { continue };

        // A fragment names one draw call; a scene object draws the model whole.
        let range: &[BakedDrawCall] = match placement.draw_call {
            Some(i) => match baked.get(i) {
                Some(call) => std::slice::from_ref(call),
                None => continue,
            },
            None => baked,
        };

        let stem = model_stem(&placement.model);
        let (name_offset, name_len) = strings.intern(stem);
        let world = to_metres_y_up(&placement.transform);

        for call in range {
            let (aabb_min, aabb_max) = world_aabb(&world, call.local_min, call.local_max);
            for axis in 0..3 {
                scene.scene_min[axis] = scene.scene_min[axis].min(aabb_min[axis]);
                scene.scene_max[axis] = scene.scene_max[axis].max(aabb_max[axis]);
            }
            scene.instances.push(Instance {
                transform: flatten(&world),
                aabb_min,
                aabb_max,
                first_index: call.first_index,
                index_count: call.index_count,
                base_vertex: call.base_vertex,
                material: call.material,
                name_offset,
                name_len,
                flags: placement.kind,
                _pad: 0,
            });
        }

        // Fold the placement into the bake hash so a different game build, or a
        // different set of placements, is detectable from the header alone.
        for byte in placement.model.as_bytes() {
            hash = (hash ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01b3);
        }
        hash = (hash ^ placement.transform.0[0][3].to_bits() as u64)
            .wrapping_mul(0x0000_0100_0000_01b3);
    }

    scene.strings = strings.blob;
    scene.bake_hash = hash;
    if scene.instances.is_empty() {
        scene.scene_min = [0.0; 3];
        scene.scene_max = [0.0; 3];
    }
    Ok(scene)
}

/// Decode a model and append its geometry to the global buffers.
fn load_model(
    resolver: &Resolver,
    path: &str,
    scene: &mut BakedScene,
    strings: &mut Strings,
    material_ids: &mut HashMap<String, u32>,
) -> Result<Option<BakedModel>> {
    let bytes = resolver.read(path).with_context(|| format!("reading {path}"))?;
    let resource = Resource::parse(&bytes).with_context(|| format!("parsing {path}"))?;
    let model = Model::from_resource(&resource).with_context(|| format!("decoding {path}"))?;

    let mut calls: BakedModel = Vec::new();
    for (mesh_index, mesh) in model.meshes.iter().enumerate() {
        if !model.mesh_in_lod0(mesh_index) {
            // Lower levels of detail would draw on top of the full-detail mesh.
            continue;
        }

        // Each of the mesh's vertex buffers lands once in the global buffer;
        // remember where so the draw calls can rebase onto it.
        let mut buffer_base = Vec::with_capacity(mesh.vertex_buffers.len());
        for buffer in &mesh.vertex_buffers {
            #[allow(clippy::cast_possible_truncation)]
            buffer_base.push(scene.vertices.len() as u32);
            for i in 0..buffer.positions.len() {
                scene.vertices.push(Vertex {
                    position: buffer.positions[i],
                    normal: buffer.normals.get(i).copied().unwrap_or([0.0, 0.0, 1.0]),
                    uv: buffer.uvs.get(i).copied().unwrap_or([0.0, 0.0]),
                });
            }
        }

        for primitive in &mesh.primitives {
            let Some(&base_vertex) = buffer_base.get(primitive.vertex_buffer) else {
                continue;
            };
            let buffer = &mesh.vertex_buffers[primitive.vertex_buffer];

            #[allow(clippy::cast_possible_truncation)]
            let first_index = scene.indices.len() as u32;
            scene.indices.extend_from_slice(&primitive.indices);

            let material = *material_ids
                .entry(primitive.material.clone())
                .or_insert_with(|| {
                    let (name_offset, name_len) = strings.intern(&primitive.material);
                    #[allow(clippy::cast_possible_truncation)]
                    let id = scene.materials.len() as u32;
                    scene.materials.push(Material {
                        name_offset,
                        name_len,
                        colour: palette_colour(&primitive.material),
                        _pad: [0; 2],
                    });
                    id
                });

            let (local_min, local_max) = local_bounds(&buffer.positions, &primitive.indices);
            #[allow(clippy::cast_possible_truncation)]
            calls.push(BakedDrawCall {
                first_index,
                index_count: primitive.indices.len() as u32,
                base_vertex,
                material,
                local_min,
                local_max,
            });
        }
    }

    Ok((!calls.is_empty()).then_some(calls))
}

/// The AABB of just the vertices a draw call touches.
///
/// Merged world geometry puts several draw calls in one vertex buffer, so the
/// buffer's own bounds would be the whole aggregate rather than this piece.
fn local_bounds(positions: &[[f32; 3]], indices: &[u32]) -> ([f32; 3], [f32; 3]) {
    let mut min = [f32::INFINITY; 3];
    let mut max = [f32::NEG_INFINITY; 3];
    for &index in indices {
        let Some(p) = positions.get(index as usize) else {
            continue;
        };
        for axis in 0..3 {
            min[axis] = min[axis].min(p[axis]);
            max[axis] = max[axis].max(p[axis]);
        }
    }
    if min[0] > max[0] {
        return ([0.0; 3], [0.0; 3]);
    }
    (min, max)
}

/// Compose a Source-space placement with the conversion to Y-up metres.
///
/// Source is Z-up with X forward and Y left; the viewer is Y-up right-handed.
/// The conversion is `(x, y, z) -> (x, z, -y)` scaled by one inch, the same
/// convention mapview's exporter puts on its scene root — so a position printed
/// by either project means the same place.
fn to_metres_y_up(t: &Transform) -> [[f32; 4]; 3] {
    let m = &t.0;
    let s = SOURCE_UNIT_METRES;
    [
        [s * m[0][0], s * m[0][1], s * m[0][2], s * m[0][3]],
        [s * m[2][0], s * m[2][1], s * m[2][2], s * m[2][3]],
        [-s * m[1][0], -s * m[1][1], -s * m[1][2], -s * m[1][3]],
    ]
}

fn flatten(m: &[[f32; 4]; 3]) -> [f32; 12] {
    let mut out = [0.0; 12];
    for r in 0..3 {
        for c in 0..4 {
            out[r * 4 + c] = m[r][c];
        }
    }
    out
}

/// Transform a local AABB by walking its eight corners.
fn world_aabb(m: &[[f32; 4]; 3], min: [f32; 3], max: [f32; 3]) -> ([f32; 3], [f32; 3]) {
    let mut out_min = [f32::INFINITY; 3];
    let mut out_max = [f32::NEG_INFINITY; 3];
    for corner in 0..8u8 {
        let p = [
            if corner & 1 == 0 { min[0] } else { max[0] },
            if corner & 2 == 0 { min[1] } else { max[1] },
            if corner & 4 == 0 { min[2] } else { max[2] },
        ];
        for axis in 0..3 {
            let v = m[axis][0] * p[0] + m[axis][1] * p[1] + m[axis][2] * p[2] + m[axis][3];
            out_min[axis] = out_min[axis].min(v);
            out_max[axis] = out_max[axis].max(v);
        }
    }
    (out_min, out_max)
}

/// The part of a model path that names the asset, e.g.
/// `n0_lr0_agg_prop_metal_pipe_001_0`. This is what classification rules match.
#[must_use]
pub fn model_stem(path: &str) -> &str {
    let file = path.rsplit(['/', '\\']).next().unwrap_or(path);
    file.split_once(".vmdl").map_or(file, |(stem, _)| stem)
}

/// A stable muted colour per material, so the map reads as an architectural
/// render before any textures or classification exist.
///
/// Hue comes from a hash of the path; saturation and value are held low and
/// narrow so nothing shouts, and neighbouring surfaces stay distinguishable.
fn palette_colour(material: &str) -> [f32; 4] {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in material.as_bytes() {
        hash = (hash ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01b3);
    }
    #[allow(clippy::cast_precision_loss)]
    let hue = (hash % 3600) as f32 / 10.0;
    #[allow(clippy::cast_precision_loss)]
    let sat = 0.18 + ((hash >> 12) % 100) as f32 / 500.0;
    #[allow(clippy::cast_precision_loss)]
    let val = 0.42 + ((hash >> 24) % 100) as f32 / 400.0;
    let [r, g, b] = hsv_to_rgb(hue, sat, val);
    [r, g, b, 1.0]
}

fn hsv_to_rgb(h: f32, s: f32, v: f32) -> [f32; 3] {
    let c = v * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let m = v - c;
    let (r, g, b) = match h as u32 / 60 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    [r + m, g + m, b + m]
}

/// Serialise a scene to the `.nkp` byte layout.
#[must_use]
pub fn to_bytes(scene: &BakedScene) -> Vec<u8> {
    let header_size = nkp_format::align_up(std::mem::size_of::<Header>());
    let instances = bytemuck::cast_slice::<_, u8>(&scene.instances);
    let materials = bytemuck::cast_slice::<_, u8>(&scene.materials);
    let vertices = bytemuck::cast_slice::<_, u8>(&scene.vertices);
    let indices = bytemuck::cast_slice::<_, u8>(&scene.indices);

    let instances_offset = header_size;
    let materials_offset = nkp_format::align_up(instances_offset + instances.len());
    let strings_offset = nkp_format::align_up(materials_offset + materials.len());
    let vertices_offset = nkp_format::align_up(strings_offset + scene.strings.len());
    let indices_offset = nkp_format::align_up(vertices_offset + vertices.len());
    let total = nkp_format::align_up(indices_offset + indices.len());

    #[allow(clippy::cast_possible_truncation)]
    let header = Header {
        magic: MAGIC,
        version: VERSION,
        unit_metres: SOURCE_UNIT_METRES,
        bake_hash: scene.bake_hash,
        scene_min: scene.scene_min,
        scene_max: scene.scene_max,
        instances_offset: instances_offset as u64,
        instance_count: scene.instances.len() as u32,
        material_count: scene.materials.len() as u32,
        materials_offset: materials_offset as u64,
        strings_offset: strings_offset as u64,
        strings_len: scene.strings.len() as u64,
        vertices_offset: vertices_offset as u64,
        vertex_count: scene.vertices.len() as u64,
        indices_offset: indices_offset as u64,
        index_count: scene.indices.len() as u64,
    };

    let mut out = vec![0u8; total];
    out[..std::mem::size_of::<Header>()].copy_from_slice(bytemuck::bytes_of(&header));
    out[instances_offset..instances_offset + instances.len()].copy_from_slice(instances);
    out[materials_offset..materials_offset + materials.len()].copy_from_slice(materials);
    out[strings_offset..strings_offset + scene.strings.len()].copy_from_slice(&scene.strings);
    out[vertices_offset..vertices_offset + vertices.len()].copy_from_slice(vertices);
    out[indices_offset..indices_offset + indices.len()].copy_from_slice(indices);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_stems_drop_the_directory_and_suffix() {
        assert_eq!(
            model_stem("maps/de_nuke/worldnodes/n0_lr0_agg_prop_metal_pipe_001_0.vmdl"),
            "n0_lr0_agg_prop_metal_pipe_001_0"
        );
        assert_eq!(model_stem("a/b/c.vmdl_c"), "c");
    }

    /// A point 100 units up in Source (+Z) must come out 2.54 m up in +Y.
    #[test]
    fn the_conversion_puts_source_z_on_y() {
        let m = to_metres_y_up(&Transform::IDENTITY);
        let p = [0.0, 0.0, 100.0];
        let out = [
            m[0][0] * p[0] + m[0][1] * p[1] + m[0][2] * p[2] + m[0][3],
            m[1][0] * p[0] + m[1][1] * p[1] + m[1][2] * p[2] + m[1][3],
            m[2][0] * p[0] + m[2][1] * p[1] + m[2][2] * p[2] + m[2][3],
        ];
        assert!((out[0]).abs() < 1e-6, "{out:?}");
        assert!((out[1] - 2.54).abs() < 1e-6, "{out:?}");
        assert!((out[2]).abs() < 1e-6, "{out:?}");
    }

    /// The translation of a real pipe fragment, carried through the conversion.
    #[test]
    fn a_fragment_translation_survives_the_conversion() {
        let t = Transform([
            [-1.0, 0.0, 0.0, 858.0],
            [0.0, -1.0, 0.0, -896.0],
            [0.0, 0.0, 1.0, -932.0],
        ]);
        let m = to_metres_y_up(&t);
        // Source (858, -896, -932) -> Y-up metres (858, -932, 896) * 0.0254.
        assert!((m[0][3] - 858.0 * 0.0254).abs() < 1e-4, "{m:?}");
        assert!((m[1][3] + 932.0 * 0.0254).abs() < 1e-4, "{m:?}");
        assert!((m[2][3] - 896.0 * 0.0254).abs() < 1e-4, "{m:?}");
    }

    #[test]
    fn world_aabb_covers_a_rotated_box() {
        // 90 degrees about Y, so local X maps to world -Z.
        let m = [
            [0.0, 0.0, 1.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [-1.0, 0.0, 0.0, 0.0],
        ];
        let (min, max) = world_aabb(&m, [0.0, 0.0, 0.0], [2.0, 1.0, 1.0]);
        assert!((min[2] + 2.0).abs() < 1e-6, "{min:?} {max:?}");
        assert!((max[0] - 1.0).abs() < 1e-6, "{min:?} {max:?}");
    }
}
