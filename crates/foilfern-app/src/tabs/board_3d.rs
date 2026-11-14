//! Port of editor/project/board/board3dtab.{h,cpp}, using Bevy instead of Qt/OpenGL.
//! Owned snapshots cross the worker boundary; Slint owns the window and input.
use super::{TabId, TabUpdate, feature, project_index};
use crate::{
    app::{State, with_current_state},
    project::AppProject,
};
use foilfern_app_ui as ui;
use foilfern_canvas::{
    Modifiers, PointerButton, PointerKind,
    kurbo::{Point, Vec2},
};
use foilfern_core::project::{AssemblyVariantId, BoardId};
use foilfern_i18n::tr;
use foilfern_render3d::{
    BoardMesh, BoardSnapshot, OrbitCamera,
    renderer::{Frame, Renderer, Visibility},
};
use std::{
    rc::Rc,
    sync::{Arc, mpsc},
};

struct Request {
    snapshot: Arc<BoardSnapshot>,
    camera: OrbitCamera,
    fit: bool,
    visibility: Visibility,
    size: (u32, u32),
    generation: u64,
}
struct Rendered {
    generation: u64,
    result: Result<Frame, String>,
    warnings: Vec<String>,
    camera: Option<OrbitCamera>,
    fitted: Option<OrbitCamera>,
}

pub struct Board3dTab {
    id: TabId,
    project: Rc<AppProject>,
    board: BoardId,
    title: String,
    revision: u64,
    snapshot: Option<Arc<BoardSnapshot>>,
    camera: OrbitCamera,
    fitted: OrbitCamera,
    fit: bool,
    visibility: Visibility,
    size: (u32, u32),
    logical_height: f32,
    generation: u64,
    frame: i32,
    image: slint::Image,
    refreshing: bool,
    error: String,
    drag: Option<(PointerButton, Point)>,
    requests: Option<mpsc::Sender<Request>>,
}

impl Board3dTab {
    pub fn new(project: Rc<AppProject>, board: BoardId) -> Self {
        let camera = OrbitCamera::fit([[0.; 3], [100., 100., 1.6]]);
        let mut tab = Self {
            id: TabId::new(),
            project,
            board,
            title: String::new(),
            revision: u64::MAX,
            snapshot: None,
            camera,
            fitted: camera,
            fit: true,
            visibility: Visibility::default(),
            size: (0, 0),
            logical_height: 1.,
            generation: 0,
            frame: 0,
            image: slint::Image::default(),
            refreshing: false,
            error: String::new(),
            drag: None,
            requests: None,
        };
        tab.rebuild_if_modified();
        tab.camera = tab.fitted;
        tab
    }
    pub fn id(&self) -> TabId {
        self.id
    }
    pub fn project(&self) -> &Rc<AppProject> {
        &self.project
    }
    pub fn board(&self) -> BoardId {
        self.board
    }
    pub fn is_refreshing(&self) -> bool {
        self.refreshing
    }
    pub fn error(&self) -> &str {
        &self.error
    }
    pub fn bump_frame(&mut self) {
        self.frame = self.frame.wrapping_add(1);
    }
    pub fn ui_data(&self) -> ui::TabData {
        let shared = self.project.shared().lock();
        let undo = shared.editor.undo_stack();
        ui::TabData {
            r#type: ui::TabType::Board3d,
            title: self.title.as_str().into(),
            read_only: !self.project.is_writable(),
            unsaved_changes: shared.has_unsaved_changes(),
            undo_text: undo.undo_text().unwrap_or_default().into(),
            redo_text: undo.redo_text().unwrap_or_default().into(),
            features: ui::TabFeatures {
                zoom: feature(true),
                save: feature(self.project.is_writable()),
                undo: feature(undo.can_undo()),
                redo: feature(undo.can_redo()),
                ..Default::default()
            },
            ..Default::default()
        }
    }
    pub fn derived_ui_data(&self, projects: &[Rc<AppProject>]) -> ui::Board3dTabData {
        let index = self
            .project
            .shared()
            .lock()
            .project()
            .board_index(self.board)
            .map_or(-1, |i| i as i32);
        ui::Board3dTabData {
            project_index: project_index(projects, &self.project),
            board_index: index,
            background_color: slint::Color::from_rgb_u8(28, 31, 36).into(),
            foreground_color: slint::Color::from_rgb_u8(235, 235, 235).into(),
            cursor: super::editing::mouse_cursor(
                Some(foilfern_editor::fsm::CursorShape::OpenHand),
                self.drag.is_some(),
            ),
            devices_alpha: self.visibility.devices,
            solderresist_alpha: self.visibility.solder_resist,
            silkscreen_alpha: self.visibility.silkscreen,
            solderpaste_alpha: self.visibility.solder_paste,
            refreshing: self.refreshing,
            error: self.error.as_str().into(),
            frame: self.frame,
        }
    }
    pub fn set_derived_ui_data(&mut self, data: &ui::Board3dTabData) -> TabUpdate {
        let visibility = Visibility {
            devices: data.devices_alpha.clamp(0., 1.),
            solder_resist: data.solderresist_alpha.clamp(0., 1.),
            silkscreen: data.silkscreen_alpha.clamp(0., 1.),
            solder_paste: data.solderpaste_alpha.clamp(0., 1.),
        };
        if self.visibility == visibility {
            return TabUpdate::default();
        }
        self.visibility = visibility;
        self.request_render();
        TabUpdate {
            data_changed: true,
            ..Default::default()
        }
    }
    pub fn rebuild_if_modified(&mut self) -> TabUpdate {
        let revision = self.project.revision();
        if revision == self.revision {
            return TabUpdate::default();
        }
        self.revision = revision;
        let result = {
            let shared = self.project.shared().lock();
            let project = shared.project();
            if let Some(board) = project.board(self.board) {
                self.title = format!("{} · 3D", board.properties().name);
            }
            let variant = project
                .circuit()
                .assembly_variants()
                .first()
                .map(|v| AssemblyVariantId(v.uuid()));
            BoardSnapshot::from_project(project, self.board, variant)
        };
        match result {
            Ok(snapshot) => {
                let mut low = [f32::INFINITY, f32::INFINITY, 0.];
                let mut high = [
                    f32::NEG_INFINITY,
                    f32::NEG_INFINITY,
                    snapshot.physical.thickness.to_mm() as f32,
                ];
                for vertex in snapshot.physical.outlines.iter().flat_map(|p| p.vertices()) {
                    let p = [vertex.pos.x.to_mm() as f32, vertex.pos.y.to_mm() as f32];
                    for axis in 0..2 {
                        low[axis] = low[axis].min(p[axis]);
                        high[axis] = high[axis].max(p[axis]);
                    }
                }
                if low.iter().chain(&high).all(|v| v.is_finite()) {
                    self.fitted = OrbitCamera::fit([low, high]);
                }
                self.snapshot = Some(Arc::new(snapshot));
                self.refreshing = true;
                self.error.clear();
                self.request_render();
            }
            Err(error) => {
                self.error = error_message(error);
                self.refreshing = false;
            }
        }
        TabUpdate {
            repaint: true,
            data_changed: true,
            ..Default::default()
        }
    }
    pub fn trigger(&mut self, action: ui::TabAction) -> TabUpdate {
        match action {
            ui::TabAction::ZoomFit => {
                self.camera = self.fitted;
                self.fit = true;
            }
            ui::TabAction::ZoomIn => {
                self.camera.zoom(1.);
                self.fit = false;
            }
            ui::TabAction::ZoomOut => {
                self.camera.zoom(-1.);
                self.fit = false;
            }
            ui::TabAction::LayersTop => {
                self.camera.side(false);
                self.fit = false;
            }
            ui::TabAction::LayersBottom => {
                self.camera.side(true);
                self.fit = false;
            }
            _ => return TabUpdate::default(),
        }
        self.request_render();
        TabUpdate {
            data_changed: true,
            ..Default::default()
        }
    }
    pub fn pointer_event(
        &mut self,
        kind: PointerKind,
        button: PointerButton,
        pos: Point,
        modifiers: Modifiers,
    ) -> TabUpdate {
        match kind {
            PointerKind::Down
                if matches!(
                    button,
                    PointerButton::Left | PointerButton::Middle | PointerButton::Right
                ) =>
            {
                self.drag = Some((button, pos))
            }
            PointerKind::Up | PointerKind::Cancel => self.drag = None,
            PointerKind::Move => {
                if let Some((button, last)) = self.drag {
                    self.fit = false;
                    let delta = [(pos.x - last.x) as f32, (pos.y - last.y) as f32];
                    if button == PointerButton::Left && !modifiers.shift {
                        self.camera.orbit(delta);
                    } else {
                        let aspect = (self.size.1 as f32 / self.size.0.max(1) as f32).max(1.);
                        self.camera.pan(delta, self.logical_height / aspect);
                    }
                    self.drag = Some((button, pos));
                    self.request_render();
                }
            }
            _ => {}
        }
        TabUpdate {
            data_changed: true,
            ..Default::default()
        }
    }
    pub fn key_pressed(&mut self, event: &slint::language::KeyEvent) -> (bool, TabUpdate) {
        if event.modifiers.control || event.modifiers.meta || event.modifiers.alt {
            return (false, TabUpdate::default());
        }
        let action = match event.text.as_str() {
            "1" => ui::TabAction::LayersTop,
            "2" => ui::TabAction::LayersBottom,
            "0" | "f" | "F" => ui::TabAction::ZoomFit,
            "+" | "=" => ui::TabAction::ZoomIn,
            "-" => ui::TabAction::ZoomOut,
            _ => return (false, TabUpdate::default()),
        };
        (true, self.trigger(action))
    }
    pub fn scroll_event(&mut self, delta: Vec2) -> TabUpdate {
        self.fit = false;
        self.camera.zoom((delta.y / 120.) as f32);
        self.request_render();
        TabUpdate {
            data_changed: true,
            ..Default::default()
        }
    }
    pub fn render_scene(&mut self, width: f32, height: f32, scale: f32) -> slint::Image {
        if width < 2. || height < 2. {
            return self.image.clone();
        }
        self.logical_height = height;
        let (width, height) = (width * scale, height * scale);
        let reduction = (4096. / width).min(4096. / height).min(1.);
        let size = (
            (width * reduction).round().max(1.) as u32,
            (height * reduction).round().max(1.) as u32,
        );
        if size != self.size {
            self.size = size;
            self.request_render();
        }
        self.image.clone()
    }
    fn request_render(&mut self) {
        if self.size.0 == 0 || self.size.1 == 0 {
            return;
        }
        let Some(snapshot) = self.snapshot.as_ref().cloned() else {
            return;
        };
        if self.requests.is_none() {
            match start_worker(self.id) {
                Ok(sender) => self.requests = Some(sender),
                Err(error) => {
                    self.error = error_message(error);
                    self.refreshing = false;
                    return;
                }
            }
        }
        self.generation = self.generation.wrapping_add(1);
        self.refreshing = true;
        let request = Request {
            snapshot,
            camera: self.camera,
            fit: self.fit,
            visibility: self.visibility,
            size: self.size,
            generation: self.generation,
        };
        if self
            .requests
            .as_ref()
            .is_some_and(|sender| sender.send(request).is_err())
        {
            self.error = error_message("3D renderer stopped");
            self.requests = None;
            self.refreshing = false;
        }
    }
    fn accept(&mut self, rendered: Rendered) -> TabUpdate {
        if rendered.generation != 0 && rendered.generation != self.generation {
            return TabUpdate::default();
        }
        self.refreshing = false;
        if let Some(fitted) = rendered.fitted {
            self.fitted = fitted;
        }
        if self.fit
            && let Some(camera) = rendered.camera
        {
            self.camera = camera;
            self.fit = false;
        }
        match rendered.result {
            Ok(frame) => {
                self.image = slint::Image::from_rgba8(slint::SharedPixelBuffer::clone_from_slice(
                    &frame.rgba,
                    frame.width,
                    frame.height,
                ));
                self.error = if rendered.warnings.is_empty() {
                    String::new()
                } else {
                    error_message(rendered.warnings.join("\n"))
                };
            }
            Err(error) => {
                self.error = error_message(error);
                self.requests = None;
            }
        }
        TabUpdate {
            repaint: true,
            data_changed: true,
            ..Default::default()
        }
    }
}

fn error_message(error: impl std::fmt::Display) -> String {
    tr!("Board3dTab", "Failed to render 3D board: {0}", error)
}

fn start_worker(id: TabId) -> std::io::Result<mpsc::Sender<Request>> {
    let (sender, receiver) = mpsc::channel::<Request>();
    std::thread::Builder::new()
        .name("bevy-board".into())
        .spawn(move || {
            let mut generation = 0;
            let result =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<(), String> {
                    let mut renderer = Renderer::new().map_err(|e| e.to_string())?;
                    log::info!("3D GPU adapter: {}", renderer.adapter);
                    let mut previous = None::<Arc<BoardSnapshot>>;
                    let mut warnings = Vec::new();
                    let mut fitted = OrbitCamera::fit([[0.; 3], [100., 100., 1.6]]);
                    while let Ok(mut request) = receiver.recv() {
                        // Coalesce drag events, retaining the latest owned scene snapshot.
                        while let Ok(next) = receiver.try_recv() {
                            request = next;
                        }
                        generation = request.generation;
                        if previous
                            .as_ref()
                            .is_none_or(|p| !Arc::ptr_eq(p, &request.snapshot))
                        {
                            let mesh = BoardMesh::from_snapshot(&request.snapshot)
                                .map_err(|e| e.to_string())?;
                            fitted = OrbitCamera::fit(mesh.bounds);
                            renderer.set_mesh(&mesh);
                            warnings = mesh.warnings;
                            previous = Some(request.snapshot);
                        }
                        let camera = if request.fit { fitted } else { request.camera };
                        let result = renderer
                            .render(request.size.0, request.size.1, camera, request.visibility)
                            .map_err(|e| e.to_string());
                        deliver(
                            id,
                            Rendered {
                                generation,
                                result,
                                warnings: warnings.clone(),
                                camera: Some(camera),
                                fitted: Some(fitted),
                            },
                        );
                    }
                    Ok(())
                }));
            let error = match result {
                Ok(Ok(())) => return,
                Ok(Err(error)) => error,
                Err(_) => "3D renderer failed to initialize or render the board".into(),
            };
            deliver(
                id,
                Rendered {
                    generation,
                    result: Err(error),
                    warnings: Vec::new(),
                    camera: None,
                    fitted: None,
                },
            );
        })?;
    Ok(sender)
}
fn deliver(id: TabId, rendered: Rendered) {
    let _ = slint::invoke_from_event_loop(move || {
        with_current_state(|state| state.board_3d_rendered(id, rendered))
    });
}
impl State {
    pub(crate) fn open_board_3d_tab(&mut self, project: i32, board: i32) {
        let Some(project) = usize::try_from(project)
            .ok()
            .and_then(|i| self.projects.get(i))
            .cloned()
        else {
            return;
        };
        let Some((id, _)) = usize::try_from(board)
            .ok()
            .and_then(|i| project.board_at(i))
        else {
            return;
        };
        if let Some((section, index)) = self.find_tab_where(|t| matches!(t, super::Tab::Board3d(b) if Rc::ptr_eq(b.project(), &project) && b.board() == id)) {
            self.set_current_tab(section, index as i32, true);
        } else {
            self.add_tab_maybe_current(super::Tab::Board3d(Box::new(Board3dTab::new(project, id))), true);
        }
    }
    fn board_3d_rendered(&mut self, id: TabId, rendered: Rendered) {
        let Some((section, index)) = self.find_tab(id) else {
            return;
        };
        let Some(super::Tab::Board3d(tab)) = self.sections[section].tab_mut(index) else {
            return;
        };
        let update = tab.accept(rendered);
        self.apply_update(section, index, update);
    }
}
