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
    let symbol = SymbolScene::build(&native.symbol, font.as_ref(), &symbol_scheme);
    let footprint = native
        .package
        .footprints()
        .get(0)
        .ok_or_else(|| ToolError::invalid("Converted package has no footprint."))?;
    let package = FootprintScene::build(footprint, font.as_ref(), &package_scheme);
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
        "mapping":mapping,"model":{"available":available,"alignment_verified":false,"assets":models},
        "images":[{"index":0,"kind":"symbol"},{"index":1,"kind":"footprint"}],
        "install":{"lcsc":source.lcsc,"cache":"offline","expected_source_sha256":source.sha256,"expected_model_sha256":models.first().and_then(|m|m["sha256"].as_str())}
    }))?.warnings(native.warnings.clone()).image(symbol_image).image(package_image);
    Ok(if available { output } else { output.partial() })
}
