//! Port of libs/librepcb/core/serialization/fileformatmigrationv1.{h,cpp}.
//!
//! The project upgrade steps which upstream implements as protected virtual
//! methods (so that `FileFormatMigrationUnstable` can override them) are
//! the provided methods of the trait [`V1ProjectSteps`];
//! [`upgrade_project()`] walks the project like upstream
//! `FileFormatMigrationV1::upgradeProject()` and calls them.

use std::collections::BTreeSet;

use foilfern_i18n::tr;

use super::{
    FileFormatMigration, MigrationMessage, MigrationResult, MigrationSeverity, append_string,
    append_token, as_list, build_message, child_mut, child_str, children_mut, element_dirs,
    index_file_entries, remove_children, remove_legacy_workspace_files, rename_child,
    set_child_value, upgrade_file, upgrade_version_file, version,
};
use crate::fileio::{FileSystem, TransactionalDirectory, VersionFile};
use crate::serialization::{List, SExpression};
use crate::types::{PositiveLength, Version};

/// Upstream `FileFormatMigrationV1::ProjectContext`.
#[derive(Debug, Default)]
pub(super) struct ProjectContext {
    // Counters for emitting messages.
    board_count: usize,
    has_gerber_output_job: bool,
}

/// The project upgrade steps of [`V1Migration`] (upstream's protected
/// virtual methods of `FileFormatMigrationV1`), overridden by
/// [`UnstableMigration`](super::UnstableMigration).
pub(super) trait V1ProjectSteps: FileFormatMigration + Sized {
    /// Upstream `upgradeMetadata()`.
    fn upgrade_metadata(
        &self,
        root: &mut SExpression,
        messages: &mut Vec<MigrationMessage>,
    ) -> MigrationResult<()> {
        // FileProofName does no longer allow string consisting of only dots
        // (e.g. "..") so we rename them.
        let version_node = child_mut(root, "version/@0")?;
        if let Some(new_version) = upgrade_file_proof_name(version_node.value()?) {
            version_node.set_value(new_version)?;
            // Not translated because it's unlikely someone will ever see this
            // message.
            messages.push(build_message(
                self,
                MigrationSeverity::Note,
                "Project version has been adjusted due to more restrictive naming \
                 requirements. Please review the new version number."
                    .to_owned(),
                Some(1),
            ));
        }
        Ok(())
    }

    /// Upstream `upgradeSettings()`.
    fn upgrade_settings(
        &self,
        root: &mut SExpression,
        messages: &mut Vec<MigrationMessage>,
    ) -> MigrationResult<()> {
        // The manual BOM export has been removed. If the user has configured
        // custom BOM attributes, just remind him to use output jobs now.
        let custom_bom_attributes = root
            .required_child("custom_bom_attributes")?
            .children_named("attribute")
            .map(|node| Ok(child_str(node, "@0")?.to_owned()))
            .collect::<MigrationResult<Vec<_>>>()?;
        if !custom_bom_attributes.is_empty() {
            messages.push(build_message(
                self,
                MigrationSeverity::Note,
                tr!(
                    "librepcb::FileFormatMigrationV1",
                    "The project has set custom attributes for the BOM export ({0}). But in \
                     LibrePCB 2.0, the manual BOM export has been removed in favor of the more \
                     powerful output jobs feature. Please use output jobs now to generate the \
                     BOM. When you add a new BOM output job, those custom attributes will \
                     automatically be imported.",
                    custom_bom_attributes.join(", ")
                ),
                Some(1),
            ));
        }
        Ok(())
    }

    /// Upstream `upgradeOutputJobs()`.
    fn upgrade_output_jobs(
        &self,
        root: &mut SExpression,
        context: &mut ProjectContext,
    ) -> MigrationResult<()> {
        let job_indices: Vec<usize> = root
            .children()
            .iter()
            .enumerate()
            .filter(|(_, c)| c.as_list().is_some_and(|l| l.name() == "job"))
            .map(|(i, _)| i)
            .collect();
        for index in job_indices {
            let job = &mut as_list(root)?.children_mut()[index];
            match child_str(job, "type/@0")? {
                "graphics" => {
                    for content in children_mut(job, "content") {
                        upgrade_graphics_job_content(content)?;
                    }
                }
                "gerber_excellon" => context.has_gerber_output_job = true,
                "gerber_x3" => {
                    rename_child(job, "top", "components_top")?;
                    rename_child(job, "bottom", "components_bot")?;
                    let list = as_list(job)?;
                    let glue_top = list.append_list("glue_top");
                    append_token(glue_top, "create", "false");
                    append_string(
                        glue_top,
                        "output",
                        "assembly/{{PROJECT}}_{{VERSION}}_GLUE_{{VARIANT}}_TOP.gbr",
                    );
                    // Note: Upstream adds the line breaks to the root node,
                    // not to the job node.
                    as_list(root)?.ensure_line_break();
                    let job = &mut as_list(root)?.children_mut()[index];
                    let glue_bot = as_list(job)?.append_list("glue_bot");
                    append_token(glue_bot, "create", "false");
                    append_string(
                        glue_bot,
                        "output",
                        "assembly/{{PROJECT}}_{{VERSION}}_GLUE_{{VARIANT}}_BOT.gbr",
                    );
                    as_list(root)?.ensure_line_break();
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// Upstream `upgradeCircuit()`.
    fn upgrade_circuit(
        &self,
        root: &mut SExpression,
        messages: &mut Vec<MigrationMessage>,
    ) -> MigrationResult<()> {
        // Assembly variants.
        let mut renamed_assembly_variants = 0;
        for variant in children_mut(root, "variant") {
            // FileProofName does no longer allow string consisting of only
            // dots (e.g. "..") so we rename them. We don't do conflict
            // resolution here as it is very unlikely to ever happen.
            let name = child_mut(variant, "name/@0")?;
            if let Some(new_name) = upgrade_file_proof_name(name.value()?) {
                name.set_value(new_name)?;
                renamed_assembly_variants += 1;
            }
        }
        if renamed_assembly_variants > 0 {
            // Not translated because it's unlikely someone will ever see this
            // message.
            messages.push(build_message(
                self,
                MigrationSeverity::Note,
                "Assembly variants have been renamed due to more restrictive naming \
                 requirements. Please review the new names."
                    .to_owned(),
                Some(renamed_assembly_variants),
            ));
        }

        // Net classes.
        for net_class in children_mut(root, "netclass") {
            let list = as_list(net_class)?;
            append_token(list, "default_trace_width", "inherit");
            append_token(list, "default_via_drill_diameter", "inherit");
            append_token(list, "min_copper_copper_clearance", "0");
            append_token(list, "min_copper_width", "0");
            append_token(list, "min_via_drill_diameter", "0");
        }
        Ok(())
    }

    /// Upstream `upgradeSchematic()`.
    fn upgrade_schematic(&self, root: &mut SExpression) -> MigrationResult<()> {
        for symbol in children_mut(root, "symbol") {
            upgrade_texts(symbol, true)?; // Lock texts depending on layer.
        }
        upgrade_texts(root, false) // Do not lock any schematic text.
    }

    /// Upstream `upgradeBoard()`.
    fn upgrade_board(&self, root: &mut SExpression) -> MigrationResult<()> {
        upgrade_board(root)
    }
}

/// Upgrades a project (upstream `FileFormatMigrationV1::upgradeProject()`,
/// also used by the unstable migration).
pub(super) fn upgrade_project<M: V1ProjectSteps>(
    m: &M,
    dir: &mut TransactionalDirectory,
    messages: &mut Vec<MigrationMessage>,
) -> MigrationResult<()> {
    // ATTENTION: Do not actually perform any upgrade in this function!
    // Instead, just call the methods of `V1ProjectSteps` and
    // `FileFormatMigration` which do the upgrade. This allows the unstable
    // migration to override them with partial upgrades.

    let mut context = ProjectContext::default();

    // Version File.
    upgrade_version_file(m, dir, ".librepcb-project")?;

    // Library elements.
    for mut sub in element_dirs(dir, "library/sym", ".librepcb-sym") {
        m.upgrade_symbol(&mut sub)?;
    }
    for mut sub in element_dirs(dir, "library/pkg", ".librepcb-pkg") {
        m.upgrade_package(&mut sub)?;
    }
    for mut sub in element_dirs(dir, "library/cmp", ".librepcb-cmp") {
        m.upgrade_component(&mut sub)?;
    }
    for mut sub in element_dirs(dir, "library/dev", ".librepcb-dev") {
        m.upgrade_device(&mut sub)?;
    }

    // Get schematics and boards list.
    // This is important to upgrade only the used schematics/boards. If there
    // are unused files left over in the project, they could cause the upgrade
    // to fail. It's better to just ignore the unused files (if any).
    let schematic_files = index_file_entries(dir, "schematics/schematics.lp", "schematic")?;
    let board_files = index_file_entries(dir, "boards/boards.lp", "board")?;

    upgrade_file(dir, "project/metadata.lp", |root| {
        m.upgrade_metadata(root, messages)
    })?;
    upgrade_file(dir, "project/settings.lp", |root| {
        m.upgrade_settings(root, messages)
    })?;
    upgrade_file(dir, "project/jobs.lp", |root| {
        m.upgrade_output_jobs(root, &mut context)
    })?;
    upgrade_file(dir, "circuit/circuit.lp", |root| {
        m.upgrade_circuit(root, messages)
    })?;
    for fp in &schematic_files {
        upgrade_file(dir, fp, |root| m.upgrade_schematic(root))?;
    }
    for fp in &board_files {
        context.board_count += 1;
        upgrade_file(dir, fp, |root| m.upgrade_board(root))?;
    }

    // Emit messages at the very end to avoid duplicate messages caused by
    // multiple schematics/boards.
    if (context.board_count > 0) && !context.has_gerber_output_job {
        messages.push(build_message(
            m,
            MigrationSeverity::Warning,
            tr!(
                "librepcb::FileFormatMigrationV1",
                "The dedicated Gerber/Excellon generator dialog has been removed in favor of \
                 the more powerful output jobs, and the corresponding output settings will be \
                 removed from boards in an upcoming release. It is recommended to add a \
                 Gerber/Excellon output job now, as this allows to migrate the old export \
                 settings (choose \"Import Old Settings\")."
            ),
            Some(1),
        ));
    }
    Ok(())
}

/// Upgrades the pads of a device (part of upstream `upgradeDevice()`, also
/// used by the unstable migration).
pub(super) fn upgrade_device_pads(dir: &mut TransactionalDirectory) -> MigrationResult<()> {
    upgrade_file(dir, "device.lp", |root| {
        // Pinout.
        for pad in children_mut(root, "pad") {
            append_token(as_list(pad)?, "optional", "false");
        }
        Ok(())
    })
}

/// Migration from file format 1 to 2 (upstream `FileFormatMigrationV1`).
#[derive(Debug, Clone)]
pub struct V1Migration {
    from_version: Version,
    to_version: Version,
}

impl V1Migration {
    /// Creates the migration.
    pub fn new() -> Self {
        Self {
            from_version: version("1"),
            to_version: version("2"),
        }
    }
}

impl Default for V1Migration {
    fn default() -> Self {
        Self::new()
    }
}

impl V1ProjectSteps for V1Migration {}

impl FileFormatMigration for V1Migration {
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
            append_token(as_list(root)?, "grid_interval", "2.54");
            upgrade_texts(root, true)
        })
    }

    fn upgrade_package(&self, dir: &mut TransactionalDirectory) -> MigrationResult<()> {
        upgrade_version_file(self, dir, ".librepcb-pkg")?;
        upgrade_file(dir, "package.lp", |root| {
            let list = as_list(root)?;
            append_token(list, "grid_interval", "2.54");
            append_token(list, "min_copper_clearance", "0.2");

            // Footprints.
            for footprint in children_mut(root, "footprint") {
                upgrade_footprint(footprint)?;
            }
            Ok(())
        })
    }

    fn upgrade_component(&self, dir: &mut TransactionalDirectory) -> MigrationResult<()> {
        upgrade_version_file(self, dir, ".librepcb-cmp")
    }

    fn upgrade_device(&self, dir: &mut TransactionalDirectory) -> MigrationResult<()> {
        upgrade_version_file(self, dir, ".librepcb-dev")?;
        upgrade_device_pads(dir)
    }

    fn upgrade_organization(&self, _dir: &mut TransactionalDirectory) -> MigrationResult<()> {
        // Didn't exist yet.
        Ok(())
    }

    fn upgrade_library(&self, dir: &mut TransactionalDirectory) -> MigrationResult<()> {
        upgrade_version_file(self, dir, ".librepcb-lib")
    }

    fn upgrade_project(
        &self,
        dir: &mut TransactionalDirectory,
        messages: &mut Vec<MigrationMessage>,
    ) -> MigrationResult<()> {
        upgrade_project(self, dir, messages)
    }

    fn upgrade_workspace_data(&self, dir: &mut TransactionalDirectory) -> MigrationResult<()> {
        // Create version file.
        dir.write(
            ".librepcb-data",
            &VersionFile::new(self.to_version.clone()).to_bytes(),
        )?;

        // Remove legacy files.
        remove_legacy_workspace_files(
            dir,
            &["cache_v3", "cache_v4", "cache_v5", "cache_v6", "cache_v7"],
        )?;

        // Upgrade settings.
        let settings_fp = "settings.lp";
        if dir.file_exists(settings_fp) {
            upgrade_file(dir, settings_fp, |root| {
                if let Some(node) = root.child_mut("api_endpoints") {
                    for (index, child) in children_mut(node, "url").enumerate() {
                        let list = as_list(child)?;
                        list.set_name("endpoint");
                        let first = if index == 0 { "true" } else { "false" };
                        append_token(list, "libraries", "true");
                        append_token(list, "parts", first);
                        append_token(list, "order", first);
                    }
                }
                Ok(())
            })?;
        }
        Ok(())
    }
}

/// Upgrades a footprint node of a package (part of upstream
/// `upgradePackage()`).
fn upgrade_footprint(footprint: &mut SExpression) -> MigrationResult<()> {
    // Add tags depending on name.
    let name = child_str(footprint, "name/@0")?.to_lowercase();
    let list = as_list(footprint)?;
    for (pattern, tag) in [
        ("density level a", "ipc-density-level-a"),
        ("density level b", "ipc-density-level-b"),
        ("density level c", "ipc-density-level-c"),
        ("hand", "hand-soldering"),
        ("reflow", "reflow-soldering"),
        ("wave", "wave-soldering"),
        ("large", "extra-large-pads"),
    ] {
        if name.contains(pattern) {
            append_string(list, "tag", tag);
        }
    }

    // Pads.
    for pad in children_mut(footprint, "pad") {
        // Revert possibly made manual change as a workaround for bug, see
        // https://librepcb.discourse.group/t/migrating-libraries-from-old-version-1-0-pressfit-problem/810
        if child_str(pad, "function/@0")? == "press_fit" {
            set_child_value(pad, "function/@0", "pressfit")?;
        }
    }

    // Stroke texts.
    let unlocked_layers: BTreeSet<&str> =
        ["top_names", "top_values", "bot_names", "bot_values"].into();
    for text in children_mut(footprint, "stroke_text") {
        let lock = !unlocked_layers.contains(child_str(text, "layer/@0")?);
        append_token(as_list(text)?, "lock", if lock { "true" } else { "false" });
    }
    Ok(())
}

/// Upgrades the content of a graphics output job (part of upstream
/// `upgradeOutputJobs()`).
fn upgrade_graphics_job_content(content: &mut SExpression) -> MigrationResult<()> {
    let add_layer = |list: &mut List, layer: &str, color: &str| {
        let node = list.append_list("layer");
        node.push(SExpression::token(layer));
        append_string(node, "color", color);
    };
    let content_type = child_str(content, "type/@0")?.to_owned();
    if content_type == "schematic" {
        let list = as_list(content)?;
        // Add the new layer for image borders.
        add_layer(list, "schematic_image_borders", "#ff808080");
        // Add the new layer for buses.
        add_layer(list, "schematic_buses", "#ff008eff");
        // Add the new layer for bus labels.
        add_layer(list, "schematic_bus_labels", "#ff008eff");
    } else if content_type == "board" {
        // We don't need to check the option value since "realistic" was the
        // only supported option in v1.
        let removed_options = remove_children(content, "option");
        if removed_options > 0 {
            set_child_value(content, "type/@0", "board_rendering")?;
            remove_children(content, "layer");
            let mirror = child_str(content, "mirror/@0")? == "true";
            let list = as_list(content)?;
            if mirror {
                add_layer(list, "board_copper_bottom", "#ffbc9c69");
                add_layer(list, "board_legend_bottom", "#00000000");
                add_layer(list, "board_outlines", "#ff465046");
                add_layer(list, "board_stop_mask_bottom", "#00000000");
            } else {
                add_layer(list, "board_copper_top", "#ffbc9c69");
                add_layer(list, "board_legend_top", "#00000000");
                add_layer(list, "board_outlines", "#ff465046");
                add_layer(list, "board_stop_mask_top", "#00000000");
            }
        }
    }
    Ok(())
}

/// Upstream `FileFormatMigrationV1::upgradeBoard()`.
fn upgrade_board(root: &mut SExpression) -> MigrationResult<()> {
    // Design rules.
    {
        let rules = as_list(child_mut(root, "design_rules")?)?;
        append_token(rules, "default_trace_width", "0.5");
        append_token(rules, "default_via_drill_diameter", "0.3");
    }

    // DRC settings.
    {
        let drc = as_list(child_mut(root, "design_rule_check")?)?;
        drc.ensure_line_break();
        {
            let child = drc.append_list("min_pcb_size");
            child.push(SExpression::token("0.0"));
            child.push(SExpression::token("0.0"));
        }
        drc.ensure_line_break();
        {
            let child = drc.append_list("max_pcb_size");
            for name in ["double_sided", "multilayer"] {
                let size = child.append_list(name);
                size.push(SExpression::token("0.0"));
                size.push(SExpression::token("0.0"));
            }
        }
        drc.ensure_line_break();
        drc.append_list("pcb_thickness");
        drc.ensure_line_break();
        append_token(drc, "max_layers", "0");
        drc.ensure_line_break();
        drc.append_list("solder_resist");
        drc.ensure_line_break();
        drc.append_list("silkscreen");
        drc.ensure_line_break();
        append_token(drc, "max_tented_via_drill_diameter", "0.5");
        drc.ensure_line_break();
    }

    // DRC approvals.
    {
        let drc = child_mut(root, "design_rule_check")?;
        let approvals_version = child_str(drc, "approvals_version/@0")?.to_owned();
        for approval in children_mut(drc, "approved") {
            let approval_type = child_mut(approval, "@0")?;
            let value = approval_type.value()?;
            if (value == "useless_via") && (approvals_version != "2") {
                approval_type.set_value("invalid_via")?;
            } else if value == "antennae_via" {
                approval_type.set_value("useless_via")?;
            }
        }
    }

    // Preferred footprint tags.
    {
        let child = as_list(root)?.append_list("preferred_footprint_tags");
        child.ensure_line_break();
        for name in ["tht_top", "tht_bot", "smt_top", "smt_bot", "common"] {
            child.append_list(name);
            child.ensure_line_break();
        }
    }

    // Devices.
    for device in children_mut(root, "device") {
        append_token(as_list(device)?, "glue", "true");
    }

    // Net segments.
    for segment in children_mut(root, "netsegment") {
        // Vias.
        for via in children_mut(segment, "via") {
            let drill: PositiveLength = via.child_value("drill/@0")?;
            let size: PositiveLength = via.child_value("size/@0")?;
            if size < drill {
                // No longer valid in LibrePCB 2.0!
                let drill_value = child_str(via, "drill/@0")?.to_owned();
                set_child_value(via, "size/@0", &drill_value)?;
            }
        }
    }

    // Planes.
    for plane in children_mut(root, "plane") {
        rename_child(plane, "min_clearance", "min_copper_clearance")?;
        let clearance = plane.required_child("min_copper_clearance/@0")?.clone();
        let list = as_list(plane)?;
        list.append_child("min_board_clearance", &clearance);
        list.append_child("min_npth_clearance", &clearance);
    }
    Ok(())
}

/// Appends `lock` to all texts of `node`: `true` if `allow_lock` and the
/// text is not a name or value (upstream `upgradeTexts()`).
fn upgrade_texts(node: &mut SExpression, allow_lock: bool) -> MigrationResult<()> {
    for text in children_mut(node, "text") {
        let layer = child_str(text, "layer/@0")?;
        let lock = allow_lock && (layer != "sym_names") && (layer != "sym_values");
        append_token(as_list(text)?, "lock", if lock { "true" } else { "false" });
    }
    Ok(())
}

/// Returns the replacement for names consisting only of dots, which are no
/// longer valid file-proof names (upstream `upgradeFileProofName()`).
fn upgrade_file_proof_name(name: &str) -> Option<String> {
    (!name.is_empty() && name.chars().all(|c| c == '.')).then(|| name.replace('.', "_"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_proof_name() {
        assert_eq!(upgrade_file_proof_name("."), Some("_".to_owned()));
        assert_eq!(upgrade_file_proof_name("..."), Some("___".to_owned()));
        assert_eq!(upgrade_file_proof_name(""), None);
        assert_eq!(upgrade_file_proof_name("v1.0"), None);
        assert_eq!(upgrade_file_proof_name(".a."), None);
    }
}
