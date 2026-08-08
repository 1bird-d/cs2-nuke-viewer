//! A free-fly camera.
//!
//! Roll-free by design: yaw and pitch only, with pitch clamped just short of
//! vertical. A camera that slowly rolls is the fastest way to make a flythrough
//! look amateur, and nothing in this project wants a horizon that is not level.

use glam::{Mat4, Quat, Vec3};

/// Where the near plane sits. Small, because the pipework is often centimetres
/// from the lens when you fly inside a run.
const NEAR: f32 = 0.05;

/// Just short of straight up, so yaw never becomes ambiguous.
const PITCH_LIMIT: f32 = std::f32::consts::FRAC_PI_2 - 0.001;

/// A yaw/pitch fly camera in Y-up metres.
pub struct Camera {
    /// Eye position, in world metres.
    pub position: Vec3,
    /// Rotation about +Y, radians.
    pub yaw: f32,
    /// Rotation about the camera's right axis, radians, clamped near vertical.
    pub pitch: f32,
    /// Vertical field of view, radians.
    pub fov_y: f32,
    /// Metres per second at neutral speed.
    pub speed: f32,
}

impl Default for Camera {
    fn default() -> Self {
        Self {
            position: Vec3::ZERO,
            yaw: 0.0,
            pitch: 0.0,
            fov_y: 70f32.to_radians(),
            speed: 12.0,
        }
    }
}

impl Camera {
    /// Unit vector the camera is looking along.
    #[must_use]
    pub fn forward(&self) -> Vec3 {
        let (sy, cy) = self.yaw.sin_cos();
        let (sp, cp) = self.pitch.sin_cos();
        Vec3::new(-sy * cp, sp, -cy * cp)
    }

    /// Unit vector to the camera's right, always level.
    #[must_use]
    pub fn right(&self) -> Vec3 {
        let (sy, cy) = self.yaw.sin_cos();
        Vec3::new(cy, 0.0, -sy)
    }

    /// Turn by a mouse delta in pixels.
    pub fn look(&mut self, dx: f32, dy: f32, sensitivity: f32) {
        self.yaw -= dx * sensitivity;
        self.pitch = (self.pitch - dy * sensitivity).clamp(-PITCH_LIMIT, PITCH_LIMIT);
    }

    /// Move along the camera axes. `up` is world up, not camera up, so rising
    /// while looking down does not drift forward.
    pub fn fly(&mut self, forward: f32, right: f32, up: f32, dt: f32, boost: f32) {
        let velocity =
            self.forward() * forward + self.right() * right + Vec3::Y * up;
        if velocity.length_squared() > 0.0 {
            self.position += velocity.normalize() * self.speed * boost * dt;
        }
    }

    /// View-projection with a reverse-Z infinite far plane.
    ///
    /// Reverse-Z spends float precision where it is needed. de_nuke is 320 m
    /// across and the near plane is 5 cm, which is exactly the ratio that makes
    /// a conventional depth buffer z-fight on distant catwalk grating.
    #[must_use]
    pub fn view_projection(&self, aspect: f32) -> Mat4 {
        let rotation = Quat::from_rotation_y(self.yaw) * Quat::from_rotation_x(self.pitch);
        let view = Mat4::from_rotation_translation(rotation, self.position).inverse();
        // The `directx` variant is the 0..1 clip-space depth range wgpu uses;
        // the `opengl` one would put the near plane at -1 and break the test.
        let proj = glam::camera::rh::proj::directx::perspective_infinite_reverse(
            self.fov_y,
            aspect.max(0.001),
            NEAR,
        );
        proj * view
    }

    /// Point the camera at a box from outside it, far enough to see all of it.
    ///
    /// Fits the box to the frustum on both axes rather than backing off by the
    /// bounding *sphere*. de_nuke's pipework is 178 m wide and 66 m tall, and
    /// the diagonal of a box that flat over-estimates badly — framing by radius
    /// left the pipes as a smudge in the middle of an empty frame.
    pub fn frame(&mut self, min: Vec3, max: Vec3, aspect: f32) {
        let centre = (min + max) * 0.5;
        let half = ((max - min) * 0.5).max(Vec3::splat(0.5));

        let tan_v = (self.fov_y * 0.5).tan();
        let tan_h = tan_v * aspect.max(0.001);
        // 8% margin so nothing sits exactly on the frame edge.
        let distance = (half.y / tan_v).max(half.x / tan_h).mul_add(1.08, half.z);

        self.position = centre + Vec3::new(0.0, distance * 0.32, distance);
        let to_centre = centre - self.position;
        self.yaw = (-to_centre.x).atan2(-to_centre.z);
        self.pitch = (to_centre.y / to_centre.length()).asin();
        self.speed = (half.length() * 0.12).clamp(2.0, 60.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_default_camera_looks_down_negative_z() {
        let camera = Camera::default();
        let forward = camera.forward();
        assert!((forward.z + 1.0).abs() < 1e-5, "{forward:?}");
        assert!(forward.x.abs() < 1e-5 && forward.y.abs() < 1e-5, "{forward:?}");
    }

    #[test]
    fn forward_and_right_stay_perpendicular_at_any_angle() {
        let mut camera = Camera::default();
        camera.yaw = 1.1;
        camera.pitch = -0.7;
        assert!(camera.forward().dot(camera.right()).abs() < 1e-5);
        assert!(camera.right().y.abs() < 1e-6, "right must stay level");
    }

    #[test]
    fn pitch_never_reaches_vertical() {
        let mut camera = Camera::default();
        camera.look(0.0, -1e6, 0.01);
        assert!(camera.pitch < std::f32::consts::FRAC_PI_2);
        camera.look(0.0, 1e6, 0.01);
        assert!(camera.pitch > -std::f32::consts::FRAC_PI_2);
    }

    /// Reverse-Z: the near plane must map to depth 1 and far away to 0.
    #[test]
    fn the_projection_is_reverse_z() {
        let camera = Camera::default();
        let vp = camera.view_projection(1.0);
        let near = vp.project_point3(Vec3::new(0.0, 0.0, -NEAR));
        let far = vp.project_point3(Vec3::new(0.0, 0.0, -10_000.0));
        assert!((near.z - 1.0).abs() < 1e-3, "near depth {}", near.z);
        assert!(far.z < 0.01, "far depth {}", far.z);
    }

    #[test]
    fn framing_a_box_points_the_camera_at_it() {
        let mut camera = Camera::default();
        let min = Vec3::new(-10.0, -2.0, -10.0);
        let max = Vec3::new(10.0, 2.0, 10.0);
        camera.frame(min, max, 16.0 / 9.0);
        let centre = (min + max) * 0.5;
        let to_centre = (centre - camera.position).normalize();
        assert!(camera.forward().dot(to_centre) > 0.999, "camera looks away");
    }

    /// Every corner of the framed box has to land inside the frustum. This is
    /// the property the old bounding-sphere framing satisfied far too loosely.
    #[test]
    fn framing_fits_the_whole_box_on_screen() {
        let aspect = 16.0 / 9.0;
        // de_nuke's pipework: wide, shallow, and nothing like a sphere.
        let min = Vec3::new(-89.0, -33.0, -49.0);
        let max = Vec3::new(89.0, 33.0, 49.0);
        let mut camera = Camera::default();
        camera.frame(min, max, aspect);
        let vp = camera.view_projection(aspect);

        let mut worst: f32 = 0.0;
        for corner in 0..8u8 {
            let p = Vec3::new(
                if corner & 1 == 0 { min.x } else { max.x },
                if corner & 2 == 0 { min.y } else { max.y },
                if corner & 4 == 0 { min.z } else { max.z },
            );
            let ndc = vp.project_point3(p);
            assert!(ndc.x.abs() <= 1.0, "corner off screen in x: {ndc:?}");
            assert!(ndc.y.abs() <= 1.0, "corner off screen in y: {ndc:?}");
            worst = worst.max(ndc.x.abs()).max(ndc.y.abs());
        }
        // And it should actually fill the frame, not sit in the middle of it.
        assert!(worst > 0.7, "box only fills {worst:.2} of the frame");
    }
}
