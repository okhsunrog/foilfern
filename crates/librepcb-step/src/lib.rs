//! Port of libs/librepcb/core/3d/{occmodel,stepexport}.{h,cpp}.
//! Pure Rust BRep import, board preparation and STEP/OBJ export. The CAD kernel
//! is separate from the project model and rendering/window-system backends.
//! Application entry points discard private derived geometry on kernel panics.
mod board;
mod error;
mod kernel;
mod materials;
mod metadata;
mod minify;

pub use error::{Error, Result};
use kernel::PlacedSolid;
use librepcb_core::{
    fileio::FileSystem,
    project::{
        AssemblyVariantId, Project,
        board::{Board, BoardPhysicalGeometry},
    },
};
pub use materials::Material as SurfaceMaterial;
pub use metadata::Error as MetadataError;
pub use minify::{Error as MinifyError, minify_step};
use monstertruck_io::step::save;
use monstertruck_meshing::prelude::PolygonMesh;
use std::collections::BTreeMap;

/// Owned CAD geometry in millimetres. No project, filesystem or GPU handles.
#[derive(Clone)]
pub struct StepModel {
    solids: Vec<PlacedSolid>,
    approximated_contours: usize,
}
static_assertions::assert_impl_all!(StepModel: Send, Sync);

/// Expanded triangle vertices and distinct surface appearances, including opacity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TessellationStats {
    pub vertices: usize,
    pub colors: usize,
}

fn operation<T>(result: anyhow::Result<T>) -> Result<T> {
    result.map_err(|error| Error::Geometry(format!("{error:#}")))
}
fn protected<T>(f: impl FnOnce() -> Result<T>) -> Result<T> {
    // The closure only creates derived geometry and reads project data.
    // No partially built geometry is published if the kernel unwinds.
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).unwrap_or(Err(Error::KernelPanicked))
}
fn check_input(bytes: &[u8]) -> Result<()> {
    if bytes.len() > 256 * 1024 * 1024 {
        return Err(Error::InputLimit);
    }
    let mut depth = 0usize;
    let mut quote = None;
    let mut comment = false;
    let mut index = 0;
    while index < bytes.len() {
        let value = bytes[index];
        if comment {
            if bytes.get(index..index + 2) == Some(b"*/") {
                comment = false;
                index += 2;
                continue;
            }
        } else if let Some(delimiter) = quote {
            if value == delimiter {
                quote = None;
            }
        } else if matches!(value, b'\'' | b'"') {
            quote = Some(value);
        } else if bytes.get(index..index + 2) == Some(b"/*") {
            comment = true;
            index += 2;
            continue;
        } else if value == b'(' {
            depth += 1;
            if depth > 256 {
                return Err(Error::InputLimit);
            }
        } else if value == b')' {
            depth = depth.checked_sub(1).ok_or(Error::InvalidNesting)?;
        }
        index += 1;
    }
    if depth != 0 || quote.is_some() || comment {
        return Err(Error::InvalidNesting);
    }
    Ok(())
}
fn import(bytes: &[u8]) -> Result<Vec<PlacedSolid>> {
    check_input(bytes)?;
    let document = metadata::StepDocument::parse(bytes)?;
    document
        .table
        .entity_report
        .require_empty()
        .map_err(|error| Error::Geometry(error.to_string()))?;
    operation(kernel::placed_solids(&document))
}
impl StepModel {
    pub fn from_step(bytes: &[u8]) -> Result<Self> {
        protected(|| {
            Ok(Self {
                solids: import(bytes)?,
                approximated_contours: 0,
            })
        })
    }
    /// Prepare mechanical board geometry. Callers rebuild planes beforehand.
    /// Via drills are omitted according to upstream's mechanical STEP policy.
    pub fn from_board(
        project: &Project,
        board: &Board,
        variant: Option<AssemblyVariantId>,
    ) -> Result<Self> {
        protected(|| {
            let data = BoardPhysicalGeometry::from_project(project, board, variant)?;
            let body = operation(board::board_brep(&data))?;
            let compressed: Vec<_> = body.solids.iter().map(|solid| solid.compress()).collect();
            let models: save::StepModels<_, _, _> = compressed.iter().collect();
            let text =
                save::CompleteStepDisplay::new(models, save::StepHeaderDescriptor::default())
                    .to_string();
            // Modeling and import geometry use different enums. This common
            // STEP bridge retains BReps, without a triangle approximation.
            let mut solids = import(text.as_bytes())?;
            let material = board::mechanical_material(&data);
            for item in &mut solids {
                item.name = "PCB".into();
                for shell in &mut item.appearance {
                    shell.fill(Some(material));
                }
            }
            let mut cache = BTreeMap::new();
            for device in &data.devices {
                let key = (device.package, device.model);
                if let std::collections::btree_map::Entry::Vacant(entry) = cache.entry(key) {
                    let path = format!("library/pkg/{}/{}.step", device.package, device.model);
                    let bytes = project.directory().read(&path)?;
                    entry.insert(import(&bytes).map_err(|error| Error::ComponentModel {
                        component: device.name.clone(),
                        source: Box::new(error),
                    })?);
                }
                let matrix = board::device_matrix(device, data.thickness.to_mm());
                for item in &cache[&key] {
                    let mut placed = item.clone();
                    kernel::transform_solid(&mut placed.solid, matrix);
                    placed.name.clone_from(&device.name);
                    solids.push(placed);
                }
            }
            Ok(Self {
                solids,
                approximated_contours: body.approximated_contours,
            })
        })
    }
    pub fn solid_count(&self) -> usize {
        self.solids.len()
    }
    pub fn approximated_contours(&self) -> usize {
        self.approximated_contours
    }
    pub fn to_step(&self) -> Result<Vec<u8>> {
        protected(|| Ok(materials::export_step(&self.solids)?.into_bytes()))
    }
    /// Headless tessellation, independent of BRep export. Save OBJ and MTL together.
    pub fn to_obj(&self, tolerance_mm: f64) -> Result<(String, String)> {
        protected(|| {
            let (mesh, groups) = self.tessellate(tolerance_mm)?;
            operation(materials::export_obj(&mesh, &groups))
        })
    }

    /// Tessellate on the CPU without a display or GPU. Counts describe this
    /// kernel's mesh; curved surfaces need not match OpenCascade's subdivision.
    pub fn tessellation_stats(&self, tolerance_mm: f64) -> Result<TessellationStats> {
        protected(|| {
            let (mesh, groups) = self.tessellate(tolerance_mm)?;
            let mut colors = Vec::new();
            for (count, material) in groups {
                let material = material.unwrap_or(materials::DEFAULT_MATERIAL);
                if count > 0 && !colors.contains(&material) {
                    colors.push(material);
                }
            }
            Ok(TessellationStats {
                vertices: mesh.tri_faces().len() * 3,
                colors: colors.len(),
            })
        })
    }

    fn tessellate(&self, tolerance_mm: f64) -> Result<(PolygonMesh, materials::MaterialGroups)> {
        if !tolerance_mm.is_finite() || tolerance_mm <= 0. {
            return Err(Error::Geometry("invalid tessellation tolerance".into()));
        }
        let mut mesh = PolygonMesh::default();
        let mut groups = Vec::new();
        for solid in &self.solids {
            let (part, materials) = operation(kernel::tessellate_solid(solid, tolerance_mm))?;
            mesh.merge(part);
            groups.extend(materials);
        }
        Ok((mesh, groups))
    }
}

/// Low-level probe access; application callers use the guarded `StepModel` API.
#[cfg(feature = "diagnostics")]
pub mod diagnostics {
    pub use crate::kernel::*;
    pub mod board {
        pub use crate::board::*;
    }
    pub mod materials {
        pub use crate::materials::*;
    }
    pub mod metadata {
        pub use crate::metadata::*;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn malformed_and_deep_inputs_return_errors() {
        for bytes in [b"".as_slice(), b"not a STEP file", b"((", b")"] {
            assert!(StepModel::from_step(bytes).is_err());
        }
        assert!(matches!(
            StepModel::from_step(&vec![b'('; 257]),
            Err(Error::InputLimit)
        ));
        assert!(check_input(b"/* ( */ '(' '' 'O''Brien' (())").is_ok());
    }
    #[test]
    fn recoverable_kernel_panics_are_errors() {
        let result: Result<()> = protected(|| panic!("simulated kernel assertion"));
        assert!(matches!(result, Err(Error::KernelPanicked)));
    }
}
