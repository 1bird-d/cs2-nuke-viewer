//! Clicking on a pipe.
//!
//! This is a CPU ray cast, not a GPU id-buffer readback. The scene is already
//! memory-mapped with world AABBs per instance, so a click becomes: reject
//! almost everything with a ray/AABB test, then run exact ray/triangle over the
//! handful of survivors, nearest first. No extra pipeline, no extra render
//! target, no frame of readback latency — and it is exact, which matters when
//! you are trying to hit one pipe among 1,927.
//!
//! Only *solid* instances are pickable. Ghosted geometry is context, and being
//! able to select the wall in front of the run you are trying to tag would make
//! the tool infuriating.

use glam::{Mat4, Vec3};
use nkp_format::{Instance, Scene};

use crate::category::{Classification, Mode};

/// A ray in world space.
pub struct Ray {
    /// Where it starts.
    pub origin: Vec3,
    /// Unit direction.
    pub direction: Vec3,
}

impl Ray {
    /// The ray through a pixel, given the inverse of the view-projection.
    ///
    /// `ndc` is in clip space: x and y in -1..1 with y up.
    ///
    /// The depths matter. The projection is reverse-Z *and infinite*, so NDC
    /// depth 0 is the far plane at infinity — unprojecting it divides by a zero
    /// w and yields a non-finite point, which then normalises to zero and makes
    /// every pick silently miss. Two finite depths are taken instead and the
    /// direction comes from the difference between them.
    #[must_use]
    pub fn through(inverse_view_proj: Mat4, ndc: (f32, f32), origin: Vec3) -> Self {
        let near = inverse_view_proj.project_point3(Vec3::new(ndc.0, ndc.1, 1.0));
        let mid = inverse_view_proj.project_point3(Vec3::new(ndc.0, ndc.1, 0.01));
        let direction = (mid - near).normalize_or_zero();
        Self {
            // Start at the near plane rather than the eye, so geometry between
            // the two — which is not drawn — cannot be picked either.
            origin: if near.is_finite() { near } else { origin },
            direction: if direction.is_finite() {
                direction
            } else {
                Vec3::ZERO
            },
        }
    }
}

/// What a click landed on.
pub struct Hit {
    /// Index into `scene.instances()`.
    pub instance: usize,
    /// Distance along the ray, in metres.
    pub distance: f32,
    /// Where it struck, in world metres.
    pub point: Vec3,
}

/// The nearest solid instance under `ray`, if any.
#[must_use]
pub fn nearest(scene: &Scene, classification: &Classification, ray: &Ray) -> Option<Hit> {
    if ray.direction == Vec3::ZERO {
        return None;
    }
    let inverse = Vec3::new(
        1.0 / ray.direction.x,
        1.0 / ray.direction.y,
        1.0 / ray.direction.z,
    );

    // Shortlist by AABB, keeping the entry distance so the exact pass can go
    // nearest-first and stop early.
    let mut candidates: Vec<(f32, usize)> = Vec::new();
    for (id, instance) in scene.instances().iter().enumerate() {
        if instance.index_count == 0 || classification.category_of(id).mode != Mode::Solid {
            continue;
        }
        if let Some(t) = slab(ray.origin, inverse, instance.aabb_min, instance.aabb_max) {
            candidates.push((t, id));
        }
    }
    candidates.sort_unstable_by(|a, b| a.0.total_cmp(&b.0));

    let vertices = scene.vertices();
    let indices = scene.indices();
    let mut best: Option<Hit> = None;

    for (entry, id) in candidates {
        // Everything left enters the box further away than the hit we already
        // have, so nothing left can beat it.
        if best.as_ref().is_some_and(|hit| entry > hit.distance) {
            break;
        }
        let instance = &scene.instances()[id];
        let Some(local) = local_ray(instance, ray) else {
            continue;
        };

        let first = instance.first_index as usize;
        let count = instance.index_count as usize;
        let base = instance.base_vertex as usize;
        let Some(range) = indices.get(first..first + count) else {
            continue;
        };

        for triangle in range.chunks_exact(3) {
            let fetch = |i: u32| -> Option<Vec3> {
                vertices
                    .get(base + i as usize)
                    .map(|v| Vec3::from(v.position))
            };
            let (Some(a), Some(b), Some(c)) =
                (fetch(triangle[0]), fetch(triangle[1]), fetch(triangle[2]))
            else {
                continue;
            };
            let Some(t) = moller_trumbore(local.0, local.1, a, b, c) else {
                continue;
            };
            // `t` is along the local ray, whose direction was not renormalised,
            // so it is directly comparable with world distance.
            if best.as_ref().is_none_or(|hit| t < hit.distance) {
                best = Some(Hit {
                    instance: id,
                    distance: t,
                    point: ray.origin + ray.direction * t,
                });
            }
        }
    }
    best
}

/// Ray/AABB slab test. Returns the entry distance, clamped to zero when the
/// ray starts inside.
fn slab(origin: Vec3, inverse_direction: Vec3, min: [f32; 3], max: [f32; 3]) -> Option<f32> {
    let mut near = f32::NEG_INFINITY;
    let mut far = f32::INFINITY;
    for axis in 0..3 {
        let t1 = (min[axis] - origin[axis]) * inverse_direction[axis];
        let t2 = (max[axis] - origin[axis]) * inverse_direction[axis];
        near = near.max(t1.min(t2));
        far = far.min(t1.max(t2));
    }
    (far >= near.max(0.0)).then(|| near.max(0.0))
}

/// Move the ray into an instance's local space.
///
/// Cheaper and more accurate than transforming every triangle into world space:
/// one matrix inverse per instance instead of three points per triangle. The
/// returned direction is deliberately *not* renormalised, so distances along it
/// come back already in world metres.
fn local_ray(instance: &Instance, ray: &Ray) -> Option<(Vec3, Vec3)> {
    let t = &instance.transform;
    let m = glam::Mat3::from_cols(
        Vec3::new(t[0], t[4], t[8]),
        Vec3::new(t[1], t[5], t[9]),
        Vec3::new(t[2], t[6], t[10]),
    );
    let determinant = m.determinant();
    if determinant.abs() < 1e-12 {
        return None;
    }
    let inverse = m.inverse();
    let translation = Vec3::new(t[3], t[7], t[11]);
    Some((
        inverse * (ray.origin - translation),
        inverse * ray.direction,
    ))
}

/// Möller–Trumbore. Two-sided, because Source geometry is inconsistently wound
/// and the renderer does not cull either.
fn moller_trumbore(origin: Vec3, direction: Vec3, a: Vec3, b: Vec3, c: Vec3) -> Option<f32> {
    const EPSILON: f32 = 1e-7;
    let edge1 = b - a;
    let edge2 = c - a;
    let h = direction.cross(edge2);
    let det = edge1.dot(h);
    if det.abs() < EPSILON {
        return None;
    }
    let inv_det = 1.0 / det;
    let s = origin - a;
    let u = inv_det * s.dot(h);
    if !(-EPSILON..=1.0 + EPSILON).contains(&u) {
        return None;
    }
    let q = s.cross(edge1);
    let v = inv_det * direction.dot(q);
    if v < -EPSILON || u + v > 1.0 + EPSILON {
        return None;
    }
    let t = inv_det * edge2.dot(q);
    (t > EPSILON).then_some(t)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The regression that made every hover and click miss: an infinite
    /// reverse-Z projection has no finite point at NDC depth 0, so building the
    /// ray from one produced a non-finite direction that normalised to zero.
    #[test]
    fn a_screen_ray_is_finite_and_points_where_the_camera_looks() {
        let camera = crate::camera::Camera::default();
        let aspect = 16.0 / 9.0;
        let view_proj = camera.view_projection(aspect);
        let inverse = view_proj.inverse();

        for ndc in [(0.0, 0.0), (-0.9, 0.9), (0.9, -0.9)] {
            let ray = Ray::through(inverse, ndc, camera.position);
            assert!(ray.direction.is_finite(), "{ndc:?} gave {:?}", ray.direction);
            assert!(
                (ray.direction.length() - 1.0).abs() < 1e-3,
                "{ndc:?} direction is not unit: {:?}",
                ray.direction
            );
            // Every ray through the frustum must go roughly the way the camera
            // is facing.
            assert!(
                ray.direction.dot(camera.forward()) > 0.5,
                "{ndc:?} points away from the view direction"
            );
        }

        // Dead centre must be the view direction itself.
        let centre = Ray::through(inverse, (0.0, 0.0), camera.position);
        assert!(
            centre.direction.dot(camera.forward()) > 0.999,
            "centre ray {:?} is not the forward vector {:?}",
            centre.direction,
            camera.forward()
        );
    }

    /// And it has to survive an off-axis camera, not just the default one.
    #[test]
    fn a_screen_ray_tracks_a_rotated_camera() {
        let mut camera = crate::camera::Camera::default();
        camera.position = Vec3::new(17.0, 22.0, 58.0);
        camera.yaw = 0.7;
        camera.pitch = -0.3;
        let inverse = camera.view_projection(16.0 / 9.0).inverse();
        let ray = Ray::through(inverse, (0.0, 0.0), camera.position);
        assert!(ray.direction.dot(camera.forward()) > 0.999, "{:?}", ray.direction);
    }

    #[test]
    fn a_ray_hits_a_triangle_it_points_at() {
        let hit = moller_trumbore(
            Vec3::new(0.25, 0.25, 1.0),
            Vec3::new(0.0, 0.0, -1.0),
            Vec3::ZERO,
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
        );
        assert!((hit.expect("should hit") - 1.0).abs() < 1e-5);
    }

    #[test]
    fn a_ray_misses_a_triangle_beside_it() {
        assert!(moller_trumbore(
            Vec3::new(5.0, 5.0, 1.0),
            Vec3::new(0.0, 0.0, -1.0),
            Vec3::ZERO,
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
        )
        .is_none());
    }

    /// Winding must not matter: the renderer draws both faces, so picking has
    /// to agree with it.
    #[test]
    fn triangles_are_pickable_from_behind() {
        let back = moller_trumbore(
            Vec3::new(0.25, 0.25, -1.0),
            Vec3::new(0.0, 0.0, 1.0),
            Vec3::ZERO,
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
        );
        assert!(back.is_some(), "back face should still be pickable");
    }

    #[test]
    fn the_slab_test_accepts_a_box_ahead_and_rejects_one_behind() {
        let origin = Vec3::new(0.0, 0.0, 10.0);
        let inverse = Vec3::new(f32::INFINITY, f32::INFINITY, -1.0);
        assert!(slab(origin, inverse, [-1.0, -1.0, -1.0], [1.0, 1.0, 1.0]).is_some());
        assert!(slab(origin, inverse, [-1.0, -1.0, 20.0], [1.0, 1.0, 22.0]).is_none());
    }

    #[test]
    fn a_ray_starting_inside_a_box_enters_at_zero() {
        let inverse = Vec3::new(f32::INFINITY, f32::INFINITY, -1.0);
        let t = slab(Vec3::ZERO, inverse, [-1.0, -1.0, -1.0], [1.0, 1.0, 1.0]);
        assert_eq!(t, Some(0.0));
    }

    /// The real thing: fire rays at the baked map and check we hit pipework and
    /// nothing else. Skipped when the scene has not been baked, so a fresh
    /// clone still passes `cargo test`.
    #[test]
    fn rays_into_the_baked_scene_hit_only_solid_pipework() {
        let path = std::path::Path::new("../../scenes/de_nuke.nkp");
        let Ok(scene) = nkp_format::Scene::open(path) else {
            eprintln!("skipping: {} has not been baked", path.display());
            return;
        };
        let mut classification = Classification::new(&scene).expect("classify");
        classification.preset_pipes_only();

        // Aim at real pipes rather than sweeping blindly: a fan across open air
        // mostly misses, because pipes are thin. Take a spread of pipe
        // instances and fire at the centre of each one's bounding box, standing
        // well back so the ray has to travel through the scene to get there.
        let pipes: Vec<usize> = scene
            .instances()
            .iter()
            .enumerate()
            .filter(|(id, _)| classification.category_of(*id).id == "pipe")
            .map(|(id, _)| id)
            .collect();
        assert!(pipes.len() > 1000, "expected the pipe kit, found {}", pipes.len());

        let mut hits = 0;
        let mut aimed = 0;
        for &id in pipes.iter().step_by(pipes.len() / 60) {
            let instance = &scene.instances()[id];
            let centre = (Vec3::from(instance.aabb_min) + Vec3::from(instance.aabb_max)) * 0.5;
            // Stand 25 m away, up and to one side, so the ray is oblique.
            let origin = centre + Vec3::new(12.0, 18.0, 12.0);
            let ray = Ray {
                origin,
                direction: (centre - origin).normalize(),
            };
            aimed += 1;

            if let Some(hit) = nearest(&scene, &classification, &ray) {
                hits += 1;
                assert!(hit.distance > 0.0, "hit behind the camera");
                // It need not be the pipe we aimed at — another piece may be in
                // the way — but it must be *solid*. That is the invariant:
                // ghosted context and hidden geometry are never pickable.
                assert_eq!(
                    classification.category_of(hit.instance).mode,
                    Mode::Solid,
                    "picked {} which is not solid",
                    classification.category_of(hit.instance).id
                );
                let along = (hit.point - origin).length();
                assert!(
                    (along - hit.distance).abs() < 0.05,
                    "hit point {along:.3} m does not match distance {:.3} m",
                    hit.distance
                );
            }
        }
        assert!(
            hits * 2 >= aimed,
            "only {hits} of {aimed} rays aimed at a pipe hit anything"
        );
    }

    /// End to end, the way the viewer actually does it: build the ray from a
    /// screen pixel and a real camera, and hit the containment shell that is
    /// filling the middle of the frame.
    #[test]
    fn the_centre_pixel_picks_what_is_in_front_of_the_camera() {
        let path = std::path::Path::new("../../scenes/de_nuke.nkp");
        let Ok(scene) = nkp_format::Scene::open(path) else {
            return;
        };
        let mut classification = Classification::new(&scene).expect("classify");
        classification.preset_plant();

        // Standing off the big shell, looking straight at it along -Z.
        let mut camera = crate::camera::Camera::default();
        camera.position = Vec3::new(17.0, 22.0, 58.0);
        camera.yaw = 0.0;
        camera.pitch = 0.0;
        let inverse = camera.view_projection(16.0 / 9.0).inverse();
        let ray = Ray::through(inverse, (0.0, 0.0), camera.position);
        assert!(ray.direction.is_finite() && ray.direction.length() > 0.5);

        let hit = nearest(&scene, &classification, &ray)
            .expect("the containment shell is dead ahead and must be picked");
        assert!(
            hit.distance > 40.0 && hit.distance < 60.0,
            "hit at {:.1} m, expected the shell face around 47 m",
            hit.distance
        );
        let instance = &scene.instances()[hit.instance];
        let material = scene
            .materials()
            .get(instance.material as usize)
            .map_or("", |m| scene.material_name(m));
        assert!(
            material.contains("nuke_silo"),
            "picked {material} instead of the shell"
        );
    }

    /// Hidden and ghosted geometry must be unpickable, or tagging a run means
    /// fighting the wall in front of it.
    #[test]
    fn nothing_is_picked_when_everything_is_hidden() {
        let path = std::path::Path::new("../../scenes/de_nuke.nkp");
        let Ok(scene) = nkp_format::Scene::open(path) else {
            return;
        };
        let mut classification = Classification::new(&scene).expect("classify");
        classification.set_all(Mode::Hidden);
        let ray = Ray {
            origin: Vec3::new(20.0, 70.0, 40.0),
            direction: (Vec3::new(20.0, 4.0, 25.0) - Vec3::new(20.0, 70.0, 40.0)).normalize(),
        };
        assert!(nearest(&scene, &classification, &ray).is_none());
    }
}
