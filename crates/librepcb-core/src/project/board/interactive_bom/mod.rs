//! Port of libs/librepcb/core/project/board/boardinteractivehtmlbomgenerator
//! and core/export/interactivehtmlbom.{h,cpp}.
//!
//! The existing interactive-html-bom Rust crate owns HTML/CSS/JS generation.
//! This adapter resolves project geometry and assembly data, preserving the
//! source's first-pad-outline/first-hole representation and outer-layer-only
//! copper. It is read-only; callers rebuild plane fragments before extraction.
mod drawings;
mod footprint;

use super::Board;
use crate::{
    export::Timestamp,
    project::{AssemblyVariantId, Error, NetSignalId, Project, ProjectAttributeLookup, Result},
    types::{Layer, Point},
    utils::toolbox::compare_numeric,
};
use interactive_html_bom as ib;
use std::collections::BTreeMap;

/// Owned, renderer-independent HTML BOM data, prepared from one assembly.
/// The crate's public data can also be consumed before generating the HTML.
pub use interactive_html_bom::InteractiveHtmlBom;

/// Prepares an interactive assembly document without changing the project.
#[derive(Debug, Clone)]
pub struct BoardInteractiveBomGenerator<'a> {
    project: &'a Project,
    board: &'a Board,
    variant: AssemblyVariantId,
    custom_attributes: Vec<String>,
    component_order: Vec<String>,
}
static_assertions::assert_impl_all!(BoardInteractiveBomGenerator<'_>: Send, Sync);
static_assertions::assert_impl_all!(InteractiveHtmlBom: Send, Sync);

struct BomItem {
    reference: String,
    footprint: usize,
    bottom: bool,
}
fn point(p: Point) -> (f32, f32) {
    (p.x.to_mm() as f32, -p.y.to_mm() as f32)
}
fn side(layer: Layer) -> Option<ib::Layer> {
    match layer {
        Layer::TOP_COPPER => Some(ib::Layer::Front),
        Layer::BOT_COPPER => Some(ib::Layer::Back),
        _ => None,
    }
}
impl<'a> BoardInteractiveBomGenerator<'a> {
    pub fn new(project: &'a Project, board: &'a Board, variant: AssemblyVariantId) -> Self {
        Self {
            project,
            board,
            variant,
            custom_attributes: Vec::new(),
            component_order: Vec::new(),
        }
    }
    pub fn set_custom_attributes(&mut self, attributes: Vec<String>) {
        self.custom_attributes = attributes;
    }
    pub fn set_component_order(&mut self, order: Vec<String>) {
        self.component_order = order;
    }

    pub fn generate(&self, timestamp: Timestamp) -> Result<InteractiveHtmlBom> {
        let variant = self
            .project
            .circuit()
            .assembly_variant(self.variant)
            .ok_or(Error::NotFound {
                kind: crate::project::EntityKind::AssemblyVariant,
                uuid: self.variant.0,
            })?;
        let metadata = self.project.metadata();
        let mut title = metadata.name.to_string();
        if self.project.circuit().assembly_variants().len() > 1 {
            title.push_str(&format!(" ({})", variant.name()));
        }
        let (low, high) = self
            .board
            .bounding_rect(self.project.library())?
            .unwrap_or((Point::ORIGIN, Point::ORIGIN));
        let date = match timestamp {
            Timestamp::Local(dt) | Timestamp::Utc(dt) => dt.format("%Y-%m-%d %H:%M:%S").to_string(),
            Timestamp::Offset(dt) => dt.format("%Y-%m-%d %H:%M:%S").to_string(),
        };
        let mut data = InteractiveHtmlBom::new(
            &title,
            &metadata.author,
            metadata.version.as_str(),
            &date,
            (low.x.to_mm() as f32, -high.y.to_mm() as f32),
            (high.x.to_mm() as f32, -low.y.to_mm() as f32),
        );
        data.fields = vec!["Value / MPN".into(), "Package".into()];
        data.fields.extend(self.custom_attributes.iter().cloned());
        self.board_drawings(&mut data)?;
        self.copper(&mut data)?;
        let mut groups: BTreeMap<Vec<String>, Vec<BomItem>> = BTreeMap::new();
        for device in self.board.devices().values() {
            let component = self
                .project
                .circuit()
                .component_instance(device.component())
                .ok_or(Error::InexistentComponent(device.component().0))?;
            let parts = device.parts(self.project.circuit(), Some(self.variant))?;
            let lookup =
                ProjectAttributeLookup::device(self.project, self.board, device, parts.first());
            let mpn = lookup.value("MPN")?.unwrap_or_default();
            let mut value = lookup.substitute(&lookup.value("VALUE")?.unwrap_or_default())?;
            if !value.contains(&mpn) {
                if value.trim().is_empty() {
                    value = mpn;
                } else {
                    value.push_str(&format!(" ({mpn})"));
                }
            }
            let mut fields = vec![
                value
                    .split('\n')
                    .filter(|s| !s.is_empty())
                    .collect::<Vec<_>>()
                    .join(" "),
                lookup.value("PACKAGE")?.unwrap_or_default(),
            ];
            for attribute in &self.custom_attributes {
                fields.push(lookup.substitute(&lookup.value(attribute)?.unwrap_or_default())?);
            }
            let id = self.footprint(&mut data, device, &fields, !parts.is_empty())?;
            if !parts.is_empty() {
                groups.entry(fields).or_default().push(BomItem {
                    reference: component.name().to_string(),
                    footprint: id,
                    bottom: device.mirrored(),
                });
            }
        }
        let mut groups: Vec<_> = groups.into_values().collect();
        for group in &mut groups {
            group.sort_by(|a, b| compare_numeric(&a.reference, &b.reference));
        }
        let priority = |items: &[BomItem]| {
            items
                .iter()
                .filter_map(|item| {
                    let prefix = item
                        .reference
                        .trim_end_matches(|c: char| c.is_ascii_digit());
                    self.component_order.iter().position(|p| p == prefix)
                })
                .min()
                .unwrap_or(self.component_order.len())
        };
        groups.sort_by(|a, b| {
            priority(a)
                .cmp(&priority(b))
                .then_with(|| compare_numeric(&a[0].reference, &b[0].reference))
        });
        for group in groups {
            let rows = |bottom: Option<bool>| {
                group
                    .iter()
                    .filter(|item| bottom.is_none_or(|b| item.bottom == b))
                    .map(|item| ib::RefMap::new(&item.reference, item.footprint))
                    .collect::<Vec<_>>()
            };
            let front = rows(Some(false));
            let back = rows(Some(true));
            if !front.is_empty() {
                data.bom_front.push(front);
            }
            if !back.is_empty() {
                data.bom_back.push(back);
            }
            data.bom_both.push(rows(None));
        }
        Ok(data)
    }
    fn net(&self, id: Option<NetSignalId>) -> Option<&str> {
        id.and_then(|id| self.project.circuit().net_signal(id))
            .map(|net| net.name().as_str())
    }
}
