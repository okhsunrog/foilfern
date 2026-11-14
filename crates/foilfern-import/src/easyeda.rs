//! Native EasyEDA supplier-part conversion (no LibrePCB upstream counterpart).
//!
//! Existing Rust converters were inspected before this adapter was introduced:
//! easyeda2kicad-rs 1.1.2 snaps symbol coordinates, recenters footprints, invents
//! pin-1 graphics and omits some primitives. Those transformations are unsuitable
//! for a faithful source import. This adapter retains source geometry and uses
//! svgtypes/kurbo for SVG paths and the existing KiCad-to-native converter for
//! native library objects, rather than duplicating its mapping/serialization.

use std::collections::BTreeMap;
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

use chrono::{DateTime, Utc};
use foilfern_core::attribute::{Attribute, AttributeKey, AttributeList, AttributeType};
use foilfern_core::fileio::{
    FilePath, FileSystem, TransactionalDirectory, TransactionalFileSystem,
};
use foilfern_core::library::LibraryBaseElement;
use foilfern_core::library::{
    cmp::Component,
    dev::{Device, Part},
    pkg::Package,
    sym::Symbol,
};
use foilfern_core::serialization::{Mode, SExpression};
use foilfern_core::types::{Length, Point, SimpleString};
use foilfern_core::utils::message_logger::{LogLevel, MessageLogger};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::UuidGenerator;
use crate::kicad::types::{KiCadFootprint, KiCadSymbol};
use crate::kicad::{KiCadLibraryConverter, KiCadLibraryConverterSettings, NoImportedElements};

mod model;
mod shapes;
pub use model::{ModelReference, model_reference};

/// Invalid or unsupported source; conversion never substitutes zero geometry.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// STEP parsing/geometry validation failed.
    #[error(transparent)]
    Cad(#[from] foilfern_step::Error),
    /// Incomplete, non-finite or unsupported source geometry/mapping.
    #[error("EasyEDA import: {0}")]
    Source(String),
    /// Invalid generated intermediate structure.
    #[error(transparent)]
    Parse(#[from] foilfern_core::serialization::Error),
    /// Native conversion error.
    #[error(transparent)]
    Convert(#[from] crate::kicad::Error),
    /// Native library/file error.
    #[error(transparent)]
    Library(#[from] foilfern_core::library::Error),
    /// Native type validation error.
    #[error(transparent)]
    Type(#[from] foilfern_core::types::Error),
    /// Filesystem error.
    #[error(transparent)]
    File(#[from] foilfern_core::fileio::Error),
    /// Source serialization error.
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

/// Result of a synchronous conversion, ready for review or atomic installation.
#[derive(Debug)]
pub struct NativePart {
    /// Native schematic symbol.
    pub symbol: Symbol,
    /// Native circuit component with its pin mapping.
    pub component: Component,
    /// Native footprint package.
    pub package: Package,
    /// Native device with exact manufacturer/supplier part information.
    pub device: Device,
    /// Explicitly omitted non-electrical source artwork or conversion messages.
    pub warnings: Vec<String>,
}

fn text<'a>(value: &'a Value, key: &str) -> Result<&'a str, Error> {
    value[key]
        .as_str()
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| Error::Source(format!("Missing text field {key:?}.")))
}

fn quote(value: &str) -> Result<String, Error> {
    Ok(SExpression::string(value)
        .to_string_with_mode(Mode::Permissive)?
        .trim_end()
        .to_owned())
}

fn stable_uuids(lcsc: &str) -> UuidGenerator {
    let namespace = format!("FoilfernEasyEDA/v1/{lcsc}/");
    let counter = AtomicU64::new(0);
    UuidGenerator::from_fn(move || {
        let next = counter.fetch_add(1, Ordering::Relaxed);
        let mut digest = Sha256::digest(format!("{namespace}{next}"));
        // Native files require a v4-shaped UUID. These stable imported IDs use
        // SHA-256 entropy and explicitly set the required version/variant bits.
        digest[6] = (digest[6] & 0x0f) | 0x40;
        digest[8] = (digest[8] & 0x3f) | 0x80;
        let hex: String = digest[..16].iter().map(|b| format!("{b:02x}")).collect();
        let uuid = format!(
            "{}-{}-{}-{}-{}",
            &hex[..8],
            &hex[8..12],
            &hex[12..16],
            &hex[16..20],
            &hex[20..]
        );
        // The formatted hash has fixed hexadecimal lengths and v4/DCE bits.
        uuid.parse().expect("hash formatted as native UUID")
    })
}

/// Converts a validated provider result (the response's result object).
/// `sha256` is the original source snapshot hash, not a hash of normalized JSON.
/// No workspace or project is modified by this function.
pub fn convert(raw: &Value, lcsc: &str, sha256: &str) -> Result<NativePart, Error> {
    if lcsc.len() < 2
        || !lcsc.starts_with('C')
        || !lcsc[1..].bytes().all(|b| b.is_ascii_digit())
        || sha256.len() != 64
        || !sha256.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err(Error::Source("Invalid source identity/hash.".into()));
    }
    let parameters = &raw["dataStr"]["head"]["c_para"];
    if text(parameters, "Supplier Part")? != lcsc {
        return Err(Error::Source(
            "Supplier part does not match the source identity.".into(),
        ));
    }
    let mpn = text(parameters, "Manufacturer Part")?;
    let manufacturer = parameters["Manufacturer"].as_str().unwrap_or_default();
    if raw["subparts"]
        .as_array()
        .is_some_and(|parts| !parts.is_empty())
    {
        return Err(Error::Source(
            "Multi-unit source symbols require explicit unit mapping.".into(),
        ));
    }
    let name = format!("{lcsc}_{mpn}");
    let log = MessageLogger::new();
    let mut warnings = Vec::new();
    let (symbol_text, footprint_text, pin_numbers, pad_numbers) =
        shapes::convert(raw, &name, &mut warnings)?;
    if pin_numbers.is_empty() || pin_numbers != pad_numbers {
        return Err(Error::Source(format!(
            "Pin/pad mapping is incomplete: symbol-only {:?}; footprint-only {:?}.",
            pin_numbers.difference(&pad_numbers).collect::<Vec<_>>(),
            pad_numbers.difference(&pin_numbers).collect::<Vec<_>>()
        )));
    }
    let symbol_node = SExpression::parse(symbol_text.as_bytes(), None, Mode::Permissive)?;
    let footprint_node = SExpression::parse(footprint_text.as_bytes(), None, Mode::Permissive)?;
    let ki_symbol = KiCadSymbol::parse(&symbol_node, &log)?;
    let ki_footprint = KiCadFootprint::parse(&footprint_node, &log)?;
    let settings = KiCadLibraryConverterSettings {
        uuids: stable_uuids(lcsc),
        author: "EasyEDA Import".into(),
        created: raw["updateTime"]
            .as_i64()
            .and_then(DateTime::<Utc>::from_timestamp_millis)
            .unwrap_or(DateTime::<Utc>::UNIX_EPOCH),
        keywords: format!("easyeda,lcsc,{lcsc},{mpn}"),
        ..Default::default()
    };
    let mut converter = KiCadLibraryConverter::new(&NoImportedElements, settings);
    let source_path = FilePath::new(
        std::env::current_dir()
            .map_err(|e| Error::Source(e.to_string()))?
            .join("EasyEDA"),
    )
    .ok_or_else(|| Error::Source("Invalid source context path.".into()))?;
    let key = |kind| format!("FoilfernEasyEDA::{lcsc}::{kind}");
    let package = converter.create_package(
        &source_path,
        &ki_footprint,
        &key("package"),
        &BTreeMap::new(),
        &log,
    )?;
    let [gate] = ki_symbol.gates.as_slice() else {
        return Err(Error::Source("Expected one source symbol unit.".into()));
    };
    let mut symbol =
        converter.create_symbol(&source_path, &ki_symbol, gate, &key("symbol"), &log)?;
    // EasyEDA's fetched library symbols often omit instance reference/value
    // positions. Place our generated native fields above the actual geometry,
    // rather than leaving both placeholders on top of each other at the origin.
    let mut positions = symbol
        .pins()
        .iter()
        .map(|p| p.position())
        .collect::<Vec<_>>();
    positions.extend(
        symbol
            .polygons()
            .iter()
            .flat_map(|p| p.path().vertices().iter().map(|v| v.pos)),
    );
    positions.extend(
        symbol
            .circles()
            .iter()
            .map(|c| Point::new(c.center().x, c.center().y + *c.diameter() / 2)),
    );
    let top = positions
        .iter()
        .map(|p| p.y)
        .max()
        .unwrap_or(Length::new(0));
    let left = positions
        .iter()
        .map(|p| p.x)
        .min()
        .unwrap_or(Length::new(0));
    let right = positions
        .iter()
        .map(|p| p.x)
        .max()
        .unwrap_or(Length::new(0));
    let center = left + (right - left) / 2;
    for text in symbol.texts_mut().iter_mut() {
        let offset = match text.text().as_str() {
            "{{NAME}}" => 3_810_000,
            "{{VALUE}}" => 1_905_000,
            _ => continue,
        };
        text.set_position(Point::new(center, top + Length::new(offset)));
    }
    let component = converter.create_component(
        &source_path,
        &ki_symbol,
        &ki_symbol.gates,
        &key("component"),
        &[key("symbol")],
        &log,
    )?;
    let mut device = converter.create_device(
        &source_path,
        &ki_symbol,
        &ki_symbol.gates,
        &key("device"),
        &key("component"),
        &key("package"),
        &log,
    )?;
    if device
        .pad_signal_map()
        .iter()
        .any(|pad| pad.signal_uuid().is_none())
    {
        return Err(Error::Source(
            "Native conversion lost a pad-to-signal mapping.".into(),
        ));
    }
    let mut attributes = AttributeList::new();
    for (name, value) in [("LCSC", lcsc), ("SOURCE_SHA256", sha256)] {
        attributes.push(
            Attribute::new(AttributeKey::new(name)?, AttributeType::String, value, None)
                .map_err(|e| Error::Source(e.to_string()))?,
        );
    }
    device.parts_mut().push(Part::new(
        SimpleString::new(mpn)?,
        SimpleString::new(manufacturer)?,
        attributes,
    ));
    device
        .directory_mut()
        .write("easyeda-source.json", &serde_json::to_vec(raw)?)?;
    for message in log.messages() {
        if message.level == LogLevel::Critical {
            return Err(Error::Source(message.message));
        }
        if message.level == LogLevel::Warning {
            warnings.push(message.message);
        }
    }
    Ok(NativePart {
        symbol,
        component,
        package,
        device,
        warnings,
    })
}

impl NativePart {
    /// Writes all four objects through one native filesystem transaction.
    /// Existing element directories are refused to preserve manual edits.
    pub fn install(&mut self, library: &FilePath) -> Result<(), Error> {
        self.install_with_library(library, None)
    }

    /// Optionally creates the containing library in the same transaction as
    /// the four elements. A supplied library never replaces existing metadata.
    pub fn install_with_library(
        &mut self,
        library: &FilePath,
        mut create: Option<foilfern_core::library::Library>,
    ) -> Result<(), Error> {
        let fs = Arc::new(TransactionalFileSystem::open_rw(library)?);
        let mut directory = TransactionalDirectory::new(Arc::clone(&fs), "");
        if let Some(new) = create.as_mut() {
            // open_rw acquired this transaction's lock before inspecting the
            // root; every other file/directory still means user-owned data.
            if directory.files("").iter().any(|name| name != ".lock")
                || !directory.dirs("").is_empty()
            {
                return Err(Error::Source(
                    "Refusing to replace a nonempty library directory.".into(),
                ));
            }
            new.save_to(&mut directory)?;
        }
        let identities = [
            ("sym", self.symbol.metadata().uuid()),
            ("cmp", self.component.metadata().uuid()),
            ("pkg", self.package.metadata().uuid()),
            ("dev", self.device.metadata().uuid()),
        ];
        for (kind, uuid) in identities {
            if !directory
                .dirs(kind)
                .iter()
                .all(|name| name != &uuid.to_string())
            {
                return Err(Error::Source(format!(
                    "Refusing to overwrite existing {kind}/{uuid}."
                )));
            }
        }
        self.symbol
            .save_to(&mut directory.subdir(&format!("sym/{}", self.symbol.metadata().uuid())))?;
        self.component
            .save_to(&mut directory.subdir(&format!("cmp/{}", self.component.metadata().uuid())))?;
        self.package
            .save_to(&mut directory.subdir(&format!("pkg/{}", self.package.metadata().uuid())))?;
        self.device
            .save_to(&mut directory.subdir(&format!("dev/{}", self.device.metadata().uuid())))?;
        fs.save()?;
        Ok(())
    }
}
