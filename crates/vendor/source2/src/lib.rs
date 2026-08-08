//! Decoding of Source 2 compiled resources.
//!
//! Two layers:
//!
//! * [`Resource`] parses the outer container of a `*_c` file: the header and
//!   the block directory (`DATA`, `RERL`, `RED2`, `NTRO`, `VBIB`, ...). Every
//!   block is addressable as raw bytes regardless of whether its contents are
//!   understood.
//! * [`kv3`] decodes binary KeyValues3 payloads, which is how most modern
//!   Source 2 blocks store their contents. All container versions 0 through 5
//!   are supported, with LZ4 and zstd decompression.
//!
//! [`rerl`] decodes the external reference list, and [`ResourceType`]
//! classifies a resource by its file extension. On top of those sit the typed
//! decoders: [`world`] and [`mesh`] for geometry, [`material`] for `vmat_c`,
//! [`entity`] for the `vents_c` entity lumps that place spawn points, and
//! [`texture`] for `vtex_c` — the one remaining struct-format payload,
//! which decodes to plain RGBA8 whatever block compression it arrived in.
//!
//! # Robustness
//!
//! Every parser is bounds-checked, rejects negative and absurd sizes, and
//! limits recursion depth. Malformed input produces a [`Source2Error`], never a
//! panic — the test suite includes truncation and bit-flip sweeps to keep that
//! true.
//!
//! # Example
//!
//! ```no_run
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let bytes = std::fs::read("world.vwrld_c")?;
//! let resource = source2::Resource::parse(&bytes)?;
//!
//! for block in &resource.blocks {
//!     println!("{} ({} bytes)", block.fourcc, block.size);
//! }
//!
//! for reference in resource.external_references()? {
//!     println!("depends on {}", reference.name);
//! }
//!
//! if let Some(Ok(doc)) = resource.data_kv3() {
//!     println!("KV3 v{}, root is {}", doc.version, doc.root.type_name());
//! }
//! # Ok(())
//! # }
//! ```

#![deny(missing_docs)]

pub mod entity;
mod error;
pub mod kv3;
pub mod material;
pub mod mesh;
pub mod meshopt;
pub mod nav;
pub mod nm;
pub mod phys;
mod reader;
pub mod rerl;
mod resource;
mod restype;
pub mod skeleton;
pub mod texture;
pub mod world;

pub use entity::{Entity, EntityLump, EntityValue};
pub use error::{Result, Source2Error};
pub use kv3::{Kv3Document, Kv3Id, KvFlag, KvValue};
pub use material::{AlphaMode, Material};
pub use mesh::{Mesh, Model, Primitive, VertexBuffer, VertexFormat};
pub use nav::{NavArea, NavConnection, NavCrossing, NavGenParams, NavHullParams, NavMesh};
pub use nm::{NmClip, NmSkeleton};
pub use phys::{CollisionAttribute, PhysAggregate, PhysShape, ShapeKind};
pub use rerl::ExternalReference;
pub use resource::{BlockRef, FourCc, Resource, KNOWN_HEADER_VERSION};
pub use restype::ResourceType;
pub use skeleton::{Bone, BoneTransform, Pose, Skeleton};
pub use texture::{Image, Texture, TextureCodec, TextureFormat};
pub use world::{AggregateSceneObject, SceneObject, Transform, World, WorldNode};
