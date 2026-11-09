//! Autorouting with FreeRouting (<https://freerouting.org>) through the
//! Specctra DSN export and session import.
//!
//! No upstream counterpart as a class: in upstream, the user exports a DSN
//! file ("Export Specctra DSN"), runs FreeRouting manually and imports the
//! session ("Import Specctra SES"). [`FreeroutingRouter`] automates these
//! steps:
//!
//! 1. [`export_specctra_dsn()`] (DSN + [`SpecctraExportManifest`]),
//! 2. FreeRouting in its command line mode as a subprocess
//!    (`-de <dsn> -do <ses> --gui.enabled=false`) in a temporary directory,
//!    with a timeout, optional maximum number of passes and threads;
//!    FreeRouting writes the session only when routing completed, so
//!    success is detected by a non-empty session file,
//! 3. the strict [`ImportSpecctraSession`] (one undo group), which rejects
//!    the session if the project changed meanwhile or the router moved
//!    components.
//!
//! [`FreeroutingRouter::route()`] runs all steps on a [`ProjectEditor`]. For
//! concurrent use (MCP), run the steps separately so the editor lock is not
//! held while FreeRouting runs: export under the read lock,
//! [`FreeroutingRouter::run()`] without lock, import under the write lock
//! (the manifest detects changes in between).
//!
//! The DSN file passed to FreeRouting differs from the export in one
//! point: the resolution is lowered from 1/1000000 mm (upstream's value) to
//! 1/100000 mm ([`dsn_for_freerouting()`]). With upstream's resolution,
//! FreeRouting 2.4.1 reports clearance violations between pads which are far
//! apart and leaves trivial connections unrouted; with 10 nm it routes them.
//! The session import accepts any resolution (coordinates are rounded to
//! 10 nm, within the import's matching tolerance).
//!
//! FreeRouting is located by [`FreeroutingConfig::detect()`]: the jar from
//! `LIBREPCB_FREEROUTING_JAR` (run with the first Java >= 25 of
//! `LIBREPCB_FREEROUTING_JAVA`, `JAVA_HOME` and `PATH`), or a `freerouting`
//! launcher on the `PATH` (installed by `scripts/cloud-setup.sh`).

use std::ffi::OsString;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::{Command as Process, ExitStatus, Stdio};
use std::time::{Duration, Instant};

use librepcb_core::project::board::BoardAirWiresBuilder;
use librepcb_core::project::{BoardId, Project};

use crate::commands::{
    ImportSpecctraSession, SpecctraExportManifest, SpecctraImportResult, export_specctra_dsn,
};
use crate::editor::ProjectEditor;

/// Minimum Java version of the supported FreeRouting releases (2.4.x).
pub const FREEROUTING_MIN_JAVA_VERSION: u32 = 25;

/// Maximum accepted size of a session file (a sanity limit).
const MAX_SESSION_SIZE: u64 = 256 * 1024 * 1024;

/// Number of log lines kept in errors and reports.
const LOG_TAIL_LINES: usize = 40;

/// Errors of the FreeRouting integration.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum FreeroutingError {
    /// FreeRouting or a suitable Java runtime was not found.
    #[error("FreeRouting not found: {0}")]
    NotFound(String),
    /// A file or process operation failed.
    #[error("FreeRouting I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// FreeRouting did not finish within the timeout (the process was
    /// killed).
    #[error("FreeRouting did not finish within {timeout:?}. Output:\n{log}")]
    Timeout {
        /// The timeout.
        timeout: Duration,
        /// The last lines of the output.
        log: String,
    },
    /// FreeRouting failed or did not write a session file.
    #[error("FreeRouting failed ({status}). Output:\n{log}")]
    Failed {
        /// Exit status or reason.
        status: String,
        /// The last lines of the output.
        log: String,
    },
    /// Export or import failed.
    #[error(transparent)]
    Editor(#[from] crate::error::Error),
}

/// How FreeRouting is started.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FreeroutingLauncher {
    /// `java -Djava.awt.headless=true -jar <jar> ...`.
    Jar {
        /// The Java executable.
        java: PathBuf,
        /// The FreeRouting jar.
        jar: PathBuf,
    },
    /// A launcher taking the FreeRouting arguments (e.g. the `freerouting`
    /// wrapper script of `scripts/cloud-setup.sh`).
    Executable(PathBuf),
}

/// Configuration of [`FreeroutingRouter`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FreeroutingConfig {
    /// How to start FreeRouting.
    pub launcher: FreeroutingLauncher,
    /// Maximum run time; the process is killed afterwards (default: 10
    /// minutes).
    pub timeout: Duration,
    /// Maximum number of auto-routing passes (`-mp`, default: FreeRouting's
    /// default).
    pub max_passes: Option<u32>,
    /// Number of optimizer threads (`-mt`, default: 1, which makes the
    /// result reproducible; `None`: FreeRouting's default).
    pub threads: Option<u32>,
    /// Additional command line arguments.
    pub extra_args: Vec<String>,
    /// Directory for the DSN/SES files and the log (default: a temporary
    /// directory which is removed afterwards).
    pub work_dir: Option<PathBuf>,
}

impl FreeroutingConfig {
    /// Creates a configuration with the default options.
    pub fn new(launcher: FreeroutingLauncher) -> Self {
        Self {
            launcher,
            timeout: Duration::from_secs(600),
            max_passes: None,
            threads: Some(1),
            extra_args: Vec::new(),
            work_dir: None,
        }
    }

    /// Locates FreeRouting (see the module documentation).
    pub fn detect() -> Result<Self, FreeroutingError> {
        if let Some(jar) = std::env::var_os("LIBREPCB_FREEROUTING_JAR") {
            let jar = PathBuf::from(jar);
            if !jar.is_file() {
                return Err(FreeroutingError::NotFound(format!(
                    "LIBREPCB_FREEROUTING_JAR does not exist: {}",
                    jar.display()
                )));
            }
            let java = detect_java()?;
            return Ok(Self::new(FreeroutingLauncher::Jar { java, jar }));
        }
        if let Ok(exe) = which::which("freerouting") {
            return Ok(Self::new(FreeroutingLauncher::Executable(exe)));
        }
        Err(FreeroutingError::NotFound(
            "set LIBREPCB_FREEROUTING_JAR to the FreeRouting jar or install a `freerouting` \
             launcher in the PATH (see scripts/cloud-setup.sh)"
                .to_owned(),
        ))
    }
}

/// Returns the major version of a Java runtime (`java -version`).
fn java_version(java: &Path) -> Option<u32> {
    let output = Process::new(java)
        .arg("-version")
        .stdin(Stdio::null())
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&output.stderr).into_owned()
        + &String::from_utf8_lossy(&output.stdout);
    // E.g. `openjdk version "25.0.4.1" 2025-...` or `java version "1.8.0_1"`.
    let version = text
        .lines()
        .find(|l| l.contains(" version \""))?
        .split('"')
        .nth(1)?;
    let mut parts = version.split(['.', '_', '-', '+']);
    let major: u32 = parts.next()?.parse().ok()?;
    if major == 1 {
        parts.next()?.parse().ok()
    } else {
        Some(major)
    }
}

/// Returns the first Java runtime with at least
/// [`FREEROUTING_MIN_JAVA_VERSION`] of `LIBREPCB_FREEROUTING_JAVA`,
/// `JAVA_HOME` and `PATH`.
fn detect_java() -> Result<PathBuf, FreeroutingError> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(java) = std::env::var_os("LIBREPCB_FREEROUTING_JAVA") {
        candidates.push(java.into());
    }
    if let Some(home) = std::env::var_os("JAVA_HOME") {
        candidates.push(Path::new(&home).join("bin").join("java"));
    }
    if let Ok(java) = which::which("java") {
        candidates.push(java);
    }
    let mut found = Vec::new();
    for java in candidates {
        match java_version(&java) {
            Some(v) if v >= FREEROUTING_MIN_JAVA_VERSION => return Ok(java),
            Some(v) => found.push(format!("{} (Java {v})", java.display())),
            None => {}
        }
    }
    Err(FreeroutingError::NotFound(format!(
        "no Java runtime >= {FREEROUTING_MIN_JAVA_VERSION} found (set LIBREPCB_FREEROUTING_JAVA); \
         found: [{}]",
        found.join(", ")
    )))
}

/// Unrouted connections of a board.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RoutingStats {
    /// Number of air wires (unrouted connections).
    pub air_wires: usize,
    /// Number of nets with air wires.
    pub unrouted_nets: usize,
}

/// Returns the unrouted connections of a board (computes the air wires of
/// all nets, without modifying the project).
pub fn routing_stats(project: &Project, board: BoardId) -> RoutingStats {
    let Some(b) = project.board(board) else {
        return RoutingStats::default();
    };
    let builder = BoardAirWiresBuilder::new(b, project.library(), project.circuit());
    let mut stats = RoutingStats::default();
    for net in project.circuit().net_signals().keys() {
        let count = builder.build_air_wires(*net).len();
        if count > 0 {
            stats.air_wires += count;
            stats.unrouted_nets += 1;
        }
    }
    stats
}

/// Result of [`FreeroutingRouter::route()`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FreeroutingReport {
    /// Unrouted connections before routing.
    pub before: RoutingStats,
    /// Unrouted connections after the import.
    pub after: RoutingStats,
    /// The result of the session import.
    pub import: SpecctraImportResult,
    /// Run time of FreeRouting.
    pub elapsed: Duration,
    /// The last lines of FreeRouting's output.
    pub log: String,
}

/// The session written by FreeRouting, see [`FreeroutingRouter::run()`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FreeroutingOutput {
    /// The session file content.
    pub session: String,
    /// Run time.
    pub elapsed: Duration,
    /// The last lines of the output.
    pub log: String,
}

/// Routes boards with FreeRouting, see the module documentation.
#[derive(Debug, Clone)]
pub struct FreeroutingRouter {
    config: FreeroutingConfig,
}

impl FreeroutingRouter {
    /// Creates a router.
    pub fn new(config: FreeroutingConfig) -> Self {
        Self { config }
    }

    /// Creates a router with the detected FreeRouting installation.
    pub fn detect() -> Result<Self, FreeroutingError> {
        FreeroutingConfig::detect().map(Self::new)
    }

    /// Returns the configuration.
    pub fn config(&self) -> &FreeroutingConfig {
        &self.config
    }

    /// Routes a board (default: the primary board) of the editor's project:
    /// export, FreeRouting, strict import as one undo group. Existing
    /// traces are passed to FreeRouting and kept (or improved) by it.
    pub fn route(
        &self,
        editor: &mut ProjectEditor,
        board: Option<BoardId>,
    ) -> Result<FreeroutingReport, FreeroutingError> {
        let export = export_specctra_dsn(editor.project(), board)?;
        let board = export.manifest.board;
        let before = routing_stats(editor.project(), board);
        let output = self.run(&dsn_for_freerouting(&export.dsn))?;
        let import = import_session(editor, output.session, export.manifest)?;
        let after = routing_stats(editor.project(), board);
        Ok(FreeroutingReport {
            before,
            after,
            import,
            elapsed: output.elapsed,
            log: output.log,
        })
    }

    /// Runs FreeRouting on a DSN file (see [`dsn_for_freerouting()`]) and
    /// returns the session.
    pub fn run(&self, dsn: &[u8]) -> Result<FreeroutingOutput, FreeroutingError> {
        let temp;
        let dir = match &self.config.work_dir {
            Some(dir) => {
                std::fs::create_dir_all(dir)?;
                dir.clone()
            }
            None => {
                temp = tempfile::Builder::new()
                    .prefix("librepcb-freerouting-")
                    .tempdir()?;
                temp.path().to_path_buf()
            }
        };
        let dsn_path = dir.join("board.dsn");
        let ses_path = dir.join("board.ses");
        let log_path = dir.join("freerouting.log");
        std::fs::write(&dsn_path, dsn)?;
        if ses_path.exists() {
            std::fs::remove_file(&ses_path)?;
        }

        let mut args: Vec<OsString> = vec![
            "-de".into(),
            dsn_path.clone().into(),
            "-do".into(),
            ses_path.clone().into(),
            "--gui.enabled=false".into(),
        ];
        if let Some(passes) = self.config.max_passes {
            args.push("-mp".into());
            args.push(passes.to_string().into());
        }
        if let Some(threads) = self.config.threads {
            args.push("-mt".into());
            args.push(threads.to_string().into());
        }
        args.extend(self.config.extra_args.iter().map(OsString::from));
        let mut process = match &self.config.launcher {
            FreeroutingLauncher::Jar { java, jar } => {
                let mut p = Process::new(java);
                p.arg("-Djava.awt.headless=true").arg("-jar").arg(jar);
                p
            }
            FreeroutingLauncher::Executable(exe) => Process::new(exe),
        };
        let log_file = File::create(&log_path)?;
        process
            .args(&args)
            .current_dir(&dir)
            .stdin(Stdio::null())
            .stdout(log_file.try_clone()?)
            .stderr(log_file);

        log::info!("Running FreeRouting: {process:?}");
        let start = Instant::now();
        let mut child = process.spawn()?;
        let status: ExitStatus = loop {
            if let Some(status) = child.try_wait()? {
                break status;
            }
            if start.elapsed() > self.config.timeout {
                // Best effort: the process may have exited meanwhile.
                let _ = child.kill();
                let _ = child.wait();
                return Err(FreeroutingError::Timeout {
                    timeout: self.config.timeout,
                    log: log_tail(&log_path),
                });
            }
            std::thread::sleep(Duration::from_millis(100));
        };
        let elapsed = start.elapsed();
        let log = log_tail(&log_path);
        if !status.success() {
            return Err(FreeroutingError::Failed {
                status: status.to_string(),
                log,
            });
        }
        let size = std::fs::metadata(&ses_path).map(|m| m.len()).unwrap_or(0);
        if size == 0 {
            return Err(FreeroutingError::Failed {
                status: "no session file written".to_owned(),
                log,
            });
        }
        if size > MAX_SESSION_SIZE {
            return Err(FreeroutingError::Failed {
                status: format!("session file too large ({size} bytes)"),
                log,
            });
        }
        let session =
            String::from_utf8(std::fs::read(&ses_path)?).map_err(|_| FreeroutingError::Failed {
                status: "session file is not valid UTF-8".to_owned(),
                log: log.clone(),
            })?;
        Ok(FreeroutingOutput {
            session,
            elapsed,
            log,
        })
    }
}

/// The resolution line of the DSN export.
const EXPORT_RESOLUTION: &[u8] = b"\n (resolution mm 1000000)\n";

/// The resolution passed to FreeRouting.
const FREEROUTING_RESOLUTION: &[u8] = b"\n (resolution mm 100000)\n";

/// Returns the DSN file passed to FreeRouting: the export with the
/// resolution lowered to 1/100000 mm (see the module documentation).
pub fn dsn_for_freerouting(dsn: &[u8]) -> Vec<u8> {
    match dsn
        .windows(EXPORT_RESOLUTION.len())
        .position(|w| w == EXPORT_RESOLUTION)
    {
        Some(i) => [
            &dsn[..i],
            FREEROUTING_RESOLUTION,
            &dsn[i + EXPORT_RESOLUTION.len()..],
        ]
        .concat(),
        None => dsn.to_vec(),
    }
}

/// Imports a session strictly (one undo group).
fn import_session(
    editor: &mut ProjectEditor,
    session: String,
    manifest: SpecctraExportManifest,
) -> Result<SpecctraImportResult, FreeroutingError> {
    Ok(editor.execute(ImportSpecctraSession {
        board: Some(manifest.board),
        session,
        manifest: Some(manifest),
    })?)
}

/// Returns the last lines of a log file.
fn log_tail(path: &Path) -> String {
    let content = std::fs::read(path).unwrap_or_default();
    let content = String::from_utf8_lossy(&content);
    let lines: Vec<&str> = content.lines().collect();
    let start = lines.len().saturating_sub(LOG_TAIL_LINES);
    lines[start..].join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolution_for_freerouting() {
        let dsn = b"(pcb x\n (parser\n )\n (resolution mm 1000000)\n (unit mm)\n)\n";
        assert_eq!(
            dsn_for_freerouting(dsn),
            b"(pcb x\n (parser\n )\n (resolution mm 100000)\n (unit mm)\n)\n"
        );
        assert_eq!(dsn_for_freerouting(b"(pcb x)"), b"(pcb x)");
    }

    #[test]
    fn log_tail_of_missing_file() {
        assert_eq!(log_tail(Path::new("/nonexistent/freerouting.log")), "");
    }

    #[test]
    fn missing_launcher_fails() {
        let router = FreeroutingRouter::new(FreeroutingConfig::new(
            FreeroutingLauncher::Executable("/nonexistent/freerouting".into()),
        ));
        assert!(matches!(
            router.run(b"(pcb x)"),
            Err(FreeroutingError::Io(_))
        ));
    }
}
