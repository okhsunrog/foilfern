//! Port of libs/librepcb/core/project/board/items/bi_device.{h,cpp} (and
//! `bi_pad.{h,cpp}` for footprint pads, which become computed views).
//!
//! Differences to upstream: the device stores only its persistent state
//! (component instance, library device/footprint/model, placement, flags,
//! attributes, stroke texts). The library elements are referenced by UUID
//! and resolved through the project library when needed; the validation
//! upstream does in the constructor happens when the device is added to a
//! board (see the project mutations). Footprint pads are
//! [`FootprintPadView`]s computed from the library footprint, the device's
//! pad-signal map and the component's signal connections; the hole stop
//! mask offsets are computed by [`BoardDevice::hole_stop_mask_offsets()`].

use std::collections::{BTreeMap, BTreeSet};

use super::pad_data::PadOnBoard;
use super::{BoardDesignRules, BoardStrokeTextData};
use crate::attribute::AttributeList;
use crate::geometry::{ComponentSide, Pad, PadGeometry, StrokeTextList, TraceAnchor, property};
use crate::library::LibraryBaseElement;
use crate::library::dev::{Device, Part};
use crate::library::pkg::{Footprint, FootprintPad, Package, PackagePad};
use crate::project::circuit::Circuit;
use crate::project::error::Result;
use crate::project::id::{ComponentInstanceId, ComponentSignalRef, NetSignalId};
use crate::project::library::ProjectLibrary;
use crate::serialization::{self, DeserializeObject, List, SExpression, SerializeObject};
use crate::types::{Angle, Layer, Length, MaskConfig, Point, Uuid};
use crate::utils::transform::{HasTransform, Transform};

/// A device (footprint of a component instance) placed on a board.
///
/// A board holds at most one device per component instance, so the device
/// is identified by its [`component()`](Self::component).
///
/// Serde: an object with the fields below (`stroke_texts` keyed by UUID).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct BoardDevice {
    component: ComponentInstanceId,
    lib_device: Uuid,
    lib_footprint: Uuid,
    lib_model: Option<Uuid>,
    position: Point,
    rotation: Angle,
    mirrored: bool,
    locked: bool,
    glue: bool,
    attributes: AttributeList,
    #[serde(with = "super::keyed_map")]
    stroke_texts: BTreeMap<Uuid, BoardStrokeTextData>,
}

impl BoardDevice {
    /// Returns parts of this device for the selected assembly variant, or
    /// all variants if `None` (upstream `BI_Device::getParts()`). Options
    /// without parts yield an empty part carrying the option attributes.
    pub fn parts(
        &self,
        circuit: &Circuit,
        variant: Option<crate::project::AssemblyVariantId>,
    ) -> Result<Vec<Part>> {
        let component = circuit
            .component_instance(self.component)
            .ok_or(crate::project::Error::InexistentComponent(self.component.0))?;
        let mut parts = Vec::new();
        for option in component.assembly_options() {
            if option.device() == self.lib_device
                && variant.is_none_or(|variant| option.assembly_variants().contains(&variant))
            {
                parts.extend(option.parts().iter().cloned());
                if option.parts().is_empty() {
                    parts.push(Part::new(
                        Default::default(),
                        Default::default(),
                        option.attributes().clone(),
                    ));
                }
            }
        }
        Ok(parts)
    }

    /// Creates a device without attributes, stroke texts and 3D model.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        component: ComponentInstanceId,
        lib_device: Uuid,
        lib_footprint: Uuid,
        position: Point,
        rotation: Angle,
        mirrored: bool,
        locked: bool,
        glue: bool,
    ) -> Self {
        Self {
            component,
            lib_device,
            lib_footprint,
            lib_model: None,
            position,
            rotation,
            mirrored,
            locked,
            glue,
            attributes: AttributeList::new(),
            stroke_texts: BTreeMap::new(),
        }
    }

    /// Creates a device like upstream's `BI_Device` constructor does for a
    /// new device: the attributes are copied from the library device, and
    /// the footprint's stroke texts are added (transformed to the device
    /// placement) if `load_initial_stroke_texts` is set.
    ///
    /// Fails if the footprint does not exist in the package.
    #[allow(clippy::too_many_arguments)]
    pub fn from_library(
        component: ComponentInstanceId,
        device: &Device,
        package: &Package,
        lib_footprint: Uuid,
        position: Point,
        rotation: Angle,
        mirrored: bool,
        locked: bool,
        glue: bool,
        load_initial_stroke_texts: bool,
    ) -> Result<Self> {
        let footprint = package.footprints().required_by_uuid(&lib_footprint)?;
        let mut dev = Self::new(
            component,
            device.metadata().uuid(),
            lib_footprint,
            position,
            rotation,
            mirrored,
            locked,
            glue,
        );
        dev.attributes = device.attributes().clone();
        if load_initial_stroke_texts {
            for text in dev.default_stroke_texts(footprint) {
                let text = BoardStrokeTextData::from_stroke_text(&text, locked);
                dev.stroke_texts.insert(text.uuid(), text);
            }
        }
        Ok(dev)
    }

    /// Returns the component instance (upstream
    /// `getComponentInstanceUuid()`).
    pub fn component(&self) -> ComponentInstanceId {
        self.component
    }

    property!(
        /// Returns the UUID of the library device.
        copy lib_device: Uuid, set_lib_device
    );
    property!(
        /// Returns the UUID of the footprint in the library package.
        copy lib_footprint: Uuid, set_lib_footprint
    );
    property!(
        /// Returns the UUID of the 3D model in the library package, if any.
        copy lib_model: Option<Uuid>, set_lib_model
    );
    property!(
        /// Returns the position.
        copy position: Point, set_position
    );
    property!(
        /// Returns the rotation.
        copy rotation: Angle, set_rotation
    );
    property!(
        /// Returns whether the device is mirrored (bottom side).
        copy mirrored: bool, set_mirrored
    );
    property!(
        /// Returns whether the device is locked.
        copy locked: bool, set_locked
    );
    property!(
        /// Returns whether glue is enabled.
        copy glue: bool, set_glue
    );
    property!(
        /// Returns the attributes.
        ref attributes: AttributeList, set_attributes
    );

    /// Returns the stroke texts, keyed by UUID.
    pub fn stroke_texts(&self) -> &BTreeMap<Uuid, BoardStrokeTextData> {
        &self.stroke_texts
    }

    /// Adds or replaces a stroke text, returns the replaced one.
    pub fn insert_stroke_text(&mut self, text: BoardStrokeTextData) -> Option<BoardStrokeTextData> {
        self.stroke_texts.insert(text.uuid(), text)
    }

    /// Removes a stroke text.
    pub fn remove_stroke_text(&mut self, uuid: &Uuid) -> Option<BoardStrokeTextData> {
        self.stroke_texts.remove(uuid)
    }

    /// Returns the transformation of the device.
    pub fn transform(&self) -> Transform {
        Transform::from_obj(self)
    }
    /// Resolves the placed library footprint without retaining cached state.
    pub fn footprint<'a>(&self, library: &'a ProjectLibrary) -> Result<&'a Footprint> {
        Ok(ResolvedDevice::resolve(self, library)?.footprint)
    }

    /// Returns the stroke texts of the footprint, transformed to the device
    /// placement (upstream `getDefaultStrokeTexts()`).
    pub fn default_stroke_texts(&self, footprint: &Footprint) -> StrokeTextList {
        let transform = self.transform();
        footprint
            .stroke_texts()
            .iter()
            .map(|text| {
                let mut text = text.clone();
                text.set_position(transform.map(&text.position()));
                text.set_rotation(transform.map_mirrorable(text.rotation()));
                text.set_mirrored(transform.map_mirror(text.mirrored()));
                text.set_layer(transform.map(&text.layer()));
                text
            })
            .collect()
    }

    /// Returns the UUID of the default 3D model of the footprint (the first
    /// model of the package used by the footprint, upstream
    /// `getDefaultLibModelUuid()`).
    pub fn default_lib_model(&self, package: &Package) -> Option<Uuid> {
        package
            .models_for_footprint(&self.lib_footprint)
            .first()
            .map(|m| m.uuid())
    }

    /// Returns the stop mask offsets of the footprint's holes (`None` = no
    /// stop mask opening), upstream `getHoleStopMasks()`.
    pub fn hole_stop_mask_offsets(
        footprint: &Footprint,
        rules: &BoardDesignRules,
    ) -> BTreeMap<Uuid, Option<Length>> {
        footprint
            .holes()
            .iter()
            .map(|hole| {
                let offset = match hole.stop_mask_config() {
                    MaskConfig::Off => None,
                    MaskConfig::Manual(offset) => Some(offset),
                    MaskConfig::Automatic => {
                        Some(*rules.stop_mask_clearance().calc_value(*hole.diameter()))
                    }
                };
                (hole.uuid(), offset)
            })
            .collect()
    }

    /// Returns the footprint pads of the device, computed from the library
    /// device, package and footprint and the component's signal
    /// connections.
    ///
    /// Fails if the library device, package or footprint does not exist or
    /// the device references a pad which is not in the package or
    /// pad-signal map (checked when the device is added to a board).
    pub fn pads<'a>(
        &'a self,
        library: &'a ProjectLibrary,
        circuit: &'a Circuit,
    ) -> Result<Vec<FootprintPadView<'a>>> {
        let resolved = ResolvedDevice::resolve(self, library)?;
        let component = circuit.component_instance(self.component);
        let lib_component = component.and_then(|c| library.component(&c.lib_component()));
        let transform = self.transform();
        resolved
            .footprint
            .pads()
            .iter()
            .map(|footprint_pad| {
                let package_pad = footprint_pad
                    .package_pad_uuid()
                    .map(|uuid| resolved.package.pads().required_by_uuid(&uuid))
                    .transpose()?;
                let signal = footprint_pad
                    .package_pad_uuid()
                    .map(|uuid| resolved.device.pad_signal_map().required_by_uuid(&uuid))
                    .transpose()?
                    .and_then(|item| item.signal_uuid());
                let net = signal.and_then(|s| component.and_then(|c| c.signal(&s)?.net()));
                let signal_name = signal.and_then(|s| {
                    lib_component
                        .and_then(|c| c.signals().by_uuid(&s))
                        .map(|s| s.name().as_str())
                });
                let net_name = net.and_then(|n| circuit.net_signal(n).map(|n| n.name().as_str()));
                Ok(FootprintPadView {
                    device: self,
                    footprint_pad,
                    package_pad,
                    signal: signal.map(|signal| ComponentSignalRef {
                        component: self.component,
                        signal,
                    }),
                    signal_name,
                    net,
                    net_name,
                    position: transform.map(&footprint_pad.pad().position()),
                    rotation: transform.map_mirrorable(footprint_pad.pad().rotation()),
                })
            })
            .collect()
    }

    /// Returns the view of one footprint pad, see [`pads()`](Self::pads).
    pub fn pad<'a>(
        &'a self,
        pad: &Uuid,
        library: &'a ProjectLibrary,
        circuit: &'a Circuit,
    ) -> Result<Option<FootprintPadView<'a>>> {
        Ok(self
            .pads(library, circuit)?
            .into_iter()
            .find(|p| p.uuid() == *pad))
    }
}

impl HasTransform for BoardDevice {
    fn position(&self) -> Point {
        self.position
    }

    fn rotation(&self) -> Angle {
        self.rotation
    }

    fn mirrored(&self) -> bool {
        self.mirrored
    }
}

/// The library elements of a device.
pub(crate) struct ResolvedDevice<'a> {
    pub device: &'a Device,
    pub package: &'a Package,
    pub footprint: &'a Footprint,
}

impl<'a> ResolvedDevice<'a> {
    /// Looks up the library device, package and footprint of `dev`.
    pub fn resolve(dev: &BoardDevice, library: &'a ProjectLibrary) -> Result<Self> {
        use crate::project::error::Error;
        let device = library
            .device(&dev.lib_device)
            .ok_or(Error::MissingLibraryDevice(dev.lib_device))?;
        let package = library
            .package(&device.package_uuid())
            .ok_or(Error::MissingLibraryPackage(device.package_uuid()))?;
        let footprint = package.footprints().required_by_uuid(&dev.lib_footprint)?;
        Ok(Self {
            device,
            package,
            footprint,
        })
    }
}

/// A pad of a device's footprint on the board (upstream `BI_Pad` of a
/// device), computed on demand, see [`BoardDevice::pads()`].
#[derive(Debug, Clone)]
pub struct FootprintPadView<'a> {
    device: &'a BoardDevice,
    footprint_pad: &'a FootprintPad,
    package_pad: Option<&'a PackagePad>,
    signal: Option<ComponentSignalRef>,
    signal_name: Option<&'a str>,
    net: Option<NetSignalId>,
    net_name: Option<&'a str>,
    position: Point,
    rotation: Angle,
}

impl<'a> FootprintPadView<'a> {
    /// Returns the UUID of the footprint pad.
    pub fn uuid(&self) -> Uuid {
        self.footprint_pad.uuid()
    }

    /// Returns the device.
    pub fn device(&self) -> &'a BoardDevice {
        self.device
    }

    /// Returns the library footprint pad.
    pub fn footprint_pad(&self) -> &'a FootprintPad {
        self.footprint_pad
    }

    /// Returns the pad properties (in footprint coordinates).
    pub fn properties(&self) -> &'a Pad {
        self.footprint_pad.pad()
    }

    /// Returns the package pad the footprint pad is connected to, if any.
    pub fn package_pad(&self) -> Option<&'a PackagePad> {
        self.package_pad
    }

    /// Returns the component signal the pad is connected to, if any
    /// (upstream `getComponentSignalInstance()`).
    pub fn component_signal(&self) -> Option<ComponentSignalRef> {
        self.signal
    }

    /// Returns the net of the pad (the net of its component signal).
    pub fn net(&self) -> Option<NetSignalId> {
        self.net
    }

    /// Returns the position on the board.
    pub fn position(&self) -> Point {
        self.position
    }

    /// Returns the rotation on the board.
    pub fn rotation(&self) -> Angle {
        self.rotation
    }

    /// Returns whether the pad is mirrored (the device is mirrored).
    pub fn mirrored(&self) -> bool {
        self.device.mirrored
    }

    fn on_board(&self) -> PadOnBoard<'a> {
        PadOnBoard {
            pad: self.footprint_pad.pad(),
            mirrored: self.device.mirrored,
        }
    }

    /// Returns the component side on the board (upstream
    /// `getComponentSide()`).
    pub fn component_side(&self) -> ComponentSide {
        self.on_board().component_side()
    }

    /// Returns the copper layer where the pad is soldered (upstream
    /// `getSolderLayer()`).
    pub fn solder_layer(&self) -> Layer {
        self.on_board().solder_layer()
    }

    /// Whether the pad has copper on `layer` (upstream `isOnLayer()`).
    pub fn is_on_layer(&self, layer: Layer) -> bool {
        self.on_board().is_on_layer(layer)
    }

    /// Returns the trace anchor of the pad.
    pub fn trace_anchor(&self) -> TraceAnchor {
        TraceAnchor::FootprintPad {
            device: self.device.component.0,
            pad: self.uuid(),
        }
    }

    /// Returns the name of the package pad, or the UUID of the footprint
    /// pad if it is not connected (upstream `getPadNameOrUuid()`).
    pub fn name_or_uuid(&self) -> String {
        self.package_pad
            .map_or_else(|| self.uuid().to_string(), |p| p.name().to_string())
    }

    /// Returns the text displayed on the pad: pad name, signal name and net
    /// name (upstream `updateText()`).
    pub fn text(&self) -> String {
        let mut text = self
            .package_pad
            .map(|p| p.name().to_string())
            .unwrap_or_default();
        if let Some(full_name) = self.signal_name {
            // Ignore a leading slash when looking for the separator.
            let short_name = full_name
                .char_indices()
                .skip(1)
                .find(|(_, c)| *c == '/')
                .map_or(full_name, |(i, _)| &full_name[..i]);
            if (full_name != text) && (short_name != text) {
                text.push(':');
                text.push_str(short_name);
            }
        }
        if text.chars().count() > 8 {
            text = text.chars().take(6).collect::<String>() + "…";
        }
        if let Some(net_name) = self.net_name {
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str(net_name);
        }
        text
    }

    /// Returns the geometries on the copper layers of the board and on the
    /// stop mask and solder paste layers (upstream `getGeometries()`);
    /// `connected_layers` are the layers of the traces connected to the pad
    /// (see [`Board::pad_trace_layers()`](super::Board::pad_trace_layers)).
    pub fn geometries(
        &self,
        copper_layers: &BTreeSet<Layer>,
        rules: &BoardDesignRules,
        connected_layers: &BTreeSet<Layer>,
    ) -> BTreeMap<Layer, Vec<PadGeometry>> {
        self.on_board()
            .geometries(copper_layers, rules, connected_layers)
    }
}

impl SerializeObject for BoardDevice {
    fn serialize(&self, root: &mut List) {
        root.append_value(&self.component);
        root.ensure_line_break();
        root.append_child("lib_device", &self.lib_device);
        root.ensure_line_break();
        root.append_child("lib_footprint", &self.lib_footprint);
        root.ensure_line_break();
        root.append_child("lib_3d_model", &self.lib_model);
        root.ensure_line_break();
        self.position.serialize(root.append_list("position"));
        root.append_child("rotation", &self.rotation);
        root.append_child("flip", &self.mirrored);
        root.append_child("lock", &self.locked);
        root.append_child("glue", &self.glue);
        root.ensure_line_break();
        self.attributes.serialize(root);
        root.ensure_line_break();
        for text in self.stroke_texts.values() {
            root.ensure_line_break();
            text.serialize(root.append_list("stroke_text"));
        }
        root.ensure_line_break();
    }
}

impl DeserializeObject for BoardDevice {
    fn deserialize(node: &SExpression) -> serialization::Result<Self> {
        let stroke_texts = node
            .children_named("stroke_text")
            .map(|child| BoardStrokeTextData::deserialize(child).map(|t| (t.uuid(), t)))
            .collect::<serialization::Result<_>>()?;
        Ok(Self {
            component: node.child_value("@0")?,
            lib_device: node.child_value("lib_device/@0")?,
            lib_footprint: node.child_value("lib_footprint/@0")?,
            lib_model: node.child_value("lib_3d_model/@0")?,
            position: Point::deserialize(node.required_child("position")?)?,
            rotation: node.child_value("rotation/@0")?,
            mirrored: node.child_value("flip/@0")?,
            locked: node.child_value("lock/@0")?,
            glue: node.child_value("glue/@0")?,
            attributes: AttributeList::deserialize(node)?,
            stroke_texts,
        })
    }
}
