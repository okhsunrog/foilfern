//! Native STEP attachment using the source's explicit 3D placement.

use foilfern_core::fileio::{FilePath, FileSystem, file_utils};
use foilfern_core::library::LibraryBaseElement;
use foilfern_core::library::pkg::PackageModel;
use foilfern_core::types::{Angle, ElementName, Length};
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::{Error, NativePart, stable_uuids};

/// Explicit source model identity and placement in native board coordinates.
#[derive(Debug, Clone, Serialize)]
pub struct ModelReference {
    /// Source asset UUID, not the native model UUID.
    pub provider_uuid: String,
    /// Source model title.
    pub name: String,
    /// Translation in millimetres relative to the unchanged footprint origin.
    pub position_mm: [f64; 3],
    /// Source Euler rotation in degrees.
    pub rotation_deg: [f64; 3],
}

fn number(value: &str) -> Result<f64, Error> {
    value
        .trim()
        .parse::<f64>()
        .ok()
        .filter(|n| n.is_finite() && n.abs() < 1e10)
        .ok_or_else(|| Error::Source(format!("Invalid model coordinate {value:?}.")))
}
fn scalar(value: &Value) -> Result<f64, Error> {
    match value.as_str() {
        Some(text) => number(text),
        None => value
            .as_f64()
            .filter(|n| n.is_finite() && n.abs() < 1e10)
            .ok_or_else(|| Error::Source("Missing model origin coordinate.".into())),
    }
}
fn vector<const N: usize>(value: &str) -> Result<[f64; N], Error> {
    let values = value
        .split(',')
        .map(number)
        .collect::<Result<Vec<_>, _>>()?;
    values
        .try_into()
        .map_err(|_| Error::Source("Invalid model coordinate vector.".into()))
}

/// Extracts one source model reference. Multiple independently placed assets
/// cannot be represented by a native footprint's shared transform, so are refused.
pub fn model_reference(raw: &Value) -> Result<Option<ModelReference>, Error> {
    let data = &raw["packageDetail"]["dataStr"];
    let mut found = None;
    for shape in data["shape"].as_array().into_iter().flatten() {
        let Some(shape) = shape.as_str().and_then(|s| s.strip_prefix("SVGNODE~")) else {
            continue;
        };
        let node: Value = serde_json::from_str(shape)?;
        let attributes = &node["attrs"];
        let Some(uuid) = attributes["uuid"].as_str() else {
            continue;
        };
        if found.is_some() {
            return Err(Error::Source(
                "Multiple independently placed source models are unsupported.".into(),
            ));
        }
        let uuid = uuid.to_ascii_lowercase();
        if uuid.len() != 32 || !uuid.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(Error::Source("Invalid source model UUID.".into()));
        }
        let [x, y] = vector::<2>(
            attributes["c_origin"]
                .as_str()
                .ok_or_else(|| Error::Source("Source model has no placement origin.".into()))?,
        )?;
        let origin = [
            scalar(&data["head"]["x"])?,
            scalar(&data["head"]["y"])?,
            0.0,
        ];
        let rotation = vector::<3>(attributes["c_rotation"].as_str().unwrap_or("0,0,0"))?;
        let z = match attributes.get("z") {
            None => 0.0,
            Some(Value::String(value)) => {
                if let Some(value) = value.strip_suffix("mm") {
                    number(value)?
                } else if let Some(value) = value.strip_suffix("mil") {
                    number(value)? * 0.0254
                } else {
                    number(value)? * 0.0254
                }
            }
            Some(value) => scalar(value)? * 0.0254,
        };
        found = Some(ModelReference {
            provider_uuid: uuid.clone(),
            name: attributes["title"].as_str().unwrap_or(&uuid).to_owned(),
            position_mm: [(x - origin[0]) * 0.254, (origin[1] - y) * 0.254, z],
            rotation_deg: rotation,
        });
    }
    Ok(found)
}

impl NativePart {
    /// Checks the asset checksum and STEP geometry before attaching it. Raw STEP
    /// bytes are retained; the source model pose is applied without recentering.
    pub fn attach_step(
        &mut self,
        lcsc: &str,
        reference: &ModelReference,
        file: &FilePath,
        expected_sha256: &str,
    ) -> Result<(), Error> {
        let size = std::fs::metadata(file.as_path())
            .map_err(|e| Error::Source(e.to_string()))?
            .len();
        if size > 128 * 1024 * 1024 {
            return Err(Error::Source(
                "STEP asset exceeds the 128 MiB limit.".into(),
            ));
        }
        let bytes = file_utils::read_file(file)?;
        let hash: String = Sha256::digest(&bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        if hash != expected_sha256 {
            return Err(Error::Source("STEP asset checksum mismatch.".into()));
        }
        let model = foilfern_step::StepModel::from_step(&bytes)?;
        if model.solid_count() == 0 {
            return Err(Error::Source("STEP asset contains no solids.".into()));
        }
        let [x, y, z] = reference.position_mm;
        let position = (
            Length::from_mm(x)?,
            Length::from_mm(y)?,
            Length::from_mm(z)?,
        );
        let [x, y, z] = reference.rotation_deg;
        let rotation = (
            Angle::from_deg(x)?,
            Angle::from_deg(y)?,
            Angle::from_deg(z)?,
        );
        let uuid = stable_uuids(&format!("{lcsc}/model/{}", reference.provider_uuid)).generate();
        if self.package.models().by_uuid(&uuid).is_some() {
            return Err(Error::Source("Model is already attached.".into()));
        }
        let mut name = ElementName::clean(&reference.name);
        if name.is_empty() {
            name = reference.provider_uuid.clone();
        }
        let package_model = PackageModel::new(uuid, ElementName::new(name)?);
        let footprint = self.package.footprints_mut().get_mut(0).ok_or_else(|| {
            Error::Source("Package has no footprint for model attachment.".into())
        })?;
        let mut models = footprint.models().clone();
        models.insert(uuid);
        footprint.set_model_position(position);
        footprint.set_model_rotation(rotation);
        footprint.set_models(models);
        self.package
            .directory_mut()
            .write(&package_model.file_name(), &bytes)?;
        self.package.directory_mut().write(
            &format!("easyeda-model-{uuid}.json"),
            &serde_json::to_vec(&serde_json::json!({"source":reference,"sha256":hash}))?,
        )?;
        self.package.models_mut().push(package_model);
        self.warnings
            .retain(|warning| !warning.contains("no STEP asset has been attached"));
        Ok(())
    }
}
