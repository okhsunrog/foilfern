//! Port of libs/librepcb/core/job/{outputjob,unknownoutputjob,
//! gerberexcellonoutputjob,gerberx3outputjob,pickplaceoutputjob,bomoutputjob,
//! netlistoutputjob,projectjsonoutputjob,lppzoutputjob,copyoutputjob,
//! archiveoutputjob,board3doutputjob,interactivehtmlbomoutputjob}.{h,cpp}.

use super::{GraphicsJob, ObjectSet};
use crate::project::board::BoardFabricationOutputSettings;
use crate::serialization::{
    self, DeserializeObject, HasName, HasUuid, List, ListTagName, SExpression,
    SerializableObjectList, SerializeObject,
};
use crate::types::{Angle, ElementName, Uuid};
use std::collections::{BTreeMap, BTreeSet};

/// Optional output file.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct EnabledOutput {
    pub create: bool,
    pub output: String,
}
impl DeserializeObject for EnabledOutput {
    fn deserialize(n: &SExpression) -> serialization::Result<Self> {
        Ok(Self {
            create: n.child_value("create/@0")?,
            output: n.child_value("output/@0")?,
        })
    }
}
impl SerializeObject for EnabledOutput {
    fn serialize(&self, r: &mut List) {
        r.append_child("create", &self.create);
        r.append_child("output", &self.output);
    }
}
/// Pick and place CSV configuration.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PickPlaceJob {
    pub boards: ObjectSet<Uuid>,
    pub variants: ObjectSet<Uuid>,
    pub comment: bool,
    pub tht: bool,
    pub smt: bool,
    pub mixed: bool,
    pub fiducial: bool,
    pub other: bool,
    pub top: EnabledOutput,
    pub bottom: EnabledOutput,
    pub both: EnabledOutput,
}
/// Gerber X3 components and glue configuration.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct GerberX3Job {
    pub boards: ObjectSet<Uuid>,
    pub variants: ObjectSet<Uuid>,
    pub components_top: EnabledOutput,
    pub components_bottom: EnabledOutput,
    pub glue_top: EnabledOutput,
    pub glue_bottom: EnabledOutput,
}
/// Standalone interactive HTML assembly BOM configuration.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct InteractiveBomJob {
    pub view_mode: BomViewMode,
    pub highlight_pin1: HighlightPin1,
    pub dark_mode: bool,
    pub rotation: Angle,
    pub offset_back_rotation: bool,
    pub show_silkscreen: bool,
    pub show_fabrication: bool,
    pub show_pads: bool,
    pub show_tracks: bool,
    pub show_zones: bool,
    pub checkboxes: Vec<String>,
    pub component_order: Vec<String>,
    pub custom_attributes: Vec<String>,
    pub boards: ObjectSet<Uuid>,
    pub variants: ObjectSet<Uuid>,
    pub output: String,
}
macro_rules! token_enum {
    ($name:ident {$($v:ident => $s:literal),+ $(,)?}) => {
        #[derive(Debug,Clone,Copy,PartialEq,Eq,serde::Serialize,serde::Deserialize)]
        pub enum $name {$($v),+}
        impl crate::serialization::ToSExpression for $name {
            fn to_sexpression(&self)->SExpression {SExpression::token(match self {$(Self::$v=>$s),+})}
        }
        impl crate::serialization::FromSExpression for $name {
            fn from_sexpression(n:&SExpression)->serialization::Result<Self> {Ok(match n.value()? {$($s=>Self::$v),+, s=>return Err(serialization::Error::InvalidToken(s.into()))})}
        }
    };
}
pub(super) use token_enum;
token_enum!(BomViewMode {BomOnly=>"bom_only",LeftRight=>"left_right",TopBottom=>"top_bottom"});
token_enum!(HighlightPin1 {None=>"none",Selected=>"selected",All=>"all"});
/// Configuration of an output job. UUID/name/options are common to all jobs.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum JobKind {
    Graphics(GraphicsJob),
    GerberExcellon {
        boards: ObjectSet<Uuid>,
        settings: Box<BoardFabricationOutputSettings>,
    },
    PickPlace(PickPlaceJob),
    GerberX3(GerberX3Job),
    Netlist {
        boards: ObjectSet<Uuid>,
        output: String,
    },
    Bom {
        custom_attributes: Vec<String>,
        boards: ObjectSet<Option<Uuid>>,
        variants: ObjectSet<Uuid>,
        output: String,
    },
    InteractiveBom(InteractiveBomJob),
    Board3d {
        boards: ObjectSet<Uuid>,
        variants: ObjectSet<Option<Uuid>>,
        output: String,
    },
    ProjectJson {
        output: String,
    },
    Lppz {
        output: String,
    },
    Copy {
        substitute_variables: bool,
        boards: ObjectSet<Option<Uuid>>,
        variants: ObjectSet<Option<Uuid>>,
        input: String,
        output: String,
    },
    Archive {
        inputs: BTreeMap<Uuid, String>,
        output: String,
    },
    /// Original complete node, including common fields, for forward compatibility.
    Unknown(SExpression),
}
impl JobKind {
    pub fn type_name(&self) -> &str {
        match self {
            Self::Graphics(_) => "graphics",
            Self::GerberExcellon { .. } => "gerber_excellon",
            Self::PickPlace(_) => "pnp",
            Self::GerberX3(_) => "gerber_x3",
            Self::Netlist { .. } => "netlist",
            Self::Bom { .. } => "bom",
            Self::InteractiveBom(_) => "interactive_bom",
            Self::Board3d { .. } => "3d_model",
            Self::ProjectJson { .. } => "project_json",
            Self::Lppz { .. } => "lppz",
            Self::Copy { .. } => "copy",
            Self::Archive { .. } => "archive",
            Self::Unknown(n) => n
                .child("type/@0")
                .and_then(|v| v.value().ok())
                .unwrap_or("unknown"),
        }
    }
}
/// Named, UUID-identified output job. Fields are edited on detached values,
/// then installed with `Mutation::SetOutputJobs` for projects.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct OutputJob {
    uuid: Uuid,
    name: ElementName,
    pub kind: JobKind,
    pub options: BTreeMap<String, Vec<SExpression>>,
}
static_assertions::assert_impl_all!(OutputJob:Send,Sync);
impl OutputJob {
    pub fn new(uuid: Uuid, name: ElementName, kind: JobKind) -> Self {
        Self {
            uuid,
            name,
            kind,
            options: BTreeMap::new(),
        }
    }
    pub fn uuid(&self) -> Uuid {
        self.uuid
    }
    pub fn name(&self) -> &ElementName {
        &self.name
    }
    pub fn set_uuid(&mut self, uuid: Uuid) {
        self.uuid = uuid;
    }
    pub fn set_name(&mut self, name: ElementName) {
        self.name = name;
    }
    pub fn dependencies(&self) -> BTreeSet<Uuid> {
        if let JobKind::Archive { inputs, .. } = &self.kind {
            inputs.keys().copied().collect()
        } else {
            BTreeSet::new()
        }
    }
    pub fn remove_dependency(&mut self, uuid: Uuid) {
        if let JobKind::Archive { inputs, .. } = &mut self.kind {
            inputs.remove(&uuid);
        }
    }
}
impl HasUuid for OutputJob {
    fn uuid(&self) -> Uuid {
        self.uuid
    }
}
impl HasName for OutputJob {
    fn name(&self) -> &str {
        self.name.as_str()
    }
}
pub struct OutputJobTag;
impl ListTagName for OutputJobTag {
    const TAG_NAME: &'static str = "job";
}
pub type OutputJobList = SerializableObjectList<OutputJob, OutputJobTag>;

pub(super) fn read_options(
    n: &SExpression,
) -> serialization::Result<BTreeMap<String, Vec<SExpression>>> {
    let mut options: BTreeMap<String, Vec<SExpression>> = BTreeMap::new();
    for c in n.children_named("option") {
        options
            .entry(c.child_value("@0")?)
            .or_default()
            .push(c.clone());
    }
    Ok(options)
}
pub(super) fn write_options(r: &mut List, options: &BTreeMap<String, Vec<SExpression>>) {
    for n in options.values().flatten() {
        r.ensure_line_break();
        r.append_value(n);
    }
}
fn strings(n: &SExpression, key: &str) -> serialization::Result<Vec<String>> {
    n.children_named(key).map(|n| n.child_value("@0")).collect()
}
impl DeserializeObject for OutputJob {
    fn deserialize(n: &SExpression) -> serialization::Result<Self> {
        let kind = match n.required_child("type/@0")?.value()? {
            "graphics" => JobKind::Graphics(GraphicsJob::deserialize(n)?),
            "gerber_excellon" => {
                // The board format uses the same fields under a different order
                // and names its output path `base_path` instead of `output`.
                let mut copy = n.clone();
                if let Some(r) = copy.as_list_mut() {
                    r.append_child("base_path", &n.child_value::<String>("output/@0")?);
                }
                JobKind::GerberExcellon {
                    boards: ObjectSet::deserialize(n, "board")?,
                    settings: Box::new(BoardFabricationOutputSettings::deserialize(&copy)?),
                }
            }
            "pnp" => JobKind::PickPlace(PickPlaceJob {
                boards: ObjectSet::deserialize(n, "board")?,
                variants: ObjectSet::deserialize(n, "variant")?,
                comment: n.child_value("comment/@0")?,
                tht: n.child_value("tht/@0")?,
                smt: n.child_value("smt/@0")?,
                mixed: n.child_value("mixed/@0")?,
                fiducial: n.child_value("fiducial/@0")?,
                other: n.child_value("other/@0")?,
                top: EnabledOutput::deserialize(n.required_child("top")?)?,
                bottom: EnabledOutput::deserialize(n.required_child("bottom")?)?,
                both: EnabledOutput::deserialize(n.required_child("both")?)?,
            }),
            "gerber_x3" => JobKind::GerberX3(GerberX3Job {
                boards: ObjectSet::deserialize(n, "board")?,
                variants: ObjectSet::deserialize(n, "variant")?,
                components_top: EnabledOutput::deserialize(n.required_child("components_top")?)?,
                components_bottom: EnabledOutput::deserialize(n.required_child("components_bot")?)?,
                glue_top: EnabledOutput::deserialize(n.required_child("glue_top")?)?,
                glue_bottom: EnabledOutput::deserialize(n.required_child("glue_bot")?)?,
            }),
            "netlist" => JobKind::Netlist {
                boards: ObjectSet::deserialize(n, "board")?,
                output: n.child_value("output/@0")?,
            },
            "bom" => JobKind::Bom {
                custom_attributes: strings(n, "custom_attribute")?,
                boards: ObjectSet::deserialize(n, "board")?,
                variants: ObjectSet::deserialize(n, "variant")?,
                output: n.child_value("output/@0")?,
            },
            "interactive_bom" => JobKind::InteractiveBom(InteractiveBomJob {
                view_mode: n.child_value("view_mode/@0")?,
                highlight_pin1: n.child_value("highlight_pin1/@0")?,
                dark_mode: n.child_value("dark_mode/@0")?,
                rotation: n.child_value("rotation/@0")?,
                offset_back_rotation: n.child_value("offset_back_rotation/@0")?,
                show_silkscreen: n.child_value("show_silkscreen/@0")?,
                show_fabrication: n.child_value("show_fabrication/@0")?,
                show_pads: n.child_value("show_pads/@0")?,
                show_tracks: n.child_value("show_tracks/@0")?,
                show_zones: n.child_value("show_zones/@0")?,
                checkboxes: n
                    .child_value::<String>("checkboxes/@0")?
                    .split(',')
                    .map(str::to_owned)
                    .collect(),
                component_order: n
                    .child_value::<String>("component_order/@0")?
                    .split(',')
                    .map(str::to_owned)
                    .collect(),
                custom_attributes: strings(n, "custom_attribute")?,
                boards: ObjectSet::deserialize(n, "board")?,
                variants: ObjectSet::deserialize(n, "variant")?,
                output: n.child_value("output/@0")?,
            }),
            "3d_model" => JobKind::Board3d {
                boards: ObjectSet::deserialize(n, "board")?,
                variants: ObjectSet::deserialize(n, "variant")?,
                output: n.child_value("output/@0")?,
            },
            "project_json" => JobKind::ProjectJson {
                output: n.child_value("output/@0")?,
            },
            "lppz" => JobKind::Lppz {
                output: n.child_value("output/@0")?,
            },
            "copy" => JobKind::Copy {
                substitute_variables: n.child_value("substitute_variables/@0")?,
                boards: ObjectSet::deserialize(n, "board")?,
                variants: ObjectSet::deserialize(n, "variant")?,
                input: n.child_value("input/@0")?,
                output: n.child_value("output/@0")?,
            },
            "archive" => JobKind::Archive {
                inputs: n
                    .children_named("input")
                    .map(|c| Ok((c.child_value("@0")?, c.child_value("destination/@0")?)))
                    .collect::<serialization::Result<_>>()?,
                output: n.child_value("output/@0")?,
            },
            _ => JobKind::Unknown(n.clone()),
        };
        Ok(Self {
            uuid: n.child_value("@0")?,
            name: n.child_value("name/@0")?,
            kind,
            options: read_options(n)?,
        })
    }
}
impl SerializeObject for OutputJob {
    fn serialize(&self, r: &mut List) {
        if let JobKind::Unknown(n) = &self.kind {
            // UnknownOutputJob::serialize ignores all edits to common fields.
            if let Some(n) = n.as_list() {
                *r = n.clone();
            }
            return;
        }
        r.append_value(&self.uuid);
        r.append_child("name", &self.name);
        r.ensure_line_break();
        r.append_child("type", &SExpression::token(self.kind.type_name()));
        match &self.kind {
            JobKind::Graphics(j) => j.serialize(r),
            JobKind::GerberExcellon {
                boards,
                settings: s,
            } => {
                let mut tmp = List::new("settings");
                s.serialize(&mut tmp);
                let tmp = SExpression::from(tmp);
                for key in [
                    "outlines",
                    "copper_top",
                    "copper_inner",
                    "copper_bot",
                    "soldermask_top",
                    "soldermask_bot",
                    "silkscreen_top",
                    "silkscreen_bot",
                    "solderpaste_top",
                    "solderpaste_bot",
                    "drills",
                ] {
                    r.ensure_line_break();
                    // All named nodes are emitted by the serializer above.
                    if let Some(c) = tmp.child(key) {
                        r.append_value(c);
                    }
                }
                r.ensure_line_break();
                boards.serialize(r, "board");
                r.ensure_line_break();
                r.append_child("output", &s.output_base_path);
            }
            JobKind::PickPlace(j) => {
                r.append_child("comment", &j.comment);
                r.ensure_line_break();
                for (k, v) in [
                    ("tht", j.tht),
                    ("smt", j.smt),
                    ("mixed", j.mixed),
                    ("fiducial", j.fiducial),
                    ("other", j.other),
                ] {
                    r.append_child(k, &v);
                }
                r.ensure_line_break();
                j.boards.serialize(r, "board");
                r.ensure_line_break();
                j.variants.serialize(r, "variant");
                for (k, v) in [("top", &j.top), ("bottom", &j.bottom), ("both", &j.both)] {
                    r.ensure_line_break();
                    v.serialize(r.append_list(k));
                }
            }
            JobKind::GerberX3(j) => {
                r.ensure_line_break();
                j.boards.serialize(r, "board");
                r.ensure_line_break();
                j.variants.serialize(r, "variant");
                for (k, v) in [
                    ("components_top", &j.components_top),
                    ("components_bot", &j.components_bottom),
                    ("glue_top", &j.glue_top),
                    ("glue_bot", &j.glue_bottom),
                ] {
                    r.ensure_line_break();
                    v.serialize(r.append_list(k));
                    r.ensure_line_break();
                }
            }
            JobKind::Netlist { boards, output } => {
                r.ensure_line_break();
                boards.serialize(r, "board");
                r.ensure_line_break();
                r.append_child("output", output);
            }
            JobKind::Bom {
                custom_attributes,
                boards,
                variants,
                output,
            } => {
                r.ensure_line_break();
                for a in custom_attributes {
                    r.append_child("custom_attribute", a);
                    r.ensure_line_break();
                }
                boards.serialize(r, "board");
                r.ensure_line_break();
                variants.serialize(r, "variant");
                r.ensure_line_break();
                r.append_child("output", output);
            }
            JobKind::InteractiveBom(j) => {
                r.ensure_line_break();
                r.append_child("view_mode", &j.view_mode);
                r.append_child("highlight_pin1", &j.highlight_pin1);
                r.append_child("dark_mode", &j.dark_mode);
                r.ensure_line_break();
                r.append_child("rotation", &j.rotation);
                r.append_child("offset_back_rotation", &j.offset_back_rotation);
                r.ensure_line_break();
                r.append_child("show_silkscreen", &j.show_silkscreen);
                r.append_child("show_fabrication", &j.show_fabrication);
                r.ensure_line_break();
                r.append_child("show_pads", &j.show_pads);
                r.append_child("show_tracks", &j.show_tracks);
                r.append_child("show_zones", &j.show_zones);
                r.ensure_line_break();
                r.append_child("checkboxes", &j.checkboxes.join(","));
                r.ensure_line_break();
                r.append_child("component_order", &j.component_order.join(","));
                r.ensure_line_break();
                for a in &j.custom_attributes {
                    r.append_child("custom_attribute", a);
                    r.ensure_line_break();
                }
                j.boards.serialize(r, "board");
                r.ensure_line_break();
                j.variants.serialize(r, "variant");
                r.ensure_line_break();
                r.append_child("output", &j.output);
            }
            JobKind::Board3d {
                boards,
                variants,
                output,
            } => {
                r.ensure_line_break();
                boards.serialize(r, "board");
                r.ensure_line_break();
                variants.serialize(r, "variant");
                r.ensure_line_break();
                r.append_child("output", output);
            }
            JobKind::ProjectJson { output } | JobKind::Lppz { output } => {
                r.ensure_line_break();
                r.append_child("output", output);
            }
            JobKind::Copy {
                substitute_variables,
                boards,
                variants,
                input,
                output,
            } => {
                r.append_child("substitute_variables", substitute_variables);
                r.ensure_line_break();
                boards.serialize(r, "board");
                r.ensure_line_break();
                variants.serialize(r, "variant");
                r.ensure_line_break();
                r.append_child("input", input);
                r.ensure_line_break();
                r.append_child("output", output);
            }
            JobKind::Archive { inputs, output } => {
                r.ensure_line_break();
                for (uuid, destination) in inputs {
                    let c = r.append_list("input");
                    c.append_value(uuid);
                    c.append_child("destination", destination);
                    r.ensure_line_break();
                }
                r.append_child("output", output);
            }
            JobKind::Unknown(_) => {}
        }
        write_options(r, &self.options);
        r.ensure_line_break();
    }
}
