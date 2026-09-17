//! Explicit GPU acceptance tests. Run with --ignored on a machine with a GPU;
//! ordinary headless CI still runs the layout and terminal-state regressions.
use super::*;

struct Target {
    device: wgpu::Device,
    queue: wgpu::Queue,
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    width: u32,
    height: u32,
}

impl Target {
    fn new(width: u32, height: u32) -> Self {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter = pollster::block_on(instance.request_adapter(&Default::default()))
            .expect("GPU adapter required");
        let (device, queue) =
            pollster::block_on(adapter.request_device(&Default::default())).unwrap();
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("r105 acceptance target"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        Self {
            device,
            queue,
            texture,
            view,
            width,
            height,
        }
    }

    fn read(&self) -> Vec<u8> {
        let stride = (self.width * 4).div_ceil(256) * 256;
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: u64::from(stride * self.height),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = self.device.create_command_encoder(&Default::default());
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &self.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(stride),
                    rows_per_image: Some(self.height),
                },
            },
            wgpu::Extent3d {
                width: self.width,
                height: self.height,
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit([encoder.finish()]);
        let (tx, rx) = std::sync::mpsc::channel();
        buffer
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| tx.send(result).unwrap());
        self.device
            .poll(wgpu::PollType::wait_indefinitely())
            .unwrap();
        rx.recv().unwrap().unwrap();
        let mapped = buffer.slice(..).get_mapped_range().unwrap();
        mapped
            .chunks_exact(stride as usize)
            .flat_map(|row| row[..self.width as usize * 4].iter().copied())
            .collect()
    }

    fn pass<'a>(&'a self, encoder: &'a mut wgpu::CommandEncoder) -> wgpu::RenderPass<'a> {
        encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &self.view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        })
    }
}

#[test]
#[ignore = "requires GPU; explicit native-window acceptance gate"]
fn gpu_rectangles_keep_independent_geometry_and_bounds() {
    let target = Target::new(64, 64);
    let mut rects = RectRenderer::new(&target.device, wgpu::TextureFormat::Rgba8Unorm);
    rects.prepare(
        &target.device,
        &target.queue,
        (64.0, 64.0),
        &[
            (BoxRect::new(4.0, 4.0, 12.0, 12.0), [1.0, 0.0, 0.0, 1.0]),
            (BoxRect::new(36.0, 36.0, 12.0, 12.0), [0.0, 1.0, 0.0, 1.0]),
        ],
    );
    let mut encoder = target.device.create_command_encoder(&Default::default());
    {
        let mut pass = target.pass(&mut encoder);
        rects.draw(&mut pass, 0..1);
        rects.draw(&mut pass, 1..2);
    }
    target.queue.submit([encoder.finish()]);
    let pixels = target.read();
    for y in 0..64 {
        for x in 0..64 {
            let expected = if (4..16).contains(&x) && (4..16).contains(&y) {
                [255, 0, 0, 255]
            } else if (36..48).contains(&x) && (36..48).contains(&y) {
                [0, 255, 0, 255]
            } else {
                [0, 0, 0, 255]
            };
            assert_eq!(
                &pixels[(y * 64 + x) * 4..(y * 64 + x) * 4 + 4],
                &expected,
                "pixel {x},{y}"
            );
        }
    }
}

#[test]
#[ignore = "requires GPU; explicit native-window acceptance gate"]
fn gpu_overlay_occludes_terminal_text_at_both_densities() {
    for scale in [1.0, 2.0] {
        let target = Target::new((480.0 * scale) as u32, (320.0 * scale) as u32);
        let mut fonts = FontSystem::new();
        let mut swash = SwashCache::new();
        let cache = Cache::new(&target.device);
        let mut atlas = TextAtlas::new(
            &target.device,
            &target.queue,
            &cache,
            wgpu::TextureFormat::Rgba8Unorm,
        );
        let mut viewport = Viewport::new(&target.device, &cache);
        viewport.update(
            &target.queue,
            Resolution {
                width: target.width,
                height: target.height,
            },
        );
        let mut renderers: Vec<_> = (0..2)
            .map(|_| {
                glyphon::TextRenderer::new(
                    &mut atlas,
                    &target.device,
                    MultisampleState::default(),
                    None,
                )
            })
            .collect();
        let mut terminal = Buffer::new(&mut fonts, Metrics::new(FONT_SIZE, LINE_HEIGHT));
        let mut empty = Buffer::new(&mut fonts, Metrics::new(FONT_SIZE, LINE_HEIGHT));
        let mut last = String::new();
        set_cached(
            &mut fonts,
            &mut terminal,
            &mut last,
            &"TERMINAL OUTPUT MUST NOT BLEED THROUGH PANELS\n".repeat(15),
            480.0,
        );
        set_cached(&mut fonts, &mut empty, &mut String::new(), "", 480.0);
        let panel = BoxRect::new(40.0, 40.0, 400.0, 240.0);
        let areas = [
            text_area(
                &terminal,
                BoxRect::new(0.0, 0.0, 480.0, 320.0),
                scale,
                FG,
                0.0,
                None,
            ),
            text_area(&empty, panel, scale, FG, 0.0, None),
        ];
        for (renderer, area) in renderers.iter_mut().zip(areas) {
            renderer
                .prepare(
                    &target.device,
                    &target.queue,
                    &mut fonts,
                    &mut atlas,
                    &viewport,
                    [area],
                    &mut swash,
                )
                .unwrap();
        }
        let mut rects = RectRenderer::new(&target.device, wgpu::TextureFormat::Rgba8Unorm);
        rects.prepare(
            &target.device,
            &target.queue,
            (480.0, 320.0),
            &[(panel, [0.0, 0.0, 1.0, 1.0])],
        );
        let mut encoder = target.device.create_command_encoder(&Default::default());
        {
            let mut pass = target.pass(&mut encoder);
            paint_layers(
                &rects,
                &renderers,
                &atlas,
                &viewport,
                &mut pass,
                vec![0..0, 0..1],
            )
            .unwrap();
        }
        target.queue.submit([encoder.finish()]);
        let pixels = target.read();
        for y in (40.0 * scale) as usize..(280.0 * scale) as usize {
            for x in (40.0 * scale) as usize..(440.0 * scale) as usize {
                let i = (y * target.width as usize + x) * 4;
                assert_eq!(
                    &pixels[i..i + 4],
                    &[0, 0, 255, 255],
                    "text leaked at {x},{y} scale={scale}"
                );
            }
        }
        assert!(
            pixels.as_chunks::<4>().0.iter().any(|pixel| pixel[0] > 50),
            "terminal text was not rendered"
        );
    }
}
