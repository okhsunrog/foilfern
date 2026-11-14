//! Port of libs/librepcb/core/3d/occmodel.cpp's surface appearance boundary.
//! Uses step-p21's AST; Monstertruck 0.4.1 has no appearance import/export.
//! The adapter retains surface RGB/opacity, with face > shell > solid priority.
//! Unsupported style semantics fail rather than silently becoming gray.
use crate::{
    PlacedSolid,
    metadata::{Records, Result, StepDocument, invalid, list, number, reference, text},
};
use monstertruck_io::step::{load::step_p21::ast::Parameter, save};
use monstertruck_meshing::prelude::{PolygonMesh, obj};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Write,
};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Material {
    /// STEP COLOUR_RGB values, retained without a gamma conversion.
    pub color: [f64; 3],
    pub opacity: f64,
}
pub const DEFAULT_MATERIAL: Material = Material {
    color: [0.65; 3],
    opacity: 1.,
};
pub type Appearance = Vec<Vec<Option<Material>>>;
pub type MaterialGroups = Vec<(usize, Option<Material>)>;

pub fn export_obj(
    mesh: &PolygonMesh,
    groups: &[(usize, Option<Material>)],
) -> anyhow::Result<(String, String)> {
    anyhow::ensure!(
        groups.iter().map(|(count, _)| count).sum::<usize>() == mesh.tri_faces().len(),
        "OBJ material/triangle count mismatch"
    );
    let mut bytes = Vec::new();
    obj::write(mesh, &mut bytes)?;
    let geometry = String::from_utf8(bytes)?;
    let mut materials = Vec::new();
    let mut names = Vec::new();
    for &(count, value) in groups {
        let material = value.unwrap_or(DEFAULT_MATERIAL);
        let index = if let Some(index) = materials.iter().position(|value| *value == material) {
            index
        } else {
            materials.push(material);
            materials.len() - 1
        };
        names.extend(std::iter::repeat_n(index, count));
    }
    let mut output = String::from("mtllib assembly.mtl\n");
    let mut last = None;
    let mut faces = names.into_iter();
    for line in geometry.lines() {
        if line.starts_with("f ") {
            let index = faces
                .next()
                .ok_or_else(|| anyhow::anyhow!("OBJ face count changed"))?;
            if last != Some(index) {
                writeln!(output, "usemtl surface_{index}")?;
                last = Some(index);
            }
        }
        writeln!(output, "{line}")?;
    }
    anyhow::ensure!(faces.next().is_none(), "OBJ omitted material faces");
    let mut mtl = String::new();
    for (index, material) in materials.iter().enumerate() {
        let [r, g, b] = material.color;
        writeln!(
            mtl,
            "newmtl surface_{index}\nKd {r:.17} {g:.17} {b:.17}\nd {:.17}\n",
            material.opacity
        )?;
    }
    Ok((output, mtl))
}

fn merge_material(value: &mut Option<Material>, next: Option<Material>, id: u64) -> Result<()> {
    if let Some(next) = next {
        if let Some(current) = value
            && *current != next
        {
            return Err(invalid(id, "conflicting surface styles"));
        }
        *value = Some(next);
    }
    Ok(())
}

impl Records {
    fn style(&self, id: u64, visiting: &mut BTreeSet<u64>) -> Result<Option<Material>> {
        if !visiting.insert(id) || visiting.len() > 64 {
            return Err(invalid(id, "cyclic or excessive style chain"));
        }
        let records = self
            .0
            .get(&id)
            .ok_or_else(|| invalid(id, "unresolved style reference"))?;
        let record = records
            .first()
            .ok_or_else(|| invalid(id, "empty style record"))?;
        let args = list(&record.parameter, id)?;
        let mut result = None;
        match record.name.as_str() {
            "CURVE_STYLE" | "POINT_STYLE" => (), // not surface appearance
            "COLOUR_RGB" => {
                let [_, red, green, blue] = args else {
                    return Err(invalid(id, "invalid COLOUR_RGB"));
                };
                let color = [number(red, id)?, number(green, id)?, number(blue, id)?];
                if color.iter().any(|v| !(0. ..=1.).contains(v)) {
                    return Err(invalid(id, "RGB outside 0..1"));
                }
                result = Some(Material { color, opacity: 1. });
            }
            "DRAUGHTING_PRE_DEFINED_COLOUR" => {
                let [name] = args else {
                    return Err(invalid(id, "invalid named colour"));
                };
                let color = match text(name, id)? {
                    "black" => [0., 0., 0.],
                    "white" => [1., 1., 1.],
                    "red" => [1., 0., 0.],
                    "green" => [0., 1., 0.],
                    "blue" => [0., 0., 1.],
                    "cyan" => [0., 1., 1.],
                    "magenta" => [1., 0., 1.],
                    "yellow" => [1., 1., 0.],
                    _ => return Err(invalid(id, "unsupported named colour")),
                };
                result = Some(Material { color, opacity: 1. });
            }
            "SURFACE_STYLE_USAGE" => {
                let [Parameter::Enumeration(side), style] = args else {
                    return Err(invalid(id, "invalid surface style usage"));
                };
                if side != "BOTH" {
                    return Err(invalid(id, "directional surface styles are unsupported"));
                }
                result = self.style(reference(style, id)?, visiting)?;
            }
            "SURFACE_STYLE_FILL_AREA" | "FILL_AREA_STYLE_COLOUR" => {
                let value = args
                    .last()
                    .ok_or_else(|| invalid(id, "missing style value"))?;
                result = self.style(reference(value, id)?, visiting)?;
            }
            "SURFACE_SIDE_STYLE" => {
                let [_, values] = args else {
                    return Err(invalid(id, "invalid surface side style"));
                };
                let mut fill = None;
                let mut rendering = None;
                for value in list(values, id)? {
                    let child = reference(value, id)?;
                    let slot = if self.part(child, "SURFACE_STYLE_RENDERING").is_some()
                        || self
                            .part(child, "SURFACE_STYLE_RENDERING_WITH_PROPERTIES")
                            .is_some()
                    {
                        &mut rendering
                    } else {
                        &mut fill
                    };
                    merge_material(slot, self.style(child, visiting)?, id)?;
                }
                if let (Some(fill), Some(rendering)) = (fill, rendering)
                    && fill.color != rendering.color
                {
                    return Err(invalid(
                        id,
                        "different fill and rendering colors are unsupported",
                    ));
                }
                result = rendering.or(fill);
            }
            "PRESENTATION_STYLE_ASSIGNMENT" | "FILL_AREA_STYLE" => {
                let values = args
                    .last()
                    .ok_or_else(|| invalid(id, "missing style list"))?;
                for value in list(values, id)? {
                    merge_material(
                        &mut result,
                        self.style(reference(value, id)?, visiting)?,
                        id,
                    )?;
                }
            }
            "SURFACE_STYLE_RENDERING" | "SURFACE_STYLE_RENDERING_WITH_PROPERTIES" => {
                let [Parameter::Enumeration(method), color, properties @ ..] = args else {
                    return Err(invalid(id, "invalid surface rendering style"));
                };
                if method != "NORMAL_SHADING" {
                    return Err(invalid(id, "unsupported shading method"));
                }
                result = self.style(reference(color, id)?, visiting)?;
                let material = result
                    .as_mut()
                    .ok_or_else(|| invalid(id, "rendering has no surface color"))?;
                for properties in properties {
                    for property in list(properties, id)? {
                        let property = reference(property, id)?;
                        let [value] = self.args(property, "SURFACE_STYLE_TRANSPARENT")? else {
                            return Err(invalid(property, "unsupported rendering property"));
                        };
                        let transparency = number(value, property)?;
                        if !(0. ..=1.).contains(&transparency) {
                            return Err(invalid(property, "transparency outside 0..1"));
                        }
                        material.opacity = 1. - transparency;
                    }
                }
            }
            _ => {
                return Err(invalid(
                    id,
                    format!("unsupported surface style {}", record.name),
                ));
            }
        }
        visiting.remove(&id);
        Ok(result)
    }
    pub(crate) fn surface_styles(&self) -> Result<BTreeMap<u64, Material>> {
        let mut styles = BTreeMap::new();
        let mut owners = BTreeMap::new();
        for (&id, records) in &self.0 {
            if records.iter().any(|r| {
                r.name.contains("STYLED_ITEM")
                    && !["STYLED_ITEM", "OVER_RIDING_STYLED_ITEM"].contains(&r.name.as_str())
            }) {
                return Err(invalid(id, "context/override styled items are unsupported"));
            }
            let name = if self.part(id, "STYLED_ITEM").is_some() {
                "STYLED_ITEM"
            } else {
                "OVER_RIDING_STYLED_ITEM"
            };
            if self.part(id, name).is_none() {
                continue;
            }
            // Validate override chains even if they target a more specific face.
            self.overrides(id, None)?;
            let [_, assignments, target, ..] = self.args(id, name)? else {
                return Err(invalid(id, "invalid styled item"));
            };
            let target = reference(target, id)?;
            if !self.0.contains_key(&target) {
                return Err(invalid(id, "style target does not exist"));
            }
            let mut material = None;
            for assignment in list(assignments, id)? {
                merge_material(
                    &mut material,
                    self.style(reference(assignment, id)?, &mut BTreeSet::new())?,
                    id,
                )?;
            }
            if let Some(material) = material {
                if ![
                    "MANIFOLD_SOLID_BREP",
                    "BREP_WITH_VOIDS",
                    "CLOSED_SHELL",
                    "OPEN_SHELL",
                    "ADVANCED_FACE",
                    "FACE_SURFACE",
                ]
                .iter()
                .any(|kind| self.part(target, kind).is_some())
                {
                    return Err(invalid(target, "surface style target kind is unsupported"));
                }
                if let Some(previous) = styles.get(&target)
                    && previous != &material
                {
                    let owner = owners[&target];
                    if self.overrides(owner, Some(id))? {
                        continue;
                    }
                    if !self.overrides(id, Some(owner))? {
                        return Err(invalid(target, "conflicting styled items"));
                    }
                }
                styles.insert(target, material);
                owners.insert(target, id);
            }
        }
        Ok(styles)
    }
    fn overrides(&self, mut style: u64, ancestor: Option<u64>) -> Result<bool> {
        let mut seen = BTreeSet::new();
        loop {
            if !seen.insert(style) || seen.len() > 64 {
                return Err(invalid(style, "cyclic or excessive style override chain"));
            }
            if self.part(style, "OVER_RIDING_STYLED_ITEM").is_none() {
                if self.part(style, "STYLED_ITEM").is_none() {
                    return Err(invalid(style, "unresolved overridden style"));
                }
                return Ok(false);
            }
            let args = self.args(style, "OVER_RIDING_STYLED_ITEM")?;
            style = reference(
                args.get(3)
                    .ok_or_else(|| invalid(style, "missing overridden style"))?,
                style,
            )?;
            if Some(style) == ancestor {
                return Ok(true);
            }
        }
    }
    pub(crate) fn solid_faces(&self, id: u64) -> Result<Vec<(u64, Vec<u64>)>> {
        let solid = if self.part(id, "MANIFOLD_SOLID_BREP").is_some() {
            "MANIFOLD_SOLID_BREP"
        } else {
            "BREP_WITH_VOIDS"
        };
        let args = self.args(id, solid)?;
        let mut shells = vec![reference(
            args.get(1)
                .ok_or_else(|| invalid(id, "missing solid shell"))?,
            id,
        )?];
        if self.part(id, "BREP_WITH_VOIDS").is_some() {
            let args = self.args(id, "BREP_WITH_VOIDS")?;
            let voids = args
                .last()
                .ok_or_else(|| invalid(id, "missing void shells"))?;
            for shell in list(voids, id)? {
                shells.push(reference(shell, id)?);
            }
        }
        let mut result = Vec::new();
        for original in shells {
            let mut shell = original;
            if self.part(shell, "ORIENTED_CLOSED_SHELL").is_some()
                || self.part(shell, "ORIENTED_OPEN_SHELL").is_some()
            {
                let name = if self.part(shell, "ORIENTED_CLOSED_SHELL").is_some() {
                    "ORIENTED_CLOSED_SHELL"
                } else {
                    "ORIENTED_OPEN_SHELL"
                };
                shell = reference(
                    self.args(shell, name)?
                        .get(2)
                        .ok_or_else(|| invalid(shell, "missing oriented shell target"))?,
                    shell,
                )?;
            }
            let name = if self.part(shell, "CLOSED_SHELL").is_some() {
                "CLOSED_SHELL"
            } else {
                "OPEN_SHELL"
            };
            let args = self.args(shell, name)?;
            let faces = args
                .get(1)
                .ok_or_else(|| invalid(shell, "missing shell face list"))?;
            let mut ids = Vec::new();
            for face in list(faces, shell)? {
                let mut face = reference(face, shell)?;
                if self.part(face, "ORIENTED_FACE").is_some() {
                    face = reference(
                        self.args(face, "ORIENTED_FACE")?
                            .get(2)
                            .ok_or_else(|| invalid(face, "missing oriented face target"))?,
                        face,
                    )?;
                }
                ids.push(face);
            }
            result.push((shell, ids));
        }
        Ok(result)
    }
    pub(crate) fn appearance(
        &self,
        id: u64,
        styles: &BTreeMap<u64, Material>,
    ) -> Result<Appearance> {
        self.solid_faces(id)?
            .into_iter()
            .map(|(shell, faces)| {
                Ok(faces
                    .into_iter()
                    .map(|face| {
                        styles
                            .get(&face)
                            .or_else(|| styles.get(&shell))
                            .or_else(|| styles.get(&id))
                            .copied()
                    })
                    .collect())
            })
            .collect()
    }
}

/// Add standard AP214 styles to the BRep entities emitted by Monstertruck.
/// Export IDs are recovered from the new AST, never copied from the input file.
pub fn export_step(solids: &[PlacedSolid]) -> Result<String> {
    if solids.is_empty() {
        return Err(invalid(0, "cannot export an empty assembly"));
    }
    if solids.iter().any(|item| {
        item.solid.boundaries.is_empty()
            || item
                .solid
                .boundaries
                .iter()
                .any(|shell| shell.faces.is_empty())
    }) {
        return Err(invalid(0, "empty solid boundary cannot be exported"));
    }
    let models: save::StepModels<_, _, _> = solids.iter().map(|item| &item.solid).collect();
    let mut output =
        save::CompleteStepDisplay::new(models, save::StepHeaderDescriptor::default()).to_string();
    let document = StepDocument::parse(output.as_bytes())?;
    let ids: Vec<_> = document
        .table
        .manifold_solid_brep
        .keys()
        .copied()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    if ids.len() != solids.len() {
        return Err(invalid(0, "exported solid count changed"));
    }
    let mut next = document.records.0.keys().copied().max().unwrap_or(0) + 1;
    let context = document
        .records
        .0
        .keys()
        .copied()
        .find(|&id| {
            document
                .records
                .part(id, "GLOBAL_UNIT_ASSIGNED_CONTEXT")
                .is_some()
        })
        .ok_or_else(|| invalid(0, "missing export context"))?;
    let mut entities = String::new();
    let mut assignments: Vec<(Material, u64)> = Vec::new();
    let mut styled = Vec::new();
    for (item, id) in solids.iter().zip(ids) {
        let faces = document.records.solid_faces(id)?;
        if faces.len() != item.appearance.len() {
            return Err(invalid(id, "appearance boundary count changed"));
        }
        for ((_, faces), materials) in faces.into_iter().zip(&item.appearance) {
            if faces.len() != materials.len() {
                return Err(invalid(id, "appearance face count changed"));
            }
            for (face, material) in faces.into_iter().zip(materials) {
                let Some(material) = material else {
                    continue;
                };
                if material
                    .color
                    .iter()
                    .chain(std::iter::once(&material.opacity))
                    .any(|value| !value.is_finite() || !(0. ..=1.).contains(value))
                {
                    return Err(invalid(face, "invalid surface material"));
                }
                let assignment = if let Some((_, assignment)) =
                    assignments.iter().find(|(value, _)| value == material)
                {
                    *assignment
                } else {
                    let start = next;
                    next += 7;
                    let [r, g, b] = material.color;
                    writeln!(entities, "#{start}=COLOUR_RGB('',{r:.17E},{g:.17E},{b:.17E});\n#{}=FILL_AREA_STYLE_COLOUR('',#{start});\n#{}=FILL_AREA_STYLE('',(#{}));\n#{}=SURFACE_STYLE_FILL_AREA(#{});\n#{}=SURFACE_SIDE_STYLE('',(#{}));\n#{}=SURFACE_STYLE_USAGE(.BOTH.,#{});\n#{}=PRESENTATION_STYLE_ASSIGNMENT((#{}));",start+1,start+2,start+1,start+3,start+2,start+4,start+3,start+5,start+4,start+6,start+5).map_err(|_| invalid(start, "formatting styles failed"))?;
                    // For translucent faces use a rendering style, preserving alpha.
                    if material.opacity < 1. {
                        let transparent = next;
                        let rendering = next + 1;
                        next += 2;
                        writeln!(entities,"#{transparent}=SURFACE_STYLE_TRANSPARENT({:.17E});\n#{rendering}=SURFACE_STYLE_RENDERING_WITH_PROPERTIES(.NORMAL_SHADING.,#{start},(#{transparent}));",1.-material.opacity).map_err(|_| invalid(start, "formatting transparency failed"))?;
                        entities = entities.replace(
                            &format!("#{}=SURFACE_SIDE_STYLE('',(#{}));", start + 4, start + 3),
                            &format!(
                                "#{}=SURFACE_SIDE_STYLE('',(#{},#{rendering}));",
                                start + 4,
                                start + 3
                            ),
                        );
                    }
                    assignments.push((*material, start + 6));
                    start + 6
                };
                let style = next;
                next += 1;
                writeln!(
                    entities,
                    "#{style}=STYLED_ITEM('',(#{assignment}),#{face});"
                )
                .map_err(|_| invalid(style, "formatting styled item failed"))?;
                styled.push(format!("#{style}"));
            }
        }
    }
    if !styled.is_empty() {
        writeln!(
            entities,
            "#{next}=MECHANICAL_DESIGN_GEOMETRIC_PRESENTATION_REPRESENTATION('',({}),#{context});",
            styled.join(",")
        )
        .map_err(|_| invalid(next, "formatting presentation failed"))?;
        let end = output
            .rfind("ENDSEC;")
            .ok_or_else(|| invalid(0, "missing DATA terminator"))?;
        output.insert_str(end, &entities);
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use monstertruck_io::step::load::step_p21::ast::DataSection;
    fn records(source: &str) -> Records {
        let data: DataSection = format!("DATA;{source}ENDSEC;").parse().unwrap();
        Records::new(&data).unwrap()
    }
    const TOPOLOGY: &str = "#1=MANIFOLD_SOLID_BREP('',#2);#2=CLOSED_SHELL('',(#3,#4));#3=ADVANCED_FACE('',(),#5,.T.);#4=ADVANCED_FACE('',(),#5,.T.);#5=PLANE('',#6);";
    #[test]
    fn obj_materials_survive_independent_reader() {
        use monstertruck_meshing::prelude::*;
        use std::io::Cursor;
        let mesh = PolygonMesh::new(
            StandardAttributes {
                positions: vec![
                    Point3::new(0., 0., 0.),
                    Point3::new(1., 0., 0.),
                    Point3::new(0., 1., 0.),
                    Point3::new(0., 0., 1.),
                ],
                ..Default::default()
            },
            Faces::from_iter([[0, 1, 2], [0, 2, 3], [0, 3, 1], [1, 3, 2]]),
        );
        let red = Material {
            color: [1., 0., 0.],
            opacity: 0.25,
        };
        let (obj, mtl) = export_obj(&mesh, &[(1, Some(red)), (2, None), (1, Some(red))]).unwrap();
        let (models, materials) = tobj::load_obj_buf(
            &mut Cursor::new(obj),
            &tobj::LoadOptions::default(),
            |path| {
                assert_eq!(path, std::path::Path::new("assembly.mtl"));
                tobj::load_mtl_buf(&mut Cursor::new(mtl.as_bytes()))
            },
        )
        .unwrap();
        let materials = materials.unwrap();
        let actual: Vec<_> = models
            .iter()
            .map(|model| {
                let material = &materials[model.mesh.material_id.unwrap()];
                (
                    model.mesh.indices.len() / 3,
                    material.diffuse.unwrap(),
                    material.dissolve.unwrap(),
                )
            })
            .collect();
        assert_eq!(
            actual,
            vec![
                (1, [1., 0., 0.], 0.25),
                (2, [0.65; 3], 1.),
                (1, [1., 0., 0.], 0.25)
            ]
        );
        assert!(export_obj(&mesh, &[(3, None)]).is_err());
    }
    fn style(start: u64, target: u64, color: &str, parent: Option<u64>) -> String {
        let styled = if let Some(parent) = parent {
            format!(
                "OVER_RIDING_STYLED_ITEM('',(#{}),#{target},#{parent})",
                start + 1
            )
        } else {
            format!("STYLED_ITEM('',(#{}),#{target})", start + 1)
        };
        format!(
            "#{start}={styled};#{}=PRESENTATION_STYLE_ASSIGNMENT((#{}));#{}=SURFACE_STYLE_USAGE(.BOTH.,#{});#{}=SURFACE_SIDE_STYLE('',(#{}));#{}=SURFACE_STYLE_FILL_AREA(#{});#{}=FILL_AREA_STYLE('',(#{}));#{}=FILL_AREA_STYLE_COLOUR('',#{});#{}=COLOUR_RGB('',{color});",
            start + 1,
            start + 2,
            start + 2,
            start + 3,
            start + 3,
            start + 4,
            start + 4,
            start + 5,
            start + 5,
            start + 6,
            start + 6,
            start + 7,
            start + 7
        )
    }
    #[test]
    fn face_override_retains_body_default_and_chain_order() {
        // Override IDs deliberately precede their base, so entity order cannot
        // be mistaken for semantic priority.
        let source = format!(
            "{TOPOLOGY}{}{}{}",
            style(100, 1, "1.,0.,0.", None),
            style(20, 3, "0.,0.,1.", Some(100)),
            style(10, 3, "0.,1.,0.", Some(20))
        );
        let data = records(&source);
        let values = data.appearance(1, &data.surface_styles().unwrap()).unwrap();
        assert_eq!(values[0][0].unwrap().color, [0., 1., 0.]);
        assert_eq!(values[0][1].unwrap().color, [1., 0., 0.]);
    }
    #[test]
    fn transparency_named_colors_and_invalid_styles() {
        let data = records(
            "#1=DRAUGHTING_PRE_DEFINED_COLOUR('cyan');#2=SURFACE_STYLE_TRANSPARENT(0.75);#3=SURFACE_STYLE_RENDERING_WITH_PROPERTIES(.NORMAL_SHADING.,#1,(#2));",
        );
        assert_eq!(
            data.style(3, &mut BTreeSet::new()).unwrap(),
            Some(Material {
                color: [0., 1., 1.],
                opacity: 0.25
            })
        );
        let combined = records(
            "#1=COLOUR_RGB('',0.,1.,1.);#2=SURFACE_STYLE_TRANSPARENT(0.75);#3=SURFACE_STYLE_RENDERING_WITH_PROPERTIES(.NORMAL_SHADING.,#1,(#2));#4=FILL_AREA_STYLE_COLOUR('',#1);#5=FILL_AREA_STYLE('',(#4));#6=SURFACE_STYLE_FILL_AREA(#5);#7=SURFACE_SIDE_STYLE('',(#6,#3));",
        );
        assert_eq!(
            combined.style(7, &mut BTreeSet::new()).unwrap(),
            Some(Material {
                color: [0., 1., 1.],
                opacity: 0.25
            })
        );
        for source in [
            "#1=COLOUR_RGB('',2.,0.,0.);",
            "#1=SURFACE_SIDE_STYLE('',(#1));",
            "#1=SURFACE_STYLE_USAGE(.POSITIVE.,#2);#2=COLOUR_RGB('',1.,0.,0.);",
        ] {
            assert!(records(source).style(1, &mut BTreeSet::new()).is_err());
        }
    }
    #[test]
    fn unrelated_style_conflicts_and_override_cycles_fail() {
        let conflict = format!(
            "{TOPOLOGY}{}{}",
            style(20, 3, "1.,0.,0.", None),
            style(100, 3, "0.,1.,0.", None)
        );
        assert!(records(&conflict).surface_styles().is_err());
        let cycle = format!(
            "{TOPOLOGY}{}{}",
            style(20, 3, "1.,0.,0.", Some(100)),
            style(100, 3, "0.,1.,0.", Some(20))
        );
        assert!(records(&cycle).surface_styles().is_err());
    }
}
