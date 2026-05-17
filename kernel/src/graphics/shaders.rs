use core::simd::f32x4;

use crate::graphics::color::Rgba8888UNORM;
use crate::graphics::pipeline::{PSIn, PixelShader, VSIn, VSOut, Vertex3D, VertexShader};
use crate::graphics::resources::ConstantBuffer;
use crate::graphics::transform::Matrix4x4;
use crate::serial_println;

pub struct FlatColorPS {
    pub color: Rgba8888UNORM,
}

impl PixelShader for FlatColorPS {
    fn run(&self, input: &mut PSIn) {
        input.render_target[0] = self.color.to_u32_xrgb();
    }
}

pub struct TextureSamplePS {
    pub texture_slot: usize,
}

impl PixelShader for TextureSamplePS {
    fn run(&self, input: &mut PSIn) {
        if let Some(texture) = input.textures.get(self.texture_slot) {
            let u = input.attributes[0];
            let v = input.attributes[1];

            let nx = input.attributes[2];
            let ny = input.attributes[3];

            //let color = texture.sample_nearest(u, v);

            // let color = Rgba8888UNORM::from_rgbf32(u, v, 0f32);

            //let color = Rgba8888UNORM::from_rgbf32(nx, ny, 0f32);
            let color = Rgba8888UNORM::from_rgbf32(
                nx * 0.5f32 + 0.5f32,
                ny * 0.5f32 + 0.5f32,
                input.attributes[1] * 0.5f32 + 0.5f32,
            );

            // serial_println!(
            //     "PS - color: {} {} {} to uv: {}, {}",
            //     color.r,
            //     color.g,
            //     color.b,
            //     u,
            //     v
            // );
            //let color = Rgba8888UNORM::GREEN;
            input.render_target[0] = color.to_u32_xrgb();
        }
    }
}

pub struct PassThroughVS;

impl VertexShader for PassThroughVS {
    fn run(&self, input: &VSIn, output: &mut VSOut, _uniforms: &[ConstantBuffer]) {
        output.position = f32x4::from_array([
            f32::from_ne_bytes(input.vertex_data[0..4].try_into().unwrap()),
            f32::from_ne_bytes(input.vertex_data[4..8].try_into().unwrap()),
            0.0,
            1.0,
        ]);
    }
}

pub struct Basic3DVS {
    pub model_view_proj: Matrix4x4,
}

impl VertexShader for Basic3DVS {
    fn run(&self, input: &VSIn, output: &mut VSOut, _uniforms: &[ConstantBuffer]) {
        // SAFETY: Vertex3D is #[repr(C, align(16))] with three f32x4 fields.
        // f32x4 has the same layout as [f32; 4]. The total size is 48 bytes.
        // We verify that the input data is at least size_of::<Vertex3D>() bytes.
        let vertex: &Vertex3D = unsafe { &*(input.vertex_data.as_ptr() as *const Vertex3D) };

        let world_pos = self.model_view_proj.mul_vec(vertex.pos);

        //serial_println!("world pos {:?}", world_pos);

        output.position = world_pos;
        output.attributes = vertex.uv;
        output.extra = vertex.norm;
    }
}
