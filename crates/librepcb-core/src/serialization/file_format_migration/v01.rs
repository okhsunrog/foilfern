//! Port of libs/librepcb/core/serialization/fileformatmigrationv01.{h,cpp}.
//!
//! Differences to upstream:
//! - The devices of a component instance are added in UUID order (upstream
//!   iterates a `QSet`, i.e. in hash order, which only matters if several
//!   devices of one component are used in boards).
//! - Board outline circles are located by their `position` child (upstream
//!   reads the first two children of the circle node and fails, so such
//!   files could not be upgraded at all).
//! - Board outlines of the same length keep their file order when sorted
//!   (upstream uses an unstable sort).

use std::collections::{BTreeMap, BTreeSet, HashMap};

use librepcb_i18n::tr;
use unicode_normalization::UnicodeNormalization;

use super::{
    FileFormatMigration, MigrationError, MigrationMessage, MigrationResult, MigrationSeverity,
    append_string, append_token, as_list, build_message, child_mut, child_str, children_mut,
    count_children, element_dirs, index_file_entries, read_file, remove_child,
    remove_legacy_workspace_files, rename_child, replace_child, replace_token, set_child_value,
    upgrade_file, upgrade_version_file, version, write_file,
};
use crate::fileio::{FileSystem, TransactionalDirectory, VersionFile};
use crate::geometry::Path;
use crate::serialization::{
    DeserializeObject, FromSExpression, List, SExpression, SerializeObject, ToSExpression,
};
use crate::types::{
    Alignment, Angle, HAlign, Length, Orientation, Point, PositiveLength, SimpleString,
    UnsignedLength, Uuid, VAlign, Version,
};
use crate::utils::painter_path::PainterPathPx;

/// Upstream `FileFormatMigrationV01::Text`.
#[derive(Debug, Clone)]
struct Text {
    uuid: Uuid,
    layer_name: String,
    text: String,
    position: Point,
    rotation: Angle,
    height: PositiveLength,
    align: Alignment,
}

/// Upstream `FileFormatMigrationV01::Symbol`.
#[derive(Debug, Clone, Default)]
struct Symbol {
    texts: Vec<Text>,
}

/// Upstream `FileFormatMigrationV01::Gate`.
#[derive(Debug, Clone)]
struct Gate {
    uuid: Uuid,
    symbol_uuid: Uuid,
}

/// Upstream `FileFormatMigrationV01::ComponentSymbolVariant`.
#[derive(Debug, Clone)]
struct ComponentSymbolVariant {
    uuid: Uuid,
    gates: Vec<Gate>,
}

/// Upstream `FileFormatMigrationV01::Component`.
#[derive(Debug, Clone, Default)]
struct Component {
    schematic_only: bool,
    symbol_variants: Vec<ComponentSymbolVariant>,
}

/// Upstream `FileFormatMigrationV01::ComponentInstance`.
#[derive(Debug, Clone)]
struct ComponentInstance {
    lib_cmp_uuid: Uuid,
    lib_symb_var_uuid: Uuid,
}

/// Upstream `FileFormatMigrationV01::ProjectContext`.
#[derive(Debug, Default)]
struct ProjectContext {
    // Project library.
    symbols: HashMap<Uuid, Symbol>,
    components: HashMap<Uuid, Component>,
    component_instances: BTreeMap<Uuid, ComponentInstance>,
    devices_used_in_boards: BTreeMap<Uuid, BTreeSet<Uuid>>,

    // Project.
    project_uuid: String,

    // Counters for emitting messages.
    removed_erc_approvals: usize,
    holes_count: usize,
    non_round_via_count: usize,
    plane_count: usize,
    plane_connect_none_count: usize,
    footprint_board_outlines_object_count: usize,
    top_level_board_outlines_object_count: usize,
    components_with_assembly_options: usize,
}

/// Migration from file format 0.1 to 1 (upstream `FileFormatMigrationV01`).
#[derive(Debug, Clone)]
pub struct V01Migration {
    from_version: Version,
    to_version: Version,
}

impl V01Migration {
    /// Creates the migration.
    pub fn new() -> Self {
        Self {
            from_version: version("0.1"),
            to_version: version("1"),
        }
    }
}

impl Default for V01Migration {
    fn default() -> Self {
        Self::new()
    }
}

impl FileFormatMigration for V01Migration {
    fn from_version(&self) -> &Version {
        &self.from_version
    }

    fn to_version(&self) -> &Version {
        &self.to_version
    }

    fn upgrade_component_category(&self, dir: &mut TransactionalDirectory) -> MigrationResult<()> {
        upgrade_version_file(self, dir, ".librepcb-cmpcat")
    }

    fn upgrade_package_category(&self, dir: &mut TransactionalDirectory) -> MigrationResult<()> {
        upgrade_version_file(self, dir, ".librepcb-pkgcat")
    }

    fn upgrade_symbol(&self, dir: &mut TransactionalDirectory) -> MigrationResult<()> {
        upgrade_version_file(self, dir, ".librepcb-sym")?;
        upgrade_file(dir, "symbol.lp", |root| {
            append_string(as_list(root)?, "generated_by", "");

            // Various strings.
            upgrade_strings(root);

            // Layers.
            upgrade_layers(root);

            // Pins.
            upgrade_inversion_characters(root, "pin", "name/@0")?;
            for pin in children_mut(root, "pin") {
                let length: UnsignedLength = pin.child_value("length/@0")?;
                let name_pos = Point::new(length + Length::new(1_270_000), Length::ZERO);
                let name_align = Alignment::new(HAlign::Left, VAlign::Center);
                let list = as_list(pin)?;
                name_pos.serialize(list.append_list("name_position"));
                list.append_child("name_rotation", &Angle::DEG0);
                list.append_child("name_height", &positive_length(2_500_000));
                name_align.serialize(list.append_list("name_align"));
            }
            Ok(())
        })
    }

    fn upgrade_package(&self, dir: &mut TransactionalDirectory) -> MigrationResult<()> {
        upgrade_version_file(self, dir, ".librepcb-pkg")?;
        upgrade_file(dir, "package.lp", |root| {
            append_string(as_list(root)?, "generated_by", "");

            // Various strings.
            upgrade_strings(root);

            // Layers.
            upgrade_layers(root);

            // Assembly type.
            append_token(as_list(root)?, "assembly_type", "auto");

            // Footprints.
            for footprint in children_mut(root, "footprint") {
                upgrade_footprint(footprint)?;
            }
            Ok(())
        })
    }

    fn upgrade_component(&self, dir: &mut TransactionalDirectory) -> MigrationResult<()> {
        upgrade_version_file(self, dir, ".librepcb-cmp")?;
        upgrade_file(dir, "component.lp", |root| {
            append_string(as_list(root)?, "generated_by", "");

            // Signals.
            upgrade_inversion_characters(root, "signal", "name/@0")?;

            // Various strings.
            upgrade_strings(root);
            Ok(())
        })
    }

    fn upgrade_device(&self, dir: &mut TransactionalDirectory) -> MigrationResult<()> {
        upgrade_version_file(self, dir, ".librepcb-dev")?;
        upgrade_file(dir, "device.lp", |root| {
            append_string(as_list(root)?, "generated_by", "");

            // Various strings.
            upgrade_strings(root);
            Ok(())
        })
    }

    fn upgrade_organization(&self, _dir: &mut TransactionalDirectory) -> MigrationResult<()> {
        // Didn't exist yet.
        Ok(())
    }

    fn upgrade_library(&self, dir: &mut TransactionalDirectory) -> MigrationResult<()> {
        upgrade_version_file(self, dir, ".librepcb-lib")?;
        upgrade_file(dir, "library.lp", |root| {
            append_string(as_list(root)?, "manufacturer", "");
            Ok(())
        })
    }

    fn upgrade_project(
        &self,
        dir: &mut TransactionalDirectory,
        messages: &mut Vec<MigrationMessage>,
    ) -> MigrationResult<()> {
        let mut context = ProjectContext::default();

        // Version File.
        upgrade_version_file(self, dir, ".librepcb-project")?;

        // Symbols.
        for mut sub in element_dirs(dir, "library/sym", ".librepcb-sym") {
            let root = read_file(&sub, "symbol.lp")?;
            let uuid: Uuid = root.child_value("@0")?;
            let mut symbol = Symbol::default();

            // Texts.
            for text in root.children_named("text") {
                symbol.texts.push(Text {
                    uuid: text.child_value("@0")?,
                    layer_name: child_str(text, "layer/@0")?.to_owned(),
                    text: child_str(text, "value/@0")?.to_owned(),
                    position: Point::deserialize(text.required_child("position")?)?,
                    rotation: text.child_value("rotation/@0")?,
                    height: text.child_value("height/@0")?,
                    align: Alignment::deserialize(text.required_child("align")?)?,
                });
            }

            context.symbols.insert(uuid, symbol);

            self.upgrade_symbol(&mut sub)?;
        }

        // Packages.
        for mut sub in element_dirs(dir, "library/pkg", ".librepcb-pkg") {
            let root = read_file(&sub, "package.lp")?;

            // Footprints.
            for footprint in root.children_named("footprint") {
                context.holes_count += count_children(footprint, "hole");
                for geometry in footprint
                    .children_named("polygon")
                    .chain(footprint.children_named("circle"))
                {
                    if child_str(geometry, "layer/@0")? == "brd_outlines" {
                        context.footprint_board_outlines_object_count += 1;
                    }
                }
            }

            self.upgrade_package(&mut sub)?;
        }

        // Components.
        for mut sub in element_dirs(dir, "library/cmp", ".librepcb-cmp") {
            let root = read_file(&sub, "component.lp")?;
            let uuid: Uuid = root.child_value("@0")?;
            let mut component = Component {
                schematic_only: root.child_value("schematic_only/@0")?,
                symbol_variants: Vec::new(),
            };

            // Symbol variants.
            for variant in root.children_named("variant") {
                let mut symb_var = ComponentSymbolVariant {
                    uuid: variant.child_value("@0")?,
                    gates: Vec::new(),
                };

                // Gates.
                for gate in variant.children_named("gate") {
                    symb_var.gates.push(Gate {
                        uuid: gate.child_value("@0")?,
                        symbol_uuid: gate.child_value("symbol/@0")?,
                    });
                }

                component.symbol_variants.push(symb_var);
            }

            context.components.insert(uuid, component);

            self.upgrade_component(&mut sub)?;
        }

        // Devices.
        for mut sub in element_dirs(dir, "library/dev", ".librepcb-dev") {
            self.upgrade_device(&mut sub)?;
        }

        // Get schematics list.
        // This is important to upgrade only the used schematics. If there are
        // unused schematic files left over in the project, they could cause
        // the upgrade to fail. It's better to just ignore the unused files (if
        // any).
        let schematic_files = index_file_entries(dir, "schematics/schematics.lp", "schematic")?;

        // Get boards list (same reasoning as for the schematics).
        let board_files = index_file_entries(dir, "boards/boards.lp", "board")?;

        // Scan boards.
        for fp in &board_files {
            let root = read_file(dir, fp)?;
            for device in root.children_named("device") {
                let cmp_uuid: Uuid = device.child_value("@0")?;
                let lib_dev_uuid: Uuid = device.child_value("lib_device/@0")?;
                context
                    .devices_used_in_boards
                    .entry(cmp_uuid)
                    .or_default()
                    .insert(lib_dev_uuid);
            }
        }

        // Output jobs.
        write_file(dir, "project/jobs.lp", &SExpression::list("librepcb_jobs"))?;

        // Metadata.
        upgrade_file(dir, "project/metadata.lp", |root| {
            context.project_uuid = child_str(root, "@0")?.to_owned();
            let version = child_mut(root, "version/@0")?;
            let new_version = to_file_proof_name(version.value()?, "latest");
            version.set_value(new_version)?;
            Ok(())
        })?;

        // Settings.
        upgrade_file(dir, "project/settings.lp", |root| {
            upgrade_strings(root);
            let list = as_list(root)?;
            list.append_list("custom_bom_attributes");
            append_string(list, "output_directory", "./output/{{VERSION}}/");
            append_token(list, "default_lock_component_assembly", "false");
            Ok(())
        })?;

        // Circuit.
        {
            let fp = "circuit/circuit.lp";
            let mut root = read_file(dir, fp)?;
            upgrade_circuit(&mut root, &mut context)?;
            write_file(dir, fp, &root)?;

            // Component instances.
            for component in root.children_named("component") {
                let uuid: Uuid = component.child_value("@0")?;
                context.component_instances.insert(
                    uuid,
                    ComponentInstance {
                        lib_cmp_uuid: component.child_value("lib_component/@0")?,
                        lib_symb_var_uuid: component.child_value("lib_variant/@0")?,
                    },
                );
            }
        }

        // ERC.
        upgrade_file(dir, "circuit/erc.lp", |root| {
            upgrade_erc(root, &mut context)
        })?;

        // Schematics.
        for fp in &schematic_files {
            upgrade_file(dir, fp, |root| upgrade_schematic(root, &context))?;
        }

        // Boards.
        for fp in &board_files {
            // Board content.
            upgrade_file(dir, fp, |root| upgrade_board(root, &mut context))?;

            // User settings.
            let fp = fp.replace("/board.lp", "/settings.user.lp");
            if dir.file_exists(&fp) {
                upgrade_file(dir, &fp, upgrade_board_user_settings)?;
            }
        }

        // Emit messages at the very end to avoid duplicate messages caused by
        // multiple schematics/boards.
        let mut emit = |severity, message: String, count| {
            messages.push(build_message(self, severity, message, Some(count)));
        };
        if context.components_with_assembly_options > 0 {
            emit(
                MigrationSeverity::Note,
                tr!(
                    "librepcb::FileFormatMigrationV01",
                    "Components were automatically populated with assembly information \
                     required for the new, built-in MPN management and assembly variant \
                     mechanism. If the BOM or PnP export is used, please review the output \
                     and correct MPNs and attributes manually in the component properties \
                     dialog where needed."
                ),
                context.components_with_assembly_options,
            );
        }
        if context.removed_erc_approvals > 0 {
            emit(
                MigrationSeverity::Note,
                tr!(
                    "librepcb::FileFormatMigrationV01",
                    "Some particular ERC message approvals cannot be migrated and therefore \
                     have been removed. Please check the remaining ERC messages and approve \
                     them if desired."
                ),
                context.removed_erc_approvals,
            );
        }
        if context.holes_count > 0 {
            emit(
                MigrationSeverity::Note,
                tr!(
                    "librepcb::FileFormatMigrationV01",
                    "All non-plated holes (NPTH) now have automatic solder resist openings \
                     added on both board sides. The expansion value is configured in the \
                     board design rules but can be overridden in the hole properties dialog."
                ),
                context.holes_count,
            );
        }
        if context.non_round_via_count > 0 {
            emit(
                MigrationSeverity::Warning,
                tr!(
                    "librepcb::FileFormatMigrationV01",
                    "Non-circular via shapes are no longer supported, all vias were changed \
                     to circular now."
                ),
                context.non_round_via_count,
            );
        }
        if context.plane_count > 0 {
            emit(
                MigrationSeverity::Note,
                tr!(
                    "librepcb::FileFormatMigrationV01",
                    "Plane area calculations have been adjusted, manual review and running \
                     the DRC is recommended."
                ),
                context.plane_count,
            );
        }
        if context.plane_connect_none_count > 0 {
            emit(
                MigrationSeverity::Warning,
                tr!(
                    "librepcb::FileFormatMigrationV01",
                    "Vias within planes with connect style 'None' are now fully connected to \
                     the planes since the connect style is no longer respected for vias. You \
                     might want to remove traces now which are no longer needed to connect \
                     these vias."
                ),
                context.plane_connect_none_count,
            );
        }
        if (context.footprint_board_outlines_object_count > 0)
            || (context.top_level_board_outlines_object_count > 1)
        {
            emit(
                MigrationSeverity::Warning,
                tr!(
                    "librepcb::FileFormatMigrationV01",
                    "Board cutouts now have a dedicated layer, thus nested board outline \
                     polygons and circles have automatically been moved to the cutouts \
                     layer. As the auto-detection is not perfect, please check if each \
                     cutout has been converted correctly. The easiest way is to review the \
                     PCB in the 3D viewer."
                ),
                context.footprint_board_outlines_object_count
                    + context.top_level_board_outlines_object_count,
            );
        }
        Ok(())
    }

    fn upgrade_workspace_data(&self, dir: &mut TransactionalDirectory) -> MigrationResult<()> {
        // Create version file.
        dir.write(
            ".librepcb-data",
            &VersionFile::new(self.to_version.clone()).to_bytes(),
        )?;

        // Remove legacy files.
        remove_legacy_workspace_files(dir, &["cache", "cache_v1", "cache_v2", "library_cache"])?;

        // Upgrade settings.
        let settings_fp = "settings.lp";
        if dir.file_exists(settings_fp) {
            upgrade_file(dir, settings_fp, |root| {
                if let Some(node) = root.child_mut("repositories") {
                    for child in children_mut(node, "repository") {
                        as_list(child)?.set_name("url");
                    }
                    as_list(node)?.set_name("api_endpoints");
                }
                replace_token(root, "board_placement_top", "board_legend_top");
                replace_token(root, "board_placement_bottom", "board_legend_bottom");
                Ok(())
            })?;
        }
        Ok(())
    }
}

/// Creates a positive length from a (positive) literal.
fn positive_length(nm: i64) -> PositiveLength {
    // Invariant: only called with positive literals.
    PositiveLength::new(Length::new(nm)).expect("positive literal")
}

/// Returns `-angle` as node (upstream `serialize(-deserialize<Angle>(node))`).
fn negated_angle(node: &SExpression) -> MigrationResult<SExpression> {
    Ok((-Angle::from_sexpression(node)?).to_sexpression())
}

/// Negates the value `rotation/@0` if `mirror/@0` is `true` (upstream
/// swapping the transformation order of mirror/rotate).
fn negate_rotation_if_mirrored(node: &mut SExpression) -> MigrationResult<()> {
    if node.child_value::<bool>("mirror/@0")? {
        let rotation = negated_angle(node.required_child("rotation/@0")?)?;
        replace_child(node, "rotation/@0", rotation)?;
    }
    Ok(())
}

/// Upgrades a footprint node of a package (part of upstream
/// `upgradePackage()`).
fn upgrade_footprint(footprint: &mut SExpression) -> MigrationResult<()> {
    let list = as_list(footprint)?;

    // Add 3D model position.
    let position = list.append_list("3d_position");
    for _ in 0..3 {
        position.push(SExpression::token("0.0"));
    }

    // Add 3D model rotation.
    let rotation = list.append_list("3d_rotation");
    for _ in 0..3 {
        rotation.push(SExpression::token("0.0"));
    }

    // Pads.
    for pad in children_mut(footprint, "pad") {
        // In the file format 0.1, footprint pads did not have their own UUID,
        // but only the UUID of the package pad they were connected to. To get
        // a deterministic UUID when upgrading a v0.1 footprint pad to v1, we
        // simply use the package pad UUID as the footprint pad UUID too.
        // See https://github.com/LibrePCB/LibrePCB/issues/445
        let uuid: Uuid = pad.child_value("@0")?;
        as_list(pad)?.append_child("package_pad", &uuid);

        // Convert shape & corner radius.
        let shape = child_str(pad, "shape/@0")?;
        let is_round_shape = shape == "round";
        let is_rect_shape = shape == "rect";
        append_token(
            as_list(pad)?,
            "radius",
            if is_round_shape { "1.0" } else { "0.0" },
        );
        if is_round_shape || is_rect_shape {
            replace_child(pad, "shape/@0", SExpression::token("roundrect"))?;
        }

        // Convert holes.
        // Note: In the Gerber export, drills on SMT pads were ignored thus we
        // delete such drills now to keep the same behavior.
        // To get a deterministic UUID, the pad's UUID is reused for the hole.
        let is_tht = child_str(pad, "side/@0")? == "tht";
        let drill: UnsignedLength = pad.child_value("drill/@0")?;
        let list = as_list(pad)?;
        if is_tht && (drill > Length::ZERO) {
            let hole = list.append_list("hole");
            hole.append_value(&uuid);
            hole.append_child("diameter", &drill);
            let vertex = hole.append_list("vertex");
            let position = vertex.append_list("position");
            position.append_value(&Length::ZERO); // X
            position.append_value(&Length::ZERO); // Y
            vertex.append_child("angle", &Angle::DEG0);
        }
        if is_tht {
            // THT is no longer a valid value. Since footprints are always
            // drawn from the top view, it should be safe to set it to "top"
            // now.
            replace_child(pad, "side/@0", SExpression::token("top"))?;
        }

        // Add mask configs.
        let list = as_list(pad)?;
        append_token(list, "stop_mask", "auto");
        append_token(
            list,
            "solder_paste",
            if drill > Length::ZERO { "off" } else { "auto" },
        );

        // Add function.
        append_token(list, "function", "unspecified");

        // Add copper clearance.
        append_token(list, "clearance", "0.0");
    }

    // Polygons and circles.
    for name in ["polygon", "circle"] {
        for node in children_mut(footprint, name) {
            if child_str(node, "layer/@0")?.ends_with("_courtyard") {
                set_child_value(node, "width/@0", "0.0")?;
            }
        }
    }

    // Stroke texts.
    for text in children_mut(footprint, "stroke_text") {
        negate_rotation_if_mirrored(text)?;
    }

    // Holes.
    upgrade_holes(footprint, false)?;

    // Move cutouts from the board outlines layer to the new cutouts layer.
    upgrade_cutouts(footprint, None)
}

/// Upstream `upgradeCircuit()`.
fn upgrade_circuit(root: &mut SExpression, context: &mut ProjectContext) -> MigrationResult<()> {
    upgrade_strings(root);

    // Add default assembly variant. Use the project's UUID as assembly
    // variant UUID to make the migration deterministic.
    {
        let node = as_list(root)?.append_list("variant");
        node.push(SExpression::token(context.project_uuid.as_str()));
        node.append_child("name", &SExpression::string("Std"));
        node.append_child("description", &SExpression::string("Standard assembly"));
    }

    // Add assembly options & parts to components.
    for component in children_mut(root, "component") {
        let cmp_uuid: Uuid = component.child_value("@0")?;
        let lib_cmp_uuid: Uuid = component.child_value("lib_component/@0")?;
        let is_logo = lib_cmp_uuid.to_string() == "b91cf23a-4f07-4b99-8f52-0b42304aef20";
        let add_to_assembly_variant = !context
            .components
            .get(&lib_cmp_uuid)
            .is_some_and(|c| c.schematic_only)
            && !is_logo;
        let mut lib_device_uuids = context
            .devices_used_in_boards
            .get(&cmp_uuid)
            .cloned()
            .unwrap_or_default();
        if let Some(uuid) = component.child_value::<Option<Uuid>>("lib_device/@0")? {
            lib_device_uuids.insert(uuid);
        }

        if !lib_device_uuids.is_empty() {
            let mut mpn = String::new();
            let mut manufacturer = String::new();
            let list = as_list(component)?;
            let mut consumed = Vec::new(); // Indices, descending.
            for (index, attribute) in list.children().iter().enumerate().rev() {
                if attribute.as_list().is_none_or(|l| l.name() != "attribute") {
                    continue;
                }
                let key = child_str(attribute, "@0")?;
                if key == "MPN" {
                    mpn = SimpleString::clean(child_str(attribute, "value/@0")?).into_string();
                    consumed.push(index);
                } else if key == "MANUFACTURER" {
                    manufacturer =
                        SimpleString::clean(child_str(attribute, "value/@0")?).into_string();
                    consumed.push(index);
                }
            }
            for index in consumed {
                list.children_mut().remove(index);
            }

            for dev_uuid in &lib_device_uuids {
                let device = list.append_list("device");
                device.push(SExpression::token(dev_uuid.to_string()));
                if !mpn.is_empty() || !manufacturer.is_empty() {
                    let part = device.append_list("part");
                    part.append_value(&mpn);
                    part.append_child("manufacturer", &manufacturer);
                }
                if add_to_assembly_variant {
                    append_token(device, "variant", &context.project_uuid);
                }
            }
            context.components_with_assembly_options += 1;
        }

        remove_child(component, "lib_device")?;
        append_token(as_list(component)?, "lock_assembly", "false");
    }
    Ok(())
}

/// Upstream `upgradeErc()`.
fn upgrade_erc(root: &mut SExpression, context: &mut ProjectContext) -> MigrationResult<()> {
    let mut new_root = List::new(root.name()?);
    for node in root.children_named("approved") {
        let msg_class = child_str(node, "class/@0")?;
        let instance = child_str(node, "instance/@0")?;
        let message = child_str(node, "message/@0")?;
        let first = instance.split('/').next().unwrap_or_default();
        let last = instance.split('/').next_back().unwrap_or_default();
        if (msg_class == "NetClass") && (message == "Unused") {
            let child = new_root.append_list("approved");
            child.push(SExpression::token("unused_netclass"));
            append_token(child, "netclass", instance);
        } else if (msg_class == "NetSignal")
            && ((message == "Unused") || (message == "ConnectedToLessThanTwoPins"))
        {
            let child = new_root.append_list("approved");
            child.push(SExpression::token("open_net"));
            append_token(child, "net", instance);
        } else if (message == "UnconnectedRequiredSignal")
            || (message == "ForcedNetSignalNameConflict")
        {
            let child = new_root.append_list("approved");
            child.push(SExpression::token("unconnected_required_signal"));
            child.ensure_line_break();
            append_token(child, "component", first);
            child.ensure_line_break();
            append_token(child, "signal", last);
            child.ensure_line_break();
        } else {
            context.removed_erc_approvals += 1;
        }
    }
    *root = SExpression::List(new_root);
    Ok(())
}

/// Upstream `upgradeSchematic()`.
fn upgrade_schematic(root: &mut SExpression, context: &ProjectContext) -> MigrationResult<()> {
    upgrade_strings(root);
    upgrade_grid(root)?;
    upgrade_layers(root);

    // Symbols.
    for symbol_node in children_mut(root, "symbol") {
        let cmp_uuid: Uuid = symbol_node.child_value("component/@0")?;
        let gate_uuid: Uuid = symbol_node.child_value("lib_gate/@0")?;
        let cmp_inst = context
            .component_instances
            .get(&cmp_uuid)
            .ok_or(MigrationError::ComponentInstanceNotFound(cmp_uuid))?;
        let lib_cmp = context
            .components
            .get(&cmp_inst.lib_cmp_uuid)
            .ok_or(MigrationError::ComponentNotFound(cmp_inst.lib_cmp_uuid))?;
        let cmp_symb_var = lib_cmp
            .symbol_variants
            .iter()
            .find(|v| v.uuid == cmp_inst.lib_symb_var_uuid)
            .ok_or(MigrationError::SymbolVariantNotFound(
                cmp_inst.lib_symb_var_uuid,
            ))?;
        let gate = cmp_symb_var
            .gates
            .iter()
            .find(|g| g.uuid == gate_uuid)
            .ok_or(MigrationError::GateNotFound(gate_uuid))?;
        let symbol = context
            .symbols
            .get(&gate.symbol_uuid)
            .ok_or(MigrationError::SymbolNotFound(gate.symbol_uuid))?;
        let sym_pos = Point::deserialize(symbol_node.required_child("position")?)?;
        let sym_rot: Angle = symbol_node.child_value("rotation/@0")?;
        let sym_mirror: bool = symbol_node.child_value("mirror/@0")?;
        let list = as_list(symbol_node)?;
        for text in &symbol.texts {
            let mut position = text.position.rotated(sym_rot, Point::ORIGIN);
            if sym_mirror {
                position = position.mirrored(Orientation::Horizontal, Point::ORIGIN);
            }
            position += sym_pos;
            let rotation = if sym_mirror {
                Angle::DEG180 - sym_rot - text.rotation
            } else {
                sym_rot + text.rotation
            };
            let align = if sym_mirror {
                text.align.mirrored_v()
            } else {
                text.align
            };

            let node = list.append_list("text");
            node.append_value(&text.uuid);
            node.append_child("layer", &text.layer_name);
            node.append_child("value", &text.text);
            align.serialize(node.append_list("align"));
            node.append_child("height", &text.height);
            position.serialize(node.append_list("position"));
            node.append_child("rotation", &rotation);
        }

        // Swap transformation order of mirror/rotate.
        negate_rotation_if_mirrored(symbol_node)?;
    }

    // Net segments.
    for segment in children_mut(root, "netsegment") {
        // Net labels.
        for label in children_mut(segment, "label") {
            as_list(label)?.append_child("mirror", &false);
        }
    }
    Ok(())
}

/// Upstream `upgradeBoard()`.
fn upgrade_board(root: &mut SExpression, context: &mut ProjectContext) -> MigrationResult<()> {
    upgrade_strings(root);
    upgrade_grid(root)?;
    upgrade_board_design_rules(root)?;
    upgrade_board_drc_settings(root)?;
    upgrade_layers(root);
    upgrade_cutouts(root, Some(context))?;

    // Board setup.
    let list = as_list(root)?;
    append_token(list, "thickness", "1.6");
    append_token(list, "solder_resist", "green");
    append_token(list, "silkscreen", "white");

    // Fabrication output settings.
    {
        let node = child_mut(root, "fabrication_output_settings")?;
        let drills = child_mut(node, "drills")?;
        let is_default_suffix = child_str(drills, "suffix_merged/@0")?.contains("DRILLS");
        let drills = as_list(drills)?;
        drills.append_child("g85_slots", &false);
        if is_default_suffix {
            // Default suffixes.
            append_string(
                drills,
                "suffix_buried",
                "_DRILLS-PLATED-{{START_LAYER}}-{{END_LAYER}}.drl",
            );
        } else {
            // Protel suffixes.
            append_string(
                drills,
                "suffix_buried",
                "_L{{START_NUMBER}}-L{{END_NUMBER}}.drl",
            );
        }

        let mut silk_layers_top = remove_child(child_mut(node, "silkscreen_top")?, "layers")?;
        let mut silk_layers_bot = remove_child(child_mut(node, "silkscreen_bot")?, "layers")?;
        as_list(&mut silk_layers_top)?.set_name("silkscreen_layers_top");
        as_list(&mut silk_layers_bot)?.set_name("silkscreen_layers_bot");
        let list = as_list(root)?;
        list.push(silk_layers_top);
        list.push(silk_layers_bot);
    }

    // Devices.
    for device in children_mut(root, "device") {
        negate_rotation_if_mirrored(device)?;
        append_token(as_list(device)?, "lock", "false");
        for text in children_mut(device, "stroke_text") {
            negate_rotation_if_mirrored(text)?;
            append_token(as_list(text)?, "lock", "false");
        }
        rename_child(device, "mirror", "flip")?;
        append_token(as_list(device)?, "lib_3d_model", "none");
    }

    // Net segments.
    let stop_mask_max_via_diameter: UnsignedLength =
        root.child_value("design_rules/stopmask_max_via_drill_diameter/@0")?;
    for segment in children_mut(root, "netsegment") {
        // Vias.
        for via in children_mut(segment, "via") {
            if child_str(via, "shape/@0")? != "round" {
                context.non_round_via_count += 1;
            }
            remove_child(via, "shape")?;
            let drill: PositiveLength = via.child_value("drill/@0")?;
            let list = as_list(via)?;
            append_token(list, "from", "top_cu");
            append_token(list, "to", "bot_cu");
            if drill > stop_mask_max_via_diameter {
                append_token(list, "exposure", "auto");
            } else {
                append_token(list, "exposure", "off");
            }
        }
    }

    // Polygons.
    for polygon in children_mut(root, "polygon") {
        append_token(as_list(polygon)?, "lock", "false");
    }

    // Stroke texts.
    for text in children_mut(root, "stroke_text") {
        negate_rotation_if_mirrored(text)?;
        append_token(as_list(text)?, "lock", "false");
    }

    // Holes.
    context.holes_count += count_children(root, "hole");
    upgrade_holes(root, true)?;

    // Planes.
    for plane in children_mut(root, "plane") {
        context.plane_count += 1;
        if child_str(plane, "connect_style/@0")? == "none" {
            context.plane_connect_none_count += 1;
        }
        let thermal_gap = plane.required_child("min_clearance/@0")?.clone();
        let thermal_spoke = plane.required_child("min_width/@0")?.clone();
        let list = as_list(plane)?;
        list.append_child("thermal_gap", &thermal_gap);
        list.append_child("thermal_spoke", &thermal_spoke);
        append_token(list, "lock", "false");
        rename_child(plane, "keep_orphans", "keep_islands")?;
    }
    Ok(())
}

/// Upstream `upgradeBoardUserSettings()`.
fn upgrade_board_user_settings(root: &mut SExpression) -> MigrationResult<()> {
    upgrade_layers(root);

    // Layer colors.
    for layer in children_mut(root, "layer") {
        for tag_name in ["color", "color_hl"] {
            if layer.child(tag_name).is_some() {
                remove_child(layer, tag_name)?;
            }
        }
    }
    Ok(())
}

/// Upstream `upgradeBoardDesignRules()`.
fn upgrade_board_design_rules(root: &mut SExpression) -> MigrationResult<()> {
    let node = child_mut(root, "design_rules")?;
    remove_child(node, "name")?;
    remove_child(node, "description")?;
    for child in as_list(node)?.children_mut() {
        if let Some(list) = child.as_list_mut() {
            let name = list
                .name()
                .replace("restring_pad_", "pad_annular_ring_")
                .replace("restring_via_", "via_annular_ring_")
                .replace("creammask_", "solderpaste_");
            list.set_name(name);
        }
    }
    for param in [
        "stopmask_clearance",
        "solderpaste_clearance",
        "pad_annular_ring",
        "via_annular_ring",
    ] {
        let mut new_child = List::new(param);
        for property in ["ratio", "min", "max"] {
            let old_child = remove_child(node, &format!("{param}_{property}"))?;
            new_child.append_child(property, old_child.required_child("@0")?);
        }
        as_list(node)?.push(SExpression::List(new_child));
    }
    let child = as_list(child_mut(node, "pad_annular_ring")?)?;
    append_token(child, "outer", "full");
    append_token(child, "inner", "full");
    Ok(())
}

/// Upstream `upgradeBoardDrcSettings()`.
fn upgrade_board_drc_settings(root: &mut SExpression) -> MigrationResult<()> {
    let node = as_list(root)?.append_list("design_rule_check");
    for (name, value) in [
        ("min_copper_copper_clearance", "0.2"),
        ("min_copper_board_clearance", "0.3"),
        ("min_copper_npth_clearance", "0.25"),
        ("min_drill_drill_clearance", "0.35"),
        ("min_drill_board_clearance", "0.5"),
        ("min_silkscreen_stopmask_clearance", "0.127"),
        ("min_copper_width", "0.2"),
        ("min_annular_ring", "0.2"),
        ("min_npth_drill_diameter", "0.3"),
        ("min_pth_drill_diameter", "0.3"),
        ("min_npth_slot_width", "1.0"),
        ("min_pth_slot_width", "0.7"),
        ("min_silkscreen_width", "0.15"),
        ("min_silkscreen_text_height", "0.8"),
        ("min_outline_tool_diameter", "2.0"),
        ("blind_vias_allowed", "false"),
        ("buried_vias_allowed", "false"),
        ("allowed_npth_slots", "single_segment_straight"),
        ("allowed_pth_slots", "single_segment_straight"),
        ("approvals_version", "0.2"),
    ] {
        append_token(node, name, value);
    }
    Ok(())
}

/// Upstream `upgradeGrid()`.
fn upgrade_grid(node: &mut SExpression) -> MigrationResult<()> {
    remove_child(child_mut(node, "grid")?, "type")?;
    Ok(())
}

/// Moves nested board outlines to the new cutouts layer (upstream
/// `upgradeCutouts()`). `context` is `Some` for boards, `None` for
/// footprints.
fn upgrade_cutouts(
    node: &mut SExpression,
    context: Option<&mut ProjectContext>,
) -> MigrationResult<()> {
    // Collect all outline objects.
    struct OutlineObject {
        index: usize,
        outline: Path,
        length_mm: f64,
    }
    let mut outline_objects = Vec::new();
    let children = node.children();
    let on_outlines_layer = |child: &SExpression, name: &str| -> MigrationResult<bool> {
        Ok(child.as_list().is_some_and(|l| l.name() == name)
            && child_str(child, "layer/@0")? == "brd_outlines")
    };
    for (index, child) in children.iter().enumerate() {
        if on_outlines_layer(child, "polygon")? {
            let outline = Path::deserialize(child)?;
            let length_mm = outline.total_straight_length().to_mm();
            outline_objects.push(OutlineObject {
                index,
                outline,
                length_mm,
            });
        }
    }
    for (index, child) in children.iter().enumerate() {
        if on_outlines_layer(child, "circle")? {
            let position = Point::deserialize(child.required_child("position")?)?;
            let diameter: PositiveLength = child.child_value("diameter/@0")?;
            outline_objects.push(OutlineObject {
                index,
                outline: Path::circle(diameter).translated(position),
                length_mm: diameter.to_mm() * std::f64::consts::PI,
            });
        }
    }

    // Sort by outline length ascending.
    outline_objects.sort_by(|a, b| a.length_mm.total_cmp(&b.length_mm));

    // Discard the outline which is considered as the outer most board
    // outline.
    if let Some(last) = outline_objects.last() {
        if let Some(context) = context {
            // In boards, the longest outline is considered as the board
            // outlines.
            outline_objects.pop();
            context.top_level_board_outlines_object_count += outline_objects.len();
        } else {
            // In footprints, the longest outline is only considered as the
            // board outlines if there is any pad located *within* the
            // outline.
            let path = PainterPathPx::from_paths(std::iter::once(&last.outline));
            for pad in node.children_named("pad") {
                let pad_position = Point::deserialize(pad.required_child("position")?)?;
                if path.contains_point(pad_position.to_px()) {
                    outline_objects.pop();
                    break;
                }
            }
        }
    }

    // Move all remaining outlines to the new cutouts layer.
    let list = as_list(node)?;
    for obj in outline_objects {
        set_child_value(
            &mut list.children_mut()[obj.index],
            "layer/@0",
            "brd_cutouts",
        )?;
    }
    Ok(())
}

/// Upstream `upgradeHoles()`.
fn upgrade_holes(node: &mut SExpression, is_board_hole: bool) -> MigrationResult<()> {
    for hole in children_mut(node, "hole") {
        let pos = Point::deserialize(hole.required_child("position")?)?;
        let list = as_list(hole)?;
        append_token(list, "stop_mask", "auto");
        let vertex = list.append_list("vertex");
        pos.serialize(vertex.append_list("position"));
        vertex.append_child("angle", &Angle::DEG0);
        if is_board_hole {
            append_token(list, "lock", "false");
        }
    }
    Ok(())
}

/// Upstream `upgradeLayers()`.
fn upgrade_layers(node: &mut SExpression) {
    replace_token(node, "sch_scheet_frames", "sch_frames");
    replace_token(node, "brd_sheet_frames", "brd_frames");
    replace_token(node, "brd_milling_pth", "brd_plated_cutouts");

    // Remove nodes on never officially existing layer "brd_keepout".
    let mut search = List::new("layer");
    search.push(SExpression::token("brd_keepout"));
    node.remove_children_with_node_recursive(&SExpression::List(search));

    replace_token(node, "top_placement", "top_legend");
    replace_token(node, "bot_placement", "bot_legend");
}

/// Replaces the leading `/` (the old inversion character) of the values at
/// `value_path` of all children named `child_name` by `!`, unless the new
/// name is already used (upstream `upgradeInversionCharacters()`).
fn upgrade_inversion_characters(
    root: &mut SExpression,
    child_name: &str,
    value_path: &str,
) -> MigrationResult<()> {
    let reserved_values = root
        .children_named(child_name)
        .map(|child| Ok(child_str(child, value_path)?.to_owned()))
        .collect::<MigrationResult<BTreeSet<_>>>()?;
    for child in children_mut(root, child_name) {
        let node = child_mut(child, value_path)?;
        if let Some(rest) = node.value()?.strip_prefix('/') {
            let new_value = format!("!{rest}");
            if !reserved_values.contains(&new_value) {
                node.set_value(new_value)?;
            }
        }
    }
    Ok(())
}

/// Upstream `upgradeStrings()`.
fn upgrade_strings(root: &mut SExpression) {
    // Applied in the order of upstream's `QMap` (sorted by key).
    const REPLACEMENTS: [(&str, &str); 3] = [
        ("MODIFIED_DATE", "DATE"),
        ("MODIFIED_TIME", "TIME"),
        ("PARTNUMBER", "MPN"),
    ];
    replace_strings(root, &REPLACEMENTS);
}

/// Replaces substrings in all string nodes, recursively (upstream
/// `replaceStrings()`).
fn replace_strings(root: &mut SExpression, replacements: &[(&str, &str)]) {
    let Some(list) = root.as_list_mut() else {
        return;
    };
    for child in list.children_mut() {
        match child {
            SExpression::List(_) => replace_strings(child, replacements),
            SExpression::String(s) => {
                for (from, to) in replacements {
                    *s = s.replace(from, to);
                }
            }
            SExpression::Token(_) | SExpression::LineBreak => {}
        }
    }
}

/// Converts a string to a file-proof name the way LibrePCB 0.1 did it
/// (upstream `toFileProofName()`).
fn to_file_proof_name(name: &str, fallback: &str) -> String {
    // Perform compatibility decomposition (NFKD), remove leading and
    // trailing spaces and replace remaining spaces with "-".
    let ret = name.nfkd().collect::<String>().trim().replace(' ', "-");
    // Remove all invalid characters and truncate to the maximum allowed
    // length (only ASCII characters are left).
    let ret: String = ret
        .chars()
        .filter(|&c| c.is_ascii_alphanumeric() || "-_+().".contains(c))
        .take(20)
        .collect();
    // If there are leading or trailing spaces, remove them again ;)
    let ret = ret.trim();
    // If the result is not valid, return the fallback.
    if ret.is_empty() {
        fallback.to_owned()
    } else {
        ret.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn to_file_proof_name_cleans_like_v01() {
        assert_eq!(to_file_proof_name("v1", "latest"), "v1");
        assert_eq!(
            to_file_proof_name("  My Version 1.0 ", "x"),
            "My-Version-1.0"
        );
        assert_eq!(to_file_proof_name("Ärger", "x"), "Arger");
        assert_eq!(to_file_proof_name("€", "latest"), "latest");
        assert_eq!(to_file_proof_name("", "latest"), "latest");
        assert_eq!(
            to_file_proof_name("0123456789012345678901234", "x"),
            "01234567890123456789"
        );
    }

    #[test]
    fn inversion_characters() {
        let mut root = SExpression::parse(
            b"(sym (pin a (name \"/A\")) (pin b (name \"!A\")) (pin c (name \"/B\")))",
            None,
            crate::serialization::Mode::LibrePcb,
        )
        .unwrap();
        upgrade_inversion_characters(&mut root, "pin", "name/@0").unwrap();
        let names: Vec<_> = root
            .children_named("pin")
            .map(|p| child_str(p, "name/@0").unwrap().to_owned())
            .collect();
        assert_eq!(names, ["/A", "!A", "!B"]);
    }

    #[test]
    fn strings_are_replaced_recursively() {
        let mut root = SExpression::parse(
            b"(a \"{{PARTNUMBER}}\" PARTNUMBER (b \"MODIFIED_DATE MODIFIED_TIME\"))",
            None,
            crate::serialization::Mode::LibrePcb,
        )
        .unwrap();
        upgrade_strings(&mut root);
        assert_eq!(
            root.to_string_with_mode(crate::serialization::Mode::LibrePcb)
                .unwrap(),
            "(a \"{{MPN}}\" PARTNUMBER (b \"DATE TIME\"))\n"
        );
    }
}
