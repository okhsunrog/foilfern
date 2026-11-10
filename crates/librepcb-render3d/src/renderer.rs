//! Bevy offscreen PBR renderer. No window/event loop is created here; Slint
//! owns input and presentation. RGBA readback also works with a software UI.
//! Adapted from Bevy's headless_renderer and gpu_readback examples (0.20).
use crate::{BoardMesh, Error, OrbitCamera, Result, SurfaceRole};
use bevy::{
    app::PluginsState,
    asset::RenderAssetUsages,
    camera::RenderTarget,
    core_pipeline::tonemapping::Tonemapping,
    light::GlobalAmbientLight,
    mesh::PrimitiveTopology,
    prelude::*,
    render::{
        RenderPlugin,
        gpu_readback::{Readback, ReadbackComplete},
        render_resource::{
            Extent3d, PipelineCache, TextureDimension, TextureFormat, TextureUsages,
        },
    },
    window::ExitCondition,
};
use std::{
    sync::mpsc,
    time::{Duration, Instant},
};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Visibility {
    pub devices: f32,
    pub solder_resist: f32,
    pub silkscreen: f32,
    pub solder_paste: f32,
}
impl Default for Visibility {
    fn default() -> Self {
        Self {
            devices: 1.,
            solder_resist: 1.,
            silkscreen: 1.,
            solder_paste: 1.,
        }
    }
}
impl Visibility {
    fn opacity(self, role: SurfaceRole) -> f32 {
        match role {
            SurfaceRole::Device => self.devices,
            SurfaceRole::SolderResist => self.solder_resist,
            SurfaceRole::Silkscreen => self.silkscreen,
            SurfaceRole::SolderPaste => self.solder_paste,
            _ => 1.,
        }
        .clamp(0., 1.)
    }
}

pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// Synchronous renderer, owned exclusively by a caller-provided render thread.
pub struct Renderer {
    app: App,
    camera: Entity,
    target: Handle<Image>,
    readback: Entity,
    output: mpsc::Receiver<Frame>,
    sender: mpsc::SyncSender<Frame>,
    parts: Vec<(Entity, Handle<StandardMaterial>, SurfaceRole, f32)>,
    size: (u32, u32),
    pub adapter: String,
}

impl Renderer {
    pub fn new() -> Result<Self> {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(Self::initialize)).unwrap_or_else(
            |_| {
                Err(Error::Render(
                    "Could not initialize the GPU renderer".into(),
                ))
            },
        )
    }

    fn initialize() -> Result<Self> {
        let mut app = App::new();
        app.add_plugins(
            DefaultPlugins
                .set(WindowPlugin {
                    primary_window: None,
                    exit_condition: ExitCondition::DontExit,
                    ..default()
                })
                .disable::<bevy::render::pipelined_rendering::PipelinedRenderingPlugin>()
                .set(RenderPlugin {
                    synchronous_pipeline_compilation: true,
                    ..default()
                }),
        );
        let start = Instant::now();
        while app.plugins_state() != PluginsState::Ready {
            if start.elapsed() > Duration::from_secs(30) {
                return Err(Error::Render("GPU initialization timed out".into()));
            }
            bevy::tasks::tick_global_task_pools_on_main_thread();
            std::thread::yield_now();
        }
        app.finish();
        app.cleanup();
        let adapter = app
            .world()
            .resource::<bevy::render::renderer::RenderAdapterInfo>()
            .name
            .clone();
        app.insert_resource(GlobalAmbientLight {
            brightness: 350.,
            ..default()
        });
        app.world_mut().spawn((
            DirectionalLight {
                illuminance: 12_000.,
                shadow_maps_enabled: true,
                ..default()
            },
            Transform::from_xyz(-60., -80., 150.).looking_at(Vec3::ZERO, Vec3::Z),
        ));
        app.world_mut().spawn((
            DirectionalLight {
                illuminance: 6_000.,
                shadow_maps_enabled: false,
                ..default()
            },
            Transform::from_xyz(70., 40., -90.).looking_at(Vec3::ZERO, Vec3::Z),
        ));
        let (sender, output) = mpsc::sync_channel(2);
        let target = make_target(&mut app, 640, 480);
        let camera = app
            .world_mut()
            .spawn((
                Camera3d::default(),
                Camera {
                    clear_color: Color::srgb_u8(28, 31, 36).into(),
                    ..default()
                },
                RenderTarget::Image(target.clone().into()),
                Tonemapping::None,
                Projection::Perspective(PerspectiveProjection {
                    fov: 45_f32.to_radians(),
                    near: 0.01,
                    far: 100_000.,
                    ..default()
                }),
                Transform::from_xyz(100., -100., 100.).looking_at(Vec3::ZERO, Vec3::Z),
                Msaa::Sample4,
            ))
            .id();
        let readback = add_readback(&mut app, &target, &sender, 640, 480);
        Ok(Self {
            app,
            camera,
            target,
            readback,
            output,
            sender,
            parts: Vec::new(),
            size: (640, 480),
            adapter,
        })
    }

    pub fn set_mesh(&mut self, board: &BoardMesh) {
        for (entity, material, _, _) in self.parts.drain(..) {
            if let Some(mesh) = self.app.world().get::<Mesh3d>(entity) {
                let mesh = mesh.0.id();
                self.app
                    .world_mut()
                    .resource_mut::<Assets<Mesh>>()
                    .remove(mesh);
            }
            self.app.world_mut().despawn(entity);
            self.app
                .world_mut()
                .resource_mut::<Assets<StandardMaterial>>()
                .remove(material.id());
        }
        for part in &board.parts {
            let mesh = Mesh::new(
                PrimitiveTopology::TriangleList,
                RenderAssetUsages::RENDER_WORLD,
            )
            .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, part.positions.clone())
            .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, part.normals.clone());
            let mesh = self
                .app
                .world_mut()
                .resource_mut::<Assets<Mesh>>()
                .add(mesh);
            let [r, g, b, a] = part.color;
            let material = self
                .app
                .world_mut()
                .resource_mut::<Assets<StandardMaterial>>()
                .add(StandardMaterial {
                    base_color: Color::srgba(r, g, b, a),
                    metallic: part.metallic,
                    perceptual_roughness: part.roughness,
                    alpha_mode: if a < 1. {
                        AlphaMode::Blend
                    } else {
                        AlphaMode::Opaque
                    },
                    double_sided: false,
                    ..default()
                });
            let entity = self
                .app
                .world_mut()
                .spawn((Mesh3d(mesh), MeshMaterial3d(material.clone())))
                .id();
            self.parts.push((entity, material, part.role, a));
        }
    }

    pub fn render(
        &mut self,
        width: u32,
        height: u32,
        camera: OrbitCamera,
        visibility: Visibility,
    ) -> Result<Frame> {
        if width == 0 || height == 0 || width > 4096 || height > 4096 {
            return Err(Error::Render(
                "3D viewport must be between 1 and 4096 pixels per side".into(),
            ));
        }
        if self.size != (width, height) {
            self.app.world_mut().despawn(self.readback);
            self.app
                .world_mut()
                .resource_mut::<Assets<Image>>()
                .remove(self.target.id());
            self.target = make_target(&mut self.app, width, height);
            self.app
                .world_mut()
                .entity_mut(self.camera)
                .insert(RenderTarget::Image(self.target.clone().into()));
            self.readback = add_readback(&mut self.app, &self.target, &self.sender, width, height);
            self.size = (width, height);
        }
        let mut camera = camera;
        camera.distance *= (height as f32 / width as f32).max(1.);
        self.app.world_mut().entity_mut(self.camera).insert(
            Transform::from_translation(camera.position()).looking_at(camera.target, Vec3::Z),
        );
        for (entity, handle, role, alpha) in &self.parts {
            let opacity = visibility.opacity(*role) * alpha;
            self.app
                .world_mut()
                .entity_mut(*entity)
                .insert(if opacity > 0. {
                    bevy::prelude::Visibility::Visible
                } else {
                    bevy::prelude::Visibility::Hidden
                });
            if let Some(mut material) = self
                .app
                .world_mut()
                .resource_mut::<Assets<StandardMaterial>>()
                .get_mut(handle)
            {
                material.base_color.set_alpha(opacity);
                material.alpha_mode = if opacity < 1. {
                    AlphaMode::Blend
                } else {
                    AlphaMode::Opaque
                };
            }
        }
        let start = Instant::now();
        let mut frames = 0;
        loop {
            // Drain old frames before advancing. A fresh readback after the warm-up
            // includes the requested camera/materials and compiled shader pipelines.
            while self.output.try_recv().is_ok() {}
            self.app.update();
            frames += 1;
            let ready = self
                .app
                .sub_app(bevy::render::RenderApp)
                .world()
                .resource::<PipelineCache>()
                .waiting_pipelines()
                .next()
                .is_none();
            if frames >= 8 && ready {
                let mut latest = None;
                while let Ok(frame) = self.output.try_recv() {
                    if (frame.width, frame.height) == self.size {
                        latest = Some(frame);
                    }
                }
                if let Some(frame) = latest {
                    return Ok(frame);
                }
            }
            if start.elapsed() > Duration::from_secs(30) {
                return Err(Error::Render("GPU frame readback timed out".into()));
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    }
}

fn make_target(app: &mut App, width: u32, height: u32) -> Handle<Image> {
    let mut image = Image::new_uninit(
        Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::RENDER_WORLD,
    );
    image.texture_descriptor.usage =
        TextureUsages::TEXTURE_BINDING | TextureUsages::RENDER_ATTACHMENT | TextureUsages::COPY_SRC;
    app.world_mut().resource_mut::<Assets<Image>>().add(image)
}
fn add_readback(
    app: &mut App,
    target: &Handle<Image>,
    sender: &mpsc::SyncSender<Frame>,
    width: u32,
    height: u32,
) -> Entity {
    let sender = sender.clone();
    app.world_mut()
        .spawn(Readback::texture(target.clone()))
        .observe(move |event: On<ReadbackComplete>| {
            let row = width as usize * 4;
            let padded = row.div_ceil(256) * 256;
            let rgba = event
                .data
                .chunks(padded)
                .take(height as usize)
                .flat_map(|row_data| row_data[..row].iter().copied())
                .collect();
            // Never block the GPU/render thread when the UI is busy or closing.
            let _ = sender.try_send(Frame {
                width,
                height,
                rgba,
            });
        })
        .id()
}
