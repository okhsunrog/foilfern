//! Review a converted source bundle before its native filesystem transaction.

use foilfern_core::fileio::FileSystem;
use foilfern_core::library::LibraryBaseElement;
use foilfern_import::easyeda::NativePart;
use foilfern_network::part_source::PartSource;
use foilfern_scene::{
    ColorScheme, FootprintScene, RenderOptions, RenderSize, SymbolScene, default_stroke_font,
    render_scene, visible_bounds,
};
use serde_json::json;

use crate::error::{ToolError, ToolResult};
use crate::outcome::ToolOutput;

pub(super) fn render(source: &PartSource, native: &NativePart) -> ToolResult<ToolOutput> {
    let font = default_stroke_font();
    let symbol_scheme = ColorScheme::SCHEMATIC_LIGHT;
    let package_scheme = ColorScheme::BOARD_DARK;
    let gates = native
        .component
        .symbol_variants()
        .iter()
        .flat_map(|variant| variant.symbol_items().iter())
        .filter(|gate| gate.symbol_uuid() == native.symbol.metadata().uuid())
        .collect::<Vec<_>>();
    let [gate] = gates.as_slice() else {
        return Err(ToolError::invalid(
            "Acquired symbol has no unique component gate for pin-number preview.",
        ));
    };
    let numbers = gate
        .pin_signal_map()
        .iter()
        .map(|pin| {
            let mut names = native
                .device
                .pad_signal_map()
                .iter()
                .filter(|pad| pin.signal_uuid().is_some() && pad.signal_uuid() == pin.signal_uuid())
                .filter_map(|pad| {
                    native
                        .package
                        .pads()
                        .by_uuid(&pad.pad_uuid())
                        .map(|pad| pad.name().as_str())
                })
                .collect::<Vec<_>>();
            alphanumeric_sort::sort_str_slice(&mut names);
            names.dedup();
            (pin.pin_uuid(), names.join(", "))
        })
        .collect::<std::collections::BTreeMap<_, _>>();
    let complete_numbers = numbers.len() == native.symbol.pins().len()
        && numbers.values().all(|number| !number.is_empty());
    let symbol = SymbolScene::build_with_pin_numbers(
        &native.symbol,
        font.as_ref(),
        &symbol_scheme,
        &numbers,
    );
    let footprint = native
        .package
        .footprints()
        .get(0)
        .ok_or_else(|| ToolError::invalid("Converted package has no footprint."))?;
    let package = FootprintScene::build_with_pad_names(
        footprint,
        &native.package,
        font.as_ref(),
        &package_scheme,
    );
    let options = RenderOptions {
        size: RenderSize::Fit {
            width: 1000,
            height: 800,
        },
        ..Default::default()
    };
    let symbol_image = render_scene(
        symbol.scene(),
        visible_bounds(symbol.scene()),
        false,
        symbol_scheme.color_or_transparent("schematic_background"),
        &options,
    )?
    .to_png()?;
    let package_image = render_scene(
        package.scene(),
        visible_bounds(package.scene()),
        false,
        package_scheme.color_or_transparent("board_background"),
        &options,
    )?
    .to_png()?;
    let models = native
        .package
        .models()
        .iter()
        .map(|model| {
            let provenance = format!("easyeda-model-{}.json", model.uuid());
            let bytes = native.package.directory().read(&provenance)?;
            serde_json::from_slice::<serde_json::Value>(&bytes)
                .map_err(|e| ToolError::internal(e.to_string()))
        })
        .collect::<ToolResult<Vec<_>>>()?;
    let mapping = native.device.pad_signal_map().iter().map(|entry| {
        let pad = native.package.pads().by_uuid(&entry.pad_uuid());
        let signal = entry.signal_uuid().and_then(|uuid|native.component.signals().by_uuid(&uuid));
        json!({"pad":entry.pad_uuid(),"pad_name":pad.map(|p|p.name().as_str()),"signal":entry.signal_uuid(),"signal_name":signal.map(|s|s.name().as_str())})
    }).collect::<Vec<_>>();
    let available = !models.is_empty();
    let output = ToolOutput::new(format!("Preview {}: {} mapped pads. Native library unchanged.",source.mpn,mapping.len()),json!({
        "dry_run":true,"lcsc":source.lcsc,"mpn":source.mpn,"manufacturer":source.manufacturer,
        "source":{"url":source.source_url,"sha256":source.sha256,"snapshot":source.snapshot,"attribution":source.attribution},
        "device":native.device.metadata().uuid(),"component":native.component.metadata().uuid(),
        "electrical_metadata":foilfern_core::library::electrical_metadata::review(&native.component).summary,
        "mapping":mapping,"model":{"available":available,"alignment_verified":false,"assets":models},
        "images":[{"index":0,"kind":"symbol","pin_numbers":"device_package_mapping","rotation":0,"mirrored":false},
            {"index":1,"kind":"footprint","pad_names":true,"view_from":"top","rotation":0,"mirrored":false}],
        "symbol_pin_numbers":numbers,"pin_number_mapping_complete":complete_numbers,
        "install":{"lcsc":source.lcsc,"cache":"offline","expected_source_sha256":source.sha256,"expected_model_sha256":models.first().and_then(|m|m["sha256"].as_str())}
    }))?.warnings(native.warnings.clone()).image(symbol_image).image(package_image);
    let output = if complete_numbers {
        output
    } else {
        output.warn(
            "Some symbol pins have no mapped package pad numbers; their number labels are blank.",
        )
    };
    Ok(if available && complete_numbers {
        output
    } else {
        output.partial()
    })
}
