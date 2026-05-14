use core::simd::f32x4;

use alloc::{boxed::Box, sync::Arc};
use alloc::vec::Vec;
use crate::graphics::pipeline::{CullMode, DepthFunc, Vertex3D};
use crate::graphics::resources::{ConstantBuffer, DepthBuffer};
use crate::graphics::shaders::{Basic3DVS, FlatColorPS};
use crate::graphics::transform::{Matrix4x4, create_perspective_matrix};
use crate::graphics::{color::Rgba8888UNORM, framebuffer::FrameBufferTarget, pipeline::{BlendState, PipelineState, RasterizerState, RenderMode, VertexLayout}, renderer::RenderContext, resources::Texture, shaders::{PassThroughVS, TextureSamplePS}, window::WindowBuffer};
use embedded_graphics::prelude::*;
use embedded_graphics::{geometry::Point, pixelcolor::Rgb888, primitives::Primitive};
use embedded_graphics::primitives::{Circle, PrimitiveStyle, PrimitiveStyleBuilder, Rectangle};


pub fn draw_shapes(framebuffer_target: &mut FrameBufferTarget) {
    Rectangle::new(Point::new(0, 0), Size::new(100, 100))
        .into_styled(PrimitiveStyle::with_fill(Rgb888::RED))
        .draw(framebuffer_target)
        .unwrap();

    let style = PrimitiveStyleBuilder::new()
        .stroke_color(Rgb888::RED)
        .stroke_width(3)
        .fill_color(Rgb888::WHITE)
        .build();

    let fb_width = framebuffer_target.width as f32;
    let fb_height = framebuffer_target.height as f32;
    for i in 0..5 {
        let x = (fb_width / 9.0) * (i as f32 + 1.0) - 10.0;
        let y = (fb_height / 9.0) * (i as f32 + 1.0);
        let radius = 10.0 + i as f32 * 2.5;

        Circle::new(Point::new(x as i32, y as i32), radius as u32)
            .into_styled(style)
            .draw(framebuffer_target)
            .unwrap();
    }
}

pub fn render_shaders_2d(window_buffer: &Arc<WindowBuffer>) {
    let mut ctx = RenderContext::new();
    const SIZE: u32 = 120;
    let texture_data = Vec::from(
        (0..(SIZE * SIZE))
            .map(|i| {
                let y = i;
                if y % 2 == 0 {
                    Rgba8888UNORM::from_rgb(255, 0, 0).to_u32_rgba()
                } else {
                    Rgba8888UNORM::from_rgb(0, 255, 0).to_u32_rgba()
                }
            })
            .collect::<Vec<u32>>(),
    );

    let mut texture = Texture::from_data(SIZE, SIZE, texture_data);
    let texture_slot = ctx.bind_texture(texture);

    let texture_data = Vec::from(
        (0..(SIZE * SIZE))
            .map(|i| {
                let y = i;
                Rgba8888UNORM::from_rgb(0, 255, 255).to_u32_rgba()
            })
            .collect::<Vec<u32>>(),
    );
    let mut texture = Texture::from_data(SIZE, SIZE, texture_data);
    let texture_slot2 = ctx.bind_texture(texture);

    let pipeline = PipelineState {
        vs: Box::new(PassThroughVS),
        ps: Box::new(TextureSamplePS { texture_slot }),
        vertex_layout: VertexLayout::new_2d(),
        rasterizer_state: RasterizerState::default(),
        blend_state: BlendState::default(),
        render_mode: RenderMode::XY,
        depth_enabled: false,
        depth_write: false,
        depth_func: DepthFunc::Less,
    };

    let mut back_buffer = window_buffer.back_buffer_mut();
    let mut render_target = ctx.begin_frame(&mut back_buffer);
    render_target.clear(Rgba8888UNORM::GRAY);

    ctx.draw_rect_2d(
        10.0,
        10.0,
        SIZE as f32,
        SIZE as f32,
        &mut render_target,
        &pipeline,
    );

    let pipeline2 = PipelineState {
        vs: Box::new(PassThroughVS),
        ps: Box::new(TextureSamplePS {
            texture_slot: texture_slot2,
        }),
        vertex_layout: VertexLayout::new_2d(),
        rasterizer_state: RasterizerState::default(),
        blend_state: BlendState::default(),
        render_mode: RenderMode::XY,
        depth_enabled: false,
        depth_write: false,
        depth_func: DepthFunc::Less,
    };
    ctx.draw_rect_2d(
        25.0,
        25.0,
        SIZE as f32,
        SIZE as f32,
        &mut render_target,
        &pipeline2,
    );

    ctx.draw_triangle_2d(
        20.0 + 40.0,
        50.0 + 40.0,
        0.0,
        0.0,
        50.0 + 40.0,
        0.0 + 40.0,
        1.0,
        0.0,
        80.0 + 40.0,
        50.0 + 40.0,
        0.5,
        1.0,
        &mut render_target,
        &pipeline,
    );

    drop(render_target);
    window_buffer.present();
}

pub fn render_shaders_3d(window_buffer: &Arc<WindowBuffer>) {
    let mut ctx = RenderContext::new();
    
    // Create a checkerboard texture
    const SIZE: u32 = 64;
    let texture_data = Vec::from(
        (0..(SIZE * SIZE))
            .map(|i| {
                let x = i % SIZE;
                let y = i / SIZE;
                let checker = ((x / 8) + (y / 8)) % 2 == 0;
                if checker {
                    Rgba8888UNORM::from_rgb(255, 128, 0).to_u32_rgba() // Orange
                } else {
                    Rgba8888UNORM::from_rgb(0, 128, 255).to_u32_rgba() // Blue
                }
            })
            .collect::<Vec<u32>>(),
    );
    let texture = Texture::from_data(SIZE, SIZE, texture_data);
    let texture_slot = ctx.bind_texture(texture);

    // Set up constant buffer with MVP matrix (update this each frame for animation)
    let mut constant_data = alloc::vec![0u8; 64]; // 4x4 matrix = 64 bytes
    let cbuffer = ConstantBuffer::from_data(constant_data);
    let cbuffer_slot = ctx.bind_cbuffer(cbuffer);

    // Create perspective projection matrix
    let window_width = window_buffer.width as f32;
    let window_height = window_buffer.height as f32;
    let aspect_ratio = window_width / window_height;
    let fov = 60.0_f32.to_radians();
    let near = 0.1;
    let far = 100.0;
    
    let projection = create_perspective_matrix(fov, aspect_ratio, near, far);
    
    // Create view matrix (camera looking at origin)
    let view = Matrix4x4::look_at(
        f32x4::from_array([-2.0, -2.0, 5.0, 1.0]), // Camera position
        f32x4::from_array([0.0, 0.0, 0.0, 1.0]),  // Look at target
        f32x4::from_array([0.0, 1.0, 0.0, 0.0]),  // Up vector
    );
    
    // Create model matrix
    let model = Matrix4x4::identity();
    
    // Combine matrices
    let mvp = projection.mul(&view).mul(&model);

    // 3D pipeline with texture
    let pipeline = PipelineState {
        vs: Box::new(Basic3DVS { model_view_proj: mvp }),
        ps: Box::new(TextureSamplePS { texture_slot }),
        vertex_layout: VertexLayout::new_3d(),
        rasterizer_state: RasterizerState {
            cull_mode: CullMode::Back,
        },
        blend_state: BlendState::default(),
        render_mode: RenderMode::XYZ,
        depth_enabled: true,
        depth_write: true,
        depth_func: DepthFunc::Less,
    };

    // Create a cube (8 vertices, 36 indices for 12 triangles)
    let s = 1.0; // Half-size of cube
    
    #[rustfmt::skip]
    let vertices = alloc::vec![
        // Front face (z = s)
        Vertex3D::new(-s, -s,  s, 1.0,  0.0, 0.0,  0.0, 0.0, 1.0),
        Vertex3D::new( s, -s,  s, 1.0,  1.0, 0.0,  0.0, 0.0, 1.0),
        Vertex3D::new( s,  s,  s, 1.0,  1.0, 1.0,  0.0, 0.0, 1.0),
        Vertex3D::new(-s,  s,  s, 1.0,  0.0, 1.0,  0.0, 0.0, 1.0),
        // Back face (z = -s)
        Vertex3D::new(-s, -s, -s, 1.0,  0.0, 0.0,  0.0, 0.0, -1.0),
        Vertex3D::new( s, -s, -s, 1.0,  1.0, 0.0,  0.0, 0.0, -1.0),
        Vertex3D::new( s,  s, -s, 1.0,  1.0, 1.0,  0.0, 0.0, -1.0),
        Vertex3D::new(-s,  s, -s, 1.0,  0.0, 1.0,  0.0, 0.0, -1.0),
        // Top face (y = s)
        Vertex3D::new(-s,  s, -s, 1.0,  0.0, 0.0,  0.0, 1.0, 0.0),
        Vertex3D::new( s,  s, -s, 1.0,  1.0, 0.0,  0.0, 1.0, 0.0),
        Vertex3D::new( s,  s,  s, 1.0,  1.0, 1.0,  0.0, 1.0, 0.0),
        Vertex3D::new(-s,  s,  s, 1.0,  0.0, 1.0,  0.0, 1.0, 0.0),
        // Bottom face (y = -s)
        Vertex3D::new(-s, -s, -s, 1.0,  0.0, 0.0,  0.0, -1.0, 0.0),
        Vertex3D::new( s, -s, -s, 1.0,  1.0, 0.0,  0.0, -1.0, 0.0),
        Vertex3D::new( s, -s,  s, 1.0,  1.0, 1.0,  0.0, -1.0, 0.0),
        Vertex3D::new(-s, -s,  s, 1.0,  0.0, 1.0,  0.0, -1.0, 0.0),
        // Right face (x = s)
        Vertex3D::new( s, -s, -s, 1.0,  0.0, 0.0,  1.0, 0.0, 0.0),
        Vertex3D::new( s,  s, -s, 1.0,  1.0, 0.0,  1.0, 0.0, 0.0),
        Vertex3D::new( s,  s,  s, 1.0,  1.0, 1.0,  1.0, 0.0, 0.0),
        Vertex3D::new( s, -s,  s, 1.0,  0.0, 1.0,  1.0, 0.0, 0.0),
        // Left face (x = -s)
        Vertex3D::new(-s, -s, -s, 1.0,  0.0, 0.0,  -1.0, 0.0, 0.0),
        Vertex3D::new(-s,  s, -s, 1.0,  1.0, 0.0,  -1.0, 0.0, 0.0),
        Vertex3D::new(-s,  s,  s, 1.0,  1.0, 1.0,  -1.0, 0.0, 0.0),
        Vertex3D::new(-s, -s,  s, 1.0,  0.0, 1.0,  -1.0, 0.0, 0.0),
    ];

    // Indices for 12 triangles (2 per face)
    #[rustfmt::skip]
let indices = alloc::vec![
    // Front (+Z)
    0, 1, 2, 0, 2, 3,

    // Back (-Z)
    4, 6, 5, 4, 7, 6,

    // Top (+Y)
    8, 10, 9, 8, 11, 10,

    // Bottom (-Y)
    12, 13, 14, 12, 14, 15,

    // Right (+X)

    16, 17, 18, 16, 18, 19,

    // Left (-X)
    20, 22, 21, 20, 23, 22,
];


    // Render loop
    let mut back_buffer = window_buffer.back_buffer_mut();
    let mut render_target = ctx.begin_frame(&mut back_buffer);
    
    // Clear to dark gray
    render_target.clear(Rgba8888UNORM::from_rgb(30, 30, 30));
    
    // Create depth buffer (you might want to reuse this across frames)
    let mut depth_buffer = DepthBuffer::new(
        window_buffer.width,
        window_buffer.height,
    );
    
    // Draw textured cube
    ctx.draw_indexed_3d(
        &vertices,
        &indices,
        &mut render_target,
        &mut depth_buffer,
        &pipeline,
    );

    drop(render_target);
    window_buffer.present();
}