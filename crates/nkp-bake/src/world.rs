//! Decoding the world and its world nodes, straight off the raw KV3.
//!
//! This deliberately does not use `source2::world`. That decoder reads a
//! transform as an array of three four-element rows, which is how a
//! `m_sceneObjects` entry stores `m_vTransform` — but **not** how an aggregate
//! stores `m_fragmentTransforms`. Those are a flat typed array of twelve
//! doubles, so the row-wise reader finds a `Double` where it wants an array,
//! gives up, and returns the identity.
//!
//! On de_nuke that silently collapses 4,748 placements onto the world origin,
//! including all 1,482 copies of the pipe kit — which is the entire subject of
//! this project. [`Transform::from_kv`] here accepts both encodings.
//!
//! # Two kinds of aggregate
//!
//! `agg_merge_*` aggregates bake their props into world space and leave
//! `m_fragmentTransforms` empty; each entry of `m_aggregateMeshes` is then just
//! a draw call to issue at the identity.
//!
//! `agg_prop_*` aggregates keep one local copy of the model and instance it.
//! `m_aggregateMeshes` and `m_fragmentTransforms` run in lockstep — mesh *i* is
//! draw call `m_nDrawCallIndex` placed at transform *i*. de_nuke has 72 of
//! these covering 4,748 fragments.

use anyhow::{anyhow, Result};
use source2::{KvValue, Resource};

use crate::Resolver;

/// A row-major 3x4 transform: three rows of `[x, y, z, translation]`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Transform(pub [[f32; 4]; 3]);

impl Default for Transform {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl Transform {
    /// The identity transform.
    pub const IDENTITY: Self = Self([
        [1.0, 0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0],
    ]);

    /// Read a transform from either encoding Source 2 uses.
    ///
    /// `m_vTransform` on a scene object nests three rows of four;
    /// `m_fragmentTransforms` on an aggregate is twelve doubles laid out flat.
    /// Both appear in de_nuke's single world node, so both are accepted.
    /// Anything else yields the identity, which is what Valve's own tooling
    /// treats a missing transform as.
    #[must_use]
    pub fn from_kv(value: &KvValue) -> Self {
        let Some(items) = value.as_array() else {
            return Self::IDENTITY;
        };

        // Flat: twelve scalars, row-major.
        if items.len() == 12 && items[0].as_array().is_none() {
            let mut out = Self::IDENTITY.0;
            for (i, cell) in items.iter().enumerate() {
                if let Some(v) = cell.as_f64() {
                    #[allow(clippy::cast_possible_truncation)]
                    {
                        out[i / 4][i % 4] = v as f32;
                    }
                }
            }
            return Self(out);
        }

        // Nested: up to three rows of up to four.
        let mut out = Self::IDENTITY.0;
        for (r, row) in items.iter().take(3).enumerate() {
            let Some(cols) = row.as_array() else { continue };
            for (c, cell) in cols.iter().take(4).enumerate() {
                if let Some(v) = cell.as_f64() {
                    #[allow(clippy::cast_possible_truncation)]
                    {
                        out[r][c] = v as f32;
                    }
                }
            }
        }
        Self(out)
    }

    /// Apply the transform to a point.
    #[must_use]
    pub fn apply(&self, p: [f32; 3]) -> [f32; 3] {
        let m = &self.0;
        [
            m[0][0] * p[0] + m[0][1] * p[1] + m[0][2] * p[2] + m[0][3],
            m[1][0] * p[0] + m[1][1] * p[1] + m[1][2] * p[2] + m[1][3],
            m[2][0] * p[0] + m[2][1] * p[1] + m[2][2] * p[2] + m[2][3],
        ]
    }

    /// Whether this is (near enough) the identity.
    #[must_use]
    pub fn is_identity(&self) -> bool {
        self.0
            .iter()
            .zip(Self::IDENTITY.0.iter())
            .all(|(a, b)| a.iter().zip(b.iter()).all(|(x, y)| (x - y).abs() < 1e-6))
    }
}

/// One thing to draw: a model, which of its draw calls, and where.
#[derive(Debug, Clone)]
pub struct Placement {
    /// Model resource path.
    pub model: String,
    /// Which draw call of the model to issue, or `None` for all of them.
    pub draw_call: Option<usize>,
    /// Where to put it, in Source space.
    pub transform: Transform,
    /// One of [`nkp_format::flags`].
    pub kind: u32,
}

/// Every placement in the map, in world-node then file order.
///
/// # Errors
///
/// Fails if the world resource is missing or does not decode. A world node
/// that fails to read is skipped rather than fatal — losing one node loses part
/// of the map, but reporting nothing at all is worse.
pub fn placements(resolver: &Resolver, map: &str) -> Result<Vec<Placement>> {
    let world_path = format!("maps/{map}/world.vwrld_c");
    let bytes = resolver.read(&world_path)?;
    let resource = Resource::parse(&bytes)?;
    let doc = resource
        .data_kv3()
        .ok_or_else(|| anyhow!("{world_path} has no KV3 DATA block"))??;

    let node_prefixes: Vec<String> = doc
        .root
        .get("m_worldNodes")
        .and_then(KvValue::as_array)
        .unwrap_or(&[])
        .iter()
        .filter_map(|n| n.get("m_worldNodePrefix")?.as_str().map(str::to_string))
        .collect();
    if node_prefixes.is_empty() {
        return Err(anyhow!("{world_path} lists no world nodes"));
    }

    let mut out = Vec::new();
    for prefix in &node_prefixes {
        let path = format!("{}.vwnod_c", prefix.replace('\\', "/"));
        let Ok(bytes) = resolver.read(&path) else {
            continue;
        };
        let Ok(resource) = Resource::parse(&bytes) else {
            continue;
        };
        let Some(Ok(doc)) = resource.data_kv3() else {
            continue;
        };
        collect_node(&doc.root, &mut out);
    }
    Ok(out)
}

fn collect_node(root: &KvValue, out: &mut Vec<Placement>) {
    for object in root
        .get("m_sceneObjects")
        .and_then(KvValue::as_array)
        .unwrap_or(&[])
    {
        let Some(model) = object.get("m_renderableModel").and_then(KvValue::as_str) else {
            continue;
        };
        if model.is_empty() {
            continue;
        }
        out.push(Placement {
            model: model.to_string(),
            draw_call: None,
            transform: object
                .get("m_vTransform")
                .map_or(Transform::IDENTITY, Transform::from_kv),
            kind: nkp_format::flags::SCENE_OBJECT,
        });
    }

    for aggregate in root
        .get("m_aggregateSceneObjects")
        .and_then(KvValue::as_array)
        .unwrap_or(&[])
    {
        let Some(model) = aggregate.get("m_renderableModel").and_then(KvValue::as_str) else {
            continue;
        };
        if model.is_empty() {
            continue;
        }
        let meshes = aggregate
            .get("m_aggregateMeshes")
            .and_then(KvValue::as_array)
            .unwrap_or(&[]);
        let fragments = aggregate
            .get("m_fragmentTransforms")
            .and_then(KvValue::as_array)
            .unwrap_or(&[]);

        // No fragment transforms means the geometry is already in world space.
        // Each aggregate mesh is then just a draw call to issue once. Drawing
        // the model whole would give the same pixels, but one instance per draw
        // call is what lets a rule hide the ducts without hiding the walls they
        // were merged next to.
        if fragments.is_empty() {
            for mesh in meshes {
                out.push(Placement {
                    model: model.to_string(),
                    draw_call: draw_call_of(mesh),
                    transform: Transform::IDENTITY,
                    kind: nkp_format::flags::AGG_MERGE,
                });
            }
            continue;
        }

        // Otherwise the two arrays run in lockstep: mesh i is placed at
        // transform i. de_nuke's pipe aggregate is 1,482 of these.
        for (i, mesh) in meshes.iter().enumerate() {
            let transform = fragments.get(i).map_or(Transform::IDENTITY, Transform::from_kv);
            out.push(Placement {
                model: model.to_string(),
                draw_call: draw_call_of(mesh),
                transform,
                kind: nkp_format::flags::AGG_PROP,
            });
        }
    }
}

fn draw_call_of(mesh: &KvValue) -> Option<usize> {
    mesh.get("m_nDrawCallIndex")
        .and_then(KvValue::as_i64)
        .and_then(|i| usize::try_from(i).ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(v: f64) -> KvValue {
        KvValue::Double(v)
    }

    #[test]
    fn reads_the_nested_row_encoding_scene_objects_use() {
        let rows = KvValue::Array(vec![
            KvValue::Array(vec![d(2.0), d(0.0), d(0.0), d(10.0)]),
            KvValue::Array(vec![d(0.0), d(2.0), d(0.0), d(20.0)]),
            KvValue::Array(vec![d(0.0), d(0.0), d(2.0), d(30.0)]),
        ]);
        let t = Transform::from_kv(&rows);
        assert_eq!(t.apply([1.0, 1.0, 1.0]), [12.0, 22.0, 32.0]);
    }

    /// The regression that motivated this whole module. mapview reads this
    /// shape as the identity, which stacks every de_nuke pipe on the origin.
    #[test]
    fn reads_the_flat_twelve_encoding_fragment_transforms_use() {
        let flat = KvValue::TypedArray(vec![
            d(2.0),
            d(0.0),
            d(0.0),
            d(10.0),
            d(0.0),
            d(2.0),
            d(0.0),
            d(20.0),
            d(0.0),
            d(0.0),
            d(2.0),
            d(30.0),
        ]);
        let t = Transform::from_kv(&flat);
        assert!(!t.is_identity(), "flat 12-double transform read as identity");
        assert_eq!(t.apply([1.0, 1.0, 1.0]), [12.0, 22.0, 32.0]);
    }

    /// Taken verbatim from de_nuke's first pipe fragment: a 180 degree yaw at
    /// Source translation (858, -896, -932).
    #[test]
    fn reads_a_real_de_nuke_pipe_fragment() {
        let flat = KvValue::TypedArray(vec![
            d(-1.0),
            d(5.642_599_489_874_556e-7),
            d(0.0),
            d(858.0),
            d(-5.642_599_489_874_556e-7),
            d(-1.0),
            d(0.0),
            d(-896.0),
            d(0.0),
            d(0.0),
            d(1.0),
            d(-932.0),
        ]);
        let t = Transform::from_kv(&flat);
        let origin = t.apply([0.0, 0.0, 0.0]);
        assert!((origin[0] - 858.0).abs() < 1e-3, "{origin:?}");
        assert!((origin[1] + 896.0).abs() < 1e-3, "{origin:?}");
        assert!((origin[2] + 932.0).abs() < 1e-3, "{origin:?}");
    }

    #[test]
    fn malformed_transforms_fall_back_to_identity() {
        assert!(Transform::from_kv(&KvValue::Null).is_identity());
        assert!(Transform::from_kv(&KvValue::Array(vec![])).is_identity());
    }
}
