//! Explicit no-connect markers stored with existing schematic appearance data.

use std::collections::{BTreeMap, BTreeSet};

use librepcb_core::project::schematic::import_appearance::ImportAppearance;
use librepcb_core::project::{ComponentInstanceId, SchematicId};
use librepcb_editor::Transaction;
use serde_json::{Value, json};

use crate::error::{ErrorKind, ToolError, ToolResult};
use crate::resolve;

pub(super) fn update(
    tx: &mut Transaction<'_>,
    page: SchematicId,
    bindings: &BTreeMap<&str, ComponentInstanceId>,
    markers: &BTreeMap<String, bool>,
    diff: &mut Vec<Value>,
) -> ToolResult<()> {
    if markers.is_empty() {
        return Ok(());
    }
    let project = tx.project();
    let schematic = project
        .schematic(page)
        .ok_or_else(|| ToolError::not_found("Schematic not found."))?;
    let mut appearance = ImportAppearance::read(project, schematic)?;
    let mut seen = BTreeSet::new();
    let mut changed = false;
    for (address, &marked) in markers {
        let (key, pin) = resolve::split_pin_ref(address)?;
        let component = bindings
            .get(key)
            .ok_or_else(|| ToolError::invalid(format!("Unknown component key {key:?}.")))?;
        let resolved = resolve::pin(project, &format!("{component}.{pin}"))?;
        let mut placed = Vec::new();
        if let Some(uses) = project.component_uses(*component) {
            for (sheet, id) in uses.symbols.values() {
                if *sheet == page
                    && let Some(symbol) = schematic.symbols().get(id)
                {
                    placed.extend(
                        symbol
                            .pins(project.view())?
                            .into_iter()
                            .filter(|pin| pin.signal() == resolved.signal)
                            .map(|pin| (symbol.uuid(), pin.uuid(), pin.net())),
                    );
                }
            }
        }
        let [(symbol, pin, net)] = placed.as_slice() else {
            return Err(ToolError::invalid(format!(
                "{address} must identify exactly one placed pin on this sheet."
            )));
        };
        if !seen.insert((*symbol, *pin)) {
            return Err(ToolError::invalid(format!(
                "More than one marker addresses {address}."
            )));
        }
        if marked && net.is_some() {
            return Err(ToolError::new(
                ErrorKind::Conflict,
                format!(
                    "{address} is connected. Disconnect it before marking it as intentionally open."
                ),
            ));
        }
        if appearance.set_no_connect(*symbol, *pin, marked) {
            diff.push(json!({"field":"no_connect","pin":address,"symbol":symbol,"pin_uuid":pin,"set":marked}));
            changed = true;
        }
    }
    if changed {
        let content = (appearance != ImportAppearance::default())
            .then(|| serde_json::to_vec(&appearance))
            .transpose()?;
        tx.write_file(ImportAppearance::asset_path(schematic), content)?;
    }
    Ok(())
}
