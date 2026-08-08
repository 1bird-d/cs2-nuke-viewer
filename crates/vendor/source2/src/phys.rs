//! Physics shapes: the collision a map has that its renderer never draws.
//!
//! Every milestone so far has read the blocks that describe *appearance* —
//! vertex buffers, materials, textures. A Source 2 resource can also carry a
//! `PHYS` block, which describes the shape the physics engine collides against,
//! and for some resources that is the *only* thing in the file. CS2's invisible
//! player-clip brushes are exactly that: a compiled model with no render mesh
//! at all, whose whole content is a convex hull nobody was ever meant to see.
//!
//! `PHYS` is plain binary KV3, so nothing new has to be decoded to reach it —
//! only interpreted. Following ValveResourceFormat's `PhysAggregateData`, the
//! block is:
//!
//! ```text
//! m_collisionAttributes[]   what each shape collides *as* and *with*
//! m_parts[]
//!   m_rnShape
//!     m_spheres[]  m_capsules[]        analytic shapes, not read here
//!     m_hulls[]                        convex pieces, as a half-edge mesh
//!     m_meshes[]                       triangle soups, for concave collision
//! ```
//!
//! # Two shape encodings
//!
//! A **mesh** is the easy one: `m_Vertices` is a run of three-float positions
//! and `m_Triangles` a run of three 32-bit indices, both as opaque byte
//! strings.
//!
//! A **hull** is a half-edge structure, which is how a convex solid is stored
//! when the engine needs its faces, its planes and its adjacency rather than
//! just its triangles:
//!
//! | Field | Layout | Meaning |
//! | --- | --- | --- |
//! | `m_VertexPositions` | 12 bytes each | the corners |
//! | `m_Edges` | 4 bytes each | `next`, `twin`, `origin`, `face` |
//! | `m_Faces` | 1 byte each | the half-edge a face starts at |
//! | `m_Vertices` | 1 byte each | a half-edge leaving each corner |
//!
//! A face is recovered by starting at its half-edge and following `next` until
//! the walk returns to where it began, collecting each half-edge's `origin`.
//! **Every index in that structure is a single byte**, which caps a hull at 256
//! corners and 256 half-edges — small convex pieces are the whole point of the
//! representation — and which is why the face walk range-checks rather than
//! trusting: a file that says otherwise is corrupt, and a corrupt hull should
//! cost its own geometry and nothing else.
//!
//! Older resources put the corner positions in `m_Vertices` as three-float
//! triples and have no `m_VertexPositions` at all, so that is the fallback.
//!
//! # What "blocks a player" means
//!
//! Not a flag on the shape. Each shape carries `m_nCollisionAttributeIndex`
//! into `m_collisionAttributes`, and an attribute names the groups it interacts
//! *as*. CS2's own name for the group that stops a player and nothing else is
//! `playerclip`, and it is what [`CollisionAttribute::blocks_player`] looks
//! for. A map's whole invisible-wall set is the shapes whose attribute declares
//! it — sixteen hulls and twelve thousand triangles on de_dust2 — and the
//! neighbouring attributes are the reason a name test beats a guess: the same
//! file also carries `csgo_grenadeclip`, `sky` and one whose `InteractExclude`
//! is `player`, none of which stop anybody walking.

use crate::error::{Result, Source2Error};
use crate::kv3::KvValue;
use crate::resource::{FourCc, Resource};

/// The block holding an aggregate's physics shapes.
pub const PHYS: FourCc = FourCc::new(b"PHYS");

/// The collision group CS2 gives geometry that stops a player and is not drawn.
pub const PLAYER_CLIP_GROUP: &str = "playerclip";

/// Groups whose presence says a shape stops a line of sight outright.
///
/// `solid` is the plain statement of it. `blocklos` is CS2's own name for a
/// nodraw brush placed to break sight lines and nothing else — literally
/// "block line of sight" — so a shape declaring it is a sight blocker by the
/// mapper's explicit intent even though it is not ordinary world. Both are
/// rare: twelve triangles each on de_nuke and de_ancient, and absent from the
/// other six maps.
pub const SIGHT_BLOCKING_GROUPS: [&str; 2] = ["solid", "blocklos"];

/// Bytes per hull or mesh vertex: three little-endian `f32`.
const VERTEX_STRIDE: usize = 12;
/// Bytes per triangle: three little-endian `i32`.
const TRIANGLE_STRIDE: usize = 12;
/// Bytes per half-edge: `next`, `twin`, `origin`, `face`, one byte each.
const EDGE_STRIDE: usize = 4;

/// What one shape collides as, and what it refuses to collide with.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CollisionAttribute {
    /// `m_CollisionGroupString`, e.g. `Default` or `ConditionallySolid`.
    pub group: String,
    /// `m_InteractAsStrings`: the groups this shape acts as.
    pub interact_as: Vec<String>,
    /// `m_InteractWithStrings`.
    pub interact_with: Vec<String>,
    /// `m_InteractExcludeStrings`: what it explicitly does not stop.
    pub interact_exclude: Vec<String>,
}

impl CollisionAttribute {
    /// Whether a shape with this attribute is CS2 player clipping.
    ///
    /// The test is on `InteractAs` alone. `InteractExclude` naming `player` is
    /// the *opposite* case — geometry a player walks through — and is already
    /// excluded by this attribute not declaring the group in the first place.
    #[must_use]
    pub fn blocks_player(&self) -> bool {
        self.interact_as
            .iter()
            .any(|g| g.eq_ignore_ascii_case(PLAYER_CLIP_GROUP))
    }

    /// Whether a shape with this attribute stops a line of sight.
    ///
    /// This is the **opposite selection** to [`Self::blocks_player`], and the
    /// distinction matters: player clipping is invisible, so it stops a player
    /// and stops no light at all. A field-of-view volume that ended at a
    /// `playerclip` brush would be cut off in mid-air across every doorway on
    /// the map.
    ///
    /// Two things qualify.
    ///
    /// * **Ordinary solid world**: an attribute that names no interaction
    ///   groups and excludes nothing. Declaring no group is what "this is just
    ///   the map" looks like in the file — every named group in CS2's table is
    ///   a *departure* from being a wall (`playerclip`, `csgo_grenadeclip`,
    ///   `passbullets`, `window`, `sky`, `ladder`, `navclip`), so the shapes
    ///   with nothing to say are the walls. Excluding nothing is what separates
    ///   it from the `exclude [player]` set, which is geometry a player walks
    ///   straight through.
    /// * **An explicit sight blocker**: an attribute naming any of
    ///   [`SIGHT_BLOCKING_GROUPS`].
    ///
    /// On all eight shipped maps the first clause selects exactly one attribute
    /// per map, and it is in every case the one whose `m_CollisionGroupString`
    /// is `Default` — 122,269 triangles on de_mirage through 1,651,032 on
    /// de_inferno. The group name is deliberately **not** part of the test:
    /// what makes a shape a wall is what it interacts as, and the name is a
    /// label that corroborates the reading rather than one that drives it.
    #[must_use]
    pub fn blocks_sight(&self) -> bool {
        let names_a_blocker = self.interact_as.iter().any(|g| {
            SIGHT_BLOCKING_GROUPS
                .iter()
                .any(|w| g.eq_ignore_ascii_case(w))
        });
        names_a_blocker || (self.interact_as.is_empty() && self.interact_exclude.is_empty())
    }
}

/// Which of the two encodings a shape came out of, for reporting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShapeKind {
    /// A convex piece, stored as a half-edge solid.
    Hull,
    /// A triangle soup.
    Mesh,
}

/// One collision shape, triangulated.
#[derive(Debug, Clone, PartialEq)]
pub struct PhysShape {
    /// Which encoding it came from.
    pub kind: ShapeKind,
    /// Index into [`PhysAggregate::attributes`], or `usize::MAX` when the file
    /// named one that does not exist.
    pub attribute: usize,
    /// Corner positions, in the resource's own frame and units.
    pub positions: Vec<[f32; 3]>,
    /// Triangles, as indices into `positions`.
    pub triangles: Vec<[u32; 3]>,
}

impl PhysShape {
    /// Whether this shape's attribute says it stops a player.
    #[must_use]
    pub fn blocks_player(&self, attributes: &[CollisionAttribute]) -> bool {
        attributes
            .get(self.attribute)
            .is_some_and(CollisionAttribute::blocks_player)
    }

    /// Whether this shape's attribute says it stops a line of sight.
    #[must_use]
    pub fn blocks_sight(&self, attributes: &[CollisionAttribute]) -> bool {
        attributes
            .get(self.attribute)
            .is_some_and(CollisionAttribute::blocks_sight)
    }
}

/// A resource's `PHYS` block: its collision attributes and every shape.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PhysAggregate {
    /// The attribute table shapes index into.
    pub attributes: Vec<CollisionAttribute>,
    /// Every hull and mesh across every part, flattened.
    pub shapes: Vec<PhysShape>,
}

impl PhysAggregate {
    /// Decode a resource's `PHYS` block.
    ///
    /// # Errors
    ///
    /// Fails when the resource has no `PHYS` block, or it is not binary KV3, or
    /// the KV3 payload does not decode. A shape *inside* a well-formed block
    /// that is malformed is dropped rather than failing the whole decode: one
    /// bad hull should cost its own geometry and nothing else.
    pub fn from_resource(resource: &Resource<'_>) -> Result<Self> {
        let doc = resource
            .kv3_block(PHYS)
            .ok_or(Source2Error::MissingBlock { fourcc: "PHYS" })??;
        Ok(Self::from_kv(&doc.root))
    }

    /// Whether a resource carries a `PHYS` block at all.
    #[must_use]
    pub fn present_in(resource: &Resource<'_>) -> bool {
        resource.has_block(PHYS)
    }

    /// Read an already-decoded `PHYS` document.
    #[must_use]
    pub fn from_kv(root: &KvValue) -> Self {
        let attributes = root
            .get("m_collisionAttributes")
            .and_then(KvValue::as_array)
            .unwrap_or_default()
            .iter()
            .map(attribute)
            .collect();

        let mut shapes = Vec::new();
        for part in root
            .get("m_parts")
            .and_then(KvValue::as_array)
            .unwrap_or_default()
        {
            let Some(shape) = part.get("m_rnShape") else {
                continue;
            };
            for hull in shape
                .get("m_hulls")
                .and_then(KvValue::as_array)
                .unwrap_or_default()
            {
                if let Some(decoded) = decode_hull(hull) {
                    shapes.push(decoded);
                }
            }
            for mesh in shape
                .get("m_meshes")
                .and_then(KvValue::as_array)
                .unwrap_or_default()
            {
                if let Some(decoded) = decode_mesh(mesh) {
                    shapes.push(decoded);
                }
            }
        }

        Self { attributes, shapes }
    }

    /// Every shape whose attribute declares [`PLAYER_CLIP_GROUP`].
    pub fn player_clip_shapes(&self) -> impl Iterator<Item = &PhysShape> {
        self.shapes
            .iter()
            .filter(|s| s.blocks_player(&self.attributes))
    }

    /// Every shape whose attribute says it stops a line of sight.
    ///
    /// See [`CollisionAttribute::blocks_sight`]. This is the map's ordinary
    /// solid world, and it is what a field-of-view volume is cast against.
    pub fn sight_blocking_shapes(&self) -> impl Iterator<Item = &PhysShape> {
        self.shapes
            .iter()
            .filter(|s| s.blocks_sight(&self.attributes))
    }

    /// Total triangles across every shape.
    #[must_use]
    pub fn triangles(&self) -> usize {
        self.shapes.iter().map(|s| s.triangles.len()).sum()
    }
}

/// Read one entry of `m_collisionAttributes`.
fn attribute(value: &KvValue) -> CollisionAttribute {
    CollisionAttribute {
        group: value
            .get("m_CollisionGroupString")
            .and_then(KvValue::as_str)
            .unwrap_or_default()
            .to_string(),
        interact_as: string_list(value, "m_InteractAsStrings"),
        interact_with: string_list(value, "m_InteractWithStrings"),
        interact_exclude: string_list(value, "m_InteractExcludeStrings"),
    }
}

fn string_list(value: &KvValue, key: &str) -> Vec<String> {
    value
        .get(key)
        .and_then(KvValue::as_array)
        .unwrap_or_default()
        .iter()
        .filter_map(|v| v.as_str().map(str::to_string))
        .collect()
}

/// The shape's index into the attribute table.
///
/// A missing or negative index becomes [`usize::MAX`], which no attribute table
/// can contain, so such a shape is simply never selected by an attribute test
/// rather than silently taking attribute zero — which on every map here is the
/// ordinary solid world.
fn attribute_index(value: &KvValue) -> usize {
    match value
        .get("m_nCollisionAttributeIndex")
        .and_then(KvValue::as_i64)
    {
        Some(i) if i >= 0 => usize::try_from(i).unwrap_or(usize::MAX),
        _ => usize::MAX,
    }
}

/// Read a run of three-float vectors out of an opaque byte string.
fn positions(bytes: &[u8]) -> Vec<[f32; 3]> {
    bytes
        .chunks_exact(VERTEX_STRIDE)
        .map(|c| {
            let f = |i: usize| f32::from_le_bytes([c[i], c[i + 1], c[i + 2], c[i + 3]]);
            [f(0), f(4), f(8)]
        })
        .collect()
}

/// One half-edge: the next around its face, its twin, the corner it leaves, and
/// the face it belongs to. Only `next` and `origin` are needed to walk a face,
/// but the record is fixed width so all four are read.
#[derive(Debug, Clone, Copy)]
struct HalfEdge {
    next: u8,
    origin: u8,
}

/// Decode one `m_hulls` entry into triangles.
///
/// Returns `None` when the hull has no usable geometry at all. A hull whose
/// *some* faces are malformed keeps the rest.
fn decode_hull(value: &KvValue) -> Option<PhysShape> {
    let hull = value.get("m_Hull")?;

    // `m_VertexPositions` is the current layout. Older resources put the
    // corners in `m_Vertices` instead, where the current layout puts one byte
    // per corner naming a half-edge — so the two are told apart by which field
    // is present, not by measuring one of them.
    let positions = match hull.get("m_VertexPositions").and_then(KvValue::as_bytes) {
        Some(bytes) => positions(bytes),
        None => positions(hull.get("m_Vertices").and_then(KvValue::as_bytes)?),
    };
    if positions.is_empty() {
        return None;
    }

    let edges: Vec<HalfEdge> = hull
        .get("m_Edges")
        .and_then(KvValue::as_bytes)?
        .chunks_exact(EDGE_STRIDE)
        .map(|c| HalfEdge {
            next: c[0],
            origin: c[2],
        })
        .collect();
    let faces = hull.get("m_Faces").and_then(KvValue::as_bytes)?;

    let mut triangles = Vec::new();
    for &start in faces {
        for corner in face_polygon(start, &edges, positions.len()) {
            triangles.push(corner);
        }
    }
    if triangles.is_empty() {
        return None;
    }

    Some(PhysShape {
        kind: ShapeKind::Hull,
        attribute: attribute_index(value),
        positions,
        triangles,
    })
}

/// Walk one face of a hull and fan it into triangles.
///
/// The walk follows `next` from the face's own half-edge until it returns to
/// where it started. It is bounded by the edge count regardless, because a
/// corrupt `next` chain can be a cycle that never reaches the start, and an
/// unbounded walk on shipped data is not a risk worth taking.
fn face_polygon(start: u8, edges: &[HalfEdge], vertex_count: usize) -> Vec<[u32; 3]> {
    let mut corners: Vec<u32> = Vec::new();
    let mut current = usize::from(start);
    for _ in 0..edges.len() {
        let Some(edge) = edges.get(current) else {
            return Vec::new();
        };
        let origin = usize::from(edge.origin);
        if origin >= vertex_count {
            return Vec::new();
        }
        corners.push(u32::try_from(origin).unwrap_or(0));
        current = usize::from(edge.next);
        if current == usize::from(start) {
            break;
        }
    }
    if corners.len() < 3 {
        return Vec::new();
    }

    // A hull face is convex by construction, which is what makes a fan from its
    // first corner correct rather than merely plausible.
    (1..corners.len() - 1)
        .map(|i| [corners[0], corners[i], corners[i + 1]])
        .collect()
}

/// Decode one `m_meshes` entry into triangles.
fn decode_mesh(value: &KvValue) -> Option<PhysShape> {
    let mesh = value.get("m_Mesh")?;
    let positions = positions(mesh.get("m_Vertices").and_then(KvValue::as_bytes)?);
    if positions.is_empty() {
        return None;
    }

    let count = positions.len();
    let triangles: Vec<[u32; 3]> = mesh
        .get("m_Triangles")
        .and_then(KvValue::as_bytes)?
        .chunks_exact(TRIANGLE_STRIDE)
        .filter_map(|c| {
            let i = |k: usize| i32::from_le_bytes([c[k], c[k + 1], c[k + 2], c[k + 3]]);
            let t = [i(0), i(4), i(8)];
            // An index outside the vertex list would be a triangle nobody can
            // draw, so it is dropped rather than clamped into a sliver.
            t.iter()
                .all(|&v| v >= 0 && (v as usize) < count)
                .then(|| [t[0] as u32, t[1] as u32, t[2] as u32])
        })
        .collect();
    if triangles.is_empty() {
        return None;
    }

    Some(PhysShape {
        kind: ShapeKind::Mesh,
        attribute: attribute_index(value),
        positions,
        triangles,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kv_object(pairs: Vec<(&str, KvValue)>) -> KvValue {
        KvValue::Object(pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
    }

    fn bytes_of(values: &[f32]) -> KvValue {
        KvValue::Binary(values.iter().flat_map(|v| v.to_le_bytes()).collect())
    }

    fn indices_of(values: &[i32]) -> KvValue {
        KvValue::Binary(values.iter().flat_map(|v| v.to_le_bytes()).collect())
    }

    /// A unit square in the XY plane as a mesh shape, two triangles.
    fn square_mesh(attribute: i64) -> KvValue {
        kv_object(vec![
            ("m_nCollisionAttributeIndex", KvValue::Int(attribute)),
            (
                "m_Mesh",
                kv_object(vec![
                    (
                        "m_Vertices",
                        bytes_of(&[
                            0.0, 0.0, 0.0, //
                            1.0, 0.0, 0.0, //
                            1.0, 1.0, 0.0, //
                            0.0, 1.0, 0.0,
                        ]),
                    ),
                    ("m_Triangles", indices_of(&[0, 1, 2, 0, 2, 3])),
                ]),
            ),
        ])
    }

    /// A hull with one square face, built from four half-edges that chain round
    /// it. `next` points at the following edge and `origin` at the corner it
    /// leaves, which is the whole of what a face walk needs.
    fn square_hull(attribute: i64) -> KvValue {
        let edges = KvValue::Binary(vec![
            // next, twin, origin, face
            1, 0, 0, 0, //
            2, 0, 1, 0, //
            3, 0, 2, 0, //
            0, 0, 3, 0, //
        ]);
        kv_object(vec![
            ("m_nCollisionAttributeIndex", KvValue::Int(attribute)),
            (
                "m_Hull",
                kv_object(vec![
                    (
                        "m_VertexPositions",
                        bytes_of(&[
                            0.0, 0.0, 0.0, //
                            1.0, 0.0, 0.0, //
                            1.0, 1.0, 0.0, //
                            0.0, 1.0, 0.0,
                        ]),
                    ),
                    ("m_Edges", edges),
                    ("m_Faces", KvValue::Binary(vec![0])),
                    ("m_Vertices", KvValue::Binary(vec![0, 1, 2, 3])),
                ]),
            ),
        ])
    }

    fn aggregate(hulls: Vec<KvValue>, meshes: Vec<KvValue>, attrs: Vec<KvValue>) -> KvValue {
        kv_object(vec![
            ("m_collisionAttributes", KvValue::Array(attrs)),
            (
                "m_parts",
                KvValue::Array(vec![kv_object(vec![(
                    "m_rnShape",
                    kv_object(vec![
                        ("m_hulls", KvValue::Array(hulls)),
                        ("m_meshes", KvValue::Array(meshes)),
                    ]),
                )])]),
            ),
        ])
    }

    fn attr(group: &str, interact_as: &[&str], exclude: &[&str]) -> KvValue {
        let list = |names: &[&str]| {
            KvValue::Array(
                names
                    .iter()
                    .map(|n| KvValue::String((*n).to_string()))
                    .collect(),
            )
        };
        kv_object(vec![
            ("m_CollisionGroupString", KvValue::String(group.to_string())),
            ("m_InteractAsStrings", list(interact_as)),
            ("m_InteractExcludeStrings", list(exclude)),
        ])
    }

    #[test]
    fn a_mesh_shape_keeps_its_own_triangles() {
        let doc = aggregate(vec![], vec![square_mesh(0)], vec![]);
        let phys = PhysAggregate::from_kv(&doc);
        assert_eq!(phys.shapes.len(), 1);
        let shape = &phys.shapes[0];
        assert_eq!(shape.kind, ShapeKind::Mesh);
        assert_eq!(shape.positions.len(), 4);
        assert_eq!(shape.triangles, vec![[0, 1, 2], [0, 2, 3]]);
        assert_eq!(phys.triangles(), 2);
    }

    #[test]
    fn a_hull_face_is_walked_round_and_fanned() {
        let doc = aggregate(vec![square_hull(0)], vec![], vec![]);
        let phys = PhysAggregate::from_kv(&doc);
        assert_eq!(phys.shapes.len(), 1);
        let shape = &phys.shapes[0];
        assert_eq!(shape.kind, ShapeKind::Hull);
        // Four corners walked in `next` order become two triangles, and the
        // order is the face's own winding rather than the vertex list's.
        assert_eq!(shape.triangles, vec![[0, 1, 2], [0, 2, 3]]);
    }

    #[test]
    fn an_n_gon_face_fans_into_n_minus_two_triangles() {
        for corners in [3u8, 4, 5, 8] {
            let mut edges = Vec::new();
            for i in 0..corners {
                edges.extend_from_slice(&[(i + 1) % corners, 0, i, 0]);
            }
            let positions: Vec<f32> = (0..corners)
                .flat_map(|i| [f32::from(i), f32::from(i) * 2.0, 0.0])
                .collect();
            let hull = kv_object(vec![
                ("m_nCollisionAttributeIndex", KvValue::Int(0)),
                (
                    "m_Hull",
                    kv_object(vec![
                        ("m_VertexPositions", bytes_of(&positions)),
                        ("m_Edges", KvValue::Binary(edges)),
                        ("m_Faces", KvValue::Binary(vec![0])),
                    ]),
                ),
            ]);
            let phys = PhysAggregate::from_kv(&aggregate(vec![hull], vec![], vec![]));
            assert_eq!(
                phys.triangles(),
                usize::from(corners) - 2,
                "{corners}-gon face"
            );
        }
    }

    #[test]
    fn the_older_layout_puts_the_corners_in_m_vertices() {
        // No `m_VertexPositions` at all: the corners are three-float triples in
        // `m_Vertices` instead, which is what earlier Source 2 resources hold.
        let hull = kv_object(vec![
            ("m_nCollisionAttributeIndex", KvValue::Int(0)),
            (
                "m_Hull",
                kv_object(vec![
                    (
                        "m_Vertices",
                        bytes_of(&[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 1.0, 0.0]),
                    ),
                    (
                        "m_Edges",
                        KvValue::Binary(vec![1, 0, 0, 0, 2, 0, 1, 0, 0, 0, 2, 0]),
                    ),
                    ("m_Faces", KvValue::Binary(vec![0])),
                ]),
            ),
        ]);
        let phys = PhysAggregate::from_kv(&aggregate(vec![hull], vec![], vec![]));
        assert_eq!(phys.shapes.len(), 1);
        assert_eq!(phys.shapes[0].positions.len(), 3);
        assert_eq!(phys.shapes[0].triangles, vec![[0, 1, 2]]);
    }

    #[test]
    fn playerclip_is_read_off_the_attribute_rather_than_the_shape() {
        let attrs = vec![
            attr("Default", &[], &[]),
            attr("ConditionallySolid", &["npcclip", "playerclip"], &[]),
            attr("ConditionallySolid", &["csgo_grenadeclip"], &[]),
            // The opposite case: geometry a player walks straight through.
            attr("Default", &[], &["player"]),
        ];
        let doc = aggregate(
            vec![square_hull(1)],
            vec![square_mesh(0), square_mesh(2), square_mesh(3)],
            attrs,
        );
        let phys = PhysAggregate::from_kv(&doc);
        assert_eq!(phys.shapes.len(), 4);

        let clip: Vec<&PhysShape> = phys.player_clip_shapes().collect();
        assert_eq!(clip.len(), 1, "only the playerclip attribute qualifies");
        assert_eq!(clip[0].kind, ShapeKind::Hull);

        assert!(phys.attributes[1].blocks_player());
        assert!(!phys.attributes[0].blocks_player());
        assert!(!phys.attributes[2].blocks_player(), "a grenade clip is not");
        assert!(
            !phys.attributes[3].blocks_player(),
            "excluding the player is the opposite of clipping it"
        );
    }

    /// A decoded attribute, for the tests that are about the selection rules
    /// rather than about reading them out of a document.
    fn decoded(group: &str, as_: &[&str], exclude: &[&str]) -> CollisionAttribute {
        let list = |v: &[&str]| v.iter().map(|s| (*s).to_string()).collect();
        CollisionAttribute {
            group: group.to_string(),
            interact_as: list(as_),
            interact_with: Vec::new(),
            interact_exclude: list(exclude),
        }
    }

    #[test]
    fn what_stops_a_player_and_what_stops_a_sight_line_are_different_sets() {
        // Every attribute de_dust2's world collision model actually declares,
        // in the order the file lists them.
        let attrs = [
            decoded("Default", &[], &[]),
            decoded("ConditionallySolid", &["passbullets"], &[]),
            decoded("ConditionallySolid", &["npcclip", "playerclip"], &[]),
            decoded("default", &[], &["player"]),
            decoded("ConditionallySolid", &["csgo_grenadeclip"], &[]),
            decoded("conditionallysolid", &["sky"], &[]),
        ];
        let attrs = attrs.as_slice();
        let sight: Vec<bool> = attrs.iter().map(CollisionAttribute::blocks_sight).collect();
        let player: Vec<bool> = attrs
            .iter()
            .map(CollisionAttribute::blocks_player)
            .collect();

        // Exactly one attribute is the solid world, and it is not the one that
        // stops a player: player clipping is invisible and stops no light.
        assert_eq!(sight, vec![true, false, false, false, false, false]);
        assert_eq!(player, vec![false, false, true, false, false, false]);
        assert!(
            !attrs[2].blocks_sight(),
            "a field of view must pass straight through player clipping"
        );
        // Nor is the sky an occluder: you can see it.
        assert!(!attrs[5].blocks_sight());
        // Nor is geometry a player walks through, which is the pair that only
        // the exclude list tells apart — both name no interaction group.
        assert!(attrs[0].interact_as.is_empty() && attrs[3].interact_as.is_empty());
        assert!(attrs[0].blocks_sight() && !attrs[3].blocks_sight());
    }

    #[test]
    fn a_brush_placed_to_break_a_sight_line_counts_even_though_it_names_a_group() {
        // de_nuke ships one of these and de_ancient one without the `blocklos`:
        // nodraw brushwork whose whole purpose is stopping vision.
        let nuke = decoded(
            "ConditionallySolid",
            &["blocklight", "blocklos", "blocksound", "solid"],
            &[],
        );
        let ancient = decoded(
            "ConditionallySolid",
            &["blocklight", "blocksound", "solid"],
            &[],
        );
        assert!(nuke.blocks_sight());
        assert!(ancient.blocks_sight());
        // Case is not part of it, as with every other group name here.
        assert!(decoded("x", &["BlockLOS"], &[]).blocks_sight());
        assert!(decoded("x", &["SOLID"], &[]).blocks_sight());
        // But blocking light or sound alone is not blocking sight.
        assert!(!decoded("x", &["blocklight", "blocksound"], &[]).blocks_sight());
    }

    #[test]
    fn the_sight_selection_reaches_the_shapes_through_their_attribute() {
        let doc = aggregate(
            vec![square_hull(0)],
            vec![square_mesh(1), square_mesh(2)],
            vec![
                attr("Default", &[], &[]),
                attr("ConditionallySolid", &["npcclip", "playerclip"], &[]),
                attr("Default", &[], &["player"]),
            ],
        );
        let phys = PhysAggregate::from_kv(&doc);
        assert_eq!(phys.shapes.len(), 3);
        let solid: Vec<&PhysShape> = phys.sight_blocking_shapes().collect();
        assert_eq!(solid.len(), 1);
        assert_eq!(solid[0].kind, ShapeKind::Hull);
        // And the two selections do not overlap on this document.
        assert_eq!(phys.player_clip_shapes().count(), 1);
        assert!(!solid[0].blocks_player(&phys.attributes));
    }

    #[test]
    fn a_shape_naming_no_attribute_stops_neither_a_player_nor_a_sight_line() {
        // `usize::MAX` indexes no attribute, so both tests must decline it
        // rather than either falling through to attribute zero.
        let shape = PhysShape {
            kind: ShapeKind::Mesh,
            attribute: usize::MAX,
            positions: vec![],
            triangles: vec![],
        };
        let attrs = vec![decoded("Default", &[], &[])];
        assert!(!shape.blocks_sight(&attrs));
        assert!(!shape.blocks_player(&attrs));
    }

    #[test]
    fn the_group_name_is_matched_without_regard_to_case() {
        let doc = aggregate(
            vec![],
            vec![square_mesh(0)],
            vec![attr("conditionallysolid", &["PlayerClip"], &[])],
        );
        let phys = PhysAggregate::from_kv(&doc);
        assert_eq!(phys.player_clip_shapes().count(), 1);
    }

    #[test]
    fn a_shape_naming_no_attribute_is_never_selected() {
        // A missing index must not fall through to attribute zero, which on
        // every shipped map is the ordinary solid world.
        let mesh = kv_object(vec![(
            "m_Mesh",
            kv_object(vec![
                (
                    "m_Vertices",
                    bytes_of(&[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 1.0, 0.0]),
                ),
                ("m_Triangles", indices_of(&[0, 1, 2])),
            ]),
        )]);
        let doc = aggregate(
            vec![],
            vec![mesh],
            vec![attr("ConditionallySolid", &["playerclip"], &[])],
        );
        let phys = PhysAggregate::from_kv(&doc);
        assert_eq!(phys.shapes.len(), 1);
        assert_eq!(phys.shapes[0].attribute, usize::MAX);
        assert_eq!(phys.player_clip_shapes().count(), 0);
    }

    #[test]
    fn out_of_range_indices_cost_their_own_geometry_and_no_more() {
        // A triangle naming a corner that does not exist is dropped; the good
        // one beside it survives.
        let mesh = kv_object(vec![
            ("m_nCollisionAttributeIndex", KvValue::Int(0)),
            (
                "m_Mesh",
                kv_object(vec![
                    (
                        "m_Vertices",
                        bytes_of(&[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 1.0, 0.0]),
                    ),
                    ("m_Triangles", indices_of(&[0, 1, 2, 0, 1, 99, 0, -1, 2])),
                ]),
            ),
        ]);
        let phys = PhysAggregate::from_kv(&aggregate(vec![], vec![mesh], vec![]));
        assert_eq!(phys.triangles(), 1);

        // Likewise a hull half-edge naming a corner past the end: that face is
        // lost, not the decode.
        let hull = kv_object(vec![
            ("m_nCollisionAttributeIndex", KvValue::Int(0)),
            (
                "m_Hull",
                kv_object(vec![
                    ("m_VertexPositions", bytes_of(&[0.0, 0.0, 0.0])),
                    (
                        "m_Edges",
                        KvValue::Binary(vec![1, 0, 0, 0, 2, 0, 40, 0, 0, 0, 0, 0]),
                    ),
                    ("m_Faces", KvValue::Binary(vec![0])),
                ]),
            ),
        ]);
        assert!(
            PhysAggregate::from_kv(&aggregate(vec![hull], vec![], vec![]))
                .shapes
                .is_empty()
        );
    }

    #[test]
    fn a_next_chain_that_never_comes_back_terminates() {
        // Two half-edges pointing at each other's successor in a way that never
        // reaches the start. The walk is bounded by the edge count, so this
        // returns rather than spinning.
        let hull = kv_object(vec![
            ("m_nCollisionAttributeIndex", KvValue::Int(0)),
            (
                "m_Hull",
                kv_object(vec![
                    (
                        "m_VertexPositions",
                        bytes_of(&[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 1.0, 0.0]),
                    ),
                    // Edge 0 goes to 1, edge 1 goes to 2, edge 2 goes to 1.
                    (
                        "m_Edges",
                        KvValue::Binary(vec![1, 0, 0, 0, 2, 0, 1, 0, 1, 0, 2, 0]),
                    ),
                    ("m_Faces", KvValue::Binary(vec![0])),
                ]),
            ),
        ]);
        let phys = PhysAggregate::from_kv(&aggregate(vec![hull], vec![], vec![]));
        // Three half-edges are visited before the bound stops the walk, which
        // is a triangle's worth of corners, so one triangle comes out.
        assert_eq!(phys.triangles(), 1);
    }

    #[test]
    fn an_empty_or_shapeless_document_decodes_to_nothing() {
        assert_eq!(
            PhysAggregate::from_kv(&KvValue::Null),
            PhysAggregate::default()
        );
        let empty = aggregate(vec![], vec![], vec![]);
        assert!(PhysAggregate::from_kv(&empty).shapes.is_empty());

        // A hull with no faces, and a mesh with no triangles, are both nothing
        // to draw rather than a shape with an empty triangle list.
        let faceless = kv_object(vec![(
            "m_Hull",
            kv_object(vec![
                ("m_VertexPositions", bytes_of(&[0.0, 0.0, 0.0])),
                ("m_Edges", KvValue::Binary(vec![0, 0, 0, 0])),
                ("m_Faces", KvValue::Binary(vec![])),
            ]),
        )]);
        assert!(
            PhysAggregate::from_kv(&aggregate(vec![faceless], vec![], vec![]))
                .shapes
                .is_empty()
        );
    }

    #[test]
    fn ragged_byte_strings_are_truncated_rather_than_read_past() {
        // A vertex buffer holding two and a half positions yields two, and a
        // triangle buffer holding one and a bit yields one.
        let mesh = kv_object(vec![
            ("m_nCollisionAttributeIndex", KvValue::Int(0)),
            (
                "m_Mesh",
                kv_object(vec![
                    (
                        "m_Vertices",
                        KvValue::Binary(
                            [0.0f32, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 1.0, 0.0]
                                .iter()
                                .flat_map(|v| v.to_le_bytes())
                                .chain([0u8, 1, 2])
                                .collect(),
                        ),
                    ),
                    (
                        "m_Triangles",
                        KvValue::Binary(
                            [0i32, 1, 2]
                                .iter()
                                .flat_map(|v| v.to_le_bytes())
                                .chain([9u8, 9])
                                .collect(),
                        ),
                    ),
                ]),
            ),
        ]);
        let phys = PhysAggregate::from_kv(&aggregate(vec![], vec![mesh], vec![]));
        assert_eq!(phys.shapes[0].positions.len(), 3);
        assert_eq!(phys.triangles(), 1);
    }
}
