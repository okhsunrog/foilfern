//! Device commands: port of libs/librepcb/editor/library/cmd/
//! {cmddeviceedit,cmddevicepadsignalmapitemedit,cmdpartedit}.{h,cpp}, of
//! `DeviceTab::selectComponent()`/`selectPackage()`, of the part list and
//! part editor (libs/librepcb/editor/library/dev/{partlistmodel,
//! parteditor}.cpp) and of libs/librepcb/editor/library/dev/
//! devicepinoutbuilder.{h,cpp} (auto-connect, CSV import, interactive
//! pinout assignment).
//!
//! The component and package are passed in as data (the caller loads
//! them, e.g. with the
//! [`LibraryElementCache`](crate::library_editor::LibraryElementCache)).
//! Questions upstream asks with message boxes ("reset the pinout first?")
//! are parameters.

use std::collections::{BTreeMap, BTreeSet};

use librepcb_core::attribute::AttributeList;
use librepcb_core::library::LibraryBaseElement;
use librepcb_core::library::cmp::{Component, ComponentSignal, ComponentSignalList};
use librepcb_core::library::dev::{Device, DevicePadSignalMapItem, Part};
use librepcb_core::library::pkg::{Package, PackagePad, PackagePadList};
use librepcb_core::types::{SimpleString, Uuid};
use librepcb_core::utils::toolbox;
use librepcb_i18n::tr;

use super::symbol::not_found;
use crate::error::{Error, Result};
use crate::library_editor::ElementCommand;

/// Edits the attributes of a device (upstream: attribute list of the
/// device tab, `CmdDeviceEdit`).
#[derive(Debug, Clone, PartialEq)]
pub struct EditDeviceAttributes {
    /// The new attributes.
    pub attributes: AttributeList,
}

impl ElementCommand<Device> for EditDeviceAttributes {
    type Output = ();

    fn text(&self) -> String {
        tr!("librepcb::editor::CmdDeviceEdit", "Edit Device Properties")
    }

    fn execute(self, dev: &mut Device) -> Result<()> {
        *dev.attributes_mut() = self.attributes;
        Ok(())
    }
}

/// Changes the component of a device; pads connected to signals which do
/// not exist in the new component are disconnected (upstream
/// `DeviceTab::selectComponent()`).
#[derive(Debug, Clone)]
pub struct SetDeviceComponent<'a> {
    /// The new component.
    pub component: &'a Component,
}

impl ElementCommand<Device> for SetDeviceComponent<'_> {
    type Output = ();

    fn text(&self) -> String {
        tr!("librepcb::editor::DeviceTab", "Change Component")
    }

    fn execute(self, dev: &mut Device) -> Result<()> {
        let uuid = self.component.metadata().uuid();
        if uuid == dev.component_uuid() {
            return Ok(());
        }
        dev.set_component_uuid(uuid);
        let signals = self.component.signals().uuid_set();
        for item in dev.pad_signal_map_mut().iter_mut() {
            if item.signal_uuid().is_none_or(|s| !signals.contains(&s)) {
                item.set_signal_uuid(None);
            }
        }
        Ok(())
    }
}

/// Changes the package of a device; the pad-signal-map gets exactly one
/// (unconnected) item per new package pad, items of existing pad UUIDs
/// are kept (upstream `DeviceTab::selectPackage()`).
#[derive(Debug, Clone)]
pub struct SetDevicePackage<'a> {
    /// The new package.
    pub package: &'a Package,
}

impl ElementCommand<Device> for SetDevicePackage<'_> {
    type Output = ();

    fn text(&self) -> String {
        tr!("librepcb::editor::DeviceTab", "Change Package")
    }

    fn execute(self, dev: &mut Device) -> Result<()> {
        let uuid = self.package.metadata().uuid();
        if uuid == dev.package_uuid() {
            return Ok(());
        }
        dev.set_package_uuid(uuid);
        let pads = self.package.pads().uuid_set();
        let map = dev.pad_signal_map_mut();
        for pad in map.uuids() {
            if !pads.contains(&pad) {
                map.take_by_uuid(&pad);
            }
        }
        // Sorted for determinism (upstream iterates a QSet; the map is
        // sorted when saving anyway).
        let existing = map.uuid_set();
        let new: BTreeSet<Uuid> = pads.difference(&existing).copied().collect();
        for pad in new {
            map.push(DevicePadSignalMapItem::new(pad, None, false));
        }
        Ok(())
    }
}

/// Connects a pad to a signal and/or sets whether it is optional
/// (upstream `CmdDevicePadSignalMapItemEdit`); `None` keeps a value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetPadSignal {
    /// The package pad.
    pub pad: Uuid,
    /// New signal (`Some(None)`: unconnected).
    pub signal: Option<Option<Uuid>>,
    /// Whether the pad is optional.
    pub optional: Option<bool>,
}

impl ElementCommand<Device> for SetPadSignal {
    type Output = ();

    fn text(&self) -> String {
        tr!(
            "librepcb::editor::CmdDevicePadSignalMapItemEdit",
            "Edit Device Pinout"
        )
    }

    fn execute(self, dev: &mut Device) -> Result<()> {
        let item = dev
            .pad_signal_map_mut()
            .by_uuid_mut(&self.pad)
            .ok_or_else(|| not_found("pad", self.pad))?;
        if let Some(v) = self.signal {
            item.set_signal_uuid(v);
        }
        if let Some(v) = self.optional {
            item.set_optional(v);
        }
        Ok(())
    }
}

/// Replaces the whole pinout (upstream `DevicePinoutBuilder::setMap()`):
/// pads in `map` are connected to their signal, all others disconnected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetDevicePinout {
    /// The undo text (e.g. "Reset Pinout").
    pub text: String,
    /// Package pad → component signal.
    pub map: BTreeMap<Uuid, Uuid>,
}

impl ElementCommand<Device> for SetDevicePinout {
    type Output = ();

    fn text(&self) -> String {
        self.text.clone()
    }

    fn execute(self, dev: &mut Device) -> Result<()> {
        for item in dev.pad_signal_map_mut().iter_mut() {
            let signal = self.map.get(&item.pad_uuid()).copied();
            item.set_signal_uuid(signal);
        }
        Ok(())
    }
}

/// Returns the current pinout of a device (upstream
/// `DevicePinoutBuilder::getMap()`).
pub fn device_pinout(dev: &Device) -> BTreeMap<Uuid, Uuid> {
    dev.pad_signal_map()
        .iter()
        .filter_map(|i| Some((i.pad_uuid(), i.signal_uuid()?)))
        .collect()
}

/// Returns the pads sorted by name like upstream (numeric, case
/// insensitive).
fn sorted_pads(pads: &PackagePadList) -> Vec<&PackagePad> {
    let mut pads: Vec<_> = pads.iter().collect();
    pads.sort_by(|a, b| toolbox::compare_numeric(a.name().as_str(), b.name().as_str()));
    pads
}

/// Finds a signal by name, first case sensitive, then insensitive.
fn find_signal<'a>(signals: &'a ComponentSignalList, name: &str) -> Option<&'a ComponentSignal> {
    signals
        .by_name(name, true)
        .or_else(|| signals.by_name(name, false))
}

/// The command which resets the pinout (upstream
/// `DevicePinoutBuilder::resetAll()`).
pub fn reset_pinout() -> SetDevicePinout {
    SetDevicePinout {
        text: tr!("librepcb::editor::DevicePinoutBuilder", "Reset Pinout"),
        map: BTreeMap::new(),
    }
}

/// The command which connects every unconnected pad to the signal with
/// the same name (upstream `DevicePinoutBuilder::autoConnect()`);
/// `reset_first`: disconnect all pads before (upstream asks).
pub fn auto_connect_pinout(
    dev: &Device,
    pads: &PackagePadList,
    signals: &ComponentSignalList,
    reset_first: bool,
) -> SetDevicePinout {
    let mut map = if reset_first {
        BTreeMap::new()
    } else {
        device_pinout(dev)
    };
    for pad in sorted_pads(pads) {
        if map.contains_key(&pad.uuid()) {
            continue;
        }
        if let Some(sig) = find_signal(signals, pad.name().as_str()) {
            map.insert(pad.uuid(), sig.uuid());
        }
    }
    SetDevicePinout {
        text: tr!(
            "librepcb::editor::DevicePinoutBuilder",
            "Auto-Connect Pads To Signals"
        ),
        map,
    }
}

/// The command which loads the pinout from a CSV file with the columns
/// "pad" and "signal" (upstream `DevicePinoutBuilder::loadFromFile()`;
/// without header, the first two columns are used). Only unconnected pads
/// are connected unless `reset_first`.
pub fn pinout_from_csv(
    dev: &Device,
    pads: &PackagePadList,
    signals: &ComponentSignalList,
    csv: &str,
    reset_first: bool,
) -> SetDevicePinout {
    let text = csv.replace('\r', "");
    let lines: Vec<&str> = text.split('\n').collect();
    let header: Vec<String> = lines
        .first()
        .map(|l| l.to_lowercase().split(',').map(str::to_owned).collect())
        .unwrap_or_default();
    let pad_col = header.iter().position(|c| c == "pad").unwrap_or(0);
    let signal_col = header.iter().position(|c| c == "signal").unwrap_or(1);
    // Pad name -> signal name (the last line wins, like QMap::insert()).
    let mut pinout: BTreeMap<String, String> = BTreeMap::new();
    for line in &lines {
        let values: Vec<&str> = line.split(',').collect();
        if values.len() > pad_col.max(signal_col) {
            pinout.insert(
                values[pad_col].trim().to_owned(),
                values[signal_col].trim().to_owned(),
            );
        }
    }
    let mut map = if reset_first {
        BTreeMap::new()
    } else {
        device_pinout(dev)
    };
    let items = dev.pad_signal_map().uuid_set();
    for (pad_name, signal_name) in &pinout {
        let pad = pads
            .by_name(pad_name, true)
            .or_else(|| pads.by_name(pad_name, false));
        let signal = find_signal(signals, signal_name);
        if let (Some(pad), Some(signal)) = (pad, signal)
            && !map.contains_key(&pad.uuid())
            && items.contains(&pad.uuid())
        {
            map.insert(pad.uuid(), signal.uuid());
        }
    }
    SetDevicePinout {
        text: tr!(
            "librepcb::editor::DevicePinoutBuilder",
            "Load Pinout From File"
        ),
        map,
    }
}

/// Whether some pads are unconnected while some signals are unused
/// (upstream `DevicePinoutBuilder::hasUnconnectedPadsAndSignals()`).
pub fn has_unconnected_pads_and_signals(dev: &Device, signals: &ComponentSignalList) -> bool {
    let mut unused: BTreeSet<Uuid> = signals.iter().map(|s| s.uuid()).collect();
    let mut unconnected_pads = false;
    for item in dev.pad_signal_map().iter() {
        match item.signal_uuid() {
            Some(s) => {
                unused.remove(&s);
            }
            None => unconnected_pads = true,
        }
    }
    unconnected_pads && !unused.is_empty()
}

/// Whether an unconnected pad has the name of a signal (upstream
/// `DevicePinoutBuilder::hasAutoConnectablePads()`).
pub fn has_auto_connectable_pads(
    dev: &Device,
    pads: &PackagePadList,
    signals: &ComponentSignalList,
) -> bool {
    let names: BTreeSet<String> = signals
        .iter()
        .map(|s| s.name().as_str().to_lowercase())
        .collect();
    dev.pad_signal_map().iter().any(|item| {
        item.signal_uuid().is_none()
            && pads
                .by_uuid(&item.pad_uuid())
                .is_some_and(|p| names.contains(&p.name().as_str().to_lowercase()))
    })
}

/// A choice of the interactive pinout assignment: a signal (or `None` for
/// "unconnected") and whether it is used by another pad already.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignalChoice {
    /// The signal (`None`: leave unconnected).
    pub signal: Option<Uuid>,
    /// The signal name (empty for "unconnected").
    pub name: String,
    /// Whether the signal is connected to a pad already.
    pub used: bool,
}

/// The interactive pinout assignment (upstream `DevicePinoutBuilder`'s
/// interactive mode): walks through the unconnected pads (sorted by name)
/// and offers the signals, best matches first.
#[derive(Debug, Clone, Default)]
pub struct InteractivePinout {
    pads: Vec<(Uuid, String)>,
    current_pad: Option<usize>,
    filter: String,
    choices: Vec<SignalChoice>,
    current_choice: usize,
}

impl InteractivePinout {
    /// Starts the assignment at the first unconnected pad (upstream
    /// `startInteractiveMode()`); inactive if all pads are connected.
    pub fn start(dev: &Device, pads: &PackagePadList, signals: &ComponentSignalList) -> Self {
        let mut this = Self {
            pads: sorted_pads(pads)
                .into_iter()
                .map(|p| (p.uuid(), p.name().as_str().to_owned()))
                .collect(),
            ..Self::default()
        };
        this.load_next_pad(dev, signals, None);
        this
    }

    /// Whether the assignment is active (a pad is current).
    pub fn is_active(&self) -> bool {
        self.current_pad.is_some()
    }

    /// The current pad (UUID and name).
    pub fn current_pad(&self) -> Option<(Uuid, &str)> {
        self.current_pad
            .and_then(|i| self.pads.get(i))
            .map(|(u, n)| (*u, n.as_str()))
    }

    /// The offered signals (filtered and sorted).
    pub fn choices(&self) -> &[SignalChoice] {
        &self.choices
    }

    /// The index of the selected choice.
    pub fn current_choice(&self) -> usize {
        self.current_choice
    }

    /// The signal filter.
    pub fn filter(&self) -> &str {
        &self.filter
    }

    /// Sets the signal filter (upstream `setSignalsFilter()`).
    pub fn set_filter(&mut self, dev: &Device, signals: &ComponentSignalList, filter: &str) {
        if filter != self.filter {
            self.filter = filter.to_owned();
            self.update_choices(dev, signals);
        }
    }

    /// Selects a choice (upstream `setCurrentSignalIndex()`: wraps around;
    /// `None` selects "unconnected").
    pub fn set_current_choice(&mut self, index: Option<i64>) {
        if self.choices.is_empty() {
            self.current_choice = 0;
            return;
        }
        match index {
            None => {
                if let Some(i) = self.choices.iter().position(|c| c.signal.is_none()) {
                    self.current_choice = i;
                }
            }
            Some(index) => {
                let n = self.choices.len() as i64;
                self.current_choice = (index + n).rem_euclid(n) as usize;
            }
        }
    }

    /// Commits the selected choice for the current pad and advances to the
    /// next unconnected pad (upstream `commitInteractiveMode()`). Returns
    /// the command to execute (none if the pad stays unconnected or no
    /// choice is selectable).
    pub fn commit(&mut self, dev: &Device, signals: &ComponentSignalList) -> Option<SetPadSignal> {
        let choice = self.choices.get(self.current_choice)?.clone();
        let (pad, _) = self.current_pad()?;
        let cmd = choice.signal.map(|s| SetPadSignal {
            pad,
            signal: Some(Some(s)),
            optional: None,
        });
        // The command is not executed yet: treat the pad as connected.
        self.load_next_pad(dev, signals, cmd.as_ref().map(|c| c.pad));
        cmd
    }

    /// Stops the assignment (upstream `exitInteractiveMode()`).
    pub fn exit(&mut self) {
        self.current_pad = None;
    }

    fn load_next_pad(
        &mut self,
        dev: &Device,
        signals: &ComponentSignalList,
        just_connected: Option<Uuid>,
    ) {
        let mut next = self.current_pad.map_or(0, |i| i + 1);
        while next < self.pads.len() {
            let pad = self.pads[next].0;
            let unconnected = dev
                .pad_signal_map()
                .by_uuid(&pad)
                .is_some_and(|i| i.signal_uuid().is_none());
            if unconnected && Some(pad) != just_connected {
                self.current_pad = Some(next);
                self.filter.clear();
                self.update_choices(dev, signals);
                return;
            }
            next += 1;
        }
        self.exit();
    }

    fn update_choices(&mut self, dev: &Device, signals: &ComponentSignalList) {
        let filter = self.filter.trim().to_lowercase();
        let used: BTreeSet<Uuid> = dev
            .pad_signal_map()
            .iter()
            .filter_map(|i| i.signal_uuid())
            .collect();
        let mut choices = Vec::new();
        if filter.is_empty() {
            choices.push(SignalChoice {
                signal: None,
                name: String::new(),
                used: false,
            });
        }
        for sig in signals.iter() {
            let name = sig.name().as_str();
            if filter.is_empty() || name.to_lowercase().contains(&filter) {
                choices.push(SignalChoice {
                    signal: Some(sig.uuid()),
                    name: name.to_owned(),
                    used: used.contains(&sig.uuid()),
                });
            }
        }
        let pad_name = self
            .current_pad()
            .map(|(_, n)| n.to_lowercase())
            .unwrap_or_default();
        choices.sort_by(|a, b| {
            let (an, bn) = (a.name.to_lowercase(), b.name.to_lowercase());
            let key = |matches: bool| std::cmp::Reverse(matches);
            if !pad_name.is_empty() && (an == pad_name) != (bn == pad_name) {
                return key(an == pad_name).cmp(&key(bn == pad_name));
            }
            if !filter.is_empty() && (an == filter) != (bn == filter) {
                return key(an == filter).cmp(&key(bn == filter));
            }
            if a.used != b.used {
                return a.used.cmp(&b.used);
            }
            if !filter.is_empty() && an.starts_with(&filter) != bn.starts_with(&filter) {
                return key(an.starts_with(&filter)).cmp(&key(bn.starts_with(&filter)));
            }
            if a.signal.is_none() != b.signal.is_none() {
                return key(a.signal.is_none()).cmp(&key(b.signal.is_none()));
            }
            toolbox::compare_numeric(&a.name, &b.name)
        });
        self.choices = choices;
        self.current_choice = 0;
    }
}

// --- Parts ---

fn part_mut(dev: &mut Device, index: usize) -> Result<&mut Part> {
    dev.parts_mut()
        .get_mut(index)
        .ok_or_else(|| not_found("part", index))
}

/// Adds a part (upstream `PartListModel::apply()` with a new part); fails
/// if MPN or manufacturer are empty. Returns its index.
#[derive(Debug, Clone, PartialEq)]
pub struct AddPart {
    /// The part.
    pub part: Part,
}

impl ElementCommand<Device> for AddPart {
    type Output = usize;

    fn text(&self) -> String {
        tr!("librepcb::editor::CmdListElementInsert", "Add {0}", "part")
    }

    fn execute(self, dev: &mut Device) -> Result<usize> {
        if self.part.mpn().is_empty() || self.part.manufacturer().is_empty() {
            return Err(Error::InvalidArgument(
                "The MPN and the manufacturer of a part must not be empty.".to_owned(),
            ));
        }
        Ok(dev.parts_mut().push(self.part))
    }
}

/// Edits a part (upstream `CmdPartEdit`); `None` keeps a value.
#[derive(Debug, Clone, PartialEq)]
pub struct EditPart {
    /// Index of the part.
    pub index: usize,
    /// New MPN.
    pub mpn: Option<SimpleString>,
    /// New manufacturer.
    pub manufacturer: Option<SimpleString>,
    /// New attributes.
    pub attributes: Option<AttributeList>,
}

impl ElementCommand<Device> for EditPart {
    type Output = ();

    fn text(&self) -> String {
        tr!("librepcb::editor::CmdPartEdit", "Edit Part")
    }

    fn execute(self, dev: &mut Device) -> Result<()> {
        let part = part_mut(dev, self.index)?;
        if let Some(v) = self.mpn {
            part.set_mpn(v);
        }
        if let Some(v) = self.manufacturer {
            part.set_manufacturer(v);
        }
        if let Some(v) = self.attributes {
            *part.attributes_mut() = v;
        }
        Ok(())
    }
}

/// Moves a part one position up (upstream `CmdPartsSwap`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MovePartUp {
    /// Index of the part.
    pub index: usize,
}

impl ElementCommand<Device> for MovePartUp {
    type Output = ();

    fn text(&self) -> String {
        tr!("librepcb::editor::CmdListElementsSwap", "Move {0}", "part")
    }

    fn execute(self, dev: &mut Device) -> Result<()> {
        part_mut(dev, self.index)?;
        if self.index > 0 {
            dev.parts_mut().swap(self.index, self.index - 1);
        }
        Ok(())
    }
}

/// Duplicates a part (inserted after it). Returns the index of the copy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DuplicatePart {
    /// Index of the part.
    pub index: usize,
}

impl ElementCommand<Device> for DuplicatePart {
    type Output = usize;

    fn text(&self) -> String {
        tr!("librepcb::editor::CmdListElementInsert", "Add {0}", "part")
    }

    fn execute(self, dev: &mut Device) -> Result<usize> {
        let copy = part_mut(dev, self.index)?.clone();
        Ok(dev.parts_mut().insert(self.index + 1, copy))
    }
}

/// Removes a part (upstream `CmdPartRemove`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemovePart {
    /// Index of the part.
    pub index: usize,
}

impl ElementCommand<Device> for RemovePart {
    type Output = ();

    fn text(&self) -> String {
        tr!(
            "librepcb::editor::CmdListElementRemove",
            "Remove {0}",
            "part"
        )
    }

    fn execute(self, dev: &mut Device) -> Result<()> {
        dev.parts_mut()
            .take(self.index)
            .ok_or_else(|| not_found("part", self.index))?;
        Ok(())
    }
}
