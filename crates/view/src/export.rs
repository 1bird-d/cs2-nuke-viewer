//! Cutting a small scene out of a big one.
//!
//! The full bake is 131 MB because it holds all of de_nuke — every wall, every
//! crate, every light fitting. The web viewer only ever draws the plant, and
//! GitHub refuses a file over 100 MB anyway, so what gets published is a subset
//! containing the plant categories and nothing else.
//!
//! The saving is not just the instances that go away. Because geometry is
//! shared — 1,927 pipe placements reference one copy of the pipe — dropping a
//! category usually drops its vertices entirely, and what remains is repacked
//! so no unreferenced vertex is carried along.
//!
//! No textures are involved at any stage. The `.nkp` format has never held
//! any: every surface draws in a flat per-category colour.

use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result};
use nkp_format::{Header, Instance, Material, Scene, Vertex, MAGIC, SOURCE_UNIT_METRES, VERSION};

use crate::category::{Classification, Mode};

/// What was dropped and what was kept, for reporting.
pub struct Summary {
    pub instances_in: usize,
    pub instances_out: usize,
    pub vertices_in: usize,
    pub vertices_out: usize,
    pub triangles_out: usize,
    pub bytes_in: usize,
    pub bytes_out: usize,
}

/// Write a scene holding only the instances whose category is currently
/// visible — solid or ghosted — in `classification`.
///
/// Driving the subset off the live classification means the export is exactly
/// what the viewer is showing, so there is no second list of categories to keep
/// in step with the first.
///
/// # Errors
///
/// Fails if the output cannot be written.
pub fn write_subset(
    scene: &Scene,
    classification: &Classification,
    path: &Path,
) -> Result<Summary> {
    let instances = scene.instances();
    let vertices = scene.vertices();
    let indices = scene.indices();

    let mut out_instances: Vec<Instance> = Vec::new();
    let mut out_vertices: Vec<Vertex> = Vec::new();
    let mut out_indices: Vec<u32> = Vec::new();
    let mut out_materials: Vec<Material> = Vec::new();
    let mut strings: Vec<u8> = Vec::new();

    // Geometry is shared, so a naive copy would duplicate the pipe 1,927 times.
    // Key on the exact index range an instance draws.
    let mut geometry: HashMap<(u32, u32, u32), (u32, u32)> = HashMap::new();
    let mut materials: HashMap<u32, u32> = HashMap::new();

    let mut scene_min = [f32::INFINITY; 3];
    let mut scene_max = [f32::NEG_INFINITY; 3];

    let intern = |strings: &mut Vec<u8>, text: &str| -> (u32, u32) {
        #[allow(clippy::cast_possible_truncation)]
        let offset = strings.len() as u32;
        strings.extend_from_slice(text.as_bytes());
        #[allow(clippy::cast_possible_truncation)]
        (offset, text.len() as u32)
    };

    for (id, instance) in instances.iter().enumerate() {
        if classification.category_of(id).mode == Mode::Hidden {
            continue;
        }

        let key = (
            instance.first_index,
            instance.index_count,
            instance.base_vertex,
        );
        let (new_first, new_base) = if let Some(found) = geometry.get(&key) {
            *found
        } else {
            let start = instance.first_index as usize;
            let end = start + instance.index_count as usize;
            let Some(slice) = indices.get(start..end) else {
                continue;
            };
            // Only the vertices this range actually touches travel with it.
            let (lo, hi) = slice
                .iter()
                .fold((u32::MAX, 0u32), |(lo, hi), i| (lo.min(*i), hi.max(*i)));
            if slice.is_empty() {
                continue;
            }
            let base = instance.base_vertex as usize;
            let Some(span) = vertices.get(base + lo as usize..=base + hi as usize) else {
                continue;
            };

            #[allow(clippy::cast_possible_truncation)]
            let new_base = out_vertices.len() as u32;
            #[allow(clippy::cast_possible_truncation)]
            let new_first = out_indices.len() as u32;
            out_vertices.extend_from_slice(span);
            out_indices.extend(slice.iter().map(|i| i - lo));
            geometry.insert(key, (new_first, new_base));
            (new_first, new_base)
        };

        let old_material = instance.material;
        #[allow(clippy::cast_possible_truncation)]
        let new_material = *materials.entry(old_material).or_insert_with(|| {
            let name = scene
                .materials()
                .get(old_material as usize)
                .map_or("", |m| scene.material_name(m));
            let (name_offset, name_len) = intern(&mut strings, name);
            let colour = scene
                .materials()
                .get(old_material as usize)
                .map_or([0.5, 0.5, 0.5, 1.0], |m| m.colour);
            out_materials.push(Material {
                name_offset,
                name_len,
                colour,
                _pad: [0; 2],
            });
            (out_materials.len() - 1) as u32
        });

        let (name_offset, name_len) = intern(&mut strings, scene.instance_name(instance));

        for axis in 0..3 {
            scene_min[axis] = scene_min[axis].min(instance.aabb_min[axis]);
            scene_max[axis] = scene_max[axis].max(instance.aabb_max[axis]);
        }

        out_instances.push(Instance {
            first_index: new_first,
            base_vertex: new_base,
            material: new_material,
            name_offset,
            name_len,
            ..*instance
        });
    }

    // An empty subset would write a header claiming an infinite bounding box.
    if out_instances.is_empty() {
        anyhow::bail!("nothing to export: every category is hidden");
    }

    let bytes = to_bytes(
        &out_instances,
        &out_materials,
        &strings,
        &out_vertices,
        &out_indices,
        scene.header().bake_hash,
        scene_min,
        scene_max,
    );

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    std::fs::write(path, &bytes).with_context(|| format!("writing {}", path.display()))?;

    Ok(Summary {
        instances_in: instances.len(),
        instances_out: out_instances.len(),
        vertices_in: vertices.len(),
        vertices_out: out_vertices.len(),
        triangles_out: out_indices.len() / 3,
        bytes_in: scene.bytes().len(),
        bytes_out: bytes.len(),
    })
}

/// Serialise to the `.nkp` byte layout.
///
/// Deliberately a copy of the bake's writer rather than a shared one: taking a
/// dependency on `nkp-bake` here would pull the whole VPK and Source 2 decoder
/// stack into the viewer for the sake of forty lines of `copy_from_slice`.
#[allow(clippy::too_many_arguments)]
fn to_bytes(
    instances: &[Instance],
    materials: &[Material],
    strings: &[u8],
    vertices: &[Vertex],
    indices: &[u32],
    bake_hash: u64,
    scene_min: [f32; 3],
    scene_max: [f32; 3],
) -> Vec<u8> {
    let header_size = nkp_format::align_up(std::mem::size_of::<Header>());
    let instance_bytes = bytemuck::cast_slice::<_, u8>(instances);
    let material_bytes = bytemuck::cast_slice::<_, u8>(materials);
    let vertex_bytes = bytemuck::cast_slice::<_, u8>(vertices);
    let index_bytes = bytemuck::cast_slice::<_, u8>(indices);

    let instances_offset = header_size;
    let materials_offset = nkp_format::align_up(instances_offset + instance_bytes.len());
    let strings_offset = nkp_format::align_up(materials_offset + material_bytes.len());
    let vertices_offset = nkp_format::align_up(strings_offset + strings.len());
    let indices_offset = nkp_format::align_up(vertices_offset + vertex_bytes.len());
    let total = nkp_format::align_up(indices_offset + index_bytes.len());

    #[allow(clippy::cast_possible_truncation)]
    let header = Header {
        magic: MAGIC,
        version: VERSION,
        unit_metres: SOURCE_UNIT_METRES,
        bake_hash,
        scene_min,
        scene_max,
        instances_offset: instances_offset as u64,
        instance_count: instances.len() as u32,
        material_count: materials.len() as u32,
        materials_offset: materials_offset as u64,
        strings_offset: strings_offset as u64,
        strings_len: strings.len() as u64,
        vertices_offset: vertices_offset as u64,
        vertex_count: vertices.len() as u64,
        indices_offset: indices_offset as u64,
        index_count: indices.len() as u64,
    };

    let mut out = vec![0u8; total];
    out[..std::mem::size_of::<Header>()].copy_from_slice(bytemuck::bytes_of(&header));
    out[instances_offset..instances_offset + instance_bytes.len()].copy_from_slice(instance_bytes);
    out[materials_offset..materials_offset + material_bytes.len()].copy_from_slice(material_bytes);
    out[strings_offset..strings_offset + strings.len()].copy_from_slice(strings);
    out[vertices_offset..vertices_offset + vertex_bytes.len()].copy_from_slice(vertex_bytes);
    out[indices_offset..indices_offset + index_bytes.len()].copy_from_slice(index_bytes);
    out
}
