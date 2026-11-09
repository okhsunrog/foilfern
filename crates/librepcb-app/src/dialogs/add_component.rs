//! The "add component" dialog of the schematic editor.
//!
//! Port of libs/librepcb/editor/project/addcomponentdialog.{h,cpp}: search
//! in the workspace library database (components, devices and parts), a
//! component category tree when not searching, the symbol variant of the
//! selected component and previews of its symbol and of the footprint of
//! the selected device. The UI is `ui/dialogs/addcomponentdialog.slint`.
//!
//! Differences to upstream: the symbol preview shows the first gate of the
//! symbol variant only; there is no live part information (prices,
//! availability) and no context menu; the choice of a part (MPN) selects
//! its device.

use std::collections::BTreeSet;

use librepcb_app_ui as ui;
use librepcb_core::fileio::{FilePath, TransactionalDirectory, TransactionalFileSystem};
use librepcb_core::library::LibraryBaseElement;
use librepcb_core::library::cmp::Component;
use librepcb_core::library::dev::Device;
use librepcb_core::library::pkg::Package;
use librepcb_core::library::sym::Symbol;
use librepcb_core::types::Uuid;
use librepcb_core::workspace::{CategoryTreeNode, ElementKind, LibraryDb};
use librepcb_editor::fsm::schematic::ComponentChoice;
use librepcb_i18n::tr;
use librepcb_scene::{ColorScheme, FootprintScene, SymbolScene};
use librepcb_scene::{RenderOptions, RenderSize, render_scene};
use slint::{Image, Rgba8Pixel, SharedPixelBuffer, SharedString};
use std::rc::Rc;
use std::sync::Arc;

use crate::models::UiModel;

/// Size of the preview images in pixels.
const PREVIEW_WIDTH: u32 = 460;
const PREVIEW_HEIGHT: u32 = 260;

/// A row of the component tree.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Row {
    Component(usize),
    Device(usize, usize),
    Part(usize, usize, usize),
}

#[derive(Debug, Clone)]
struct DeviceNode {
    uuid: Uuid,
    name: String,
    package: String,
    parts: Vec<String>,
    expanded: bool,
    deprecated: bool,
}

#[derive(Debug, Clone)]
struct ComponentNode {
    uuid: Uuid,
    name: String,
    devices: Vec<DeviceNode>,
    expanded: bool,
    deprecated: bool,
}

/// The state of the dialog (models shown by the UI, the selection).
pub struct AddComponentDialog {
    db: Arc<LibraryDb>,
    locale_order: Vec<String>,
    norm_order: Vec<String>,
    category_tree: Vec<CategoryTreeNode>,
    expanded_categories: BTreeSet<Uuid>,
    category_rows: Vec<Option<Uuid>>,
    categories: Rc<UiModel<ui::TreeViewItemData>>,
    nodes: Vec<ComponentNode>,
    rows: Vec<Row>,
    components: Rc<UiModel<ui::TreeViewItemData>>,
    component: Option<Component>,
    variants: Vec<Uuid>,
    variant: Option<usize>,
    device: Option<Uuid>,
    /// The selected row of the component tree.
    selected: Option<Row>,
    /// Texts and previews for the UI.
    pub view: AddComponentView,
}

/// What the dialog shows besides the trees.
#[derive(Clone, Default)]
pub struct AddComponentView {
    /// Name of the selected component.
    pub component_name: String,
    /// Its description.
    pub component_description: String,
    /// The symbol variants (only shown if more than one).
    pub symbol_variants: Vec<String>,
    /// The chosen variant.
    pub symbol_variant_index: i32,
    /// Symbol preview.
    pub symbol_preview: Option<Image>,
    /// Name of the selected device (with package).
    pub device_name: String,
    /// Footprint preview.
    pub footprint_preview: Option<Image>,
    /// Error message.
    pub error: String,
}

fn tree_item(level: i32, text: String, has_children: bool, expanded: bool) -> ui::TreeViewItemData {
    ui::TreeViewItemData {
        level,
        text: text.into(),
        has_children,
        expanded,
        ..ui::TreeViewItemData::default()
    }
}

/// Converts a rendered RGBA image to a Slint image.
pub fn to_slint_image(image: &librepcb_scene::RgbaImage) -> Image {
    Image::from_rgba8(SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(
        &image.data,
        image.width,
        image.height,
    ))
}

fn open_dir(dir: &FilePath) -> Option<TransactionalDirectory> {
    let fs = TransactionalFileSystem::open_ro(dir).ok()?;
    Some(TransactionalDirectory::new(Arc::new(fs), ""))
}

impl AddComponentDialog {
    /// Creates the dialog for the workspace library database with the
    /// project's locale and norm order, searching for `search_term`.
    pub fn new(
        db: Arc<LibraryDb>,
        locale_order: Vec<String>,
        norm_order: Vec<String>,
        search_term: &str,
    ) -> Self {
        let category_tree = db
            .category_tree(ElementKind::ComponentCategory, &locale_order)
            .unwrap_or_default();
        let mut dialog = Self {
            db,
            locale_order,
            norm_order,
            category_tree,
            expanded_categories: BTreeSet::new(),
            category_rows: Vec::new(),
            categories: UiModel::shared(Vec::new()),
            nodes: Vec::new(),
            rows: Vec::new(),
            components: UiModel::shared(Vec::new()),
            component: None,
            variants: Vec::new(),
            variant: None,
            device: None,
            selected: None,
            view: AddComponentView::default(),
        };
        dialog.rebuild_categories();
        dialog.set_selected_component(None);
        if !search_term.trim().is_empty() {
            dialog.search(search_term, true);
        }
        dialog
    }

    /// The category tree model.
    pub fn categories(&self) -> &Rc<UiModel<ui::TreeViewItemData>> {
        &self.categories
    }

    /// The component tree model.
    pub fn components(&self) -> &Rc<UiModel<ui::TreeViewItemData>> {
        &self.components
    }

    fn rebuild_categories(&mut self) {
        fn add(
            nodes: &[CategoryTreeNode],
            level: i32,
            expanded: &BTreeSet<Uuid>,
            rows: &mut Vec<Option<Uuid>>,
            items: &mut Vec<ui::TreeViewItemData>,
        ) {
            for n in nodes {
                let open = expanded.contains(&n.uuid);
                rows.push(Some(n.uuid));
                items.push(tree_item(
                    level,
                    n.name.clone(),
                    !n.children.is_empty(),
                    open,
                ));
                if open {
                    add(&n.children, level + 1, expanded, rows, items);
                }
            }
        }
        let mut rows = Vec::new();
        let mut items = Vec::new();
        add(
            &self.category_tree,
            0,
            &self.expanded_categories,
            &mut rows,
            &mut items,
        );
        rows.push(None);
        items.push(tree_item(
            0,
            tr!(
                "librepcb::editor::CategoryTreeModelLegacy",
                "(Without Category)"
            ),
            false,
            false,
        ));
        self.category_rows = rows;
        self.categories.replace_all(items);
    }

    /// A category row was written by the UI (expanded/collapsed).
    pub fn category_row_written(&mut self, row: usize, data: &ui::TreeViewItemData) {
        let Some(Some(uuid)) = self.category_rows.get(row).copied() else {
            return;
        };
        let changed = if data.expanded {
            self.expanded_categories.insert(uuid)
        } else {
            self.expanded_categories.remove(&uuid)
        };
        if changed {
            self.rebuild_categories();
        }
    }

    /// Upstream `setSelectedCategory()`: lists the components of a category.
    pub fn select_category(&mut self, row: usize) {
        let Some(category) = self.category_rows.get(row).copied() else {
            return;
        };
        self.view.error.clear();
        let components = self
            .db
            .by_category(ElementKind::Component, category, None)
            .unwrap_or_default();
        self.nodes = components
            .into_iter()
            .filter_map(|c| self.component_node(c, false, &|_| true))
            .collect();
        self.sort_nodes();
        self.selected = None;
        self.rebuild_rows();
        self.set_selected_component(None);
    }

    fn translations(&self, kind: ElementKind, dir: &FilePath) -> (String, bool) {
        let name = self
            .db
            .translations(kind, dir, &self.locale_order)
            .ok()
            .flatten()
            .map(|t| t.name)
            .unwrap_or_default();
        let deprecated = self
            .db
            .metadata(kind, dir)
            .ok()
            .flatten()
            .is_some_and(|m| m.deprecated);
        (name, deprecated)
    }

    fn device_node(&self, uuid: Uuid, parts_filter: Option<&str>) -> Option<DeviceNode> {
        let dir = self.db.latest(ElementKind::Device, uuid).ok()??;
        let (name, deprecated) = self.translations(ElementKind::Device, &dir);
        let package = self
            .db
            .device_metadata(&dir)
            .ok()
            .flatten()
            .and_then(|m| self.db.latest(ElementKind::Package, m.package_uuid).ok()?)
            .map(|p| self.translations(ElementKind::Package, &p).0)
            .unwrap_or_default();
        let parts = match parts_filter {
            Some(keyword) => self.db.find_parts_of_device(uuid, keyword),
            None => self.db.device_parts(uuid),
        }
        .unwrap_or_default()
        .iter()
        .map(|p| {
            if p.manufacturer().is_empty() {
                p.mpn().to_string()
            } else {
                format!("{} | {}", p.mpn(), p.manufacturer())
            }
        })
        .collect();
        Some(DeviceNode {
            uuid,
            name,
            package,
            parts,
            expanded: false,
            deprecated,
        })
    }

    fn component_node(
        &self,
        uuid: Uuid,
        expanded: bool,
        device_filter: &dyn Fn(Uuid) -> bool,
    ) -> Option<ComponentNode> {
        let dir = self.db.latest(ElementKind::Component, uuid).ok()??;
        let (name, deprecated) = self.translations(ElementKind::Component, &dir);
        let mut devices: Vec<DeviceNode> = self
            .db
            .component_devices(uuid)
            .unwrap_or_default()
            .into_iter()
            .filter(|d| device_filter(*d))
            .filter_map(|d| self.device_node(d, None))
            .collect();
        devices.sort_by(|a, b| super::natural_cmp(&a.name.to_lowercase(), &b.name.to_lowercase()));
        Some(ComponentNode {
            uuid,
            name,
            devices,
            expanded,
            deprecated,
        })
    }

    fn sort_nodes(&mut self) {
        self.nodes
            .sort_by(|a, b| super::natural_cmp(&a.name.to_lowercase(), &b.name.to_lowercase()));
    }

    /// Upstream `searchComponents()` (filter text edited; an empty filter
    /// shows the category tree again).
    pub fn search(&mut self, input: &str, select_first_device: bool) {
        self.view.error.clear();
        let input = input.trim();
        self.nodes.clear();
        if input.chars().count() > 1 {
            let find = |kind| self.db.find(kind, input).unwrap_or_default();
            let matching_components = find(ElementKind::Component);
            let matching_devices = find(ElementKind::Device);
            let part_devices = self.db.find_devices_of_parts(input).unwrap_or_default();
            let mut added: BTreeSet<Uuid> = BTreeSet::new();
            for c in &matching_components {
                if let Some(node) = self.component_node(*c, false, &|_| true) {
                    added.extend(node.devices.iter().map(|d| d.uuid));
                    self.nodes.push(node);
                }
            }
            let mut devices = part_devices;
            for d in matching_devices.iter() {
                if !devices.contains(d) {
                    devices.push(*d);
                }
            }
            for d in devices.into_iter().filter(|d| !added.contains(d)) {
                let Some(dir) = self.db.latest(ElementKind::Device, d).ok().flatten() else {
                    continue;
                };
                let Some(meta) = self.db.device_metadata(&dir).ok().flatten() else {
                    continue;
                };
                let filter = (!matching_devices.contains(&d)).then_some(input);
                let Some(mut dev) = self.device_node(d, filter) else {
                    continue;
                };
                dev.expanded = true;
                match self
                    .nodes
                    .iter_mut()
                    .find(|n| n.uuid == meta.component_uuid)
                {
                    Some(node) => node.devices.push(dev),
                    None => {
                        let Some(mut node) =
                            self.component_node(meta.component_uuid, true, &|_| false)
                        else {
                            continue;
                        };
                        node.devices.push(dev);
                        self.nodes.push(node);
                    }
                }
            }
            let device_count: usize = self.nodes.iter().map(|n| n.devices.len()).sum();
            let expand_all = device_count <= 10 || self.nodes.len() <= 1;
            for n in &mut self.nodes {
                n.expanded |= expand_all;
            }
        }
        self.sort_nodes();
        self.selected = None;
        self.rebuild_rows();
        self.set_selected_component(None);
        if select_first_device && !self.nodes.is_empty() {
            self.nodes[0].expanded = true;
            self.rebuild_rows();
            let row = if self.nodes[0].devices.is_empty() {
                0
            } else {
                1
            };
            self.select_row(row);
        } else if !self.rows.is_empty() {
            self.select_row(0);
        }
    }

    fn rebuild_rows(&mut self) {
        let mut rows = Vec::new();
        let mut items = Vec::new();
        for (ci, c) in self.nodes.iter().enumerate() {
            rows.push(Row::Component(ci));
            let mut item = tree_item(
                0,
                format!("{} [{}]", c.name, c.devices.len()),
                !c.devices.is_empty(),
                c.expanded,
            );
            item.hint = if c.deprecated {
                tr!("LibraryTreeView", "Deprecated").into()
            } else {
                SharedString::new()
            };
            items.push(item);
            if !c.expanded {
                continue;
            }
            for (di, d) in c.devices.iter().enumerate() {
                rows.push(Row::Device(ci, di));
                let text = if d.package.is_empty() || d.name.contains(&d.package) {
                    d.name.clone()
                } else {
                    format!("{} [{}]", d.name, d.package)
                };
                let mut item = tree_item(1, text, !d.parts.is_empty(), d.expanded);
                if d.deprecated {
                    item.hint = tr!("LibraryTreeView", "Deprecated").into();
                }
                items.push(item);
                if !d.expanded {
                    continue;
                }
                for (pi, p) in d.parts.iter().enumerate() {
                    rows.push(Row::Part(ci, di, pi));
                    items.push(tree_item(2, p.clone(), false, false));
                }
            }
        }
        self.rows = rows;
        self.components.replace_all(items);
    }

    /// A component row was written by the UI (expanded/collapsed).
    pub fn component_row_written(&mut self, row: usize, data: &ui::TreeViewItemData) {
        match self.rows.get(row).cloned() {
            Some(Row::Component(c)) if self.nodes[c].expanded != data.expanded => {
                self.nodes[c].expanded = data.expanded;
                self.rebuild_rows();
            }
            Some(Row::Device(c, d)) if self.nodes[c].devices[d].expanded != data.expanded => {
                self.nodes[c].devices[d].expanded = data.expanded;
                self.rebuild_rows();
            }
            _ => {}
        }
    }

    /// The selected row of the component tree (`-1`: none).
    pub fn current_row(&self) -> i32 {
        self.selected
            .as_ref()
            .and_then(|s| self.rows.iter().position(|r| r == s))
            .map_or(-1, |i| i as i32)
    }

    /// Upstream `treeComponents_currentItemChanged()`.
    pub fn select_row(&mut self, row: usize) {
        self.selected = self.rows.get(row).cloned();
        let (c, d) = match self.rows.get(row) {
            Some(Row::Component(c)) => (*c, None),
            Some(Row::Device(c, d)) | Some(Row::Part(c, d, _)) => (*c, Some(*d)),
            None => return,
        };
        let cmp = self.nodes[c].uuid;
        if self.component.as_ref().map(|x| x.metadata().uuid()) != Some(cmp) {
            let component = self
                .db
                .latest(ElementKind::Component, cmp)
                .ok()
                .flatten()
                .and_then(|dir| open_dir(&dir))
                .and_then(|dir| Component::open(dir).ok());
            if component.is_none() {
                self.view.error = tr!(
                    "librepcb::editor::AddComponentDialog",
                    "Failed to open the component."
                );
            }
            self.set_selected_component(component);
        }
        let device = d.map(|d| self.nodes[c].devices[d].uuid);
        if device != self.device {
            self.set_selected_device(device);
        }
    }

    fn set_selected_component(&mut self, component: Option<Component>) {
        self.view.component_name = tr!(
            "librepcb::editor::AddComponentDialog",
            "No component selected"
        );
        self.view.component_description.clear();
        self.view.symbol_variants.clear();
        self.variants.clear();
        self.variant = None;
        self.component = component;
        self.set_selected_device(None);
        if let Some(cmp) = &self.component {
            self.view.component_name = cmp.metadata().names().value(&self.locale_order).to_string();
            self.view.component_description = cmp
                .metadata()
                .descriptions()
                .value(&self.locale_order)
                .clone();
            for v in cmp.symbol_variants().iter() {
                let mut text = v.names().value(&self.locale_order).to_string();
                if !v.norm().is_empty() {
                    text += &format!(" [{}]", v.norm());
                }
                self.view.symbol_variants.push(text);
                self.variants.push(v.uuid());
            }
            if !self.variants.is_empty() {
                self.variant = Some(
                    cmp.symbol_variant_index_by_norm(&self.norm_order)
                        .unwrap_or(0),
                );
            }
        }
        self.view.symbol_variant_index = self.variant.map_or(-1, |i| i as i32);
        self.update_symbol_preview();
    }

    /// A symbol variant was chosen.
    pub fn select_symbol_variant(&mut self, index: usize) {
        if index < self.variants.len() {
            self.variant = Some(index);
            self.view.symbol_variant_index = index as i32;
            self.update_symbol_preview();
        }
    }

    fn update_symbol_preview(&mut self) {
        self.view.symbol_preview = None;
        let (Some(cmp), Some(variant)) = (&self.component, self.variant) else {
            return;
        };
        let Some(v) = cmp.symbol_variants().get(variant) else {
            return;
        };
        let Some(item) = v.symbol_items().first() else {
            return;
        };
        let symbol = self
            .db
            .latest(ElementKind::Symbol, item.symbol_uuid())
            .ok()
            .flatten()
            .and_then(|dir| open_dir(&dir))
            .and_then(|dir| Symbol::open(dir).ok());
        let Some(symbol) = symbol else { return };
        let font = librepcb_scene::default_stroke_font();
        let scene = SymbolScene::build(&symbol, font.as_ref(), &ColorScheme::SCHEMATIC_LIGHT);
        let options = RenderOptions {
            size: RenderSize::Fit {
                width: PREVIEW_WIDTH,
                height: PREVIEW_HEIGHT,
            },
            margin: 10.0,
            background: None,
        };
        if let Ok(image) = render_scene(
            scene.scene(),
            scene.content_bounds(),
            false,
            scene.background(),
            &options,
        ) {
            self.view.symbol_preview = Some(to_slint_image(&image));
        }
    }

    fn set_selected_device(&mut self, device: Option<Uuid>) {
        self.device = device;
        self.view.device_name = tr!("librepcb::editor::AddComponentDialog", "No device selected");
        self.view.footprint_preview = None;
        let Some(device) = device else { return };
        let dev = self
            .db
            .latest(ElementKind::Device, device)
            .ok()
            .flatten()
            .and_then(|dir| open_dir(&dir))
            .and_then(|dir| Device::open(dir).ok());
        let Some(dev) = dev else { return };
        let pkg = self
            .db
            .latest(ElementKind::Package, dev.package_uuid())
            .ok()
            .flatten()
            .and_then(|dir| open_dir(&dir))
            .and_then(|dir| Package::open(dir).ok());
        let dev_name = dev.metadata().names().value(&self.locale_order).to_string();
        let Some(pkg) = pkg else {
            self.view.device_name = dev_name;
            return;
        };
        let pkg_name = pkg.metadata().names().value(&self.locale_order).to_string();
        self.view.device_name = if dev_name.to_lowercase().contains(&pkg_name.to_lowercase()) {
            dev_name
        } else {
            format!("{dev_name} [{pkg_name}]")
        };
        let Some(footprint) = pkg.footprints().first() else {
            return;
        };
        let font = librepcb_scene::default_stroke_font();
        let scene = FootprintScene::build(footprint, font.as_ref(), &ColorScheme::BOARD_DARK);
        let options = RenderOptions {
            size: RenderSize::Fit {
                width: PREVIEW_WIDTH,
                height: PREVIEW_HEIGHT,
            },
            margin: 10.0,
            background: None,
        };
        let background = ColorScheme::BOARD_DARK.color_or_transparent("board_background");
        if let Ok(image) = render_scene(
            scene.scene(),
            scene.content_bounds(),
            false,
            background,
            &options,
        ) {
            self.view.footprint_preview = Some(to_slint_image(&image));
        }
    }

    /// The current choice (upstream `accept()`), `None` if no component
    /// with a symbol variant is selected.
    pub fn choice(&self) -> Option<ComponentChoice> {
        let cmp = self.component.as_ref()?;
        let variant = self.variants.get(self.variant?)?;
        Some(ComponentChoice {
            component: cmp.metadata().uuid(),
            symbol_variant: Some(*variant),
            device: self.device,
        })
    }
}

impl std::fmt::Debug for AddComponentDialog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("librepcb::editor::AddComponentDialog")
            .field("components", &self.nodes.len())
            .finish_non_exhaustive()
    }
}
