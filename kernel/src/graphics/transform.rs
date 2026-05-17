use core::simd::f32x4;
use micromath::F32Ext;

#[derive(Clone, Copy, Debug)]
#[repr(C, align(16))]
pub struct Matrix4x4 {
    pub rows: [f32x4; 4],
}

impl Matrix4x4 {
    pub fn identity() -> Self {
        Self {
            rows: [
                f32x4::from_array([1.0, 0.0, 0.0, 0.0]),
                f32x4::from_array([0.0, 1.0, 0.0, 0.0]),
                f32x4::from_array([0.0, 0.0, 1.0, 0.0]),
                f32x4::from_array([0.0, 0.0, 0.0, 1.0]),
            ]
        }
    }

    // #[inline(always)]
    // pub fn mul_vec(&self, v: f32x4) -> f32x4 {
    //     let x = f32x4::splat(v[0]) * self.rows[0];
    //     let y = f32x4::splat(v[1]) * self.rows[1];
    //     let z = f32x4::splat(v[2]) * self.rows[2];
    //     let w = f32x4::splat(v[3]) * self.rows[3];
    //     x + y + z + w
    // }

    #[inline(always)]
    pub fn mul_vec(&self, v: f32x4) -> f32x4 {
        f32x4::from_array([
            self.rows[0][0] * v[0] +
            self.rows[0][1] * v[1] +
            self.rows[0][2] * v[2] +
            self.rows[0][3] * v[3],

            self.rows[1][0] * v[0] +
            self.rows[1][1] * v[1] +
            self.rows[1][2] * v[2] +
            self.rows[1][3] * v[3],

            self.rows[2][0] * v[0] +
            self.rows[2][1] * v[1] +
            self.rows[2][2] * v[2] +
            self.rows[2][3] * v[3],

            self.rows[3][0] * v[0] +
            self.rows[3][1] * v[1] +
            self.rows[3][2] * v[2] +
            self.rows[3][3] * v[3],
        ])
    }

    pub fn mul(&self, other: &Matrix4x4) -> Matrix4x4 {
        // Matrix multiplication
        let mut result = Matrix4x4::identity();
        for i in 0..4 {
            let row = self.rows[i];
            for j in 0..4 {
                let col = f32x4::from_array([
                    other.rows[0][j],
                    other.rows[1][j],
                    other.rows[2][j],
                    other.rows[3][j],
                ]);
                // Dot product
                let product = row * col;
                result.rows[i][j] = product[0] + product[1] + product[2] + product[3];
            }
        }
        result
    }

    pub fn look_at(eye: f32x4, target: f32x4, up: f32x4) -> Matrix4x4 {
        let f = (target - eye).normalize();
        let s = f.cross(up).normalize();
        let u = s.cross(f);

        Matrix4x4 {
            rows: [
                f32x4::from_array([ s[0],  s[1],  s[2], -s.dot3(eye)]),
                f32x4::from_array([ u[0],  u[1],  u[2], -u.dot3(eye)]),
                f32x4::from_array([-f[0], -f[1], -f[2],  f.dot3(eye)]),
                f32x4::from_array([ 0.0,   0.0,   0.0,   1.0]),
            ],
        }
    }
    
    // Create a translation matrix
    pub fn translation(x: f32, y: f32, z: f32) -> Self {
        Self {
            rows: [
                f32x4::from_array([1.0, 0.0, 0.0, x]),
                f32x4::from_array([0.0, 1.0, 0.0, y]),
                f32x4::from_array([0.0, 0.0, 1.0, z]),
                f32x4::from_array([0.0, 0.0, 0.0, 1.0]),
            ]
        }
    }

    // Create a scale matrix
    pub fn scale_matrix(x: f32, y: f32, z: f32) -> Self {
        Self {
            rows: [
                f32x4::from_array([x, 0.0, 0.0, 0.0]),
                f32x4::from_array([0.0, y, 0.0, 0.0]),
                f32x4::from_array([0.0, 0.0, z, 0.0]),
                f32x4::from_array([0.0, 0.0, 0.0, 1.0]),
            ]
        }
    }

    // Create a rotation matrix around X axis
    pub fn rotation_x(angle: f32) -> Self {
        let cos = angle.cos();
        let sin = angle.sin();
        Self {
            rows: [
                f32x4::from_array([1.0, 0.0, 0.0, 0.0]),
                f32x4::from_array([0.0, cos, -sin, 0.0]),
                f32x4::from_array([0.0, sin, cos, 0.0]),
                f32x4::from_array([0.0, 0.0, 0.0, 1.0]),
            ]
        }
    }

    // Create a rotation matrix around Y axis
    pub fn rotation_y(angle: f32) -> Self {
        let cos = angle.cos();
        let sin = angle.sin();
        Self {
            rows: [
                f32x4::from_array([cos, 0.0, sin, 0.0]),
                f32x4::from_array([0.0, 1.0, 0.0, 0.0]),
                f32x4::from_array([-sin, 0.0, cos, 0.0]),
                f32x4::from_array([0.0, 0.0, 0.0, 1.0]),
            ]
        }
    }

    // Create a rotation matrix around Z axis
    pub fn rotation_z(angle: f32) -> Self {
        let cos = angle.cos();
        let sin = angle.sin();
        Self {
            rows: [
                f32x4::from_array([cos, -sin, 0.0, 0.0]),
                f32x4::from_array([sin, cos, 0.0, 0.0]),
                f32x4::from_array([0.0, 0.0, 1.0, 0.0]),
                f32x4::from_array([0.0, 0.0, 0.0, 1.0]),
            ]
        }
    }

    // Create a transformation matrix from translation, rotation, and scale
    pub fn from_trs(translation: f32x4, rotation_euler: f32x4, scale: f32x4) -> Self {
        let rot_x = Self::rotation_x(rotation_euler[0]);
        let rot_y = Self::rotation_y(rotation_euler[1]);
        let rot_z = Self::rotation_z(rotation_euler[2]);
        let rotation = rot_z.mul(&rot_y).mul(&rot_x);
        
        let scale_mat = Self::scale_matrix(scale[0], scale[1], scale[2]);
        let translation_mat = Self::translation(translation[0], translation[1], translation[2]);
        
        translation_mat.mul(&rotation).mul(&scale_mat)
    }

    // Apply translation to this matrix
    pub fn translate(&mut self, x: f32, y: f32, z: f32) {
        let translation_mat = Self::translation(x, y, z);
        *self = translation_mat.mul(self);
    }

    // Apply scale to this matrix
    pub fn scale(&mut self, x: f32, y: f32, z: f32) {
        let scale_mat = Self::scale_matrix(x, y, z);
        *self = scale_mat.mul(self);
    }

    // Apply rotation around X axis to this matrix
    pub fn rotate_x(&mut self, angle: f32) {
        let rot_mat = Self::rotation_x(angle);
        *self = rot_mat.mul(self);
    }

    // Apply rotation around Y axis to this matrix
    pub fn rotate_y(&mut self, angle: f32) {
        let rot_mat = Self::rotation_y(angle);
        *self = rot_mat.mul(self);
    }

    // Apply rotation around Z axis to this matrix
    pub fn rotate_z(&mut self, angle: f32) {
        let rot_mat = Self::rotation_z(angle);
        *self = rot_mat.mul(self);
    }

    // Apply rotation around arbitrary axis
    pub fn rotate_axis(&mut self, axis: f32x4, angle: f32) {
        let rot_mat = Self::rotation_axis_angle(axis, angle);
        *self = rot_mat.mul(self);
    }

    // Create rotation matrix from axis and angle
    pub fn rotation_axis_angle(axis: f32x4, angle: f32) -> Self {
        let c = angle.cos();
        let s = angle.sin();
        let one_c = 1.0 - c;
        let axis = axis.normalize();
        
        let x = axis[0];
        let y = axis[1];
        let z = axis[2];
        
        Self {
            rows: [
                f32x4::from_array([
                    c + x * x * one_c,
                    x * y * one_c - z * s,
                    x * z * one_c + y * s,
                    0.0,
                ]),
                f32x4::from_array([
                    y * x * one_c + z * s,
                    c + y * y * one_c,
                    y * z * one_c - x * s,
                    0.0,
                ]),
                f32x4::from_array([
                    z * x * one_c - y * s,
                    z * y * one_c + x * s,
                    c + z * z * one_c,
                    0.0,
                ]),
                f32x4::from_array([0.0, 0.0, 0.0, 1.0]),
            ]
        }
    }
}

pub trait F32x4Ext {
    fn normalize(self) -> f32x4;
    fn cross(self, other: f32x4) -> f32x4;
    fn dot3(self, other: f32x4) -> f32; // dot3 to avoid confusion with full 4-component dot
}

impl F32x4Ext for f32x4 {
    fn normalize(self) -> f32x4 {
        let len = (self[0] * self[0] + self[1] * self[1] + self[2] * self[2]).sqrt();
        if len > f32::EPSILON {
            self / f32x4::splat(len)
        } else {
            self
        }
    }

    fn cross(self, other: f32x4) -> f32x4 {
        f32x4::from_array([
            self[1] * other[2] - self[2] * other[1],
            self[2] * other[0] - self[0] * other[2],
            self[0] * other[1] - self[1] * other[0],
            0.0,
        ])
    }

    fn dot3(self, other: f32x4) -> f32 {
        self[0] * other[0] + self[1] * other[1] + self[2] * other[2]
    }
}

pub fn create_perspective_matrix(
    fov: f32,
    aspect: f32,
    near: f32,
    far: f32,
) -> Matrix4x4 {
    let f = 1.0 / (fov * 0.5).tan();

    Matrix4x4 {
        rows: [
            f32x4::from_array([f / aspect, 0.0, 0.0, 0.0]),
            f32x4::from_array([0.0, f, 0.0, 0.0]),
            f32x4::from_array([
                0.0,
                0.0,
                (far + near) / (near - far),
                (2.0 * far * near) / (near - far),
            ]),
            f32x4::from_array([0.0, 0.0, -1.0, 0.0]),
        ],
    }
}