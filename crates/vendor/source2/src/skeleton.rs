//! Bone hierarchies, and the rigid transforms that pose them.
//!
//! Two skeletons describe a CS2 character, and they are not the same list.
//!
//! * The **model skeleton** lives in a `vmdl_c`'s `DATA` block under
//!   `m_modelSkeleton`. Its bones are what the mesh's `BLENDINDICES` stream
//!   ultimately addresses, so it is the one a glTF skin has to be built from.
//!   The SAS has 94 of them, including twist, jiggle and eyeball bones that
//!   exist only to be driven by constraints.
//! * The **animation skeleton** lives in a separate `vnmskel_c` resource and is
//!   what the motion-captured clips are authored against. CS2's shared
//!   `animation/skeletons/characters/worldmodel.vnmskel` has 74, a subset of
//!   the model's plus ten weapon-attachment bones. That one is in [`crate::nm`].
//!
//! [`Pose`] is the bridge. Both skeletons agree on where a shared bone *is* in
//! model space even though they disagree about the local frames that get it
//! there — the SAS's `root_motion` carries a 120-degree axis-cycling rotation
//! the animation skeleton folds into `pelvis` instead. Retargeting therefore
//! goes through model space rather than copying local transforms across, which
//! [`Skeleton::retarget_from`] does.

use crate::kv3::KvValue;

/// A rigid transform with uniform scale: the form every Source 2 bone pose is
/// stored in, and the form glTF wants back.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BoneTransform {
    /// Translation, in Source units.
    pub translation: [f32; 3],
    /// Rotation as a quaternion in `x, y, z, w` order, which is glTF's order
    /// as well as Valve's.
    pub rotation: [f32; 4],
    /// Uniform scale.
    pub scale: f32,
}

impl Default for BoneTransform {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl BoneTransform {
    /// No translation, no rotation, unit scale.
    pub const IDENTITY: Self = Self {
        translation: [0.0; 3],
        rotation: [0.0, 0.0, 0.0, 1.0],
        scale: 1.0,
    };

    /// Read the eight-float form the `vnmskel` reference poses use:
    /// `[tx, ty, tz, scale, qx, qy, qz, qw]`.
    ///
    /// The layout is not documented anywhere; it was pinned by comparing the
    /// worldmodel skeleton's `m_parentSpaceReferencePose` against the same
    /// bones' `m_bonePosParent` and `m_boneRotParent` in the SAS model, which
    /// agree to six decimal places from `spine_0` down.
    #[must_use]
    pub fn from_kv8(value: &KvValue) -> Option<Self> {
        let a = value.as_array()?;
        if a.len() < 8 {
            return None;
        }
        #[allow(clippy::cast_possible_truncation)]
        let f = |i: usize| -> f32 { a.get(i).and_then(KvValue::as_f64).unwrap_or(0.0) as f32 };
        Some(Self {
            translation: [f(0), f(1), f(2)],
            scale: if f(3) == 0.0 { 1.0 } else { f(3) },
            rotation: normalise([f(4), f(5), f(6), f(7)]),
        })
    }

    /// Compose two transforms: `self` applied after `parent`.
    #[must_use]
    pub fn concat(parent: Self, child: Self) -> Self {
        let scaled = [
            child.translation[0] * parent.scale,
            child.translation[1] * parent.scale,
            child.translation[2] * parent.scale,
        ];
        let rotated = rotate(parent.rotation, scaled);
        Self {
            translation: [
                parent.translation[0] + rotated[0],
                parent.translation[1] + rotated[1],
                parent.translation[2] + rotated[2],
            ],
            rotation: normalise(mul(parent.rotation, child.rotation)),
            scale: parent.scale * child.scale,
        }
    }

    /// The transform that undoes this one.
    #[must_use]
    pub fn inverse(self) -> Self {
        let inv_scale = if self.scale.abs() > 1e-9 {
            1.0 / self.scale
        } else {
            1.0
        };
        let inv_rot = conjugate(self.rotation);
        let t = rotate(
            inv_rot,
            [
                -self.translation[0] * inv_scale,
                -self.translation[1] * inv_scale,
                -self.translation[2] * inv_scale,
            ],
        );
        Self {
            translation: t,
            rotation: inv_rot,
            scale: inv_scale,
        }
    }

    /// Apply the transform to a point.
    #[must_use]
    pub fn apply(self, p: [f32; 3]) -> [f32; 3] {
        let scaled = [p[0] * self.scale, p[1] * self.scale, p[2] * self.scale];
        let r = rotate(self.rotation, scaled);
        [
            r[0] + self.translation[0],
            r[1] + self.translation[1],
            r[2] + self.translation[2],
        ]
    }

    /// The equivalent column-major 4x4 matrix, which is the layout glTF's
    /// `inverseBindMatrices` accessor wants.
    #[must_use]
    pub fn to_matrix(self) -> [f32; 16] {
        let [x, y, z, w] = self.rotation;
        let s = self.scale;
        [
            (1.0 - 2.0 * (y * y + z * z)) * s,
            (2.0 * (x * y + z * w)) * s,
            (2.0 * (x * z - y * w)) * s,
            0.0,
            (2.0 * (x * y - z * w)) * s,
            (1.0 - 2.0 * (x * x + z * z)) * s,
            (2.0 * (y * z + x * w)) * s,
            0.0,
            (2.0 * (x * z + y * w)) * s,
            (2.0 * (y * z - x * w)) * s,
            (1.0 - 2.0 * (x * x + y * y)) * s,
            0.0,
            self.translation[0],
            self.translation[1],
            self.translation[2],
            1.0,
        ]
    }
}

/// Hamilton product of two `x, y, z, w` quaternions.
#[must_use]
pub fn mul(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    [
        a[3] * b[0] + a[0] * b[3] + a[1] * b[2] - a[2] * b[1],
        a[3] * b[1] - a[0] * b[2] + a[1] * b[3] + a[2] * b[0],
        a[3] * b[2] + a[0] * b[1] - a[1] * b[0] + a[2] * b[3],
        a[3] * b[3] - a[0] * b[0] - a[1] * b[1] - a[2] * b[2],
    ]
}

/// Conjugate, which inverts a unit quaternion.
#[must_use]
pub fn conjugate(q: [f32; 4]) -> [f32; 4] {
    [-q[0], -q[1], -q[2], q[3]]
}

/// Rotate a vector by a quaternion.
#[must_use]
pub fn rotate(q: [f32; 4], v: [f32; 3]) -> [f32; 3] {
    // t = 2 * (q.xyz x v); v' = v + q.w * t + q.xyz x t
    let t = [
        2.0 * (q[1] * v[2] - q[2] * v[1]),
        2.0 * (q[2] * v[0] - q[0] * v[2]),
        2.0 * (q[0] * v[1] - q[1] * v[0]),
    ];
    [
        v[0] + q[3] * t[0] + q[1] * t[2] - q[2] * t[1],
        v[1] + q[3] * t[1] + q[2] * t[0] - q[0] * t[2],
        v[2] + q[3] * t[2] + q[0] * t[1] - q[1] * t[0],
    ]
}

/// Scale a quaternion back to unit length, falling back to identity when it is
/// degenerate rather than producing NaNs.
#[must_use]
pub fn normalise(q: [f32; 4]) -> [f32; 4] {
    let len2 = q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3];
    if !len2.is_finite() || len2 < 1e-12 {
        return [0.0, 0.0, 0.0, 1.0];
    }
    let inv = 1.0 / len2.sqrt();
    [q[0] * inv, q[1] * inv, q[2] * inv, q[3] * inv]
}

/// One bone: a name, a parent, and its rest transform in the parent's frame.
#[derive(Debug, Clone)]
pub struct Bone {
    /// Bone name, e.g. `spine_0`. Case is as stored; CS2 mixes `leg_upper_L`
    /// and `leg_upper_l` between resources, so comparisons should be
    /// case-insensitive.
    pub name: String,
    /// Index of the parent bone, or `None` for a root.
    pub parent: Option<usize>,
    /// Rest transform relative to the parent.
    pub local: BoneTransform,
}

/// A bone hierarchy, in an order where every parent precedes its children.
#[derive(Debug, Clone, Default)]
pub struct Skeleton {
    /// The bones, in file order.
    pub bones: Vec<Bone>,
}

/// A set of local transforms, one per bone of some skeleton.
pub type Pose = Vec<BoneTransform>;

impl Skeleton {
    /// Decode `m_modelSkeleton` from a compiled model's `DATA` block.
    ///
    /// Returns `None` when the resource has no skeleton, which is the case for
    /// world geometry and for the placeholder stub models.
    #[must_use]
    pub fn from_model_data(data: &KvValue) -> Option<Self> {
        let root = data.get("m_modelSkeleton")?;
        let names = root.get("m_boneName")?.as_array()?;
        let parents = root.get("m_nParent")?.as_array()?;
        let positions = root.get("m_bonePosParent")?.as_array()?;
        let rotations = root.get("m_boneRotParent")?.as_array()?;
        let scales = root.get("m_boneScaleParent").and_then(KvValue::as_array);

        let count = names.len();
        if count == 0 || parents.len() != count || positions.len() != count {
            return None;
        }

        let mut bones = Vec::with_capacity(count);
        for i in 0..count {
            let parent = parents.get(i).and_then(KvValue::as_i64).unwrap_or(-1);
            // A parent must already have been emitted, or the hierarchy cannot
            // be walked in one pass. Valve's own order always satisfies this.
            let parent = usize::try_from(parent).ok().filter(|&p| p < i);
            let scale = scales
                .and_then(|s| s.get(i))
                .and_then(KvValue::as_f64)
                .unwrap_or(1.0);
            #[allow(clippy::cast_possible_truncation)]
            let scale = if scale.abs() < 1e-9 {
                1.0
            } else {
                scale as f32
            };
            bones.push(Bone {
                name: names
                    .get(i)
                    .and_then(KvValue::as_str)
                    .unwrap_or("")
                    .to_string(),
                parent,
                local: BoneTransform {
                    translation: vec3(positions.get(i)),
                    rotation: normalise(vec4(rotations.get(i))),
                    scale,
                },
            });
        }
        Some(Self { bones })
    }

    /// How many bones there are.
    #[must_use]
    pub fn len(&self) -> usize {
        self.bones.len()
    }

    /// Whether the skeleton has no bones.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.bones.is_empty()
    }

    /// Index of a bone by name, matched case-insensitively.
    #[must_use]
    pub fn index_of(&self, name: &str) -> Option<usize> {
        self.bones
            .iter()
            .position(|b| b.name.eq_ignore_ascii_case(name))
    }

    /// The rest pose, as local transforms.
    #[must_use]
    pub fn rest_pose(&self) -> Pose {
        self.bones.iter().map(|b| b.local).collect()
    }

    /// Accumulate a set of local transforms into model space.
    ///
    /// A pose shorter than the skeleton falls back to the bone's rest
    /// transform, so a partial pose is usable rather than an error.
    #[must_use]
    pub fn to_model_space(&self, pose: &[BoneTransform]) -> Pose {
        let mut out = Vec::with_capacity(self.bones.len());
        for (i, bone) in self.bones.iter().enumerate() {
            let local = pose.get(i).copied().unwrap_or(bone.local);
            let world = match bone.parent {
                Some(p) => BoneTransform::concat(out[p], local),
                None => local,
            };
            out.push(world);
        }
        out
    }

    /// The inverse of each bone's rest transform in model space, which is
    /// exactly glTF's `inverseBindMatrices`.
    #[must_use]
    pub fn inverse_bind_matrices(&self) -> Vec<[f32; 16]> {
        self.to_model_space(&self.rest_pose())
            .into_iter()
            .map(|t| t.inverse().to_matrix())
            .collect()
    }

    /// Map each of this skeleton's bones onto `other`'s by name.
    #[must_use]
    pub fn map_bones_from(&self, other: &Self) -> Vec<Option<usize>> {
        self.bones.iter().map(|b| other.index_of(&b.name)).collect()
    }

    /// Retarget a pose authored on `source` onto this skeleton.
    ///
    /// `mapping[i]` is the index in `source` of this skeleton's bone *i*, as
    /// [`Skeleton::map_bones_from`] produces. Bones with no counterpart keep
    /// their rest transform and simply ride on whatever their parent does,
    /// which is what makes the SAS's thirty twist and jiggle bones behave
    /// sensibly rather than being dropped.
    ///
    /// The transfer goes through model space in both directions, because the
    /// two skeletons choose different local frames for the root even though
    /// they place every shared bone identically.
    #[must_use]
    pub fn retarget_from(
        &self,
        source: &Self,
        source_pose: &[BoneTransform],
        mapping: &[Option<usize>],
    ) -> Pose {
        let source_model = source.to_model_space(source_pose);

        let mut model: Pose = Vec::with_capacity(self.bones.len());
        let mut local: Pose = Vec::with_capacity(self.bones.len());
        for (i, bone) in self.bones.iter().enumerate() {
            let parent_model = bone.parent.map_or(BoneTransform::IDENTITY, |p| model[p]);
            match mapping
                .get(i)
                .copied()
                .flatten()
                .and_then(|s| source_model.get(s))
            {
                Some(&target) => {
                    model.push(target);
                    local.push(BoneTransform::concat(parent_model.inverse(), target));
                }
                None => {
                    model.push(BoneTransform::concat(parent_model, bone.local));
                    local.push(bone.local);
                }
            }
        }
        local
    }
}

fn vec3(value: Option<&KvValue>) -> [f32; 3] {
    let mut out = [0.0f32; 3];
    if let Some(a) = value.and_then(KvValue::as_array) {
        for (i, slot) in out.iter_mut().enumerate() {
            #[allow(clippy::cast_possible_truncation)]
            if let Some(v) = a.get(i).and_then(KvValue::as_f64) {
                *slot = v as f32;
            }
        }
    }
    out
}

fn vec4(value: Option<&KvValue>) -> [f32; 4] {
    let mut out = [0.0, 0.0, 0.0, 1.0];
    if let Some(a) = value.and_then(KvValue::as_array) {
        for (i, slot) in out.iter_mut().enumerate() {
            #[allow(clippy::cast_possible_truncation)]
            if let Some(v) = a.get(i).and_then(KvValue::as_f64) {
                *slot = v as f32;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: [f32; 3], b: [f32; 3], tol: f32) -> bool {
        a.iter().zip(b.iter()).all(|(x, y)| (x - y).abs() < tol)
    }

    /// The 120-degree axis cycle that the SAS's `root_motion` bone carries.
    const CYCLE: [f32; 4] = [0.5, 0.5, 0.5, 0.5];

    #[test]
    fn rotation_matches_the_known_axis_cycle() {
        // (0.5, 0.5, 0.5, 0.5) sends x to y, y to z and z to x.
        assert!(approx(
            rotate(CYCLE, [1.0, 0.0, 0.0]),
            [0.0, 1.0, 0.0],
            1e-5
        ));
        assert!(approx(
            rotate(CYCLE, [0.0, 1.0, 0.0]),
            [0.0, 0.0, 1.0],
            1e-5
        ));
        assert!(approx(
            rotate(CYCLE, [0.0, 0.0, 1.0]),
            [1.0, 0.0, 0.0],
            1e-5
        ));
    }

    #[test]
    fn inverse_undoes_a_transform() {
        let t = BoneTransform {
            translation: [3.0, -4.0, 5.0],
            rotation: CYCLE,
            scale: 2.0,
        };
        let p = [1.0, 2.0, 3.0];
        assert!(approx(t.inverse().apply(t.apply(p)), p, 1e-4));
        let round = BoneTransform::concat(t, t.inverse());
        assert!(approx(round.translation, [0.0; 3], 1e-4));
        assert!((round.scale - 1.0).abs() < 1e-5);
    }

    #[test]
    fn concat_agrees_with_applying_in_order() {
        let a = BoneTransform {
            translation: [1.0, 2.0, 3.0],
            rotation: CYCLE,
            scale: 1.5,
        };
        let b = BoneTransform {
            translation: [-2.0, 0.5, 4.0],
            rotation: normalise([0.2, -0.3, 0.4, 0.8]),
            scale: 0.5,
        };
        let p = [0.7, -1.3, 2.2];
        assert!(approx(
            BoneTransform::concat(a, b).apply(p),
            a.apply(b.apply(p)),
            1e-4
        ));
    }

    #[test]
    fn eight_float_poses_split_into_translation_scale_and_rotation() {
        // The pelvis row of the worldmodel skeleton.
        let kv = KvValue::Array(
            [-2.853551, 0.0, 42.824825, 1.0, -0.5, -0.5, -0.5, 0.5]
                .iter()
                .map(|v| KvValue::Double(*v))
                .collect(),
        );
        let t = BoneTransform::from_kv8(&kv).expect("eight floats");
        assert!(approx(t.translation, [-2.853551, 0.0, 42.824825], 1e-5));
        assert!((t.scale - 1.0).abs() < 1e-6);
        assert!((t.rotation[3] - 0.5).abs() < 1e-6);
        // Short arrays are rejected rather than padded.
        assert!(BoneTransform::from_kv8(&KvValue::Array(vec![KvValue::Double(1.0)])).is_none());
    }

    /// Two chains that place the same bone in the same model-space spot from
    /// different local frames, which is exactly the model/animation skeleton
    /// situation. Retargeting has to reproduce the source's model space.
    #[test]
    fn retargeting_reproduces_model_space_across_differing_root_frames() {
        let source = Skeleton {
            bones: vec![
                Bone {
                    name: "root_motion".into(),
                    parent: None,
                    local: BoneTransform::IDENTITY,
                },
                Bone {
                    name: "pelvis".into(),
                    parent: Some(0),
                    local: BoneTransform {
                        translation: [-2.85, 0.0, 42.82],
                        rotation: normalise([-0.5, -0.5, -0.5, 0.5]),
                        scale: 1.0,
                    },
                },
            ],
        };
        let target = Skeleton {
            bones: vec![
                Bone {
                    name: "root_motion".into(),
                    parent: None,
                    local: BoneTransform {
                        translation: [0.0; 3],
                        rotation: CYCLE,
                        scale: 1.0,
                    },
                },
                Bone {
                    name: "pelvis".into(),
                    parent: Some(0),
                    local: BoneTransform {
                        translation: [0.0, 42.82, -2.85],
                        rotation: CYCLE,
                        scale: 1.0,
                    },
                },
                // A bone the source does not have keeps its rest transform.
                Bone {
                    name: "jiggle_hood".into(),
                    parent: Some(1),
                    local: BoneTransform {
                        translation: [1.0, 2.0, 3.0],
                        rotation: [0.0, 0.0, 0.0, 1.0],
                        scale: 1.0,
                    },
                },
            ],
        };

        let mapping = target.map_bones_from(&source);
        assert_eq!(mapping, vec![Some(0), Some(1), None]);

        // Pose the source: swing the pelvis.
        let mut pose = source.rest_pose();
        pose[1].translation[2] += 5.0;

        let retargeted = target.retarget_from(&source, &pose, &mapping);
        let got = target.to_model_space(&retargeted);
        let want = source.to_model_space(&pose);
        assert!(approx(got[1].translation, want[1].translation, 1e-3));

        // The unmapped bone still hangs off the animated pelvis.
        assert_eq!(retargeted[2].translation, [1.0, 2.0, 3.0]);
    }

    #[test]
    fn a_resource_without_a_skeleton_is_not_an_error() {
        assert!(Skeleton::from_model_data(&KvValue::Object(Vec::new())).is_none());
    }

    #[test]
    fn inverse_bind_matrices_undo_the_rest_pose() {
        let skeleton = Skeleton {
            bones: vec![
                Bone {
                    name: "a".into(),
                    parent: None,
                    local: BoneTransform {
                        translation: [1.0, 0.0, 0.0],
                        rotation: CYCLE,
                        scale: 1.0,
                    },
                },
                Bone {
                    name: "b".into(),
                    parent: Some(0),
                    local: BoneTransform {
                        translation: [0.0, 2.0, 0.0],
                        rotation: [0.0, 0.0, 0.0, 1.0],
                        scale: 1.0,
                    },
                },
            ],
        };
        let model = skeleton.to_model_space(&skeleton.rest_pose());
        let inverses = skeleton.inverse_bind_matrices();
        assert_eq!(inverses.len(), 2);
        // Composing bind with inverse-bind is the identity, so the last column
        // of the product is the origin.
        for (i, bone) in model.iter().enumerate() {
            let round = BoneTransform::concat(*bone, bone.inverse());
            assert!(approx(round.translation, [0.0; 3], 1e-4), "bone {i}");
            // Matrix translation column matches the transform it came from.
            let m = inverses[i];
            assert!(approx(
                [m[12], m[13], m[14]],
                bone.inverse().translation,
                1e-4
            ));
        }
    }
}
