//! Opt-in GPU check: cargo test -p sidekick --test render -- --ignored --nocapture
use eframe::{egui, egui_wgpu};

#[tokio::test]
#[ignore = "Requires a working native GPU backend"]
async fn renders_egui_frame_on_native_gpu() {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions::default())
        .await
        .expect("No native graphics adapter");
    println!("Graphics adapter: {:?}", adapter.get_info());
    let (device, queue) = adapter
        .request_device(&wgpu::DeviceDescriptor::default())
        .await
        .expect("Could not create graphics device");
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("Sidekick renderer smoke test"),
        size: wgpu::Extent3d {
            width: 256,
            height: 256,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let ctx = egui::Context::default();
    let mut output = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(256.0, 256.0),
            )),
            ..Default::default()
        },
        |ui| {
            ui.heading("Sidekick Settings");
            ui.label("Native GPU rendering");
            let mut enabled = true;
            ui.checkbox(&mut enabled, "Expose CPU usage");
        },
    );
    let primitives = ctx.tessellate(output.shapes, output.pixels_per_point);
    assert!(!primitives.is_empty());
    let screen = egui_wgpu::ScreenDescriptor {
        size_in_pixels: [256, 256],
        pixels_per_point: output.pixels_per_point,
    };
    let mut renderer = egui_wgpu::Renderer::new(&device, format, Default::default());
    for (id, deltas) in output.textures_delta.set.drain() {
        for delta in deltas {
            renderer.update_texture(&device, &queue, id, &delta);
        }
    }
    let mut encoder = device.create_command_encoder(&Default::default());
    let commands = renderer.update_buffers(&device, &queue, &mut encoder, &primitives, &screen);
    let view = texture.create_view(&Default::default());
    {
        let mut pass = encoder
            .begin_render_pass(&wgpu::RenderPassDescriptor {
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            })
            .forget_lifetime();
        renderer.render(&mut pass, &primitives, &screen);
    }
    queue.submit(commands.into_iter().chain([encoder.finish()]));
    for id in output.textures_delta.free.drain() {
        renderer.free_texture(&id);
    }
    device
        .poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: Some(std::time::Duration::from_secs(10)),
        })
        .expect("GPU render did not finish");
}
