//! LibrePCB domain model (port of libs/librepcb/core).
//!
//! Modules mirror the upstream directory layout:
//!
//! - [`application`]: the file format version.
//! - [`types`]: basic value types (lengths, angles, points, UUIDs, versions,
//!   validated string newtypes, the layer registry, ...).
//! - [`serialization`]: the S-expression file format and the traits used to
//!   (de)serialize objects from/to it.
//! - [`geometry`]: geometric primitives (paths, polygons, pads, texts,
//!   traces, vias, ...).
//! - [`font`]: stroke fonts used to render stroke texts.
//! - [`attribute`]: attributes of library elements and projects, and the
//!   `{{KEY}}` substitution in texts.
//! - [`algorithm`]: air wire calculation and net segment simplification.
//! - [`fileio`]: file paths, file utilities, the transactional file system,
//!   directory locks, ZIP and CSV files.
//! - [`export`]: generators of manufacturing data (Gerber, Excellon,
//!   IPC-D-356A, BOM and pick&place CSV).
//! - [`system_info`]: user, host and process information.
//! - [`sqlite_database`]: thin wrapper around SQLite (for the library index).
//! - [`rule_check`]: messages of rule checks (library element check, ...).
//! - [`library`]: libraries and library elements (categories, symbols,
//!   packages, components, devices, organizations) and their checks.
//! - [`job`]: output jobs (the production data a project generates).
//! - [`project`]: projects (metadata, project library, circuit, schematics,
//!   boards), their mutations, change journal, loading and saving.
//! - [`utils`]: helpers from `core/utils` needed by the above.
//! - [`workspace`]: workspaces, workspace settings and the library index
//!   (SQLite) with the library scanner.

pub mod algorithm;
pub mod application;
pub mod attribute;
pub mod export;
pub mod fileio;
pub mod font;
pub mod geometry;
pub mod import;
pub mod job;
pub mod library;
pub mod project;
pub mod rule_check;
pub mod serialization;
pub mod sqlite_database;
pub mod system_info;
pub mod types;
pub mod utils;
pub mod workspace;
