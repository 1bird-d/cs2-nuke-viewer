//! Typed decoding of Source 2 model and mesh geometry.
//!
//! A compiled model (`vmdl_c`) or mesh (`vmesh_c`) carries a `CTRL` block of
//! KV3 describing its meshes, and one `MDAT` block per mesh holding that mesh's
//! bounds and draw calls, each naming a material and a slice of one
//! vertex/index buffer pair. Where the *buffers* live depends on what compiled
//! the resource, and there are two answers.
//!
//! # Two buffer layouts
//!
//! **Map geometry** — the aggregate meshes a world compiles into — describes
//! each buffer inline in its `CTRL` entry (`m_vertexBuffers`,
//! `m_indexBuffers`), giving every one a block of its own: `MVTX` for vertices
//! and `MIDX` for indices, normally compressed with the meshoptimizer codecs in
//! [`crate::meshopt`]. There is no fixed relationship between the number of
//! each; de_dust2 has 1008 `MVTX` and 523 `MIDX`.
//!
//! **Prop models** — everything under `models/`, which is what a map's entity
//! lumps place — instead name two block *indices* per mesh, `data_block` and
//! `vbib_block`, and pack all of that mesh's buffers into the single `MBUF`
//! block the second points at. `MBUF` is CS2's name for the block older Source
//! 2 called `VBIB`, and the layout is that one: a four-word header, then buffer
//! descriptors, then 56-byte input-layout records, with **every offset counted
//! from the field that holds it** rather than from the start of the block.
//! [`parse_mesh_buffers`] reads it.
//!
//! Milestones up to v0.0.4 only ever walked map geometry, so only the first
//! layout was implemented — and because a `CTRL` entry with no `m_nDataBlock`
//! decodes to no mesh rather than to an error, every prop model in the game
//! came out empty without complaining.
//!
//! # Vertex streams
//!
//! A draw call names a *list* of vertex buffers, not one. The merged aggregate
//! geometry a map compiles its world into keeps positions in one stream and
//! everything else — texture coordinates, the packed tangent frame, vertex
//! colour — in a second, parallel one with the same element count. Reading only
//! the first stream yields geometry with no UVs at all, which is most of a map.
//! [`Mesh::vertex_buffers`] therefore holds *merged* buffers: one per distinct
//! combination of streams a draw call uses.
//!
//! [`Model::from_resource`] walks all of that and returns triangle soup ready
//! for export: positions, normals and texture coordinates already converted to
//! `f32`, with indices rebased so each draw call stands alone.

use std::collections::HashMap;

use crate::error::{Result, Source2Error};
use crate::kv3::{self, KvValue};
use crate::meshopt;
use crate::resource::{FourCc, Resource};
use crate::skeleton::Skeleton;

/// Refuse absurd buffer geometry rather than trying to allocate for it.
const MAX_ELEMENTS: i64 = 64 << 20;

/// A vertex attribute's storage format, from the DXGI format numbering Valve
/// reuses in `m_Format`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VertexFormat {
    /// `R32G32B32_FLOAT` (6): three 32-bit floats.
    Rgb32Float,
    /// `R32G32_FLOAT` (16): two 32-bit floats.
    Rg32Float,
    /// `R16G16_FLOAT` (34): two halves.
    Rg16Float,
    /// `R16G16_UNORM` (35): two normalised unsigned shorts.
    Rg16Unorm,
    /// `R16G16_SNORM` (37): two normalised signed shorts.
    Rg16Snorm,
    /// `R8G8B8A8_UNORM` (28): four normalised bytes.
    Rgba8Unorm,
    /// `R8G8B8A8_UINT` (30): four unsigned bytes, used for joint indices.
    Rgba8Uint,
    /// `R16G16B16A16_UINT` (12): four unsigned shorts, for skeletons with more
    /// than 256 bones.
    Rgba16Uint,
    /// `R32_UINT` (42): one 32-bit word. For a `NORMAL` this is Valve's packed
    /// tangent frame.
    R32Uint,
    /// A format this decoder does not read.
    Unsupported(i64),
}

impl VertexFormat {
    /// Classify a `m_Format` value.
    #[must_use]
    pub fn from_dxgi(value: i64) -> Self {
        match value {
            6 => Self::Rgb32Float,
            16 => Self::Rg32Float,
            34 => Self::Rg16Float,
            35 => Self::Rg16Unorm,
            37 => Self::Rg16Snorm,
            28 => Self::Rgba8Unorm,
            30 => Self::Rgba8Uint,
            12 => Self::Rgba16Uint,
            42 => Self::R32Uint,
            other => Self::Unsupported(other),
        }
    }

    /// Size of one element in bytes, or `None` if unsupported.
    #[must_use]
    pub fn size(self) -> Option<usize> {
        Some(match self {
            Self::Rgb32Float => 12,
            Self::Rg32Float | Self::Rgba16Uint => 8,
            Self::Rgba8Unorm | Self::Rgba8Uint | Self::R32Uint => 4,
            Self::Rg16Float | Self::Rg16Unorm | Self::Rg16Snorm => 4,
            Self::Unsupported(_) => return None,
        })
    }
}

/// One entry of a vertex buffer's input layout.
#[derive(Debug, Clone)]
pub struct LayoutField {
    /// Semantic name, e.g. `POSITION`, `NORMAL`, `TEXCOORD`.
    pub semantic: String,
    /// Semantic index, for e.g. `TEXCOORD1`.
    pub semantic_index: i64,
    /// Storage format.
    pub format: VertexFormat,
    /// Byte offset within the vertex.
    pub offset: usize,
}

/// A vertex or index buffer described by the `CTRL` block.
#[derive(Debug, Clone)]
pub struct BufferDesc {
    /// Index into [`Resource::blocks`] of the block holding the bytes.
    pub block_index: usize,
    /// Number of elements (vertices, or indices).
    pub element_count: usize,
    /// Bytes per element.
    pub element_size: usize,
    /// Whether the payload is meshoptimizer-compressed.
    pub meshopt_compressed: bool,
    /// Whether the payload is additionally zstd-wrapped. Not seen in CS2 maps.
    pub zstd_compressed: bool,
    /// Whether an index buffer uses the meshopt *sequence* codec rather than
    /// the triangle codec. Not seen in CS2 maps.
    pub index_sequence: bool,
    /// Vertex attribute layout; empty for index buffers.
    pub fields: Vec<LayoutField>,
    /// The `offset..offset + length` slice of the block that holds this
    /// buffer's bytes, when the block holds several.
    ///
    /// A map's world geometry gives each buffer a block of its own — `MVTX` and
    /// `MIDX` — so the whole block is the buffer and this is `None`. A prop
    /// model instead packs every buffer of a mesh into a single [`MBUF`] block
    /// with its own directory, and then each buffer is a range within it.
    ///
    /// [`MBUF`]: Model::from_resource
    pub range: Option<(usize, usize)>,
}

impl BufferDesc {
    fn parse(value: &KvValue) -> Option<Self> {
        let count = value.get("m_nElementCount")?.as_i64()?;
        let size = value.get("m_nElementSizeInBytes")?.as_i64()?;
        let block = value.get("m_nBlockIndex")?.as_i64()?;
        if !(0..=MAX_ELEMENTS).contains(&count)
            || !(0..=4096).contains(&size)
            || !(0..=i64::from(u32::MAX)).contains(&block)
        {
            return None;
        }

        let fields = value
            .get("m_inputLayoutFields")
            .and_then(KvValue::as_array)
            .unwrap_or(&[])
            .iter()
            .filter_map(|f| {
                let offset = f.get("m_nOffset")?.as_i64()?;
                Some(LayoutField {
                    semantic: f.get("m_pSemanticName")?.as_str()?.to_string(),
                    semantic_index: f.get("m_nSemanticIndex").and_then(KvValue::as_i64)?,
                    format: VertexFormat::from_dxgi(f.get("m_Format")?.as_i64()?),
                    offset: usize::try_from(offset).ok()?,
                })
            })
            .collect();

        let flag = |name: &str| {
            value
                .get(name)
                .and_then(KvValue::as_bool)
                .unwrap_or_default()
        };

        Some(Self {
            block_index: block as usize,
            element_count: count as usize,
            element_size: size as usize,
            meshopt_compressed: flag("m_bMeshoptCompressed"),
            zstd_compressed: flag("m_bCompressedZSTD"),
            index_sequence: flag("m_bMeshoptIndexSequence"),
            fields,
            range: None,
        })
    }

    /// Find a layout field by semantic name, case-insensitively.
    #[must_use]
    pub fn field(&self, semantic: &str, index: i64) -> Option<&LayoutField> {
        self.fields
            .iter()
            .find(|f| f.semantic.eq_ignore_ascii_case(semantic) && f.semantic_index == index)
    }

    /// This buffer's bytes as stored, before any decompression.
    ///
    /// The whole block when the buffer owns one, or its slice of a shared
    /// `MBUF` pool when it does not.
    fn stored<'a>(&self, resource: &Resource<'a>) -> Result<&'a [u8]> {
        let block = resource
            .blocks
            .get(self.block_index)
            .ok_or_else(|| Source2Error::Meshopt {
                codec: "buffer",
                detail: format!("block index {} out of range", self.block_index),
            })?;
        let whole = resource.block_bytes(block);
        match self.range {
            None => Ok(whole),
            Some((offset, length)) => {
                whole
                    .get(offset..offset.saturating_add(length))
                    .ok_or_else(|| Source2Error::Meshopt {
                        codec: "buffer",
                        detail: format!(
                            "buffer range {offset}..{} outside a {} byte block",
                            offset.saturating_add(length),
                            whole.len()
                        ),
                    })
            }
        }
    }

    /// Decompress this buffer into raw element bytes.
    fn raw<'a>(&self, resource: &Resource<'a>) -> Result<std::borrow::Cow<'a, [u8]>> {
        let bytes = self.stored(resource)?;

        if self.zstd_compressed {
            return Err(Source2Error::Meshopt {
                codec: "buffer",
                detail: "zstd-wrapped mesh buffers are not supported".into(),
            });
        }

        if !self.meshopt_compressed {
            return Ok(std::borrow::Cow::Borrowed(bytes));
        }

        let decoded = meshopt::decode_vertex_buffer(self.element_count, self.element_size, bytes)?;
        Ok(std::borrow::Cow::Owned(decoded))
    }
}

/// Bytes of one input-layout field record inside an `MBUF` directory.
///
/// A fixed 32-byte semantic name, then seven 32-bit fields: semantic index,
/// format, offset, slot, slot type and instance step rate.
const MBUF_FIELD_BYTES: usize = 56;

/// Bytes of one buffer descriptor inside an `MBUF` directory.
const MBUF_BUFFER_BYTES: usize = 24;

/// The vertex and index buffers an `MBUF` block holds.
struct MeshBuffers {
    vertex: Vec<BufferDesc>,
    index: Vec<BufferDesc>,
}

/// Read the directory at the head of an `MBUF` block.
///
/// `MBUF` is CS2's name for the block older Source 2 called `VBIB`, and the
/// layout is that one. It opens with four words — where the vertex buffer
/// descriptors are and how many, then the same for the index buffers — and
/// **every offset in the block is relative to the position of the word that
/// holds it**, not to the start of the block, which is the one thing about this
/// format that will silently produce garbage if assumed otherwise.
///
/// Each descriptor is six words: element count, element size, where its input
/// layout fields are and how many, and where its bytes are and how many. A
/// buffer whose byte count is exactly `count * size` is stored raw; anything
/// smaller is meshoptimizer-compressed, which is how the compression is
/// signalled — there is no flag.
fn parse_mesh_buffers(block_index: usize, bytes: &[u8]) -> Option<MeshBuffers> {
    let word = |at: usize| -> Option<u32> {
        let raw = bytes.get(at..at + 4)?;
        Some(u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]))
    };
    // An offset counted from the field that carries it.
    let target = |at: usize| -> Option<usize> { at.checked_add(word(at)? as usize) };

    let read_buffers = |directory: usize, count: u32| -> Option<Vec<BufferDesc>> {
        let mut out = Vec::with_capacity(count.min(4096) as usize);
        for i in 0..count as usize {
            let at = directory.checked_add(i.checked_mul(MBUF_BUFFER_BYTES)?)?;
            let element_count = word(at)? as usize;
            let element_size = word(at + 4)? as usize;
            let fields_at = target(at + 8)?;
            let field_count = word(at + 12)?;
            let data_at = target(at + 16)?;
            let total_size = word(at + 20)? as usize;

            if element_count > MAX_ELEMENTS as usize || element_size > 4096 {
                return None;
            }
            // Raw when the bytes are exactly what the elements need; the
            // absence of a flag is the format's own signal.
            let raw_size = element_count.checked_mul(element_size)?;
            let compressed = total_size != raw_size;
            if data_at.checked_add(total_size)? > bytes.len() {
                return None;
            }

            let mut fields = Vec::with_capacity(field_count.min(64) as usize);
            for f in 0..field_count as usize {
                let field = fields_at.checked_add(f.checked_mul(MBUF_FIELD_BYTES)?)?;
                let name = bytes.get(field..field + 32)?;
                let end = name.iter().position(|&b| b == 0).unwrap_or(name.len());
                fields.push(LayoutField {
                    semantic: String::from_utf8_lossy(&name[..end]).into_owned(),
                    semantic_index: i64::from(word(field + 32)?),
                    format: VertexFormat::from_dxgi(i64::from(word(field + 36)?)),
                    offset: word(field + 40)? as usize,
                });
            }

            out.push(BufferDesc {
                block_index,
                element_count,
                element_size,
                meshopt_compressed: compressed,
                zstd_compressed: false,
                index_sequence: false,
                fields,
                range: Some((data_at, total_size)),
            });
        }
        Some(out)
    };

    let vertex = read_buffers(target(0)?, word(4)?)?;
    let index = read_buffers(target(8)?, word(12)?)?;
    Some(MeshBuffers { vertex, index })
}

/// One draw call: a run of indices into one vertex buffer, with a material.
#[derive(Debug, Clone)]
pub struct DrawCallDesc {
    /// Material resource path, e.g. `materials/dev/foo.vmat`.
    pub material: String,
    /// Indices of the parallel vertex streams this draw call reads, within the
    /// mesh. Merged geometry splits positions and the rest across two.
    pub vertex_buffers: Vec<usize>,
    /// Index of the index buffer within the mesh.
    pub index_buffer: usize,
    /// First vertex this draw call uses.
    pub base_vertex: usize,
    /// How many vertices it uses.
    pub vertex_count: usize,
    /// First index.
    pub start_index: usize,
    /// How many indices.
    pub index_count: usize,
    /// Primitive topology string, e.g. `RENDER_PRIM_TRIANGLES`.
    pub primitive_type: String,
}

/// Decoded vertex attributes for one vertex buffer.
#[derive(Debug, Clone, Default)]
pub struct VertexBuffer {
    /// Vertex positions, in Source units.
    pub positions: Vec<[f32; 3]>,
    /// Vertex normals, unit length. Empty if the buffer had none.
    pub normals: Vec<[f32; 3]>,
    /// First texture coordinate set. Empty if the buffer had none.
    pub uvs: Vec<[f32; 2]>,
    /// Up to four joint influences per vertex, as stored. These are *not*
    /// skeleton bone indices: they address the mesh's slice of the model's
    /// `m_remappingTable`, which [`Model::remap_table`] returns. Empty for
    /// unskinned geometry.
    pub joints: Vec<[u16; 4]>,
    /// The weight of each influence, normalised so the four sum to one. Empty
    /// when the buffer had no weight stream.
    pub weights: Vec<[f32; 4]>,
}

impl VertexBuffer {
    /// Whether this buffer carries skinning data.
    #[must_use]
    pub fn is_skinned(&self) -> bool {
        !self.joints.is_empty() && self.joints.len() == self.positions.len()
    }
}

/// One draw call: a material and the triangles it draws.
///
/// Indices address [`Mesh::vertex_buffers`]`[vertex_buffer]` directly, so
/// several primitives sharing a buffer stay sharing it.
#[derive(Debug, Clone, Default)]
pub struct Primitive {
    /// Material resource path.
    pub material: String,
    /// Which of the mesh's vertex buffers this draws from.
    pub vertex_buffer: usize,
    /// Triangle indices into that vertex buffer.
    pub indices: Vec<u32>,
}

/// One embedded mesh: bounds, shared vertex buffers, and draw calls.
#[derive(Debug, Clone)]
pub struct Mesh {
    /// Mesh name from the `CTRL` block.
    pub name: String,
    /// Minimum corner of the mesh's bounding box, in Source units.
    pub min_bounds: [f32; 3],
    /// Maximum corner.
    pub max_bounds: [f32; 3],
    /// Decoded vertex buffers, indexed by [`Primitive::vertex_buffer`].
    pub vertex_buffers: Vec<VertexBuffer>,
    /// Draw calls, in file order.
    pub primitives: Vec<Primitive>,
}

/// One named skin: an alternative set of materials for a model.
///
/// Every group lists the same materials in the same order, so substituting a
/// skin is positional — the material at index *i* of group 0 becomes the one at
/// index *i* of the group asked for. CS2's shipped groups are things like
/// `dusty`, `new` and `tint_default`, named by an entity's `skin` property.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MaterialGroup {
    /// The name an entity's `skin` names it by, e.g. `default` or `dusty`.
    pub name: String,
    /// Material resource paths, parallel to every other group's list.
    pub materials: Vec<String>,
}

/// A decoded model: every embedded mesh with its geometry.
#[derive(Debug, Clone, Default)]
pub struct Model {
    /// The meshes embedded in the resource, in `CTRL` order.
    pub meshes: Vec<Mesh>,
    /// The `m_refLODGroupMasks` entry for each mesh, when the model's `DATA`
    /// block carries one per mesh. Bit *n* set means the mesh belongs to level
    /// of detail *n*, so `mask & 1` selects the full-detail set. Empty when the
    /// resource does not describe its levels of detail, which world geometry
    /// never does.
    pub lod_masks: Vec<u64>,
    /// `m_materialGroups`: the model's skins, in file order.
    ///
    /// Group 0 is the set the draw calls already name, so a model with no
    /// groups, or one asked for its first, needs no substitution at all. Read
    /// it through [`Model::skin_materials`].
    pub material_groups: Vec<MaterialGroup>,
    /// The model's bone hierarchy, when it has one. World geometry does not.
    pub skeleton: Option<Skeleton>,
    /// `m_remappingTable`: every mesh's joint-index-to-bone-index map, laid
    /// end to end. Read it through [`Model::remap_table`].
    pub remapping_table: Vec<u32>,
    /// `m_remappingTableStarts`: where each mesh's slice of the table begins.
    pub remapping_starts: Vec<usize>,
}

impl Model {
    /// Whether mesh `index` is part of level of detail 0, the highest.
    ///
    /// Models with no level-of-detail information answer `true` for every
    /// mesh, since there is only one set to choose from.
    #[must_use]
    pub fn mesh_in_lod0(&self, index: usize) -> bool {
        match self.lod_masks.get(index) {
            Some(mask) => mask & 1 != 0,
            None => true,
        }
    }

    /// The material substitutions one named skin asks for.
    ///
    /// Returns pairs of *(material the draw calls name, material to use
    /// instead)*, which is the substitution table for `skin`. `None` means
    /// nothing needs substituting, which covers every ordinary case: a model
    /// with no groups at all, a `skin` naming group 0 — CS2 writes that as
    /// `default` or `0` — and a `skin` this model has never heard of, which is
    /// better drawn in its default materials than not drawn.
    ///
    /// The mapping is positional, because every group lists the same materials
    /// in the same order. A pair whose two sides are equal is left out, so an
    /// empty result and `None` mean the same thing to a caller.
    #[must_use]
    pub fn skin_materials(&self, skin: &str) -> Option<Vec<(&str, &str)>> {
        let base = self.material_groups.first()?;
        // Group 0 is what the draw calls already name. CS2 writes "no
        // substitution" three ways and they all land here.
        if skin.is_empty() || skin.eq_ignore_ascii_case("0") || skin == base.name {
            return None;
        }
        let wanted = self
            .material_groups
            .iter()
            .find(|g| g.name.eq_ignore_ascii_case(skin))?;

        let pairs: Vec<(&str, &str)> = base
            .materials
            .iter()
            .zip(wanted.materials.iter())
            .filter(|(from, to)| from != to)
            .map(|(from, to)| (from.as_str(), to.as_str()))
            .collect();
        (!pairs.is_empty()).then_some(pairs)
    }

    /// Mesh `index`'s slice of the model's joint remapping table.
    ///
    /// A skinned vertex's `BLENDINDICES` do not name skeleton bones directly.
    /// Each mesh is compiled against a compact palette of only the bones that
    /// actually influence it — the SAS's five meshes use 389 palette slots
    /// between them against 94 bones — and this table turns a palette slot
    /// back into a bone index. An empty slice means the mesh's joint indices
    /// are already bone indices.
    #[must_use]
    pub fn remap_table(&self, index: usize) -> &[u32] {
        let Some(&start) = self.remapping_starts.get(index) else {
            return &[];
        };
        let end = self
            .remapping_starts
            .get(index + 1)
            .copied()
            .unwrap_or(self.remapping_table.len());
        self.remapping_table
            .get(start..end.max(start))
            .unwrap_or(&[])
    }

    /// Turn a vertex's stored joint index into a skeleton bone index.
    ///
    /// Out-of-range indices fall back to bone 0, which is a model's root, so a
    /// mesh with a bad palette entry stays attached rather than flying off.
    #[must_use]
    pub fn resolve_joint(&self, mesh_index: usize, joint: u16) -> u16 {
        let table = self.remap_table(mesh_index);
        if table.is_empty() {
            return joint;
        }
        let bone = table.get(usize::from(joint)).copied().unwrap_or(0);
        u16::try_from(bone).unwrap_or(0)
    }
}

impl Model {
    /// Decode every embedded mesh in a compiled model or mesh resource.
    ///
    /// Resources with no `CTRL` block, or no embedded meshes, decode to an
    /// empty model rather than an error.
    ///
    /// # Errors
    ///
    /// Returns [`Source2Error`] if a block that is referenced cannot be
    /// decoded. Draw calls that reference missing buffers are skipped.
    pub fn from_resource(resource: &Resource<'_>) -> Result<Self> {
        let Some(ctrl) = resource.kv3_block(FourCc::new(b"CTRL")) else {
            return Ok(Self::default());
        };
        let ctrl = ctrl?;
        let Some(embedded) = ctrl.get("embedded_meshes").and_then(KvValue::as_array) else {
            return Ok(Self::default());
        };

        let mut meshes = Vec::new();
        for entry in embedded {
            if let Some(mesh) = Self::decode_mesh(resource, entry)? {
                meshes.push(mesh);
            }
        }

        // Levels of detail, the skeleton and the joint remapping table are all
        // described in the model's own `DATA` block. A mesh resource has no
        // `DATA` block at all, so all three stay empty for one.
        let data = resource.data_kv3().and_then(Result::ok);

        // One mask per mesh; a set that does not line up with the meshes
        // decoded here is not something to guess at.
        let lod_masks = data
            .as_ref()
            .and_then(|doc| {
                doc.get("m_refLODGroupMasks")
                    .and_then(KvValue::as_array)
                    .map(|masks| masks.iter().filter_map(KvValue::as_u64).collect::<Vec<_>>())
            })
            .filter(|masks: &Vec<u64>| masks.len() == meshes.len())
            .unwrap_or_default();

        let material_groups = data
            .as_ref()
            .and_then(|doc| doc.get("m_materialGroups").and_then(KvValue::as_array))
            .unwrap_or(&[])
            .iter()
            .filter_map(|g| {
                Some(MaterialGroup {
                    name: g.get("m_name")?.as_str()?.to_string(),
                    materials: g
                        .get("m_materials")
                        .and_then(KvValue::as_array)
                        .unwrap_or(&[])
                        .iter()
                        .filter_map(|m| m.as_str().map(str::to_string))
                        .collect(),
                })
            })
            .collect();

        let skeleton = data
            .as_ref()
            .and_then(|doc| Skeleton::from_model_data(&doc.root));

        let u32s = |key: &str| -> Vec<u32> {
            data.as_ref()
                .and_then(|doc| doc.get(key).and_then(KvValue::as_array))
                .unwrap_or(&[])
                .iter()
                .map(|v| {
                    v.as_u64()
                        .or_else(|| v.as_i64().and_then(|i| u64::try_from(i).ok()))
                        .and_then(|x| u32::try_from(x).ok())
                        .unwrap_or(0)
                })
                .collect()
        };
        let remapping_table = u32s("m_remappingTable");
        let remapping_starts = u32s("m_remappingTableStarts")
            .into_iter()
            .map(|v| v as usize)
            .collect();

        Ok(Self {
            meshes,
            lod_masks,
            material_groups,
            skeleton,
            remapping_table,
            remapping_starts,
        })
    }

    fn decode_mesh(resource: &Resource<'_>, entry: &KvValue) -> Result<Option<Mesh>> {
        // The `CTRL` block comes in two shapes, and which one a resource uses
        // is decided by what compiled it rather than by any version field.
        //
        // A **map's** world geometry describes each buffer inline, in the
        // `CTRL` entry itself, and gives every one a block of its own (`MVTX`,
        // `MIDX`). A **prop model** names two block indices instead — its
        // `MDAT` metadata and its `MBUF` buffer pool — and packs all of that
        // mesh's buffers into the one `MBUF`. Milestones up to here only ever
        // walked map geometry, so only the first shape was implemented, and
        // every prop model decoded to zero meshes without complaining.
        let name = entry
            .get("m_Name")
            .or_else(|| entry.get("name"))
            .and_then(KvValue::as_str)
            .unwrap_or("")
            .to_string();

        let block_at =
            |value: Option<&KvValue>| -> Option<usize> { usize::try_from(value?.as_i64()?).ok() };

        let (vertex_buffers, index_buffers) = match block_at(entry.get("vbib_block")) {
            Some(index) => {
                let Some(block) = resource.blocks.get(index) else {
                    return Ok(None);
                };
                let Some(buffers) = parse_mesh_buffers(index, resource.block_bytes(block)) else {
                    return Err(Source2Error::Meshopt {
                        codec: "mbuf",
                        detail: format!("block {index} is not a readable buffer directory"),
                    });
                };
                (buffers.vertex, buffers.index)
            }
            None => (
                entry
                    .get("m_vertexBuffers")
                    .and_then(KvValue::as_array)
                    .unwrap_or(&[])
                    .iter()
                    .filter_map(BufferDesc::parse)
                    .collect(),
                entry
                    .get("m_indexBuffers")
                    .and_then(KvValue::as_array)
                    .unwrap_or(&[])
                    .iter()
                    .filter_map(BufferDesc::parse)
                    .collect(),
            ),
        };

        // Each embedded mesh names the block holding its own metadata.
        let Some(data_block) = entry
            .get("m_nDataBlock")
            .or_else(|| entry.get("data_block"))
            .and_then(KvValue::as_i64)
        else {
            return Ok(None);
        };
        let Some(block) = usize::try_from(data_block)
            .ok()
            .and_then(|i| resource.blocks.get(i))
        else {
            return Ok(None);
        };
        let mdat = kv3::decode(resource.block_bytes(block))?;

        let Some(scene_objects) = mdat.get("m_sceneObjects").and_then(KvValue::as_array) else {
            return Ok(None);
        };

        let mut min_bounds = [f32::MAX; 3];
        let mut max_bounds = [f32::MIN; 3];
        let mut primitives = Vec::new();
        // Raw streams are decoded at most once, then merged into one buffer
        // per distinct stream combination, also at most once.
        let mut decoded: Vec<Option<VertexBuffer>> = vec![None; vertex_buffers.len()];
        let mut merged: Vec<VertexBuffer> = Vec::new();
        let mut merged_index: HashMap<Vec<usize>, usize> = HashMap::new();
        let mut indices_cache: Vec<Option<Vec<u32>>> = vec![None; index_buffers.len()];

        for object in scene_objects {
            if let Some(v) = object.get("m_vMinBounds").and_then(vec3) {
                for i in 0..3 {
                    min_bounds[i] = min_bounds[i].min(v[i]);
                }
            }
            if let Some(v) = object.get("m_vMaxBounds").and_then(vec3) {
                for i in 0..3 {
                    max_bounds[i] = max_bounds[i].max(v[i]);
                }
            }

            for call in object
                .get("m_drawCalls")
                .and_then(KvValue::as_array)
                .unwrap_or(&[])
            {
                let Some(desc) = parse_draw_call(call) else {
                    continue;
                };
                // Only triangle lists are exported; CS2 world meshes use
                // nothing else.
                if desc.primitive_type != "RENDER_PRIM_TRIANGLES" {
                    continue;
                }
                let streams: Vec<usize> = desc
                    .vertex_buffers
                    .iter()
                    .copied()
                    .filter(|&i| i < vertex_buffers.len())
                    .collect();
                let Some(ib) = index_buffers.get(desc.index_buffer) else {
                    continue;
                };
                if streams.is_empty() {
                    continue;
                }
                for &stream in &streams {
                    if decoded[stream].is_none() {
                        decoded[stream] =
                            Some(decode_vertex_buffer(resource, &vertex_buffers[stream])?);
                    }
                }
                if indices_cache[desc.index_buffer].is_none() {
                    indices_cache[desc.index_buffer] = Some(decode_indices(resource, ib)?);
                }

                let slot = match merged_index.get(&streams) {
                    Some(&slot) => slot,
                    None => {
                        merged.push(merge_streams(&streams, &decoded));
                        merged_index.insert(streams.clone(), merged.len() - 1);
                        merged.len() - 1
                    }
                };

                let Some(all_indices) = indices_cache[desc.index_buffer].as_ref() else {
                    continue;
                };
                if let Some(primitive) = build_primitive(&desc, slot, &merged[slot], all_indices) {
                    primitives.push(primitive);
                }
            }
        }

        if min_bounds[0] > max_bounds[0] {
            min_bounds = [0.0; 3];
            max_bounds = [0.0; 3];
        }

        Ok(Some(Mesh {
            name,
            min_bounds,
            max_bounds,
            vertex_buffers: merged,
            primitives,
        }))
    }
}

fn parse_draw_call(call: &KvValue) -> Option<DrawCallDesc> {
    let non_negative = |name: &str| -> Option<usize> {
        let v = call.get(name)?.as_i64()?;
        if (0..=MAX_ELEMENTS).contains(&v) {
            usize::try_from(v).ok()
        } else {
            None
        }
    };

    let handle = |name: &str| -> usize {
        call.get(name)
            .and_then(|b| b.get("m_hBuffer"))
            .and_then(KvValue::as_i64)
            .and_then(|v| usize::try_from(v).ok())
            .unwrap_or(0)
    };

    Some(DrawCallDesc {
        material: call
            .get("m_material")
            .and_then(KvValue::as_str)
            .unwrap_or("")
            .to_string(),
        vertex_buffers: call
            .get("m_vertexBuffers")
            .and_then(KvValue::as_array)
            .unwrap_or(&[])
            .iter()
            .filter_map(|b| usize::try_from(b.get("m_hBuffer").and_then(KvValue::as_i64)?).ok())
            .collect(),
        index_buffer: handle("m_indexBuffer"),
        base_vertex: non_negative("m_nBaseVertex")?,
        vertex_count: non_negative("m_nVertexCount")?,
        start_index: non_negative("m_nStartIndex")?,
        index_count: non_negative("m_nIndexCount")?,
        primitive_type: call
            .get("m_nPrimitiveType")
            .and_then(KvValue::as_str)
            .unwrap_or("")
            .to_string(),
    })
}

/// Combine parallel vertex streams into one attribute set.
///
/// The streams cover the same vertices, so the first stream that carries an
/// attribute wins. An attribute whose length does not match the position count
/// cannot be paired up vertex for vertex and is dropped rather than guessed at.
fn merge_streams(streams: &[usize], decoded: &[Option<VertexBuffer>]) -> VertexBuffer {
    let mut out = VertexBuffer::default();
    for &stream in streams {
        let Some(buffer) = decoded.get(stream).and_then(Option::as_ref) else {
            continue;
        };
        if out.positions.is_empty() {
            out.positions.clone_from(&buffer.positions);
        }
        if out.normals.is_empty() {
            out.normals.clone_from(&buffer.normals);
        }
        if out.uvs.is_empty() {
            out.uvs.clone_from(&buffer.uvs);
        }
        if out.joints.is_empty() {
            out.joints.clone_from(&buffer.joints);
        }
        if out.weights.is_empty() {
            out.weights.clone_from(&buffer.weights);
        }
    }
    let vertices = out.positions.len();
    if out.normals.len() != vertices {
        out.normals.clear();
    }
    if out.uvs.len() != vertices {
        out.uvs.clear();
    }
    // Joints without weights, or either without a matching vertex count,
    // cannot be paired up and are dropped rather than half-applied.
    if out.joints.len() != vertices || out.weights.len() != vertices {
        out.joints.clear();
        out.weights.clear();
    }
    out
}

/// Decode every attribute of one vertex buffer.
fn decode_vertex_buffer(resource: &Resource<'_>, vb: &BufferDesc) -> Result<VertexBuffer> {
    let bytes = vb.raw(resource)?;
    let range = 0..vb.element_count;
    Ok(VertexBuffer {
        positions: read_vec3(&bytes, vb, "POSITION", range.clone()).unwrap_or_default(),
        normals: read_normals(&bytes, vb, range.clone()).unwrap_or_default(),
        uvs: read_uvs(&bytes, vb, range.clone()).unwrap_or_default(),
        joints: read_joints(&bytes, vb, range.clone()).unwrap_or_default(),
        weights: read_weights(&bytes, vb, range).unwrap_or_default(),
    })
}

/// Slice one draw call's indices out of its buffer and rebase them onto the
/// shared vertex buffer.
fn build_primitive(
    desc: &DrawCallDesc,
    buffer_index: usize,
    buffer: &VertexBuffer,
    all_indices: &[u32],
) -> Option<Primitive> {
    if desc.index_count == 0 || desc.vertex_count == 0 || buffer.positions.is_empty() {
        return None;
    }

    let end = desc.start_index.checked_add(desc.index_count)?;
    let slice = all_indices.get(desc.start_index..end)?;

    // Draw call indices are relative to its base vertex.
    let vertex_total = buffer.positions.len();
    let mut indices = Vec::with_capacity(slice.len());
    for &index in slice {
        let absolute = (index as usize).checked_add(desc.base_vertex)?;
        if absolute >= vertex_total {
            return None;
        }
        indices.push(absolute as u32);
    }

    Some(Primitive {
        material: desc.material.clone(),
        vertex_buffer: buffer_index,
        indices,
    })
}

fn decode_indices(resource: &Resource<'_>, ib: &BufferDesc) -> Result<Vec<u32>> {
    let bytes = ib.stored(resource)?;

    if ib.index_sequence {
        return Err(Source2Error::Meshopt {
            codec: "index",
            detail: "meshopt index sequences are not supported".into(),
        });
    }

    if ib.meshopt_compressed {
        return meshopt::decode_index_buffer(ib.element_count, bytes);
    }

    // Stored plainly: 16- or 32-bit indices.
    let mut out = Vec::with_capacity(ib.element_count);
    match ib.element_size {
        2 => {
            for chunk in bytes.chunks_exact(2).take(ib.element_count) {
                out.push(u32::from(u16::from_le_bytes([chunk[0], chunk[1]])));
            }
        }
        4 => {
            for chunk in bytes.chunks_exact(4).take(ib.element_count) {
                out.push(u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]));
            }
        }
        other => {
            return Err(Source2Error::Meshopt {
                codec: "index",
                detail: format!("unsupported index element size {other}"),
            })
        }
    }
    Ok(out)
}

fn vec3(value: &KvValue) -> Option<[f32; 3]> {
    let a = value.as_array()?;
    let get = |i: usize| -> Option<f32> {
        #[allow(clippy::cast_possible_truncation)]
        Some(a.get(i)?.as_f64()? as f32)
    };
    Some([get(0)?, get(1)?, get(2)?])
}

/// Read a three-float attribute for a vertex range.
fn read_vec3(
    data: &[u8],
    vb: &BufferDesc,
    semantic: &str,
    range: std::ops::Range<usize>,
) -> Option<Vec<[f32; 3]>> {
    let field = vb.field(semantic, 0)?;
    if field.format != VertexFormat::Rgb32Float {
        return None;
    }
    let mut out = Vec::with_capacity(range.len());
    for i in range {
        let base = i.checked_mul(vb.element_size)?.checked_add(field.offset)?;
        let bytes = data.get(base..base + 12)?;
        out.push([
            f32::from_le_bytes(bytes.get(0..4)?.try_into().ok()?),
            f32::from_le_bytes(bytes.get(4..8)?.try_into().ok()?),
            f32::from_le_bytes(bytes.get(8..12)?.try_into().ok()?),
        ]);
    }
    Some(out)
}

fn read_normals(
    data: &[u8],
    vb: &BufferDesc,
    range: std::ops::Range<usize>,
) -> Option<Vec<[f32; 3]>> {
    let field = vb.field("NORMAL", 0)?;
    match field.format {
        VertexFormat::Rgb32Float => read_vec3(data, vb, "NORMAL", range),
        VertexFormat::R32Uint => {
            let mut out = Vec::with_capacity(range.len());
            for i in range {
                let base = i.checked_mul(vb.element_size)?.checked_add(field.offset)?;
                let bytes = data.get(base..base + 4)?;
                out.push(decode_packed_normal(u32::from_le_bytes(
                    bytes.try_into().ok()?,
                )));
            }
            Some(out)
        }
        _ => None,
    }
}

fn read_uvs(data: &[u8], vb: &BufferDesc, range: std::ops::Range<usize>) -> Option<Vec<[f32; 2]>> {
    let field = vb.field("TEXCOORD", 0).or_else(|| {
        vb.fields
            .iter()
            .find(|f| f.semantic.eq_ignore_ascii_case("TEXCOORD"))
    })?;
    let mut out = Vec::with_capacity(range.len());
    for i in range {
        let base = i.checked_mul(vb.element_size)?.checked_add(field.offset)?;
        let size = field.format.size()?;
        let bytes = data.get(base..base + size)?;
        let uv = match field.format {
            VertexFormat::Rg32Float => [
                f32::from_le_bytes(bytes.get(0..4)?.try_into().ok()?),
                f32::from_le_bytes(bytes.get(4..8)?.try_into().ok()?),
            ],
            VertexFormat::Rg16Float => [
                half_to_f32(u16::from_le_bytes(bytes.get(0..2)?.try_into().ok()?)),
                half_to_f32(u16::from_le_bytes(bytes.get(2..4)?.try_into().ok()?)),
            ],
            VertexFormat::Rg16Unorm => [
                f32::from(u16::from_le_bytes(bytes.get(0..2)?.try_into().ok()?)) / 65535.0,
                f32::from(u16::from_le_bytes(bytes.get(2..4)?.try_into().ok()?)) / 65535.0,
            ],
            VertexFormat::Rg16Snorm => [
                f32::from(i16::from_le_bytes(bytes.get(0..2)?.try_into().ok()?)) / 32767.0,
                f32::from(i16::from_le_bytes(bytes.get(2..4)?.try_into().ok()?)) / 32767.0,
            ],
            VertexFormat::Rgba8Unorm => [
                f32::from(*bytes.first()?) / 255.0,
                f32::from(*bytes.get(1)?) / 255.0,
            ],
            _ => return None,
        };
        out.push(uv);
    }
    Some(out)
}

/// Read the joint index stream, whatever integer width it is stored in.
fn read_joints(
    data: &[u8],
    vb: &BufferDesc,
    range: std::ops::Range<usize>,
) -> Option<Vec<[u16; 4]>> {
    let field = vb.field("BLENDINDICES", 0)?;
    let size = field.format.size()?;
    let mut out = Vec::with_capacity(range.len());
    for i in range {
        let base = i.checked_mul(vb.element_size)?.checked_add(field.offset)?;
        let bytes = data.get(base..base + size)?;
        let joints = match field.format {
            // CS2 characters store four bytes; anything under 256 bones fits.
            VertexFormat::Rgba8Uint | VertexFormat::Rgba8Unorm => [
                u16::from(*bytes.first()?),
                u16::from(*bytes.get(1)?),
                u16::from(*bytes.get(2)?),
                u16::from(*bytes.get(3)?),
            ],
            VertexFormat::Rgba16Uint => [
                u16::from_le_bytes(bytes.get(0..2)?.try_into().ok()?),
                u16::from_le_bytes(bytes.get(2..4)?.try_into().ok()?),
                u16::from_le_bytes(bytes.get(4..6)?.try_into().ok()?),
                u16::from_le_bytes(bytes.get(6..8)?.try_into().ok()?),
            ],
            _ => return None,
        };
        out.push(joints);
    }
    Some(out)
}

/// Read the joint weight stream and normalise each vertex's four influences.
///
/// A vertex whose weights sum to zero is bound rigidly to its first joint,
/// which is what a single-influence vertex stored as `(255, 0, 0, 0)` means
/// anyway and what keeps a degenerate one from collapsing to the origin.
fn read_weights(
    data: &[u8],
    vb: &BufferDesc,
    range: std::ops::Range<usize>,
) -> Option<Vec<[f32; 4]>> {
    let field = vb
        .field("BLENDWEIGHT", 0)
        .or_else(|| vb.field("BLENDWEIGHTS", 0))?;
    let size = field.format.size()?;
    let mut out = Vec::with_capacity(range.len());
    for i in range {
        let base = i.checked_mul(vb.element_size)?.checked_add(field.offset)?;
        let bytes = data.get(base..base + size)?;
        let mut w = match field.format {
            VertexFormat::Rgba8Unorm | VertexFormat::Rgba8Uint => [
                f32::from(*bytes.first()?) / 255.0,
                f32::from(*bytes.get(1)?) / 255.0,
                f32::from(*bytes.get(2)?) / 255.0,
                f32::from(*bytes.get(3)?) / 255.0,
            ],
            VertexFormat::Rgba16Uint => [
                f32::from(u16::from_le_bytes(bytes.get(0..2)?.try_into().ok()?)) / 65535.0,
                f32::from(u16::from_le_bytes(bytes.get(2..4)?.try_into().ok()?)) / 65535.0,
                f32::from(u16::from_le_bytes(bytes.get(4..6)?.try_into().ok()?)) / 65535.0,
                f32::from(u16::from_le_bytes(bytes.get(6..8)?.try_into().ok()?)) / 65535.0,
            ],
            _ => return None,
        };
        let sum = w[0] + w[1] + w[2] + w[3];
        if sum > 1e-6 {
            for v in &mut w {
                *v /= sum;
            }
        } else {
            w = [1.0, 0.0, 0.0, 0.0];
        }
        out.push(w);
    }
    Some(out)
}

/// Expand an IEEE 754 binary16 value.
fn half_to_f32(bits: u16) -> f32 {
    let sign = u32::from(bits & 0x8000) << 16;
    let exponent = u32::from((bits >> 10) & 0x1f);
    let mantissa = u32::from(bits & 0x3ff);

    let value = if exponent == 0 {
        if mantissa == 0 {
            0
        } else {
            // Subnormal: normalise it.
            let mut e = -1i32;
            let mut m = mantissa;
            while m & 0x400 == 0 {
                m <<= 1;
                e -= 1;
            }
            let exp = (127 - 15 + e + 1) as u32;
            (exp << 23) | ((m & 0x3ff) << 13)
        }
    } else if exponent == 0x1f {
        0x7f80_0000 | (mantissa << 13)
    } else {
        ((exponent + 127 - 15) << 23) | (mantissa << 13)
    };

    f32::from_bits(sign | value)
}

/// Decode Valve's packed tangent frame into a unit normal.
///
/// The frame is octahedral: bits 12..21 and 22..31 hold the two 10-bit
/// coordinates, bit 0 the bitangent sign and bits 1..11 the tangent rotation.
/// Only the normal is recovered here.
#[must_use]
pub fn decode_packed_normal(packed: u32) -> [f32; 3] {
    #[allow(clippy::cast_precision_loss)]
    let coord = |bits: u32| (bits as f32 / 1023.0) * 2.0 - 1.0;

    let mut x = coord((packed >> 12) & 1023);
    let mut y = coord((packed >> 22) & 1023);
    let z = 1.0 - x.abs() - y.abs();

    // Fold the lower hemisphere back onto the octahedron.
    if z < 0.0 {
        let old_x = x;
        x = (1.0 - y.abs()) * if old_x >= 0.0 { 1.0 } else { -1.0 };
        y = (1.0 - old_x.abs()) * if y >= 0.0 { 1.0 } else { -1.0 };
    }

    let length = (x * x + y * y + z * z).sqrt();
    if length > 0.0 && length.is_finite() {
        [x / length, y / length, z / length]
    } else {
        [0.0, 0.0, 1.0]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_formats() {
        assert_eq!(VertexFormat::from_dxgi(6), VertexFormat::Rgb32Float);
        assert_eq!(VertexFormat::from_dxgi(42), VertexFormat::R32Uint);
        assert_eq!(VertexFormat::from_dxgi(999), VertexFormat::Unsupported(999));
        assert_eq!(VertexFormat::Rgb32Float.size(), Some(12));
        assert_eq!(VertexFormat::Rg32Float.size(), Some(8));
        assert_eq!(VertexFormat::Rg16Float.size(), Some(4));
        assert_eq!(VertexFormat::Unsupported(1).size(), None);
    }

    #[test]
    fn packed_normals_are_unit_length() {
        for packed in [0u32, 0xFFFF_FFFF, 0x1234_5678, 0x8000_0001, 0x7FFF_FFFF] {
            let n = decode_packed_normal(packed);
            let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
            assert!((len - 1.0).abs() < 1e-4, "{packed:#x} gave {n:?}");
        }
    }

    #[test]
    fn packed_normal_axis_directions() {
        // The centre of the 10-bit range maps to +Z.
        let up = decode_packed_normal((511 << 12) | (511 << 22));
        assert!(up[2] > 0.99, "{up:?}");
    }

    #[test]
    fn half_floats_round_trip_known_values() {
        assert!((half_to_f32(0x3C00) - 1.0).abs() < 1e-6);
        assert!((half_to_f32(0x4000) - 2.0).abs() < 1e-6);
        assert!((half_to_f32(0xC000) + 2.0).abs() < 1e-6);
        assert_eq!(half_to_f32(0x0000), 0.0);
        assert!(half_to_f32(0x7C00).is_infinite());
        // Smallest positive subnormal.
        assert!(half_to_f32(0x0001) > 0.0);
    }

    #[test]
    fn empty_resource_yields_no_meshes() {
        // A resource with no CTRL block is not an error.
        let bytes = crate::resource::tests::build_resource(1, &[(b"DATA", vec![0u8; 4])]);
        let resource = Resource::parse(&bytes).unwrap();
        assert!(Model::from_resource(&resource).unwrap().meshes.is_empty());
    }

    /// Build a model carrying only material groups, which is all
    /// `skin_materials` looks at.
    fn skinned(groups: &[(&str, &[&str])]) -> Model {
        Model {
            material_groups: groups
                .iter()
                .map(|(name, materials)| MaterialGroup {
                    name: (*name).to_string(),
                    materials: materials.iter().map(|m| (*m).to_string()).collect(),
                })
                .collect(),
            ..Model::default()
        }
    }

    #[test]
    fn a_named_skin_substitutes_positionally() {
        // The shape CS2 ships: group 0 is what the draw calls name, and every
        // other group lists the same slots in the same order.
        let model = skinned(&[
            ("default", &["bottle.vmat", "cork.vmat"]),
            ("dusty", &["bottle_dusty.vmat", "cork_dusty.vmat"]),
        ]);
        let pairs = model.skin_materials("dusty").expect("a substitution");
        assert_eq!(
            pairs,
            vec![
                ("bottle.vmat", "bottle_dusty.vmat"),
                ("cork.vmat", "cork_dusty.vmat"),
            ]
        );
        // Skin names are matched the way every other resource name is.
        assert_eq!(model.skin_materials("DUSTY"), model.skin_materials("dusty"));
    }

    #[test]
    fn the_default_skin_needs_no_substitution() {
        let model = skinned(&[
            ("default", &["bottle.vmat"]),
            ("dusty", &["bottle_dusty.vmat"]),
        ]);
        // CS2 writes "the default skin" three different ways.
        for skin in ["default", "0", ""] {
            assert!(
                model.skin_materials(skin).is_none(),
                "{skin} asked for a substitution"
            );
        }
        // So does a skin this model has never heard of: drawing it in its own
        // materials beats not drawing it.
        assert!(model.skin_materials("chartreuse").is_none());
        // And a model with no groups at all has nothing to substitute.
        assert!(Model::default().skin_materials("dusty").is_none());
    }

    #[test]
    fn slots_a_skin_leaves_alone_are_left_out() {
        // A group that changes only one of its materials yields one pair, not
        // an identity mapping for the rest.
        let model = skinned(&[
            ("default", &["body.vmat", "glass.vmat"]),
            ("new", &["body_new.vmat", "glass.vmat"]),
        ]);
        assert_eq!(
            model.skin_materials("new"),
            Some(vec![("body.vmat", "body_new.vmat")])
        );

        // A group identical to the default is no substitution at all.
        let same = skinned(&[("default", &["body.vmat"]), ("copy", &["body.vmat"])]);
        assert!(same.skin_materials("copy").is_none());

        // A short group cannot be lined up past its own end, and must not
        // panic trying: `zip` stops at the shorter of the two.
        let ragged = skinned(&[("default", &["a.vmat", "b.vmat"]), ("half", &["c.vmat"])]);
        assert_eq!(
            ragged.skin_materials("half"),
            Some(vec![("a.vmat", "c.vmat")])
        );
    }
}
