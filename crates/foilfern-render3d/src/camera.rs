//! Orbit/pan/zoom controller for upstream's 3D board view (Z-up millimetres).
use bevy::math::{Vec2, Vec3};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OrbitCamera {
    pub target: Vec3,
    pub distance: f32,
    pub yaw: f32,
    pub pitch: f32,
    radius: f32,
}
impl OrbitCamera {
    pub fn fit(bounds: [[f32; 3]; 2]) -> Self {
        let low = Vec3::from(bounds[0]);
        let high = Vec3::from(bounds[1]);
        let radius = ((high - low).length() * 0.5).max(1.);
        Self {
            target: (low + high) * 0.5,
            distance: radius * 3.2,
            radius,
            yaw: -std::f32::consts::FRAC_PI_4,
            pitch: 0.85,
        }
    }
    pub fn position(&self) -> Vec3 {
        self.target
            + Vec3::new(
                libm::cosf(self.yaw) * libm::cosf(self.pitch),
                libm::sinf(self.yaw) * libm::cosf(self.pitch),
                libm::sinf(self.pitch),
            ) * self.distance
    }
    pub fn orbit(&mut self, delta: [f32; 2]) {
        self.yaw -= delta[0] * 0.008;
        self.pitch = (self.pitch + delta[1] * 0.008).clamp(-1.55, 1.55);
    }
    pub fn pan(&mut self, delta: [f32; 2], viewport_height: f32) {
        let forward = (self.target - self.position()).normalize();
        let right = forward.cross(Vec3::Z).normalize();
        let up = right.cross(forward).normalize();
        let d = Vec2::from(delta) * self.distance * 0.9 / viewport_height.max(1.);
        self.target += -right * d.x + up * d.y;
    }
    pub fn zoom(&mut self, steps: f32) {
        self.distance = (self.distance * libm::powf(1.2, -steps.clamp(-10., 10.)))
            .clamp(self.radius * 0.03, self.radius * 100.);
    }
    pub fn side(&mut self, bottom: bool) {
        self.yaw = -std::f32::consts::FRAC_PI_2;
        self.pitch = if bottom { -1.55 } else { 1.55 };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn orbit_can_reach_bottom_and_zoom_pan_stay_finite() {
        let mut camera = OrbitCamera::fit([[0.; 3], [100., 50., 1.6]]);
        let center = camera.target;
        camera.orbit([200., -400.]);
        assert!(camera.position().z < 0.);
        camera.pan([50., 20.], 600.);
        assert_ne!(camera.target, center);
        for _ in 0..100 {
            camera.zoom(10.);
        }
        assert!(camera.distance > 0.);
        assert!(camera.position().is_finite());
    }
}
