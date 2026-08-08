//! Typed decoding of the world (`vwrld_c`) and world node (`vwnod_c`) resources.
//!
//! A map's geometry hangs off `maps/<name>/world.vwrld_c`. Its `DATA` block
//! lists world nodes by *prefix*, e.g. `maps/de_dust2/worldnodes/n0`, to which
//! `.vwnod_c` is appended. Note that the world's `RERL` does **not** mention its
//! nodes, so dependency walking has to go through these fields rather than the
//! external reference list.
//!
//! Each world node holds two kinds of renderable:
//!
//! * [`SceneObject`]s: one model each, with a transform;
//! * [`AggregateSceneObject`]s: a model that several source props were baked
//!   into. Their geometry is normally already in world space, with an empty
//!   `m_fragmentTransforms`; when fragment transforms are present each one
//!   instances the model again.

use crate::error::Result;
use crate::kv3::KvValue;
use crate::resource::Resource;

/// A 3x4 row-major transform: three rows of `[x, y, z, translation]`.
///
/// Source stores object transforms this way; the implicit fourth row is
/// `[0, 0, 0, 1]`.
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

    /// Read a transform from a KV3 array of three four-element rows.
    ///
    /// Anything else yields the identity, which is what Valve's own tooling
    /// treats a missing transform as.
    #[must_use]
    pub fn from_kv(value: &KvValue) -> Self {
        let Some(rows) = value.as_array() else {
            return Self::IDENTITY;
        };
        let mut out = Self::IDENTITY.0;
        for (r, row) in rows.iter().take(3).enumerate() {
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

/// One node listed by the world resource.
#[derive(Debug, Clone)]
pub struct WorldNodeRef {
    /// Resource path prefix, e.g. `maps/de_dust2/worldnodes/n0`. Append
    /// `.vwnod_c` to load it.
    pub prefix: String,
    /// Index of the parent node, or -1 for a root.
    pub parent: i64,
    /// Minimum corner of the node's bounds, in Source units.
    pub min_bounds: [f32; 3],
    /// Maximum corner.
    pub max_bounds: [f32; 3],
}

impl WorldNodeRef {
    /// The compiled resource path for this node.
    #[must_use]
    pub fn resource_path(&self) -> String {
        format!("{}.vwnod_c", self.prefix.replace('\\', "/"))
    }
}

/// The `vwrld_c` resource.
#[derive(Debug, Clone)]
pub struct World {
    /// The nodes holding this world's geometry.
    pub nodes: Vec<WorldNodeRef>,
    /// Entity lump resource paths.
    pub entity_lumps: Vec<String>,
}

impl World {
    /// Decode a world resource.
    ///
    /// # Errors
    ///
    /// Returns [`crate::Source2Error`] if the `DATA` block is absent or does
    /// not decode as KV3.
    pub fn from_resource(resource: &Resource<'_>) -> Result<Self> {
        let doc = resource.data_kv3().ok_or(crate::Source2Error::Meshopt {
            codec: "world",
            detail: "world resource has no KV3 DATA block".into(),
        })??;

        let nodes = doc
            .get("m_worldNodes")
            .and_then(KvValue::as_array)
            .unwrap_or(&[])
            .iter()
            .filter_map(|n| {
                Some(WorldNodeRef {
                    prefix: n.get("m_worldNodePrefix")?.as_str()?.to_string(),
                    parent: n.get("m_nParent").and_then(KvValue::as_i64).unwrap_or(-1),
                    min_bounds: vec3(n.get("m_vMinBounds")?)?,
                    max_bounds: vec3(n.get("m_vMaxBounds")?)?,
                })
            })
            .collect();

        let entity_lumps = doc
            .get("m_entityLumps")
            .and_then(KvValue::as_array)
            .unwrap_or(&[])
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect();

        Ok(Self {
            nodes,
            entity_lumps,
        })
    }
}

/// A single renderable model placed in a world node.
#[derive(Debug, Clone)]
pub struct SceneObject {
    /// Model resource path, e.g. `maps/de_dust2/worldnodes/n0_....vmdl`.
    pub model: String,
    /// Placement transform.
    pub transform: Transform,
    /// Layer this object belongs to, indexing [`WorldNode::layer_names`].
    pub layer: Option<usize>,
    /// Raw object type flags.
    pub flags: i64,
}

/// A model that several props were baked into at compile time.
#[derive(Debug, Clone)]
pub struct AggregateSceneObject {
    /// Model resource path.
    pub model: String,
    /// One transform per baked fragment. Empty means the geometry is already
    /// in world space and should be drawn once at the identity.
    pub fragment_transforms: Vec<Transform>,
    /// Layer this object belongs to.
    pub layer: Option<usize>,
    /// Combined object flags.
    pub flags: i64,
}

/// The `vwnod_c` resource.
#[derive(Debug, Clone)]
pub struct WorldNode {
    /// Individually placed models.
    pub scene_objects: Vec<SceneObject>,
    /// Baked-together models.
    pub aggregate_scene_objects: Vec<AggregateSceneObject>,
    /// Names of the layers objects can belong to.
    pub layer_names: Vec<String>,
}

impl WorldNode {
    /// Decode a world node resource.
    ///
    /// # Errors
    ///
    /// Returns [`crate::Source2Error`] if the `DATA` block is absent or does
    /// not decode as KV3.
    pub fn from_resource(resource: &Resource<'_>) -> Result<Self> {
        let doc = resource.data_kv3().ok_or(crate::Source2Error::Meshopt {
            codec: "worldnode",
            detail: "world node resource has no KV3 DATA block".into(),
        })??;

        let layer_names = doc
            .get("m_layerNames")
            .and_then(KvValue::as_array)
            .unwrap_or(&[])
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect();

        let layer_indices: Vec<i64> = doc
            .get("m_sceneObjectLayerIndices")
            .and_then(KvValue::as_array)
            .unwrap_or(&[])
            .iter()
            .filter_map(KvValue::as_i64)
            .collect();

        let scene_objects = doc
            .get("m_sceneObjects")
            .and_then(KvValue::as_array)
            .unwrap_or(&[])
            .iter()
            .enumerate()
            .filter_map(|(i, o)| {
                let model = o.get("m_renderableModel")?.as_str()?;
                if model.is_empty() {
                    return None;
                }
                Some(SceneObject {
                    model: model.to_string(),
                    transform: o
                        .get("m_vTransform")
                        .map_or(Transform::IDENTITY, Transform::from_kv),
                    layer: layer_indices.get(i).and_then(|&l| usize::try_from(l).ok()),
                    flags: o
                        .get("m_nObjectTypeFlags")
                        .and_then(KvValue::as_i64)
                        .unwrap_or(0),
                })
            })
            .collect();

        let aggregate_scene_objects = doc
            .get("m_aggregateSceneObjects")
            .and_then(KvValue::as_array)
            .unwrap_or(&[])
            .iter()
            .filter_map(|o| {
                let model = o.get("m_renderableModel")?.as_str()?;
                if model.is_empty() {
                    return None;
                }
                let fragment_transforms = o
                    .get("m_fragmentTransforms")
                    .and_then(KvValue::as_array)
                    .unwrap_or(&[])
                    .iter()
                    .map(Transform::from_kv)
                    .collect();
                Some(AggregateSceneObject {
                    model: model.to_string(),
                    fragment_transforms,
                    layer: o
                        .get("m_nLayer")
                        .and_then(KvValue::as_i64)
                        .and_then(|l| usize::try_from(l).ok()),
                    flags: o.get("m_allFlags").and_then(KvValue::as_i64).unwrap_or(0),
                })
            })
            .collect();

        Ok(Self {
            scene_objects,
            aggregate_scene_objects,
            layer_names,
        })
    }
}

fn vec3(value: &KvValue) -> Option<[f32; 3]> {
    let a = value.as_array()?;
    let get = |i: usize| -> Option<f32> {
        #[allow(clippy::cast_possible_truncation)]
        Some(a.get(i)?.as_f64()? as f32)
    };
    Some([get(0)?, get(1)?, get(2)?])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kv_f(v: f64) -> KvValue {
        KvValue::Double(v)
    }

    #[test]
    fn transform_reads_rows_and_applies() {
        let rows = KvValue::Array(vec![
            KvValue::Array(vec![kv_f(2.0), kv_f(0.0), kv_f(0.0), kv_f(10.0)]),
            KvValue::Array(vec![kv_f(0.0), kv_f(2.0), kv_f(0.0), kv_f(20.0)]),
            KvValue::Array(vec![kv_f(0.0), kv_f(0.0), kv_f(2.0), kv_f(30.0)]),
        ]);
        let t = Transform::from_kv(&rows);
        assert_eq!(t.apply([1.0, 1.0, 1.0]), [12.0, 22.0, 32.0]);
        assert!(!t.is_identity());
    }

    #[test]
    fn malformed_transforms_fall_back_to_identity() {
        assert!(Transform::from_kv(&KvValue::Null).is_identity());
        assert!(Transform::from_kv(&KvValue::Array(vec![])).is_identity());
        // Short rows keep the identity in the columns they do not cover.
        let partial = KvValue::Array(vec![KvValue::Array(vec![kv_f(1.0)])]);
        assert!(Transform::from_kv(&partial).is_identity());
    }

    #[test]
    fn node_paths_normalise_separators() {
        let node = WorldNodeRef {
            prefix: r"maps\de_dust2\worldnodes\n0".into(),
            parent: -1,
            min_bounds: [0.0; 3],
            max_bounds: [0.0; 3],
        };
        assert_eq!(node.resource_path(), "maps/de_dust2/worldnodes/n0.vwnod_c");
    }
}
