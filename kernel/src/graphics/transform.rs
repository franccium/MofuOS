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
    
//     pub fn look_at(eye: f32x4, target: f32x4, up: f32x4) -> Matrix4x4 {
//     // RH: forward points backward from target
//     let z_axis = (eye - target).normalize();
//     let x_axis = up.cross(z_axis).normalize();
//     let y_axis = z_axis.cross(x_axis);

//     Matrix4x4 {
//         rows: [
//             f32x4::from_array([
//                 x_axis[0],
//                 x_axis[1],
//                 x_axis[2],
//                 -x_axis.dot3(eye),
//             ]),
//             f32x4::from_array([
//                 y_axis[0],
//                 y_axis[1],
//                 y_axis[2],
//                 -y_axis.dot3(eye),
//             ]),
//             f32x4::from_array([
//                 z_axis[0],
//                 z_axis[1],
//                 z_axis[2],
//                 -z_axis.dot3(eye),
//             ]),
//             f32x4::from_array([
//                 0.0,
//                 0.0,
//                 0.0,
//                 1.0,
//             ]),
//         ],
//     }
// }
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
    
    pub fn rotation_y(angle: f32) -> Matrix4x4 {
        let cos = angle.cos();
        let sin = angle.sin();
        Matrix4x4 {
            rows: [
                f32x4::from_array([cos, 0.0, sin, 0.0]),
                f32x4::from_array([0.0, 1.0, 0.0, 0.0]),
                f32x4::from_array([-sin, 0.0, cos, 0.0]),
                f32x4::from_array([0.0, 0.0, 0.0, 1.0]),
            ],
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

// pub fn create_perspective_matrix(fov: f32, aspect: f32, near: f32, far: f32) -> Matrix4x4 {
//     let f = 1.0 / (fov / 2.0).tan();

//     Matrix4x4 {
//         rows: [
//             f32x4::from_array([f / aspect, 0.0, 0.0, 0.0]),
//             f32x4::from_array([0.0, f, 0.0, 0.0]),
//             f32x4::from_array([0.0, 0.0, (far + near) / (near - far), -1.0]),
//             f32x4::from_array([0.0, 0.0, (2.0 * far * near) / (near - far), 0.0]),
//         ],
//     }
// }
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