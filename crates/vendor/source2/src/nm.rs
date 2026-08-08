//! CS2's current animation system: `vnmskel_c` skeletons and `vnmclip_c` clips.
//!
//! Source 2 shipped animation as `AGRP`/`ANIM`/`ASEQ` blocks inside the model.
//! CS2 does not use that any more. Those blocks are still present on a
//! character model but hold one clip called `tools_preview`; the real motion
//! lives in thousands of stand-alone resources under `animation/anims/`,
//! authored against a handful of shared skeletons under
//! `animation/skeletons/`. The compiled forms are `vnmclip_c` and
//! `vnmskel_c`, `Nm` for the animation runtime Valve took from Esoterica, and
//! both are plain binary KV3 in their `DATA` block. What needs decoding by hand
//! is `m_compressedPoseData`, a quantised blob of per-frame bone transforms.
//!
//! # Quantisation
//!
//! The blob is a flat array of `u16`, addressed by `m_compressedPoseOffsets`
//! which is in `u16` units, not bytes. One frame is a run of tracks in
//! skeleton-bone order, and each track contributes only what
//! `m_trackCompressionSettings` says is not static:
//!
//! * **rotation**, 3 `u16`. The smallest-three encoding: the largest component
//!   is dropped and rebuilt from the other three, which are 15-bit values over
//!   ±1/sqrt(2). The dropped component's index is the top bit of the first two
//!   words.
//! * **translation**, 3 `u16`, each a plain unsigned-normalised value mapped
//!   through that axis's `m_flRangeStart` and `m_flRangeLength`.
//! * **scale**, 1 `u16`, likewise through `m_scaleRange`.
//!
//! A static component instead takes its constant from the settings —
//! `m_constantRotation` for rotation, and the *range start* for translation
//! and scale, which doubles as the constant value. Layout follows
//! ValveResourceFormat's `ModelAnimation2/AnimationClip.cs`.

use crate::error::{Result, Source2Error};
use crate::kv3::KvValue;
use crate::resource::Resource;
use crate::skeleton::{normalise, Bone, BoneTransform, Pose, Skeleton};

/// `u16` words a quantised rotation occupies.
const ROTATION_WORDS: usize = 3;
/// `u16` words a quantised translation occupies.
const TRANSLATION_WORDS: usize = 3;
/// Refuse clips longer than this rather than allocating for them.
const MAX_FRAMES: usize = 1 << 16;

/// Per-track quantisation settings, one entry per bone of the clip's skeleton.
#[derive(Debug, Clone)]
pub struct TrackSettings {
    /// `(start, length)` for each translation axis.
    pub translation_range: [(f32, f32); 3],
    /// `(start, length)` for uniform scale.
    pub scale_range: (f32, f32),
    /// Rotation to use when the track's rotation does not animate.
    pub constant_rotation: [f32; 4],
    /// Whether rotation is constant over the clip.
    pub rotation_static: bool,
    /// Whether translation is constant over the clip.
    pub translation_static: bool,
    /// Whether scale is constant over the clip.
    pub scale_static: bool,
}

impl TrackSettings {
    fn from_kv(value: &KvValue) -> Self {
        let range = |name: &str, default_start: f32| -> (f32, f32) {
            let Some(obj) = value.get(name) else {
                return (default_start, 0.0);
            };
            #[allow(clippy::cast_possible_truncation)]
            let get = |key: &str| obj.get(key).and_then(KvValue::as_f64).unwrap_or(0.0) as f32;
            (get("m_flRangeStart"), get("m_flRangeLength"))
        };
        let flag = |name: &str| value.get(name).and_then(KvValue::as_bool).unwrap_or(true);

        let mut rotation = [0.0, 0.0, 0.0, 1.0];
        if let Some(a) = value.get("m_constantRotation").and_then(KvValue::as_array) {
            for (i, slot) in rotation.iter_mut().enumerate() {
                #[allow(clippy::cast_possible_truncation)]
                if let Some(v) = a.get(i).and_then(KvValue::as_f64) {
                    *slot = v as f32;
                }
            }
        }

        Self {
            translation_range: [
                range("m_translationRangeX", 0.0),
                range("m_translationRangeY", 0.0),
                range("m_translationRangeZ", 0.0),
            ],
            scale_range: range("m_scaleRange", 1.0),
            constant_rotation: normalise(rotation),
            rotation_static: flag("m_bIsRotationStatic"),
            translation_static: flag("m_bIsTranslationStatic"),
            scale_static: flag("m_bIsScaleStatic"),
        }
    }

    /// The transform this track holds when nothing about it animates.
    fn constant(&self) -> BoneTransform {
        BoneTransform {
            translation: [
                self.translation_range[0].0,
                self.translation_range[1].0,
                self.translation_range[2].0,
            ],
            rotation: self.constant_rotation,
            scale: self.scale_range.0,
        }
    }

    /// `u16` words one frame of this track occupies.
    fn words(&self) -> usize {
        usize::from(!self.rotation_static) * ROTATION_WORDS
            + usize::from(!self.translation_static) * TRANSLATION_WORDS
            + usize::from(!self.scale_static)
    }
}

/// The skeleton a set of clips is authored against.
#[derive(Debug, Clone)]
pub struct NmSkeleton {
    /// The resource's own identifier, e.g.
    /// `animation/skeletons/characters/worldmodel.vnmskel`.
    pub id: String,
    /// Bone names, parents and parent-space reference pose.
    pub skeleton: Skeleton,
}

impl NmSkeleton {
    /// Decode a `vnmskel_c`.
    ///
    /// # Errors
    ///
    /// Fails if the resource has no KV3 `DATA` block, or the block does not
    /// carry the bone arrays.
    pub fn from_resource(resource: &Resource<'_>) -> Result<Self> {
        let doc = resource
            .data_kv3()
            .ok_or(Source2Error::NotKv3 { fourcc: "DATA" })??;
        let root = &doc.root;

        let names = root.get("m_boneIDs").and_then(KvValue::as_array).ok_or(
            Source2Error::MissingField {
                what: "m_boneIDs",
                resource: "vnmskel",
            },
        )?;
        let parents = root
            .get("m_parentIndices")
            .and_then(KvValue::as_array)
            .ok_or(Source2Error::MissingField {
                what: "m_parentIndices",
                resource: "vnmskel",
            })?;
        let poses = root
            .get("m_parentSpaceReferencePose")
            .and_then(KvValue::as_array)
            .ok_or(Source2Error::MissingField {
                what: "m_parentSpaceReferencePose",
                resource: "vnmskel",
            })?;

        if parents.len() != names.len() || poses.len() != names.len() {
            return Err(Source2Error::MissingField {
                what: "matching bone array lengths",
                resource: "vnmskel",
            });
        }

        let mut bones = Vec::with_capacity(names.len());
        for i in 0..names.len() {
            let parent = parents.get(i).and_then(KvValue::as_i64).unwrap_or(-1);
            bones.push(Bone {
                name: names
                    .get(i)
                    .and_then(KvValue::as_str)
                    .unwrap_or("")
                    .to_string(),
                parent: usize::try_from(parent).ok().filter(|&p| p < i),
                local: poses
                    .get(i)
                    .and_then(BoneTransform::from_kv8)
                    .unwrap_or(BoneTransform::IDENTITY),
            });
        }

        Ok(Self {
            id: root
                .get("m_ID")
                .and_then(KvValue::as_str)
                .unwrap_or("")
                .to_string(),
            skeleton: Skeleton { bones },
        })
    }
}

/// One decoded animation clip.
#[derive(Debug, Clone)]
pub struct NmClip {
    /// Path of the skeleton the clip is authored against.
    pub skeleton: String,
    /// Number of sampled frames. A looping clip repeats its first frame last.
    pub frame_count: usize,
    /// Playback length in seconds.
    pub duration: f32,
    /// Whether the clip is meant to be added on top of another pose rather
    /// than replacing it. Additive clips are not usable on their own.
    pub additive: bool,
    /// Per-bone quantisation settings, in the skeleton's bone order.
    pub tracks: Vec<TrackSettings>,
    /// Offsets into the pose data, in `u16` units, one per frame.
    frame_offsets: Vec<usize>,
    /// The quantised pose data as `u16` words.
    words: Vec<u16>,
    /// Total translation the root travels over the clip, if the clip carries
    /// root motion at all.
    pub root_motion_delta: [f32; 3],
    /// The average linear speed the clip's own root motion implies, in Source
    /// units per second. Zero for the in-place clips CS2 actually ships.
    pub root_motion_speed: f32,
}

impl NmClip {
    /// Decode a `vnmclip_c`.
    ///
    /// # Errors
    ///
    /// Fails if the resource has no KV3 `DATA` block or the clip's frame
    /// offsets and compressed data do not agree.
    pub fn from_resource(resource: &Resource<'_>) -> Result<Self> {
        let doc = resource
            .data_kv3()
            .ok_or(Source2Error::NotKv3 { fourcc: "DATA" })??;
        Self::from_kv(&doc.root)
    }

    /// Decode a clip from an already-parsed `DATA` root.
    ///
    /// # Errors
    ///
    /// As [`NmClip::from_resource`].
    pub fn from_kv(root: &KvValue) -> Result<Self> {
        let frame_count = root
            .get("m_nNumFrames")
            .and_then(KvValue::as_u64)
            .and_then(|v| usize::try_from(v).ok())
            .unwrap_or(0);
        if frame_count > MAX_FRAMES {
            return Err(Source2Error::InvalidSize {
                what: "clip frame count",
                value: frame_count as i64,
            });
        }

        #[allow(clippy::cast_possible_truncation)]
        let duration = root
            .get("m_flDuration")
            .and_then(KvValue::as_f64)
            .unwrap_or(0.0) as f32;

        let tracks: Vec<TrackSettings> = root
            .get("m_trackCompressionSettings")
            .and_then(KvValue::as_array)
            .unwrap_or(&[])
            .iter()
            .map(TrackSettings::from_kv)
            .collect();

        let words = match root.get("m_compressedPoseData") {
            Some(KvValue::Binary(bytes)) => bytes
                .chunks_exact(2)
                .map(|c| u16::from_le_bytes([c[0], c[1]]))
                .collect(),
            _ => Vec::new(),
        };

        let frame_offsets: Vec<usize> = root
            .get("m_compressedPoseOffsets")
            .and_then(KvValue::as_array)
            .unwrap_or(&[])
            .iter()
            .map(|v| {
                v.as_u64()
                    .and_then(|x| usize::try_from(x).ok())
                    .unwrap_or(usize::MAX)
            })
            .collect();

        if frame_count > 0 && frame_offsets.len() != frame_count {
            return Err(Source2Error::MissingField {
                what: "one pose offset per frame",
                resource: "vnmclip",
            });
        }

        let (root_motion_delta, root_motion_speed) = read_root_motion(root, duration);

        Ok(Self {
            skeleton: root
                .get("m_skeleton")
                .and_then(KvValue::as_str)
                .unwrap_or("")
                .to_string(),
            frame_count,
            duration,
            additive: root
                .get("m_bIsAdditive")
                .and_then(KvValue::as_bool)
                .unwrap_or(false),
            tracks,
            frame_offsets,
            words,
            root_motion_delta,
            root_motion_speed,
        })
    }

    /// Frames per second, derived from the frame count and duration.
    ///
    /// The last frame sits *on* the duration rather than one interval before
    /// it, so there are `frame_count - 1` intervals.
    #[must_use]
    pub fn frame_rate(&self) -> f32 {
        if self.frame_count < 2 || self.duration <= 0.0 {
            return 0.0;
        }
        #[allow(clippy::cast_precision_loss)]
        ((self.frame_count - 1) as f32 / self.duration)
    }

    /// The time in seconds at which frame `index` is sampled.
    #[must_use]
    pub fn frame_time(&self, index: usize) -> f32 {
        if self.frame_count < 2 {
            return 0.0;
        }
        #[allow(clippy::cast_precision_loss)]
        (index as f32 * self.duration / (self.frame_count - 1) as f32)
    }

    /// Decode one frame into local bone transforms, one per track.
    ///
    /// # Errors
    ///
    /// Fails if the frame index is out of range, or the frame's data runs past
    /// the end of the compressed blob.
    pub fn frame(&self, index: usize) -> Result<Pose> {
        let start = *self
            .frame_offsets
            .get(index)
            .ok_or(Source2Error::FrameOutOfRange {
                index,
                count: self.frame_count,
            })?;

        let mut cursor = start;
        let mut out = Vec::with_capacity(self.tracks.len());
        for track in &self.tracks {
            let mut transform = track.constant();

            if !track.rotation_static {
                let w = self
                    .words
                    .get(cursor..cursor + ROTATION_WORDS)
                    .ok_or(Source2Error::ClipDataTruncated { frame: index })?;
                transform.rotation = decode_rotation([w[0], w[1], w[2]]);
                cursor += ROTATION_WORDS;
            }
            if !track.translation_static {
                let w = self
                    .words
                    .get(cursor..cursor + TRANSLATION_WORDS)
                    .ok_or(Source2Error::ClipDataTruncated { frame: index })?;
                for (axis, slot) in transform.translation.iter_mut().enumerate() {
                    let (start, length) = track.translation_range[axis];
                    *slot = decode_float(w[axis], start, length);
                }
                cursor += TRANSLATION_WORDS;
            }
            if !track.scale_static {
                let w = *self
                    .words
                    .get(cursor)
                    .ok_or(Source2Error::ClipDataTruncated { frame: index })?;
                transform.scale = decode_float(w, track.scale_range.0, track.scale_range.1);
                cursor += 1;
            }

            out.push(transform);
        }
        Ok(out)
    }

    /// `u16` words one frame is expected to occupy, from the track settings.
    ///
    /// Every shipped clip's `m_compressedPoseOffsets` advances by exactly this,
    /// which is what makes it a useful consistency check.
    #[must_use]
    pub fn frame_stride(&self) -> usize {
        self.tracks.iter().map(TrackSettings::words).sum()
    }

    /// Whether the frame offsets advance by [`NmClip::frame_stride`] each time.
    #[must_use]
    pub fn offsets_are_uniform(&self) -> bool {
        let stride = self.frame_stride();
        self.frame_offsets
            .iter()
            .enumerate()
            .all(|(i, &o)| o == i * stride)
    }
}

/// Read `m_rootMotion`, returning the total translation delta and the average
/// linear speed it implies.
fn read_root_motion(root: &KvValue, duration: f32) -> ([f32; 3], f32) {
    let Some(motion) = root.get("m_rootMotion") else {
        return ([0.0; 3], 0.0);
    };
    let mut delta = [0.0f32; 3];
    if let Some(total) = motion.get("m_totalDelta").and_then(KvValue::as_array) {
        for (i, slot) in delta.iter_mut().enumerate() {
            #[allow(clippy::cast_possible_truncation)]
            if let Some(v) = total.get(i).and_then(KvValue::as_f64) {
                *slot = v as f32;
            }
        }
    }

    // The clip states its own average speed; prefer it, and fall back to the
    // delta over the duration when it is absent.
    #[allow(clippy::cast_possible_truncation)]
    let stated = motion
        .get("m_flAverageLinearVelocity")
        .and_then(KvValue::as_f64)
        .unwrap_or(0.0) as f32;
    if stated > 0.0 {
        return (delta, stated);
    }

    let distance = (delta[0] * delta[0] + delta[1] * delta[1] + delta[2] * delta[2]).sqrt();
    let speed = if duration > 0.0 {
        distance / duration
    } else {
        0.0
    };
    (delta, speed)
}

/// Expand a 16-bit unsigned-normalised value through a quantisation range.
#[must_use]
pub fn decode_float(encoded: u16, range_start: f32, range_length: f32) -> f32 {
    (f32::from(encoded) / f32::from(u16::MAX)).mul_add(range_length, range_start)
}

/// Rebuild a unit quaternion from the smallest-three encoding.
///
/// Three 15-bit components span ±1/sqrt(2), which is the largest any of the
/// three *smaller* components of a unit quaternion can be. The fourth is
/// recovered as the positive root of what is left, and rotated back into place
/// using the two-bit index split across the top bits of the first two words.
#[must_use]
pub fn decode_rotation(words: [u16; 3]) -> [f32; 4] {
    const RANGE_MIN: f32 = -std::f32::consts::FRAC_1_SQRT_2;
    const RANGE_LENGTH: f32 = 2.0 * std::f32::consts::FRAC_1_SQRT_2;
    const SCALE: f32 = RANGE_LENGTH / 0x7FFF as f32;

    let component = |v: u16| f32::from(v & 0x7FFF).mul_add(SCALE, RANGE_MIN);
    let a = component(words[0]);
    let b = component(words[1]);
    // The third word has no index bit to mask off, but masking it anyway would
    // silently drop its top bit, so it is taken whole.
    let c = component(words[2] & 0x7FFF);

    let remainder = 1.0 - (a * a + b * b + c * c);
    let d = if remainder > 0.0 {
        remainder.sqrt()
    } else {
        0.0
    };

    let largest = ((words[0] >> 14) & 0x0002) | (words[1] >> 15);
    let q = match largest {
        0 => [d, a, b, c],
        1 => [a, d, b, c],
        2 => [a, b, d, c],
        _ => [a, b, c, d],
    };
    normalise(q)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsigned_normalised_values_span_the_range() {
        assert!((decode_float(0, 5.0, 10.0) - 5.0).abs() < 1e-6);
        assert!((decode_float(u16::MAX, 5.0, 10.0) - 15.0).abs() < 1e-5);
        assert!((decode_float(u16::MAX / 2, 0.0, 2.0) - 1.0).abs() < 1e-3);
        // A zero-length range collapses to its start, which is how a static
        // component's constant is stored.
        assert!((decode_float(12345, -3.0, 0.0) + 3.0).abs() < 1e-6);
    }

    #[test]
    fn decoded_rotations_are_unit_length() {
        for words in [
            [0u16, 0, 0],
            [0xFFFF, 0xFFFF, 0xFFFF],
            [0x4000, 0x1234, 0x7FFF],
            [0x8000, 0x8000, 0x0000],
            [0x1111, 0x2222, 0x3333],
        ] {
            let q = decode_rotation(words);
            let len = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]).sqrt();
            assert!((len - 1.0).abs() < 1e-4, "{words:04x?} gave {q:?}");
        }
    }

    #[test]
    fn the_index_bits_choose_which_component_was_dropped() {
        // Mid-range in all three words is zero for each stored component, so
        // the recovered one is 1 and lands wherever the index says.
        let mid = 0x3FFF_u16;
        for (index, expected) in [(0u16, 0usize), (1, 1), (2, 2), (3, 3)] {
            let w0 = mid | ((index & 0x2) << 14);
            let w1 = mid | ((index & 0x1) << 15);
            let q = decode_rotation([w0, w1, mid]);
            assert!(
                q[expected].abs() > 0.99,
                "index {index} put the largest component at {q:?}"
            );
        }
    }

    /// Build a clip whose tracks are all static, so the decoded frame is
    /// exactly the constants, with no compressed data needed at all.
    #[test]
    fn static_tracks_need_no_compressed_data() {
        let track = TrackSettings {
            translation_range: [(1.0, 0.0), (2.0, 0.0), (3.0, 0.0)],
            scale_range: (1.0, 0.0),
            constant_rotation: [0.0, 0.0, 0.0, 1.0],
            rotation_static: true,
            translation_static: true,
            scale_static: true,
        };
        let clip = NmClip {
            skeleton: String::new(),
            frame_count: 2,
            duration: 1.0,
            additive: false,
            tracks: vec![track],
            frame_offsets: vec![0, 0],
            words: Vec::new(),
            root_motion_delta: [0.0; 3],
            root_motion_speed: 0.0,
        };
        assert_eq!(clip.frame_stride(), 0);
        let frame = clip.frame(0).expect("static frame");
        assert_eq!(frame.len(), 1);
        assert_eq!(frame[0].translation, [1.0, 2.0, 3.0]);
        assert!(clip.frame(5).is_err());
        assert!((clip.frame_rate() - 1.0).abs() < 1e-6);
    }

    #[test]
    fn a_truncated_blob_errors_rather_than_panicking() {
        let track = TrackSettings {
            translation_range: [(0.0, 1.0); 3],
            scale_range: (1.0, 0.0),
            constant_rotation: [0.0, 0.0, 0.0, 1.0],
            rotation_static: false,
            translation_static: false,
            scale_static: true,
        };
        let clip = NmClip {
            skeleton: String::new(),
            frame_count: 1,
            duration: 1.0,
            additive: false,
            tracks: vec![track],
            frame_offsets: vec![0],
            // Six words are needed; two are provided.
            words: vec![0, 0],
            root_motion_delta: [0.0; 3],
            root_motion_speed: 0.0,
        };
        assert_eq!(clip.frame_stride(), 6);
        assert!(matches!(
            clip.frame(0),
            Err(Source2Error::ClipDataTruncated { frame: 0 })
        ));
    }

    #[test]
    fn frame_times_span_the_duration() {
        let clip = NmClip {
            skeleton: String::new(),
            frame_count: 5,
            duration: 2.0,
            additive: false,
            tracks: Vec::new(),
            frame_offsets: vec![0; 5],
            words: Vec::new(),
            root_motion_delta: [0.0; 3],
            root_motion_speed: 0.0,
        };
        assert!((clip.frame_time(0)).abs() < 1e-6);
        assert!((clip.frame_time(4) - 2.0).abs() < 1e-6);
        assert!((clip.frame_time(2) - 1.0).abs() < 1e-6);
        assert!((clip.frame_rate() - 2.0).abs() < 1e-6);
    }

    #[test]
    fn track_settings_default_to_static_when_absent() {
        let settings = TrackSettings::from_kv(&KvValue::Object(Vec::new()));
        assert!(settings.rotation_static);
        assert!(settings.translation_static);
        assert!(settings.scale_static);
        assert_eq!(settings.words(), 0);
    }

    #[test]
    fn a_clip_with_no_data_decodes_to_nothing() {
        let clip = NmClip::from_kv(&KvValue::Object(Vec::new())).expect("empty clip");
        assert_eq!(clip.frame_count, 0);
        assert!(clip.tracks.is_empty());
        assert_eq!(clip.frame_rate(), 0.0);
    }
}
