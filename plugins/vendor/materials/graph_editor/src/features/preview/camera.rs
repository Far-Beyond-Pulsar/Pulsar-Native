//! Simple orbit camera for 3D material preview

pub struct OrbitCamera {
    pub yaw: f32,
    pub pitch: f32,
    pub distance: f32,
    pub target: [f32; 3],
    pub fov_y: f32,
    pub aspect: f32,
    pub near: f32,
    pub far: f32,
}

impl OrbitCamera {
    pub fn view_matrix(&self) -> [[f32; 4]; 4] {
        let (yaw_s, yaw_c) = (self.yaw.sin(), self.yaw.cos());
        let (pitch_s, pitch_c) = (self.pitch.sin(), self.pitch.cos());

        let eye = [
            self.target[0] + self.distance * pitch_c * yaw_s,
            self.target[1] + self.distance * pitch_s,
            self.target[2] + self.distance * pitch_c * yaw_c,
        ];

        let forward = [
            self.target[0] - eye[0],
            self.target[1] - eye[1],
            self.target[2] - eye[2],
        ];
        let fwd_len =
            (forward[0] * forward[0] + forward[1] * forward[1] + forward[2] * forward[2]).sqrt();
        let fwd = [forward[0] / fwd_len, forward[1] / fwd_len, forward[2] / fwd_len];

        let world_up = [0.0, 1.0, 0.0];
        let right = [
            world_up[1] * fwd[2] - world_up[2] * fwd[1],
            world_up[2] * fwd[0] - world_up[0] * fwd[2],
            world_up[0] * fwd[1] - world_up[1] * fwd[0],
        ];
        let right_len =
            (right[0] * right[0] + right[1] * right[1] + right[2] * right[2]).sqrt();
        let r = [right[0] / right_len, right[1] / right_len, right[2] / right_len];

        let up = [
            fwd[1] * r[2] - fwd[2] * r[1],
            fwd[2] * r[0] - fwd[0] * r[2],
            fwd[0] * r[1] - fwd[1] * r[0],
        ];

        [
            [r[0], up[0], fwd[0], 0.0],
            [r[1], up[1], fwd[1], 0.0],
            [r[2], up[2], fwd[2], 0.0],
            [
                -(r[0] * eye[0] + r[1] * eye[1] + r[2] * eye[2]),
                -(up[0] * eye[0] + up[1] * eye[1] + up[2] * eye[2]),
                -(fwd[0] * eye[0] + fwd[1] * eye[1] + fwd[2] * eye[2]),
                1.0,
            ],
        ]
    }

    /// Right, up, and forward basis vectors of the camera in world space —
    /// used to reconstruct view rays for the fullscreen sky pass without
    /// needing to invert the view-projection matrix.
    pub fn basis_vectors(&self) -> ([f32; 3], [f32; 3], [f32; 3]) {
        let (yaw_s, yaw_c) = (self.yaw.sin(), self.yaw.cos());
        let (pitch_s, pitch_c) = (self.pitch.sin(), self.pitch.cos());

        let eye = [
            self.target[0] + self.distance * pitch_c * yaw_s,
            self.target[1] + self.distance * pitch_s,
            self.target[2] + self.distance * pitch_c * yaw_c,
        ];

        let forward = [
            self.target[0] - eye[0],
            self.target[1] - eye[1],
            self.target[2] - eye[2],
        ];
        let fwd_len =
            (forward[0] * forward[0] + forward[1] * forward[1] + forward[2] * forward[2]).sqrt();
        let fwd = [forward[0] / fwd_len, forward[1] / fwd_len, forward[2] / fwd_len];

        let world_up = [0.0, 1.0, 0.0];
        let right = [
            world_up[1] * fwd[2] - world_up[2] * fwd[1],
            world_up[2] * fwd[0] - world_up[0] * fwd[2],
            world_up[0] * fwd[1] - world_up[1] * fwd[0],
        ];
        let right_len =
            (right[0] * right[0] + right[1] * right[1] + right[2] * right[2]).sqrt();
        let r = [right[0] / right_len, right[1] / right_len, right[2] / right_len];

        let up = [
            fwd[1] * r[2] - fwd[2] * r[1],
            fwd[2] * r[0] - fwd[0] * r[2],
            fwd[0] * r[1] - fwd[1] * r[0],
        ];

        (r, up, fwd)
    }

    /// Point the camera at the origin and pick a `distance` so that a sphere
    /// of `radius` centered on the target fits comfortably inside the view
    /// frustum, with a small margin so the mesh takes up a bit less than the
    /// full frame.
    pub fn frame_bounding_radius(&mut self, radius: f32) {
        self.target = [0.0, 0.0, 0.0];

        let tan_y = (self.fov_y * 0.5).tan();
        let tan_x = tan_y * self.aspect;
        // The tighter of the two axes determines how far back the camera
        // needs to be to fit the whole mesh.
        let tan_eff = tan_y.min(tan_x).max(0.0001);

        // >1 leaves headroom around the mesh so it reads as "a bit less
        // than the frame" rather than touching the edges exactly.
        const FRAME_MARGIN: f32 = 1.25;

        self.distance = radius / tan_eff * FRAME_MARGIN;
    }

    pub fn projection_matrix(&self) -> [[f32; 4]; 4] {
        let f = 1.0 / (self.fov_y * 0.5).tan();
        let range_inv = 1.0 / (self.far - self.near);

        [
            [f / self.aspect, 0.0, 0.0, 0.0],
            [0.0, f, 0.0, 0.0],
            [0.0, 0.0, (self.far + self.near) * range_inv, 1.0],
            [0.0, 0.0, -(self.far * self.near * 2.0) * range_inv, 0.0],
        ]
    }
}
