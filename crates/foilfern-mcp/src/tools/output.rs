//! Checks and outputs: `erc_run`, `drc_run`, `export_fabrication`,
//! `export_bom`, `export_pick_place`, `export_netlist`, `jobs_list`,
//! `jobs_run`, `render`.
//!
//! Output files default to `<project>/output/<version>/...` like the
//! default output jobs of upstream; tools return the written paths.
//! `drc_run`, `export_fabrication` and `jobs_run` rebuild planes and air
//! wires first (derived data outside the undo history, like upstream).

mod jobs;
pub use jobs::{
    JobsConfigureArgs, JobsListArgs, configure as jobs_configure, list as jobs_list_with,
};

use foilfern_core::export::{BomCsvWriter, PickPlaceSides, Timestamp};
use foilfern_core::fileio::FilePath;
use foilfern_core::job::OutputJobKind;
use foilfern_core::project::board::{
    BoardFabricationOutputSettings, ExportInfo, export_d356_netlist, export_fabrication_data,
    export_pick_place_csv,
};
use foilfern_core::project::erc::{ErcMessage, run_erc};
use foilfern_core::project::schematic::Schematic;
use foilfern_core::project::{
    AssemblyVariantId, BomGenerator, OutputJobEvent, OutputJobRunner, Project,
};
use foilfern_core::rule_check::Severity;
use foilfern_core::types::Point;
use foilfern_editor::routing_validation::drc_objects;
use foilfern_scene::{BoardSide, RenderOptions, RenderSize};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::error::{ToolError, ToolResult};
use crate::outcome::ToolOutput;
use crate::resolve;
use crate::session::{Session, absolute_path};
use crate::units::PointMm;
use crate::views;

/// Arguments of `erc_run`.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ErcArgs {
    /// Also list approved (ignored) messages (default false).
    #[serde(default)]
    pub include_approved: bool,
}

/// Arguments of `export_fabrication`.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExportFabricationArgs {
    /// Board name, index or UUID (default: first board).
    #[serde(default)]
    pub board: Option<String>,
    /// Output directory (default: the board's fabrication output settings,
    /// normally `<project>/output/<version>/gerber/`).
    #[serde(default)]
    pub output_dir: Option<String>,
}

/// Arguments of `export_bom`.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExportBomArgs {
    /// Board name, index or UUID: BOM of the devices on this board;
    /// default: the first board, or generic BOM if the project has no boards.
    #[serde(default)]
    pub board: Option<String>,
    /// Export all schematic components without board package information.
    /// Cannot be combined with board.
    #[serde(default)]
    pub schematic_only: bool,
    /// Additional CSV attribute columns, appended to project settings.
    /// A [] suffix selects per-part attributes, e.g. LCSC[].
    #[serde(default)]
    pub additional_attributes: Vec<String>,
    /// Assembly variant name or UUID (default: the first one).
    #[serde(default)]
    pub assembly_variant: Option<String>,
    /// Output CSV file (default `<project>/output/<version>/<project>_BOM-<variant>.csv`).
    #[serde(default)]
    pub output: Option<String>,
}

/// Board side argument.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SideArg {
    /// Top side.
    #[default]
    Top,
    /// Bottom side.
    Bottom,
    /// Both sides.
    Both,
}

/// Arguments of `export_pick_place`.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExportPickPlaceArgs {
    /// Board name, index or UUID (default: first board).
    #[serde(default)]
    pub board: Option<String>,
    /// Assembly variant name or UUID (default: the first one).
    #[serde(default)]
    pub assembly_variant: Option<String>,
    /// Board side(s) (default both).
    #[serde(default)]
    pub side: Option<SideArg>,
    /// Output CSV file (default `<project>/output/<version>/assembly/<project>_PnP-<side>.csv`).
    #[serde(default)]
    pub output: Option<String>,
}

/// Arguments of `export_netlist`.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExportNetlistArgs {
    /// Board name, index or UUID (default: first board).
    #[serde(default)]
    pub board: Option<String>,
    /// Output file (default `<project>/output/<version>/<project>_netlist.d356`).
    #[serde(default)]
    pub output: Option<String>,
}

/// What to render.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RenderTarget {
    /// A schematic page.
    #[default]
    Schematic,
    /// A board side.
    Board,
}

/// Arguments of `render`.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RenderArgs {
    /// "schematic" or "board" (default: "board" if `board` or `side` is
    /// given, else "schematic").
    #[serde(default)]
    pub target: Option<RenderTarget>,
    /// Schematic page name, index or UUID (default: first page).
    #[serde(default)]
    pub schematic: Option<String>,
    /// Board name, index or UUID (default: first board).
    #[serde(default)]
    pub board: Option<String>,
    /// Board side seen from ("top" default, or "bottom").
    #[serde(default)]
    pub side: Option<SideArg>,
    /// Image width in pixels (default 1600, max 4096).
    #[serde(default)]
    pub width: Option<u32>,
    /// Image height in pixels (default 1200, max 4096).
    #[serde(default)]
    pub height: Option<u32>,
    /// Crop to a rectangle in sheet millimeters, for schematic renders.
    pub image_region: Option<SchematicRegion>,
}

#[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SchematicRegion {
    /// Bottom-left corner in sheet millimeters.
    min: PointMm,
    /// Top-right corner in sheet millimeters.
    max: PointMm,
}

impl SchematicRegion {
    pub(super) fn points(self) -> ToolResult<(Point, Point)> {
        let min = self.min.to_point()?;
        let max = self.max.to_point()?;
        if min.x >= max.x || min.y >= max.y {
            return Err(ToolError::invalid(
                "The schematic region needs positive width and height.",
            ));
        }
        Ok((min, max))
    }
}

fn severity_str(s: Severity) -> &'static str {
    match s {
        Severity::Hint => "hint",
        Severity::Warning => "warning",
        Severity::Error => "error",
    }
}

pub(super) fn erc_message_json(p: &Project, message: &ErcMessage) -> Value {
    let msg = message.message();
    json!({
        "severity": severity_str(msg.severity()),
        "message": msg.message(),
        "description": msg.description(),
        "kind": format!("{:?}", message.kind()),
        "schematic": message.schematic().and_then(|id| p.schematic(id)).map(|s| s.name().as_str()),
        "approved": p.erc_approvals().contains(message.approval()),
        "locations": msg.locations().iter().map(views::path).collect::<Vec<_>>(),
    })
}

/// `erc_run`.
pub fn erc_run(session: &mut Session, args: ErcArgs) -> ToolResult<ToolOutput> {
    let open = session.project_mut()?;
    let messages = run_erc(open.project());
    // Remove approvals of messages which disappeared (upstream
    // `ProjectEditor::runErc()`, not undoable).
    open.editor.update_erc_approvals(&messages)?;
    let p = open.project();
    let mut list = Vec::new();
    let (mut errors, mut warnings, mut hints, mut approved) = (0, 0, 0, 0);
    for m in &messages {
        let is_approved = p.erc_approvals().contains(m.approval());
        let msg = m.message();
        if is_approved {
            approved += 1;
        } else {
            match msg.severity() {
                Severity::Error => errors += 1,
                Severity::Warning => warnings += 1,
                Severity::Hint => hints += 1,
            }
        }
        if is_approved && !args.include_approved {
            continue;
        }
        list.push(erc_message_json(p, m));
    }
    list.sort_by(|a, b| {
        let rank = |v: &Value| match v["severity"].as_str() {
            Some("error") => 0,
            Some("warning") => 1,
            _ => 2,
        };
        rank(a)
            .cmp(&rank(b))
            .then_with(|| a["message"].as_str().cmp(&b["message"].as_str()))
    });
    let lines: Vec<String> = list
        .iter()
        .filter(|m| m["approved"] == json!(false))
        .map(|m| {
            format!(
                "[{}] {}",
                m["severity"].as_str().unwrap_or_default().to_uppercase(),
                m["message"].as_str().unwrap_or_default()
            )
        })
        .collect();
    let summary = format!(
        "ERC ran: {errors} error(s), {warnings} warning(s), {hints} hint(s), {approved} \
         approved.{}{}",
        if lines.is_empty() { "" } else { "\n" },
        lines.join("\n")
    );
    ToolOutput::new(
        summary,
        json!({
            "ran": true,
            "errors": errors,
            "warnings": warnings,
            "hints": hints,
            "approved": approved,
            "messages": list,
        }),
    )
}

/// Default output directory `<project>/output/<version>`.
fn output_dir(p: &Project) -> ToolResult<FilePath> {
    let dir = p
        .path()
        .ok_or_else(|| ToolError::internal("the project has no directory on disk"))?;
    let version = FilePath::clean_file_name(
        p.metadata().version.as_str(),
        foilfern_core::fileio::CleanFileNameOptions::new(
            true,
            foilfern_core::fileio::FileNameCase::Keep,
        ),
        120,
    );
    Ok(dir.path_to(&format!("output/{version}")))
}

fn project_file_base(p: &Project) -> String {
    FilePath::clean_file_name(
        p.metadata().name.as_str(),
        foilfern_core::fileio::CleanFileNameOptions::new(
            true,
            foilfern_core::fileio::FileNameCase::Keep,
        ),
        120,
    )
}

fn assembly_variant(p: &Project, key: Option<&str>) -> ToolResult<(AssemblyVariantId, String)> {
    let variants = p.circuit().assembly_variants();
    let av = match key.map(str::trim).filter(|k| !k.is_empty()) {
        None => variants.iter().next(),
        Some(k) => variants.iter().find(|av| {
            av.name().as_str() == k || resolve::parse_uuid(k).is_some_and(|u| u == av.uuid())
        }),
    };
    av.map(|av| (AssemblyVariantId(av.uuid()), av.name().as_str().to_owned()))
        .ok_or_else(|| {
            ToolError::not_found(format!(
                "There is no assembly variant \"{}\".",
                key.unwrap_or_default()
            ))
        })
}

fn output_file(explicit: Option<&str>, default: FilePath) -> ToolResult<FilePath> {
    match explicit {
        Some(path) => absolute_path(path),
        None => Ok(default),
    }
}

/// `export_fabrication`.
pub fn export_fabrication(
    session: &mut Session,
    args: ExportFabricationArgs,
) -> ToolResult<ToolOutput> {
    let open = session.project_mut()?;
    let (_, board, b) = resolve::board(open.project(), args.board.as_deref())?;
    let settings: Option<BoardFabricationOutputSettings> = match &args.output_dir {
        Some(dir) => {
            let dir = absolute_path(dir)?;
            let mut s = b.settings().fabrication_output_settings.clone();
            s.output_base_path = format!("{}/{{{{PROJECT}}}}", dir.as_str());
            Some(s)
        }
        None => None,
    };
    let info = ExportInfo::now(env!("CARGO_PKG_VERSION"));
    // Exporting rebuilds the planes (derived data, not undoable).
    let files = open.editor.update_derived_data(|project| {
        export_fabrication_data(project, board, settings.as_ref(), &info)
    })??;
    let files: Vec<String> = files.iter().map(FilePath::to_native).collect();
    ToolOutput::new(
        format!(
            "Exported {} Gerber/Excellon file(s):\n{}",
            files.len(),
            files.join("\n")
        ),
        json!({ "files": files }),
    )
}

/// `export_bom`.
pub fn export_bom(session: &Session, args: ExportBomArgs) -> ToolResult<ToolOutput> {
    let p = session.project()?.project();
    if args.schematic_only && args.board.is_some() {
        return Err(ToolError::invalid(
            "board and schematic_only cannot be combined.",
        ));
    }
    let board = match &args.board {
        Some(b) => Some(resolve::board(p, Some(b))?.2),
        None if args.schematic_only => None,
        None => p.boards().first(),
    };
    let (variant, variant_name) = assembly_variant(p, args.assembly_variant.as_deref())?;
    let mut generator = BomGenerator::new(p);
    let mut attributes = p.settings().custom_bom_attributes.clone();
    for key in args.additional_attributes {
        let name = key.strip_suffix("[]").unwrap_or(&key);
        foilfern_core::attribute::AttributeKey::new(name)?;
        if !attributes.contains(&key) {
            attributes.push(key);
        }
    }
    generator.set_additional_attributes(attributes);
    let bom = generator.generate(board, variant);
    let csv = BomCsvWriter::new(&bom).generate_csv()?;
    let default = output_dir(p)?.path_to(&format!(
        "{}_BOM-{}.csv",
        project_file_base(p),
        variant_name
    ));
    let file = output_file(args.output.as_deref(), default)?;
    csv.save_to_file(&file)?;
    let content = csv.to_csv_string()?;
    let rows = content.lines().count().saturating_sub(1);
    ToolOutput::new(
        format!("Wrote BOM with {rows} line(s) to {}.", file.to_native()),
        json!({
            "file": file.to_native(),
            "assembly_variant": variant_name,
            "board": board.map(|b| b.uuid()),
            "csv": if content.len() <= 65536 { Some(content) } else { None },
        }),
    )
}

/// `export_pick_place`.
pub fn export_pick_place(session: &Session, args: ExportPickPlaceArgs) -> ToolResult<ToolOutput> {
    let p = session.project()?.project();
    let (_, board, _) = resolve::board(p, args.board.as_deref())?;
    let (variant, variant_name) = assembly_variant(p, args.assembly_variant.as_deref())?;
    let (sides, side_name) = match args.side.unwrap_or(SideArg::Both) {
        SideArg::Top => (PickPlaceSides::Top, "top"),
        SideArg::Bottom => (PickPlaceSides::Bottom, "bottom"),
        SideArg::Both => (PickPlaceSides::Both, "both"),
    };
    let default = output_dir(p)?.path_to(&format!(
        "assembly/{}_PnP-{}.csv",
        project_file_base(p),
        side_name
    ));
    let file = output_file(args.output.as_deref(), default)?;
    export_pick_place_csv(
        p,
        board,
        variant,
        sides,
        &file,
        env!("CARGO_PKG_VERSION"),
        Timestamp::now(),
    )?;
    let content = std::fs::read_to_string(file.as_path()).unwrap_or_default();
    ToolOutput::new(
        format!("Wrote pick&place data to {}.", file.to_native()),
        json!({
            "file": file.to_native(),
            "assembly_variant": variant_name,
            "side": side_name,
            "csv": if content.len() <= 65536 { Some(content) } else { None },
        }),
    )
}

/// `export_netlist`.
pub fn export_netlist(session: &Session, args: ExportNetlistArgs) -> ToolResult<ToolOutput> {
    let p = session.project()?.project();
    let (_, board, _) = resolve::board(p, args.board.as_deref())?;
    let default = output_dir(p)?.path_to(&format!("{}_netlist.d356", project_file_base(p)));
    let file = output_file(args.output.as_deref(), default)?;
    let info = ExportInfo::now(env!("CARGO_PKG_VERSION"));
    export_d356_netlist(p, board, &file, &info)?;
    ToolOutput::new(
        format!("Wrote IPC-D-356A netlist to {}.", file.to_native()),
        json!({ "file": file.to_native() }),
    )
}

/// `jobs_list`.
pub fn jobs_list(session: &Session) -> ToolResult<ToolOutput> {
    jobs_list_with(session, JobsListArgs::default())
}

fn job_type(kind: &OutputJobKind) -> String {
    let debug = format!("{kind:?}");
    debug
        .split(|c: char| !c.is_alphanumeric())
        .next()
        .unwrap_or_default()
        .to_owned()
}

/// `render`.
pub fn render(session: &Session, args: RenderArgs) -> ToolResult<ToolOutput> {
    let p = session.project()?.project();
    let width = args.width.unwrap_or(1600).clamp(64, 4096);
    let height = args.height.unwrap_or(1200).clamp(64, 4096);
    let options = RenderOptions {
        size: RenderSize::Fit { width, height },
        ..RenderOptions::default()
    };
    let target = args
        .target
        .unwrap_or(if args.board.is_some() || args.side.is_some() {
            RenderTarget::Board
        } else {
            RenderTarget::Schematic
        });
    let region = args.image_region.map(SchematicRegion::points).transpose()?;
    if region.is_some() && target == RenderTarget::Board {
        return Err(ToolError::invalid(
            "image_region requires a schematic target.",
        ));
    }
    let (png, what) = match target {
        RenderTarget::Schematic => {
            let (index, _, s) = resolve::schematic(p, args.schematic.as_deref())?;
            (
                render_schematic_preview(p, s, width, height, region)?,
                format!("schematic page {index} \"{}\"", s.name().as_str()),
            )
        }
        RenderTarget::Board => {
            let (index, id, b) = resolve::board(p, args.board.as_deref())?;
            let side = match args.side.unwrap_or(SideArg::Top) {
                SideArg::Bottom => BoardSide::Bottom,
                _ => BoardSide::Top,
            };
            (
                foilfern_scene::render_board_png(p, id, side, &options)?,
                format!(
                    "board {index} \"{}\" ({} side)",
                    b.name().as_str(),
                    if side == BoardSide::Top {
                        "top"
                    } else {
                        "bottom"
                    }
                ),
            )
        }
    };
    let mut result =
        json!({ "width": width, "height": height, "bytes": png.len(), "mime_type": "image/png" });
    if let Some((min, max)) = region {
        result["image_region"] = json!({"min":views::point(min),"max":views::point(max)});
    }
    Ok(ToolOutput::new(format!("Rendered {what} ({width}x{height} px)."), result)?.image(png))
}

pub(crate) fn render_schematic_preview(
    project: &Project,
    schematic: &Schematic,
    width: u32,
    height: u32,
    region: Option<(Point, Point)>,
) -> ToolResult<Vec<u8>> {
    if schematic.images().is_empty() {
        if let Some((min, max)) = region {
            let scene = foilfern_scene::SchematicScene::build(
                project,
                schematic.id(),
                &foilfern_scene::ColorScheme::SCHEMATIC_LIGHT,
            )?;
            return Ok(
                foilfern_scene::render_schematic_region(&scene, min, max, width, height)?
                    .to_png()?,
            );
        }
        let options = RenderOptions {
            size: RenderSize::Fit { width, height },
            ..RenderOptions::default()
        };
        return Ok(foilfern_scene::render_schematic_png(
            project,
            schematic.id(),
            &options,
        )?);
    }
    let settings = foilfern_graphics::GraphicsSettings {
        background: foilfern_core::types::Color::WHITE,
        ..Default::default()
    };
    let drawing = foilfern_graphics::schematic_drawing(project, schematic, &settings)
        .map_err(|error| ToolError::internal(error.to_string()))?;
    let exporter = foilfern_graphics::GraphicsExport::default();
    let result = if let Some((min, max)) = region {
        exporter.render_region_png(&drawing, &settings, min, max, width, height)
    } else {
        exporter.render_png(
            &foilfern_graphics::GraphicsPage { drawing, settings },
            width,
            height,
        )
    };
    result.map_err(|error| ToolError::internal(error.to_string()))
}

pub(super) fn render_schematic_comparison(
    before: &Project,
    after: &Project,
    schematic: &Schematic,
    size: (u32, u32),
    region: Option<(Point, Point)>,
    checkpoint: impl Fn() -> ToolResult<()>,
) -> ToolResult<Vec<Vec<u8>>> {
    let (width, height) = size;
    let old = before.schematic(schematic.id());
    if schematic.images().is_empty() && old.is_none_or(|sheet| sheet.images().is_empty()) {
        use foilfern_scene::{ColorScheme, SchematicScene, render_scene};
        let old_scene = old
            .map(|sheet| SchematicScene::build(before, sheet.id(), &ColorScheme::SCHEMATIC_LIGHT))
            .transpose()?;
        checkpoint()?;
        let new_scene =
            SchematicScene::build(after, schematic.id(), &ColorScheme::SCHEMATIC_LIGHT)?;
        checkpoint()?;
        let bounds = old_scene
            .as_ref()
            .and_then(SchematicScene::content_bounds)
            .into_iter()
            .chain(new_scene.content_bounds())
            .reduce(|a, b| a.union(b));
        let options = RenderOptions {
            size: RenderSize::Fit { width, height },
            ..Default::default()
        };
        let render = |scene: &SchematicScene| {
            if let Some((min, max)) = region {
                foilfern_scene::render_schematic_region(scene, min, max, width, height)
            } else {
                render_scene(
                    scene.scene(),
                    bounds,
                    false,
                    new_scene.background(),
                    &options,
                )
            }
        };
        let after_png = render(&new_scene)?.to_png()?;
        checkpoint()?;
        let before_png = if let Some(scene) = old_scene {
            render(&scene)?.to_png()?
        } else {
            foilfern_scene::RgbaImage {
                width,
                height,
                data: vec![255; width as usize * height as usize * 4],
            }
            .to_png()?
        };
        return Ok(vec![after_png, before_png]);
    }
    let settings = foilfern_graphics::GraphicsSettings {
        background: foilfern_core::types::Color::WHITE,
        ..Default::default()
    };
    let draw = |project, sheet| {
        foilfern_graphics::schematic_drawing(project, sheet, &settings)
            .map_err(|error| ToolError::internal(error.to_string()))
    };
    let original = old
        .map(|sheet| draw(before, sheet))
        .transpose()?
        .unwrap_or_default();
    checkpoint()?;
    let drawings = [draw(after, schematic)?, original];
    checkpoint()?;
    let exporter = foilfern_graphics::GraphicsExport::default();
    let map_error = |error: foilfern_graphics::Error| ToolError::internal(error.to_string());
    if let Some((min, max)) = region {
        drawings
            .iter()
            .map(|drawing| {
                checkpoint()?;
                exporter
                    .render_region_png(drawing, &settings, min, max, width, height)
                    .map_err(map_error)
            })
            .collect()
    } else {
        let renders = exporter
            .matched_pngs(&drawings, &settings, width, height)
            .map_err(map_error)?;
        checkpoint()?;
        renders
            .enumerate()
            .map(|(index, result)| {
                let png = result.map_err(map_error)?;
                if index + 1 < drawings.len() {
                    checkpoint()?;
                }
                Ok(png)
            })
            .collect()
    }
}

/// Arguments of `drc_run`.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DrcArgs {
    /// Board name, index or UUID (default: first board).
    #[serde(default)]
    pub board: Option<String>,
    /// Also list approved (ignored) messages (default false).
    #[serde(default)]
    pub include_approved: bool,
}

/// `drc_run`.
pub fn drc_run(session: &mut Session, args: DrcArgs) -> ToolResult<ToolOutput> {
    let open = session.project_mut()?;
    let (_, board, _) = resolve::board(open.project(), args.board.as_deref())?;
    // A full check rebuilds planes and air wires (derived data).
    let result = open
        .editor
        .update_derived_data(|p| p.run_drc(board, None, false, &|_| {}))?
        .map_err(|e| ToolError::internal(format!("The DRC could not run: {e}")))?;
    // Remove approvals of messages which disappeared (upstream
    // `Board::updateDrcMessageApprovals()`, not undoable).
    open.editor.update_drc_approvals(board, &result)?;
    let p = open.project();
    let approvals = p
        .board(board)
        .map(|b| b.drc_approvals().clone())
        .unwrap_or_default();
    let mut list = Vec::new();
    let (mut errors, mut warnings, mut hints, mut approved) = (0, 0, 0, 0);
    for m in &result.messages {
        let is_approved = approvals.contains(m.approval());
        let msg = m.message();
        if is_approved {
            approved += 1;
        } else {
            match msg.severity() {
                Severity::Error => errors += 1,
                Severity::Warning => warnings += 1,
                Severity::Hint => hints += 1,
            }
        }
        if is_approved && !args.include_approved {
            continue;
        }
        list.push(json!({
            "severity": severity_str(msg.severity()),
            "message": msg.message(),
            "description": msg.description(),
            "kind": format!("{:?}", m.kind()),
            "approved": is_approved,
            "objects": drc_objects(m),
            "locations": msg.locations().iter().map(views::path).collect::<Vec<_>>(),
        }));
    }
    list.sort_by(|a, b| {
        let rank = |v: &Value| match v["severity"].as_str() {
            Some("error") => 0,
            Some("warning") => 1,
            _ => 2,
        };
        rank(a)
            .cmp(&rank(b))
            .then_with(|| a["message"].as_str().cmp(&b["message"].as_str()))
    });
    let lines: Vec<String> = list
        .iter()
        .filter(|m| m["approved"] == json!(false))
        .take(50)
        .map(|m| {
            format!(
                "[{}] {}",
                m["severity"].as_str().unwrap_or_default().to_uppercase(),
                m["message"].as_str().unwrap_or_default()
            )
        })
        .collect();
    let summary = format!(
        "DRC ran: {errors} error(s), {warnings} warning(s), {hints} hint(s), {approved} \
         approved.{}{}",
        if lines.is_empty() { "" } else { "\n" },
        lines.join("\n")
    );
    let out = ToolOutput::new(
        summary,
        json!({
            "ran": true,
            "errors": errors,
            "warnings": warnings,
            "hints": hints,
            "approved": approved,
            "messages": list,
            "check_errors": result.errors,
        }),
    )?;
    Ok(if result.errors.is_empty() {
        out
    } else {
        let text = format!("Some checks failed: {}", result.errors.join("; "));
        out.partial().warn(text)
    })
}

/// Arguments of `jobs_run`.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct JobsRunArgs {
    /// Names or UUIDs of the output jobs to run (default: all).
    #[serde(default)]
    pub jobs: Vec<String>,
    /// Output directory (default: `<project>/output/<version>`).
    #[serde(default)]
    pub output_dir: Option<String>,
}

/// `jobs_run`.
pub fn jobs_run(session: &mut Session, args: JobsRunArgs) -> ToolResult<ToolOutput> {
    let open = session.project_mut()?;
    let all = open.project().output_jobs().clone();
    let skip_unsupported = args.jobs.is_empty();
    let jobs: Vec<_> = if args.jobs.is_empty() {
        all.iter().cloned().collect()
    } else {
        args.jobs
            .iter()
            .map(|key| jobs::resolve_job(open.project(), key).cloned())
            .collect::<ToolResult<_>>()?
    };
    let count = jobs.len();
    if count == 0 {
        return Ok(ToolOutput::new(
            "No output jobs to run (create one with jobs_configure).",
            json!({ "files": [] }),
        )?
        .warn("The project has no output jobs."));
    }
    let output_dir = args.output_dir.as_deref().map(absolute_path).transpose()?;
    let info = ExportInfo::now(env!("CARGO_PKG_VERSION"));
    let written = std::sync::Arc::new(parking_lot::Mutex::new(Vec::<String>::new()));
    let warnings = std::sync::Arc::new(parking_lot::Mutex::new(Vec::<String>::new()));
    let (w, warn, written_in_run) = (
        std::sync::Arc::clone(&written),
        std::sync::Arc::clone(&warnings),
        std::sync::Arc::clone(&written),
    );
    let result = open.editor.update_derived_data(move |project| {
        let mut runner = OutputJobRunner::new(project, info)?;
        runner.set_step_exporter(Some(foilfern_step::board_exporter()));
        runner.set_graphics_exporter(Some(std::sync::Arc::new(
            foilfern_graphics::ProjectGraphicsExporter::new(format!(
                "LibrePCB {}",
                env!("CARGO_PKG_VERSION")
            )),
        )));
        runner.set_observer(Some(Box::new(move |event| match event {
            OutputJobEvent::AboutToWriteFile(fp) => w.lock().push(fp.to_native()),
            OutputJobEvent::Warning(msg) => warn.lock().push(msg.to_string()),
            _ => {}
        })));
        if let Some(dir) = &output_dir {
            runner.set_output_directory(dir);
        }
        let dir = runner.output_directory().to_native();
        // Job by job, so job types which are not ported yet can be skipped
        // (when running all jobs).
        let mut skipped = Vec::new();
        for job in &jobs {
            let written_before = written_in_run.lock().len();
            match runner.run(std::slice::from_ref(job)) {
                Err(foilfern_core::project::OutputJobError::Unsupported(_)) if skip_unsupported => {
                    // The announced files were not written.
                    written_in_run.lock().truncate(written_before);
                    skipped.push(format!(
                        "Skipped the output job \"{}\" ({} jobs are not supported yet).",
                        job.name().as_str(),
                        job.type_name()
                    ));
                }
                Err(foilfern_core::project::OutputJobError::ArchiveDependencyNotRun)
                    if skip_unsupported && !skipped.is_empty() =>
                {
                    skipped.push(format!(
                        "Skipped the output job \"{}\" (it archives files of a skipped job).",
                        job.name().as_str()
                    ));
                }
                other => other?,
            }
        }
        Ok::<_, foilfern_core::project::OutputJobError>((dir, skipped))
    })?;
    let (dir, skipped) =
        result.map_err(|e| ToolError::new(crate::error::ErrorKind::Io, e.to_string()))?;
    let mut files = written.lock().clone();
    files.sort();
    files.dedup();
    let mut warnings = warnings.lock().clone();
    warnings.extend(skipped);
    Ok(ToolOutput::new(
        format!(
            "Ran {} output job(s) into {dir}: {} file(s).",
            count,
            files.len()
        ),
        json!({ "output_dir": dir, "files": files }),
    )?
    .warnings(warnings))
}
