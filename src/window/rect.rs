//! Batched rectangle geometry. Every draw owns its vertices; no mutable
//! per-draw uniform is shared across commands submitted together.
use std::ops::Range;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct BoxRect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl BoxRect {
    pub fn contains(self, x: f32, y: f32) -> bool {
        x >= self.x && y >= self.y && x < self.x + self.w && y < self.y + self.h
    }
    pub fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self {
            x,
            y,
            w: w.max(0.0),
            h: h.max(0.0),
        }
    }

    pub fn inset(self, padding: f32) -> Self {
        Self::new(
            self.x + padding,
            self.y + padding,
            self.w - padding * 2.0,
            self.h - padding * 2.0,
        )
    }

    pub fn bounds(self, scale: f32) -> glyphon::TextBounds {
        glyphon::TextBounds {
            left: (self.x * scale).ceil() as i32,
            top: (self.y * scale).ceil() as i32,
            right: ((self.x + self.w) * scale).floor() as i32,
            bottom: ((self.y + self.h) * scale).floor() as i32,
        }
    }
}

pub(super) type ColoredRect = (BoxRect, [f32; 4]);

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Vertex {
    position: [f32; 2],
    color: [f32; 4],
}

pub(super) struct RectRenderer {
    pipeline: wgpu::RenderPipeline,
    vertices: wgpu::Buffer,
    capacity: usize,
}

impl RectRenderer {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("r105 rectangle shader"),
            source: wgpu::ShaderSource::Wgsl(
                r#"
struct Out { @builtin(position) pos: vec4<f32>, @location(0) color: vec4<f32> };
@vertex fn vs(@location(0) position: vec2<f32>, @location(1) color: vec4<f32>) -> Out {
    var out: Out;
    out.pos = vec4<f32>(position, 0.0, 1.0);
    out.color = color;
    return out;
}
@fragment fn fs(in: Out) -> @location(0) vec4<f32> { return in.color; }
"#
                .into(),
            ),
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("r105 rectangles"),
            layout: None,
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<Vertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x4],
                })],
            },
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });
        let vertices = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("r105 rectangle vertices"),
            size: 256,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self {
            pipeline,
            vertices,
            capacity: 256,
        }
    }

    /// Logical coordinates are normalized once, independent of display density.
    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        size: (f32, f32),
        rects: &[ColoredRect],
    ) {
        let mut vertices = Vec::with_capacity(rects.len() * 6);
        for (rect, color) in rects {
            for [u, v] in [
                [0.0, 0.0],
                [1.0, 0.0],
                [0.0, 1.0],
                [0.0, 1.0],
                [1.0, 0.0],
                [1.0, 1.0],
            ] {
                vertices.push(Vertex {
                    position: [
                        (rect.x + u * rect.w) / size.0 * 2.0 - 1.0,
                        1.0 - (rect.y + v * rect.h) / size.1 * 2.0,
                    ],
                    color: *color,
                });
            }
        }
        let bytes = bytemuck::cast_slice(&vertices);
        if bytes.len() > self.capacity {
            self.capacity = bytes.len().next_power_of_two();
            self.vertices = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("r105 rectangle vertices"),
                size: self.capacity as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
        if !bytes.is_empty() {
            queue.write_buffer(&self.vertices, 0, bytes);
        }
    }

    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>, range: Range<usize>) {
        if range.is_empty() {
            return;
        }
        pass.set_pipeline(&self.pipeline);
        pass.set_vertex_buffer(0, self.vertices.slice(..));
        pass.draw((range.start * 6) as u32..(range.end * 6) as u32, 0..1);
    }
}
