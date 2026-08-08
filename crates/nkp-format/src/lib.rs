//! The `.nkp` baked scene container.
//!
//! One file holds everything the viewer needs to draw de_nuke: a concatenated
//! vertex buffer, a concatenated index buffer, and a flat array of instances
//! that each name an index sub-range plus a transform. It is stored
//! uncompressed and read by memory-mapping, because on an NVMe disk the mapping
//! costs nothing and decompression would cost more than the read.
//!
//! # Coordinates
//!
//! Vertex positions stay in **Source units** (one unit is one inch). The
//! conversion to Y-up metres rides on each instance's matrix, exactly as
//! mapview's glTF export puts it on the scene root. Keeping vertices in local
//! space is what lets 1,482 pipe placements share one copy of the pipe.
//!
//! Instance AABBs, by contrast, are already in world metres — the viewer needs
//! them for culling and picking and never wants to transform them again.
//!
//! # Layout
//!
//! ```text
//! [Header] [Instance × n] [Material × m] [string blob] [Vertex × v] [u32 × i]
//! ```
//!
//! Every section starts on a 16-byte boundary so the mapped slices can be cast
//! with `bytemuck` without a copy.
//!
//! # Where the bytes come from
//!
//! Natively the file is memory-mapped. In a browser there is no file and no
//! mmap, so the same scene arrives as a `Vec<u8>` off the network. Both go
//! through [`Scene::from_bytes`]; the only difference is who owns the storage,
//! which is why [`Storage`] exists.

use anyhow::{anyhow, Result};
use bytemuck::{Pod, Zeroable};

/// Magic at the head of every `.nkp`.
pub const MAGIC: [u8; 8] = *b"NKPSCEN\0";

/// Bumped whenever any struct below changes shape.
pub const VERSION: u32 = 1;

/// One Source unit is one inch.
pub const SOURCE_UNIT_METRES: f32 = 0.0254;

/// Section alignment, so mapped slices can be cast in place.
pub const ALIGN: usize = 16;

/// How an instance was placed, for reporting and for rules that care.
pub mod flags {
    /// A `m_sceneObjects` entry: one model, one transform.
    pub const SCENE_OBJECT: u32 = 1 << 0;
    /// A fragment of an `agg_prop_*` aggregate: instanced, carries a transform.
    pub const AGG_PROP: u32 = 1 << 1;
    /// A draw call of an `agg_merge_*` aggregate: geometry already world-space.
    pub const AGG_MERGE: u32 = 1 << 2;
    /// Drawn from a decal/overlay mesh, which needs a depth bias.
    pub const OVERLAY: u32 = 1 << 3;
}

/// A vertex. 32 bytes, laid out for a single interleaved buffer.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, Pod, Zeroable)]
pub struct Vertex {
    /// Position in Source units, model-local.
    pub position: [f32; 3],
    /// Unit normal, model-local.
    pub normal: [f32; 3],
    /// First texture coordinate set.
    pub uv: [f32; 2],
}

/// One drawable placement: a slice of the index buffer and where to put it.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, Pod, Zeroable)]
pub struct Instance {
    /// Row-major 3x4 world matrix, Source-local to **Y-up metres**.
    pub transform: [f32; 12],
    /// World-space AABB in metres.
    pub aabb_min: [f32; 3],
    /// World-space AABB in metres.
    pub aabb_max: [f32; 3],
    /// First index in the shared index buffer.
    pub first_index: u32,
    /// How many indices — always a multiple of three.
    pub index_count: u32,
    /// Added to every index before it reads the vertex buffer.
    pub base_vertex: u32,
    /// Index into the material table.
    pub material: u32,
    /// Byte offset of this instance's name in the string blob.
    pub name_offset: u32,
    /// Length of that name in bytes.
    pub name_len: u32,
    /// See [`flags`].
    pub flags: u32,
    /// Padding, and room for a baked category later.
    pub _pad: u32,
}

/// One material. The viewer mostly wants the name, because classification runs
/// off the `vmat` path.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, Pod, Zeroable)]
pub struct Material {
    /// Byte offset of the `vmat` path in the string blob.
    pub name_offset: u32,
    /// Length of that path in bytes.
    pub name_len: u32,
    /// Flat colour to draw with until textures exist.
    pub colour: [f32; 4],
    /// Padding to keep the struct 16-byte friendly.
    pub _pad: [u32; 2],
}

/// The file header. Offsets are absolute byte positions from the file start.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, Pod, Zeroable)]
pub struct Header {
    /// [`MAGIC`].
    pub magic: [u8; 8],
    /// [`VERSION`].
    pub version: u32,
    /// Metres per stored unit, i.e. [`SOURCE_UNIT_METRES`].
    pub unit_metres: f32,
    /// Identifies the source data this was baked from. Annotations record it
    /// so a re-bake against different game files is caught rather than
    /// silently re-pointing every saved group at the wrong pipes.
    pub bake_hash: u64,
    /// World AABB of the whole scene, in metres.
    pub scene_min: [f32; 3],
    /// World AABB of the whole scene, in metres.
    pub scene_max: [f32; 3],

    /// Byte offset of the instance array.
    pub instances_offset: u64,
    /// Number of instances.
    pub instance_count: u32,
    /// Number of materials.
    pub material_count: u32,
    /// Byte offset of the material array.
    pub materials_offset: u64,
    /// Byte offset of the string blob.
    pub strings_offset: u64,
    /// Length of the string blob in bytes.
    pub strings_len: u64,
    /// Byte offset of the vertex array.
    pub vertices_offset: u64,
    /// Number of vertices.
    pub vertex_count: u64,
    /// Byte offset of the index array.
    pub indices_offset: u64,
    /// Number of indices.
    pub index_count: u64,
}

/// Who owns the scene bytes.
///
/// Native builds map the file, which costs nothing on an NVMe disk and keeps
/// 131 MB out of the process's own memory. A browser has neither files nor
/// mmap, so there the bytes are simply owned.
enum Storage {
    #[cfg(not(target_arch = "wasm32"))]
    Mapped(memmap2::Mmap),
    Owned(Vec<u8>),
}

impl Storage {
    fn bytes(&self) -> &[u8] {
        match self {
            #[cfg(not(target_arch = "wasm32"))]
            Self::Mapped(map) => map,
            Self::Owned(vec) => vec,
        }
    }
}

/// A loaded scene.
pub struct Scene {
    storage: Storage,
    header: Header,
}

impl Scene {
    /// Map a `.nkp` from disk.
    ///
    /// # Errors
    ///
    /// Fails if the file cannot be mapped, is not a `.nkp`, is a version this
    /// build does not understand, or has a section that runs off the end.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn open(path: &std::path::Path) -> Result<Self> {
        use anyhow::Context;

        let file = std::fs::File::open(path)
            .with_context(|| format!("could not open {}", path.display()))?;
        // Safety: the scene file is ours and is not written while mapped. A
        // concurrent truncation would be a SIGBUS, which is the same exposure
        // every mmap-based loader accepts.
        let map = unsafe { memmap2::Mmap::map(&file) }
            .with_context(|| format!("could not map {}", path.display()))?;
        Self::validate(Storage::Mapped(map), &path.display().to_string())
    }

    /// Take a `.nkp` that is already in memory — downloaded, or dropped onto
    /// the page. `label` only ever appears in error messages.
    ///
    /// # Errors
    ///
    /// As [`Scene::open`], minus the ways opening a file can fail.
    pub fn from_bytes(bytes: Vec<u8>, label: &str) -> Result<Self> {
        Self::validate(Storage::Owned(bytes), label)
    }

    fn validate(storage: Storage, label: &str) -> Result<Self> {
        let bytes = storage.bytes();
        if bytes.len() < std::mem::size_of::<Header>() {
            return Err(anyhow!("{label} is too small to be a .nkp"));
        }
        let header: Header = *bytemuck::from_bytes(&bytes[..std::mem::size_of::<Header>()]);
        if header.magic != MAGIC {
            return Err(anyhow!("{label} is not a .nkp"));
        }
        if header.version != VERSION {
            return Err(anyhow!(
                "{label} is .nkp version {}, this build reads {VERSION}. \
                 Run `nukeplant.bat rebake` to rebuild the scene.",
                header.version
            ));
        }

        let scene = Self { storage, header };
        scene.check_bounds(label)?;
        Ok(scene)
    }

    fn check_bounds(&self, label: &str) -> Result<()> {
        let len = self.bytes().len();
        let end = |offset: u64, count: u64, size: usize| -> Result<()> {
            let want = offset
                .checked_add(count.saturating_mul(size as u64))
                .ok_or_else(|| anyhow!("{label}: section size overflows"))?;
            if want as usize > len {
                return Err(anyhow!(
                    "{label}: section ends at {want} but the file is {len} bytes"
                ));
            }
            Ok(())
        };
        let h = &self.header;
        end(
            h.instances_offset,
            u64::from(h.instance_count),
            std::mem::size_of::<Instance>(),
        )?;
        end(
            h.materials_offset,
            u64::from(h.material_count),
            std::mem::size_of::<Material>(),
        )?;
        end(h.strings_offset, h.strings_len, 1)?;
        end(
            h.vertices_offset,
            h.vertex_count,
            std::mem::size_of::<Vertex>(),
        )?;
        end(h.indices_offset, h.index_count, 4)?;
        Ok(())
    }

    /// The whole file, however it is being stored.
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        self.storage.bytes()
    }

    /// The file header.
    #[must_use]
    pub fn header(&self) -> &Header {
        &self.header
    }

    /// Every instance, in bake order. The index into this slice is the stable
    /// instance id that annotations refer to.
    #[must_use]
    pub fn instances(&self) -> &[Instance] {
        self.slice(
            self.header.instances_offset,
            self.header.instance_count as usize,
        )
    }

    /// Every material.
    #[must_use]
    pub fn materials(&self) -> &[Material] {
        self.slice(
            self.header.materials_offset,
            self.header.material_count as usize,
        )
    }

    /// The interleaved vertex buffer.
    #[must_use]
    pub fn vertices(&self) -> &[Vertex] {
        self.slice(
            self.header.vertices_offset,
            self.header.vertex_count as usize,
        )
    }

    /// The shared index buffer.
    #[must_use]
    pub fn indices(&self) -> &[u32] {
        self.slice(self.header.indices_offset, self.header.index_count as usize)
    }

    /// Raw bytes of the vertex buffer, for uploading without a copy.
    #[must_use]
    pub fn vertex_bytes(&self) -> &[u8] {
        bytemuck::cast_slice(self.vertices())
    }

    /// Raw bytes of the index buffer.
    #[must_use]
    pub fn index_bytes(&self) -> &[u8] {
        bytemuck::cast_slice(self.indices())
    }

    /// An instance's name, e.g. `n0_lr0_agg_prop_metal_pipe_001_0#417`.
    #[must_use]
    pub fn instance_name(&self, instance: &Instance) -> &str {
        self.string(instance.name_offset, instance.name_len)
    }

    /// A material's `vmat` path.
    #[must_use]
    pub fn material_name(&self, material: &Material) -> &str {
        self.string(material.name_offset, material.name_len)
    }

    fn string(&self, offset: u32, len: u32) -> &str {
        let start = self.header.strings_offset as usize + offset as usize;
        let end = start + len as usize;
        self.bytes()
            .get(start..end)
            .and_then(|b| std::str::from_utf8(b).ok())
            .unwrap_or_default()
    }

    fn slice<T: Pod>(&self, offset: u64, count: usize) -> &[T] {
        let start = offset as usize;
        let end = start + count * std::mem::size_of::<T>();
        // check_bounds ran at load, so this cannot be out of range.
        bytemuck::cast_slice(&self.bytes()[start..end])
    }
}

/// Round `n` up to the next [`ALIGN`] boundary.
#[must_use]
pub fn align_up(n: usize) -> usize {
    (n + ALIGN - 1) & !(ALIGN - 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn structs_are_the_sizes_the_layout_assumes() {
        assert_eq!(std::mem::size_of::<Vertex>(), 32);
        assert_eq!(std::mem::size_of::<Instance>(), 104);
        assert_eq!(std::mem::size_of::<Material>(), 32);
        assert_eq!(std::mem::size_of::<Header>(), 120);
    }

    #[test]
    fn alignment_rounds_up_only_when_it_has_to() {
        assert_eq!(align_up(0), 0);
        assert_eq!(align_up(1), 16);
        assert_eq!(align_up(16), 16);
        assert_eq!(align_up(17), 32);
    }
}
