//! Port of libs/librepcb/core/job/graphicsoutputjob.{h,cpp}.
//! Page settings are file-format data independent of the future renderer.

use super::ObjectSet;
use super::output_job::{read_options, token_enum, write_options};
use crate::serialization::{
    self, DeserializeObject, FromSExpression, List, SExpression, SerializeObject,
};
use crate::types::{Color, SimpleString, UnsignedLength, UnsignedRatio, Uuid};
use std::collections::BTreeMap;

token_enum!(GraphicsContentType {Schematic=>"schematic",Board=>"board",BoardRendering=>"board_rendering",AssemblyGuide=>"assembly_guide"});
token_enum!(PageOrientation {Auto=>"auto",Landscape=>"landscape",Portrait=>"portrait"});
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct GraphicsContent {
    pub kind: GraphicsContentType,
    pub title: String,
    pub paper: Option<String>,
    pub orientation: PageOrientation,
    pub rotate: bool,
    pub mirror: bool,
    pub scale: Option<UnsignedRatio>,
    /// Left, top, right, bottom margins.
    pub margins: [UnsignedLength; 4],
    pub dpi: u32,
    pub min_line_width: UnsignedLength,
    pub monochrome: bool,
    pub background: Color,
    pub layers: BTreeMap<String, Color>,
    pub boards: ObjectSet<Option<Uuid>>,
    pub variants: ObjectSet<Option<Uuid>>,
    pub options: BTreeMap<String, Vec<SExpression>>,
}
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct GraphicsJob {
    pub title: SimpleString,
    pub content: Vec<GraphicsContent>,
    pub output: String,
}
impl DeserializeObject for GraphicsJob {
    fn deserialize(n: &SExpression) -> serialization::Result<Self> {
        Ok(Self {
            title: n.child_value("title/@0")?,
            content: n
                .children_named("content")
                .map(GraphicsContent::deserialize)
                .collect::<serialization::Result<_>>()?,
            output: n.child_value("output/@0")?,
        })
    }
}
impl DeserializeObject for GraphicsContent {
    fn deserialize(n: &SExpression) -> serialization::Result<Self> {
        let paper = n.child_value::<String>("paper/@0")?;
        let scale = n.required_child("scale/@0")?;
        Ok(Self {
            kind: n.child_value("type/@0")?,
            title: n.child_value("title/@0")?,
            paper: if paper == "auto" { None } else { Some(paper) },
            orientation: n.child_value("orientation/@0")?,
            rotate: n.child_value("rotate/@0")?,
            mirror: n.child_value("mirror/@0")?,
            scale: if scale.value()? == "auto" {
                None
            } else {
                Some(UnsignedRatio::from_sexpression(scale)?)
            },
            margins: [
                n.child_value("margins/left/@0")?,
                n.child_value("margins/top/@0")?,
                n.child_value("margins/right/@0")?,
                n.child_value("margins/bottom/@0")?,
            ],
            dpi: n.child_value("dpi/@0")?,
            min_line_width: n.child_value("min_line_width/@0")?,
            monochrome: n.child_value("monochrome/@0")?,
            background: n.child_value("background/@0")?,
            layers: n
                .children_named("layer")
                .map(|c| Ok((c.child_value("@0")?, c.child_value("color/@0")?)))
                .collect::<serialization::Result<_>>()?,
            boards: ObjectSet::deserialize(n, "board")?,
            variants: ObjectSet::deserialize(n, "variant")?,
            options: read_options(n)?,
        })
    }
}
impl SerializeObject for GraphicsJob {
    fn serialize(&self, r: &mut List) {
        r.append_child("title", &self.title);
        r.ensure_line_break();
        for c in &self.content {
            c.serialize(r.append_list("content"));
            r.ensure_line_break();
        }
        r.append_child("output", &self.output);
    }
}
impl SerializeObject for GraphicsContent {
    fn serialize(&self, r: &mut List) {
        r.append_child("type", &self.kind);
        r.append_child("title", &self.title);
        r.ensure_line_break();
        r.append_child(
            "paper",
            &self
                .paper
                .as_ref()
                .map(|s| SExpression::string(s.clone()))
                .unwrap_or_else(|| SExpression::token("auto")),
        );
        r.append_child("orientation", &self.orientation);
        r.append_child("rotate", &self.rotate);
        r.append_child("mirror", &self.mirror);
        if let Some(scale) = self.scale {
            r.append_child("scale", &scale);
        } else {
            r.append_child("scale", &SExpression::token("auto"));
        }
        r.ensure_line_break();
        let m = r.append_list("margins");
        for (key, value) in ["left", "top", "right", "bottom"]
            .into_iter()
            .zip(self.margins)
        {
            m.append_child(key, &value);
        }
        r.ensure_line_break();
        r.append_child("dpi", &self.dpi);
        r.append_child("min_line_width", &self.min_line_width);
        r.append_child("monochrome", &self.monochrome);
        r.append_child("background", &self.background);
        r.ensure_line_break();
        for (key, color) in &self.layers {
            let c = r.append_list("layer");
            c.append_value(&SExpression::token(key));
            c.append_child("color", color);
            r.ensure_line_break();
        }
        self.boards.serialize(r, "board");
        r.ensure_line_break();
        self.variants.serialize(r, "variant");
        write_options(r, &self.options);
        r.ensure_line_break();
    }
}
