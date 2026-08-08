//! CS2 navigation meshes: the `maps/<map>.nav` file inside a map archive.
//!
//! A `.nav` is not a compiled resource — no header version, no block directory
//! — but a stand-alone binary file that begins `0xFEEDFACE`. It describes the
//! walkable surface as a set of convex **areas**, each a polygon of corners
//! drawn from one shared vertex pool, plus per-edge **connections** naming the
//! areas reachable across that edge. That is exactly a navigation graph, which
//! is what patrol routes need.
//!
//! CS2 ships version 36. Layout follows ValveResourceFormat's `NavMesh`
//! readers:
//!
//! ```text
//! u32  magic = 0xFEEDFACE
//! u32  version                    30..=36; CS2 ships 36
//! u32  sub_version
//! u32  flags                      bit 0: the mesh has been analysed
//! [v>=36] binary KV3              8-byte aligned, no length prefix
//! [v>=31] u32 corner_count; corner_count * 3 f32
//!         u32 polygon_count
//!         polygon_count * { u8 n; n * u32 corner index; [v>=35] u32 }
//! [v>=32] u32                     always 0
//! [v>=35] u32 n; n * { cstring; 48 bytes }
//! [v>=36] binary KV3
//! u32  area_count
//! area_count * {
//!     u32 id
//!     i64 dynamic attribute flags
//!     u8  hull index
//!     u32 polygon index           (v>=31; earlier embed the corners)
//!     f32                         almost always 0
//!     per corner: u32 n; n * { u32 area id, u32 edge id }
//!     u8                          always 0
//!     u32                         always 0
//!     u32 n; n * u32              ladders above
//!     u32 n; n * u32              ladders below
//! }
//! u32  ladder_count; ladder_count * { .. }
//! u32  n; n * 18 f32              unidentified
//! generation parameters           see [`NavGenParams`]
//! ```
//!
//! Custom data follows the generation parameters and is not read.
//!
//! # Why the tail matters
//!
//! Milestone 5 stopped after the areas. That was enough to *walk* the graph but
//! not to judge it: the generation parameters at the end are where the mesh
//! records the agent it was built for, including how high that agent can step
//! and how far it can jump. Those are the numbers that separate an edge a
//! runner can cross from one it would have to mount, so they are now read —
//! best effort, into [`NavMesh::gen_params`], because an unknown tail must not
//! fail an otherwise good parse.
//!
//! # Hulls
//!
//! An area records the agent *hull* it was generated for, and the format allows
//! several. All eight shipped CS2 maps carry exactly one — hull 0, the standing
//! player, radius 16 and height 71 — and their generation parameters likewise
//! define a single hull. [`NavMesh::areas_for_hull`] selects one anyway, since
//! the field is there and a hand-built mesh may use it.

use std::collections::HashMap;

use crate::error::{Result, Source2Error};
use crate::kv3;
use crate::reader::Reader;

/// The four bytes a `.nav` file starts with.
pub const NAV_MAGIC: u32 = 0xFEED_FACE;

/// Versions this reader understands. CS2 ships 36.
pub const SUPPORTED_VERSIONS: std::ops::RangeInclusive<u32> = 30..=36;

/// The hull a standing player occupies, and the one routes are built from.
pub const PLAYER_HULL: u8 = 0;

/// Step height to assume when a mesh's generation parameters did not decode.
///
/// Source's `sv_stepsize` has been 18 units since Half-Life, and is what the
/// engine's own ground movement steps up without a jump. CS2's shipped nav
/// meshes are generated slightly tighter than that, at a `max_climb` of 16, so
/// this is only ever a fallback — [`NavMesh::step_height`] prefers the map's.
pub const DEFAULT_STEP_HEIGHT: f32 = 18.0;

/// Refuse implausible counts rather than allocating for them.
const MAX_COUNT: u32 = 1 << 22;

/// One edge crossing: the area reachable, and which edge of it.
///
/// `edge_id` indexes the corner list of the area *named by this connection*,
/// not of the area holding it. The two edges are the same physical boundary
/// seen from either side, which is what makes the height change across a
/// crossing measurable rather than guessable — see [`NavArea::crossing`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NavConnection {
    /// Identifier of the area on the far side.
    pub area_id: u32,
    /// Index of the shared edge in the *target* area's corner list.
    pub edge_id: u32,
}

/// The geometry of one directed crossing between two areas.
///
/// This is what the raw connection list does not say and a route needs: how far
/// the floor rises going this way, and whether there is any floor at all
/// between the two edges.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NavCrossing {
    /// Height change going from the near area to the far one, Source units.
    /// Positive is a rise.
    pub rise: f32,
    /// Closest approach of the two edges in the horizontal plane, in Source
    /// units. Zero when they adjoin, which is what makes the crossing a step
    /// across a shared boundary rather than a leap over a void.
    pub gap: f32,
    /// A point on the near edge to route through, in Source units: the middle
    /// of the part of the edge the two areas have in common.
    pub midpoint: [f32; 3],
    /// The near edge, in Source units.
    pub near_edge: ([f32; 3], [f32; 3]),
}

/// The horizontal distance from a point to a segment.
fn point_to_segment_xy(p: [f32; 3], a: [f32; 3], b: [f32; 3]) -> f32 {
    let [vx, vy] = [b[0] - a[0], b[1] - a[1]];
    let len2 = vx * vx + vy * vy;
    let t = if len2 <= 1e-9 {
        0.0
    } else {
        (((p[0] - a[0]) * vx + (p[1] - a[1]) * vy) / len2).clamp(0.0, 1.0)
    };
    let [dx, dy] = [p[0] - (a[0] + t * vx), p[1] - (a[1] + t * vy)];
    (dx * dx + dy * dy).sqrt()
}

/// The height of a segment above the point on it nearest `p`, horizontally.
fn segment_z_at_xy(a: [f32; 3], b: [f32; 3], p: [f32; 3]) -> f32 {
    let [vx, vy] = [b[0] - a[0], b[1] - a[1]];
    let len2 = vx * vx + vy * vy;
    let t = if len2 <= 1e-9 {
        0.0
    } else {
        (((p[0] - a[0]) * vx + (p[1] - a[1]) * vy) / len2).clamp(0.0, 1.0)
    };
    a[2] + t * (b[2] - a[2])
}

/// One walkable convex area.
#[derive(Debug, Clone)]
pub struct NavArea {
    /// Identifier, unique across the file. Connections name areas by this, not
    /// by position in the list.
    pub id: u32,
    /// Which agent hull the area was generated for.
    pub hull: u8,
    /// Attribute bits: jump, crouch, no-merge and so on.
    pub attributes: i64,
    /// The area's polygon, in Source units, counter-clockwise.
    pub corners: Vec<[f32; 3]>,
    /// Connections leaving each corner's edge, in corner order.
    pub connections: Vec<Vec<NavConnection>>,
}

impl NavArea {
    /// The centroid of the polygon's corners, in Source units.
    #[must_use]
    pub fn center(&self) -> [f32; 3] {
        if self.corners.is_empty() {
            return [0.0; 3];
        }
        let mut sum = [0.0f64; 3];
        for c in &self.corners {
            for i in 0..3 {
                sum[i] += f64::from(c[i]);
            }
        }
        #[allow(clippy::cast_possible_truncation)]
        let n = self.corners.len() as f64;
        #[allow(clippy::cast_possible_truncation)]
        [
            (sum[0] / n) as f32,
            (sum[1] / n) as f32,
            (sum[2] / n) as f32,
        ]
    }

    /// Axis-aligned extent of the polygon, in Source units.
    #[must_use]
    pub fn extent(&self) -> [f32; 3] {
        if self.corners.is_empty() {
            return [0.0; 3];
        }
        let mut min = [f32::MAX; 3];
        let mut max = [f32::MIN; 3];
        for c in &self.corners {
            for i in 0..3 {
                min[i] = min[i].min(c[i]);
                max[i] = max[i].max(c[i]);
            }
        }
        [max[0] - min[0], max[1] - min[1], max[2] - min[2]]
    }

    /// Horizontal area of the polygon, in square Source units.
    ///
    /// The shoelace formula on the x/y projection, which is what "how much
    /// floor is this" means for a nav area.
    #[must_use]
    pub fn floor_area(&self) -> f32 {
        if self.corners.len() < 3 {
            return 0.0;
        }
        let mut sum = 0.0f64;
        for i in 0..self.corners.len() {
            let a = self.corners[i];
            let b = self.corners[(i + 1) % self.corners.len()];
            sum += f64::from(a[0]) * f64::from(b[1]) - f64::from(b[0]) * f64::from(a[1]);
        }
        #[allow(clippy::cast_possible_truncation)]
        ((sum / 2.0).abs() as f32)
    }

    /// Edge `i` of the polygon: corner `i` to corner `i + 1`, wrapping.
    ///
    /// The connection list is stored per corner, and corner `i` owns the edge
    /// leaving it, so a connection's position in [`NavArea::connections`] is
    /// also the index of the edge it crosses on this side.
    #[must_use]
    pub fn edge(&self, i: usize) -> Option<([f32; 3], [f32; 3])> {
        if self.corners.is_empty() {
            return None;
        }
        Some((
            self.corners[i % self.corners.len()],
            self.corners[(i + 1) % self.corners.len()],
        ))
    }

    /// Every connection this area holds, as `(edge index, connection)`.
    pub fn links(&self) -> impl Iterator<Item = (usize, NavConnection)> + '_ {
        self.connections
            .iter()
            .enumerate()
            .flat_map(|(i, list)| list.iter().map(move |c| (i, *c)))
    }

    /// The geometry of crossing this area's edge `near_edge` into `far`, whose
    /// own side of the boundary is its edge `far_edge`.
    ///
    /// Returns `None` only when either index is out of range or a polygon is
    /// degenerate, which no shipped map produces.
    #[must_use]
    pub fn crossing(
        &self,
        near_edge: usize,
        far: &NavArea,
        far_edge: usize,
    ) -> Option<NavCrossing> {
        let near = self.edge(near_edge)?;
        if far_edge >= far.corners.len() {
            return None;
        }
        let there = far.edge(far_edge)?;

        // The two edges are the same boundary from either side, so the middle
        // of ours is a point both areas agree on when they adjoin, and the
        // nearest point on theirs when they do not.
        let midpoint = [
            (near.0[0] + near.1[0]) * 0.5,
            (near.0[1] + near.1[1]) * 0.5,
            (near.0[2] + near.1[2]) * 0.5,
        ];
        let gap = point_to_segment_xy(near.0, there.0, there.1)
            .min(point_to_segment_xy(near.1, there.0, there.1))
            .min(point_to_segment_xy(there.0, near.0, near.1))
            .min(point_to_segment_xy(there.1, near.0, near.1));

        Some(NavCrossing {
            rise: segment_z_at_xy(there.0, there.1, midpoint) - midpoint[2],
            gap,
            midpoint,
            near_edge: near,
        })
    }

    /// Every area this one connects to, across all of its edges, deduplicated.
    #[must_use]
    pub fn neighbours(&self) -> Vec<u32> {
        let mut out: Vec<u32> = self
            .connections
            .iter()
            .flatten()
            .map(|c| c.area_id)
            .collect();
        out.sort_unstable();
        out.dedup();
        out
    }
}

/// One ladder connecting two heights.
///
/// Ladders are the mesh's own record of a link that is *not* a walk: a
/// character has to mount and climb it. Route generation does not use them —
/// there is no climb clip — but reading them is what lets the parse reach the
/// generation parameters behind them.
#[derive(Debug, Clone)]
pub struct NavLadder {
    /// Identifier, unique across the file.
    pub id: u32,
    /// Width in Source units.
    pub width: f32,
    /// Length in Source units.
    pub length: f32,
    /// Top of the ladder, in Source units.
    pub top: [f32; 3],
    /// Bottom of the ladder, in Source units.
    pub bottom: [f32; 3],
    /// Which compass direction it faces, as the file stores it.
    pub direction: u32,
}

/// The agent one hull of the mesh was generated for.
///
/// This is the part of a `.nav` that says what "walkable" *meant* when the mesh
/// was built. `max_climb` in particular is the engine step height: a rise no
/// greater than this is walked up, and anything more has to be jumped or
/// mounted. Reading it from the map rather than hard-coding 18 units means the
/// filter follows whatever the map was actually generated with.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NavHullParams {
    /// Whether this hull was generated at all.
    pub enabled: bool,
    /// Agent radius in Source units.
    pub radius: f32,
    /// Agent standing height in Source units.
    pub height: f32,
    /// Whether a reduced height is also generated for.
    pub short_height_enabled: bool,
    /// That reduced height, in Source units.
    pub short_height: f32,
    /// Step height: the largest rise the agent walks up rather than jumps.
    pub max_climb: f32,
    /// Steepest floor the agent walks on, in degrees.
    pub max_slope: i32,
    /// Furthest the agent will drop off a ledge, in Source units.
    pub max_jump_down_dist: f32,
    /// Base horizontal reach of a jump, in Source units.
    pub max_jump_horiz_dist_base: f32,
    /// Highest the agent can jump up onto, in Source units.
    pub max_jump_up_dist: f32,
    /// Border erosion in cells.
    pub border_erosion: i32,
}

/// The parameters the navigation mesh was generated with.
#[derive(Debug, Clone)]
pub struct NavGenParams {
    /// Generator version, which gates several of the fields.
    pub nav_gen_version: i32,
    /// Whether the map used the project's default parameters.
    pub use_project_defaults: bool,
    /// Voxelisation cell size in Source units.
    pub cell_size: f32,
    /// Voxelisation cell height in Source units.
    pub cell_height: f32,
    /// Name of the hull preset, when the version carries one.
    pub hull_preset_name: Option<String>,
    /// Path to the hull definitions, when the version carries one.
    pub hull_definitions_file: Option<String>,
    /// One entry per agent hull, in hull-index order.
    pub hulls: Vec<NavHullParams>,
}

impl NavGenParams {
    /// The parameters for one hull index.
    #[must_use]
    pub fn hull(&self, hull: u8) -> Option<&NavHullParams> {
        self.hulls.get(usize::from(hull))
    }
}

/// A decoded navigation mesh.
#[derive(Debug, Clone)]
pub struct NavMesh {
    /// File format version. CS2 ships 36.
    pub version: u32,
    /// Sub-version word following the version.
    pub sub_version: u32,
    /// Whether the mesh has been analysed, from the flags word.
    pub is_analyzed: bool,
    /// Every area in the file, all hulls together.
    pub areas: Vec<NavArea>,
    /// Ladders, read only so the parse can reach what follows them.
    pub ladders: Vec<NavLadder>,
    /// The generation parameters, when the tail after the areas decoded.
    ///
    /// `None` means the tail did not parse, which is not an error: everything
    /// routes need is in front of it.
    pub gen_params: Option<NavGenParams>,
}

impl NavMesh {
    /// Parse a `.nav` file.
    ///
    /// # Errors
    ///
    /// Returns [`Source2Error`] if the magic or version is wrong, or if a
    /// count or offset runs past the end of the file. Never panics.
    pub fn parse(data: &[u8]) -> Result<Self> {
        let mut r = Reader::new(data);

        let magic = r.u32("nav magic")?;
        if magic != NAV_MAGIC {
            return Err(Source2Error::BadNavMagic { magic });
        }
        let version = r.u32("nav version")?;
        if !SUPPORTED_VERSIONS.contains(&version) {
            return Err(Source2Error::UnsupportedNavVersion { version });
        }
        let sub_version = r.u32("nav sub-version")?;
        let flags = r.u32("nav flags")?;

        if version >= 36 {
            skip_kv3(&mut r, data)?;
        }

        let polygons = if version >= 31 {
            read_polygons(&mut r, version)?
        } else {
            Vec::new()
        };

        if version >= 32 {
            let _reserved = r.u32("nav reserved word")?;
        }

        if version >= 35 {
            let count = counted(r.u32("nav place count")?, "nav place count")?;
            for _ in 0..count {
                let _name = r.cstr("nav place name")?;
                r.take(48, "nav place payload")?;
            }
        }

        if version >= 36 {
            skip_kv3(&mut r, data)?;
        }

        let area_count = counted(r.u32("nav area count")?, "nav area count")?;
        let mut areas = Vec::with_capacity(area_count.min(4096));
        for _ in 0..area_count {
            areas.push(read_area(&mut r, version, &polygons)?);
        }

        // Everything from here on is a bonus. A tail that does not decode
        // leaves the geometry and connectivity — which is what routes need —
        // intact, so it is deliberately not allowed to fail the parse.
        let (ladders, gen_params) = read_tail(&mut r, version).unwrap_or_default();

        Ok(Self {
            version,
            sub_version,
            is_analyzed: flags & 1 != 0,
            areas,
            ladders,
            gen_params,
        })
    }

    /// The largest rise a character walks up rather than mounts, in Source
    /// units, for one agent hull.
    ///
    /// Taken from the mesh's own generation parameters (`max_climb`), which is
    /// what the areas were actually built against — every shipped CS2 map here
    /// reports 16. [`DEFAULT_STEP_HEIGHT`] stands in when the tail did not
    /// decode.
    #[must_use]
    pub fn step_height(&self, hull: u8) -> f32 {
        self.gen_params
            .as_ref()
            .and_then(|p| p.hull(hull))
            .map(|h| h.max_climb)
            .filter(|v| v.is_finite() && *v > 0.0)
            .unwrap_or(DEFAULT_STEP_HEIGHT)
    }

    /// The areas belonging to one agent hull.
    #[must_use]
    pub fn areas_for_hull(&self, hull: u8) -> Vec<&NavArea> {
        self.areas.iter().filter(|a| a.hull == hull).collect()
    }

    /// How many areas each hull has, smallest hull index first.
    #[must_use]
    pub fn hull_counts(&self) -> Vec<(u8, usize)> {
        let mut counts: HashMap<u8, usize> = HashMap::new();
        for area in &self.areas {
            *counts.entry(area.hull).or_default() += 1;
        }
        let mut out: Vec<(u8, usize)> = counts.into_iter().collect();
        out.sort_unstable();
        out
    }
}

/// Skip an embedded binary KV3 payload, which carries no length prefix and
/// starts on an 8-byte boundary.
fn skip_kv3(r: &mut Reader<'_>, data: &[u8]) -> Result<()> {
    let aligned = (r.pos() + 7) & !7;
    r.seek(aligned as u64, "nav kv3 alignment")?;
    let rest = data.get(aligned..).unwrap_or_default();
    let (_doc, consumed) = kv3::decode_prefix(rest)?;
    r.skip_signed(
        i64::try_from(consumed).map_err(|_| Source2Error::InvalidSize {
            what: "nav kv3 length",
            value: -1,
        })?,
        "nav kv3 payload",
    )
}

/// The shared corner pool and the polygons indexing into it.
fn read_polygons(r: &mut Reader<'_>, version: u32) -> Result<Vec<Vec<[f32; 3]>>> {
    let corner_count = counted(r.u32("nav corner count")?, "nav corner count")?;
    let mut corners = Vec::with_capacity(corner_count.min(1 << 16));
    for _ in 0..corner_count {
        corners.push([
            r.f32("nav corner x")?,
            r.f32("nav corner y")?,
            r.f32("nav corner z")?,
        ]);
    }

    let polygon_count = counted(r.u32("nav polygon count")?, "nav polygon count")?;
    let mut polygons = Vec::with_capacity(polygon_count.min(1 << 16));
    for _ in 0..polygon_count {
        let n = usize::from(*r.take(1, "nav polygon corner count")?.first().unwrap_or(&0));
        let mut polygon = Vec::with_capacity(n);
        for _ in 0..n {
            let index = r.u32("nav polygon corner index")? as usize;
            polygon.push(*corners.get(index).ok_or(Source2Error::NavIndexOutOfRange {
                what: "polygon corner",
                index: index as u32,
                count: corners.len(),
            })?);
        }
        if version >= 35 {
            let _unknown = r.u32("nav polygon trailer")?;
        }
        polygons.push(polygon);
    }
    Ok(polygons)
}

fn read_area(r: &mut Reader<'_>, version: u32, polygons: &[Vec<[f32; 3]>]) -> Result<NavArea> {
    let id = r.u32("nav area id")?;
    let attributes = r.i64("nav area attributes")?;
    let hull = *r.take(1, "nav area hull")?.first().unwrap_or(&0);

    let corners = if version >= 31 {
        let index = r.u32("nav area polygon index")? as usize;
        polygons
            .get(index)
            .ok_or(Source2Error::NavIndexOutOfRange {
                what: "area polygon",
                index: index as u32,
                count: polygons.len(),
            })?
            .clone()
    } else {
        let count = counted(r.u32("nav area corner count")?, "nav area corner count")?;
        let mut out = Vec::with_capacity(count.min(1 << 12));
        for _ in 0..count {
            out.push([
                r.f32("nav area corner x")?,
                r.f32("nav area corner y")?,
                r.f32("nav area corner z")?,
            ]);
        }
        out
    };

    let _almost_always_zero = r.f32("nav area reserved float")?;

    let mut connections = Vec::with_capacity(corners.len());
    for _ in 0..corners.len() {
        let count = counted(r.u32("nav connection count")?, "nav connection count")?;
        let mut edge = Vec::with_capacity(count.min(1 << 12));
        for _ in 0..count {
            edge.push(NavConnection {
                area_id: r.u32("nav connection area")?,
                edge_id: r.u32("nav connection edge")?,
            });
        }
        connections.push(edge);
    }

    // Legacy hiding-spot and spot-encounter counts, zero in every shipped file.
    let _hiding_spots = r.take(1, "nav hiding spot count")?;
    let _spot_encounters = r.u32("nav spot encounter count")?;

    for what in ["nav ladders above", "nav ladders below"] {
        let count = counted(r.u32(what)?, "nav ladder count")?;
        for _ in 0..count {
            let _ladder = r.u32("nav ladder id")?;
        }
    }

    Ok(NavArea {
        id,
        hull,
        attributes,
        corners,
        connections,
    })
}

/// The ladders and generation parameters that follow the areas.
///
/// Returns `Ok` only if the whole tail read cleanly; the caller treats anything
/// else as "the tail is not available" rather than as a broken file.
type Tail = (Vec<NavLadder>, Option<NavGenParams>);

fn read_tail(r: &mut Reader<'_>, version: u32) -> Result<Tail> {
    let ladder_count = counted(r.u32("nav ladder count")?, "nav ladder count")?;
    let mut ladders = Vec::with_capacity(ladder_count.min(1 << 12));
    for _ in 0..ladder_count {
        ladders.push(read_ladder(r, version)?);
    }

    // An unidentified block of 18 floats per entry. VRF skips it by the same
    // arithmetic and names nothing in it; every shipped map here has zero
    // entries, so there is nothing to identify from.
    let unknown = counted(r.u32("nav unknown block count")?, "nav unknown block count")?;
    let bytes = i64::try_from(unknown)
        .ok()
        .and_then(|n| n.checked_mul(18 * 4))
        .ok_or(Source2Error::InvalidSize {
            what: "nav unknown block",
            value: -1,
        })?;
    r.skip_signed(bytes, "nav unknown block")?;

    let gen_params = read_gen_params(r)?;
    Ok((ladders, Some(gen_params)))
}

fn read_ladder(r: &mut Reader<'_>, version: u32) -> Result<NavLadder> {
    let id = r.u32("nav ladder id")?;
    let width = r.f32("nav ladder width")?;
    let top = [
        r.f32("nav ladder top x")?,
        r.f32("nav ladder top y")?,
        r.f32("nav ladder top z")?,
    ];
    let bottom = [
        r.f32("nav ladder bottom x")?,
        r.f32("nav ladder bottom y")?,
        r.f32("nav ladder bottom z")?,
    ];
    let length = r.f32("nav ladder length")?;
    let direction = r.u32("nav ladder direction")?;
    // Five connected areas, and two more from version 35.
    let links = if version >= 35 { 7 } else { 5 };
    for _ in 0..links {
        let _area = r.u32("nav ladder area")?;
    }
    Ok(NavLadder {
        id,
        width,
        length,
        top,
        bottom,
        direction,
    })
}

fn read_gen_params(r: &mut Reader<'_>) -> Result<NavGenParams> {
    let nav_gen_version = r.i32("nav gen version")?;
    let use_project_defaults = r.u32("nav gen project defaults")? != 0;
    let _tile_size = r.f32("nav gen tile size")?;
    let cell_size = r.f32("nav gen cell size")?;
    let cell_height = r.f32("nav gen cell height")?;
    for what in [
        "nav gen min region size",
        "nav gen merged region size",
        "nav gen mesh sample distance",
        "nav gen max sample error",
        "nav gen max edge length",
        "nav gen max edge error",
        "nav gen verts per poly",
    ] {
        let _word = r.u32(what)?;
    }
    if nav_gen_version >= 7 {
        let _small_area_on_edge_removal = r.f32("nav gen small area removal")?;
    }
    let (hull_preset_name, hull_definitions_file) = if nav_gen_version >= 12 {
        (
            Some(r.cstr("nav gen hull preset")?),
            Some(r.cstr("nav gen hull definitions")?),
        )
    } else {
        (None, None)
    };

    let hull_count = counted(r.u32("nav gen hull count")?, "nav gen hull count")?;
    if hull_count > 16 {
        return Err(Source2Error::InvalidSize {
            what: "nav gen hull count",
            value: i64::try_from(hull_count).unwrap_or(-1),
        });
    }
    let mut hulls = Vec::with_capacity(hull_count);
    for _ in 0..hull_count {
        hulls.push(read_hull_params(r, nav_gen_version)?);
    }
    // Before version 12 the file always carried three hulls' worth of bytes
    // whatever the count said, so the unused ones still have to be stepped over.
    if nav_gen_version <= 11 {
        for _ in hull_count..3 {
            let _unused = read_hull_params(r, nav_gen_version)?;
        }
    }

    Ok(NavGenParams {
        nav_gen_version,
        use_project_defaults,
        cell_size,
        cell_height,
        hull_preset_name,
        hull_definitions_file,
        hulls,
    })
}

fn read_hull_params(r: &mut Reader<'_>, gen_version: i32) -> Result<NavHullParams> {
    let enabled = if gen_version >= 9 {
        *r.take(1, "nav hull enabled")?.first().unwrap_or(&1) != 0
    } else {
        true
    };
    let radius = r.f32("nav hull radius")?;
    let height = r.f32("nav hull height")?;
    let (short_height_enabled, short_height) = if gen_version >= 9 {
        (
            *r.take(1, "nav hull short height enabled")?
                .first()
                .unwrap_or(&0)
                != 0,
            r.f32("nav hull short height")?,
        )
    } else {
        (false, 0.0)
    };
    if gen_version >= 13 {
        let _unknown_byte = r.take(1, "nav hull unknown byte")?;
        let _unknown_float = r.f32("nav hull unknown float")?;
    }
    let max_climb = r.f32("nav hull max climb")?;
    let max_slope = r.i32("nav hull max slope")?;
    let max_jump_down_dist = r.f32("nav hull max jump down")?;
    let max_jump_horiz_dist_base = r.f32("nav hull max jump horizontal")?;
    let max_jump_up_dist = r.f32("nav hull max jump up")?;
    let border_erosion = if gen_version >= 11 {
        r.i32("nav hull border erosion")?
    } else {
        0
    };

    Ok(NavHullParams {
        enabled,
        radius,
        height,
        short_height_enabled,
        short_height,
        max_climb,
        max_slope,
        max_jump_down_dist,
        max_jump_horiz_dist_base,
        max_jump_up_dist,
        border_erosion,
    })
}

/// Reject a count that could only come from misreading the stream.
fn counted(value: u32, what: &'static str) -> Result<usize> {
    if value > MAX_COUNT {
        return Err(Source2Error::InvalidSize {
            what,
            value: i64::from(value),
        });
    }
    Ok(value as usize)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Build a minimal but structurally real version-33 nav file: old enough
    /// to skip the two KV3 payloads and the place table, new enough to use the
    /// shared polygon pool.
    pub(crate) fn build_nav(version: u32, areas: &[(u32, u8, &[usize])]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&NAV_MAGIC.to_le_bytes());
        out.extend_from_slice(&version.to_le_bytes());
        out.extend_from_slice(&1u32.to_le_bytes()); // sub-version
        out.extend_from_slice(&1u32.to_le_bytes()); // flags: analysed

        // Four corners of a unit square at two heights.
        let corners: [[f32; 3]; 8] = [
            [0.0, 0.0, 0.0],
            [64.0, 0.0, 0.0],
            [64.0, 64.0, 0.0],
            [0.0, 64.0, 0.0],
            [64.0, 0.0, 16.0],
            [128.0, 0.0, 16.0],
            [128.0, 64.0, 16.0],
            [64.0, 64.0, 16.0],
        ];
        out.extend_from_slice(&(corners.len() as u32).to_le_bytes());
        for c in corners {
            for v in c {
                out.extend_from_slice(&v.to_le_bytes());
            }
        }

        let polygons: [&[usize]; 2] = [&[0, 1, 2, 3], &[4, 5, 6, 7]];
        out.extend_from_slice(&(polygons.len() as u32).to_le_bytes());
        for polygon in polygons {
            out.push(polygon.len() as u8);
            for &i in polygon {
                out.extend_from_slice(&(i as u32).to_le_bytes());
            }
            if version >= 35 {
                out.extend_from_slice(&0u32.to_le_bytes());
            }
        }

        if version >= 32 {
            out.extend_from_slice(&0u32.to_le_bytes());
        }
        if version >= 35 {
            out.extend_from_slice(&0u32.to_le_bytes()); // no places
        }

        out.extend_from_slice(&(areas.len() as u32).to_le_bytes());
        for &(id, hull, links) in areas {
            out.extend_from_slice(&id.to_le_bytes());
            out.extend_from_slice(&0i64.to_le_bytes()); // attributes
            out.push(hull);
            out.extend_from_slice(&(id % 2).to_le_bytes()); // polygon index
            out.extend_from_slice(&0f32.to_le_bytes());
            // Four edges; the first carries every connection, the rest none.
            for edge in 0..4u32 {
                let on_this_edge = if edge == 0 { links.len() } else { 0 };
                out.extend_from_slice(&(on_this_edge as u32).to_le_bytes());
                if edge == 0 {
                    for &target in links {
                        out.extend_from_slice(&(target as u32).to_le_bytes());
                        out.extend_from_slice(&edge.to_le_bytes());
                    }
                }
            }
            out.push(0);
            out.extend_from_slice(&0u32.to_le_bytes());
            out.extend_from_slice(&0u32.to_le_bytes()); // ladders above
            out.extend_from_slice(&0u32.to_le_bytes()); // ladders below
        }
        out
    }

    #[test]
    fn parses_areas_polygons_and_connections() {
        let bytes = build_nav(33, &[(0, 0, &[1]), (1, 0, &[0]), (2, 1, &[])]);
        let nav = NavMesh::parse(&bytes).expect("parse");
        assert_eq!(nav.version, 33);
        assert_eq!(nav.sub_version, 1);
        assert!(nav.is_analyzed);
        assert_eq!(nav.areas.len(), 3);

        let first = &nav.areas[0];
        assert_eq!(first.id, 0);
        assert_eq!(first.hull, 0);
        assert_eq!(first.corners.len(), 4);
        assert_eq!(first.neighbours(), vec![1]);
        // Centre of the 64-unit square at z = 0.
        let c = first.center();
        assert!((c[0] - 32.0).abs() < 1e-3 && (c[1] - 32.0).abs() < 1e-3 && c[2].abs() < 1e-3);
        assert!((first.floor_area() - 4096.0).abs() < 1.0);
        assert_eq!(first.extent(), [64.0, 64.0, 0.0]);

        assert_eq!(nav.areas_for_hull(0).len(), 2);
        assert_eq!(nav.areas_for_hull(1).len(), 1);
        assert_eq!(nav.hull_counts(), vec![(0, 2), (1, 1)]);
    }

    #[test]
    fn version_35_adds_a_polygon_trailer_and_a_place_table() {
        let bytes = build_nav(35, &[(0, 0, &[])]);
        let nav = NavMesh::parse(&bytes).expect("parse v35");
        assert_eq!(nav.version, 35);
        assert_eq!(nav.areas.len(), 1);
    }

    #[test]
    fn rejects_a_wrong_magic_or_version() {
        let mut bytes = build_nav(33, &[(0, 0, &[])]);
        bytes[0] ^= 0xFF;
        assert!(matches!(
            NavMesh::parse(&bytes),
            Err(Source2Error::BadNavMagic { .. })
        ));

        let mut bytes = build_nav(33, &[(0, 0, &[])]);
        bytes[4..8].copy_from_slice(&99u32.to_le_bytes());
        assert!(matches!(
            NavMesh::parse(&bytes),
            Err(Source2Error::UnsupportedNavVersion { version: 99 })
        ));
    }

    #[test]
    fn an_out_of_range_polygon_index_is_an_error() {
        let mut bytes = build_nav(33, &[(0, 0, &[])]);
        // The area's polygon index sits after id, attributes and hull.
        let area_start = bytes.len() - (4 + 8 + 1 + 4 + 4 + 16 + 1 + 4 + 4 + 4);
        let index_at = area_start + 4 + 8 + 1;
        bytes[index_at..index_at + 4].copy_from_slice(&999u32.to_le_bytes());
        assert!(matches!(
            NavMesh::parse(&bytes),
            Err(Source2Error::NavIndexOutOfRange { .. })
        ));
    }

    #[test]
    fn truncation_never_panics() {
        let bytes = build_nav(35, &[(0, 0, &[1]), (1, 0, &[0])]);
        for cut in 0..bytes.len() {
            let _ = NavMesh::parse(&bytes[..cut]);
        }
    }

    #[test]
    fn bit_flips_never_panic() {
        let bytes = build_nav(33, &[(0, 0, &[1]), (1, 1, &[0])]);
        for i in 0..bytes.len() {
            for mask in [0x01u8, 0x40, 0xFF] {
                let mut corrupt = bytes.clone();
                corrupt[i] ^= mask;
                if let Ok(nav) = NavMesh::parse(&corrupt) {
                    for area in &nav.areas {
                        let _ = area.center();
                        let _ = area.extent();
                        let _ = area.floor_area();
                        let _ = area.neighbours();
                    }
                    let _ = nav.hull_counts();
                }
            }
        }
    }

    #[test]
    fn an_empty_mesh_is_not_an_error() {
        let bytes = build_nav(33, &[]);
        let nav = NavMesh::parse(&bytes).expect("parse");
        assert!(nav.areas.is_empty());
        assert!(nav.areas_for_hull(PLAYER_HULL).is_empty());
        assert!(nav.hull_counts().is_empty());
    }

    #[test]
    fn degenerate_areas_report_zero_rather_than_dividing_by_nothing() {
        let area = NavArea {
            id: 0,
            hull: 0,
            attributes: 0,
            corners: Vec::new(),
            connections: Vec::new(),
        };
        assert_eq!(area.center(), [0.0; 3]);
        assert_eq!(area.extent(), [0.0; 3]);
        assert_eq!(area.floor_area(), 0.0);
        assert!(area.neighbours().is_empty());
    }
}
