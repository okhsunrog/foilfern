//! Native output-job discovery and configuration through the shared tool registry.

use foilfern_core::job::{
    Board3DOutputJob, GraphicsContentType, GraphicsOutputJob, ObjectSet, OutputJob, OutputJobKind,
    OutputJobList,
};
use foilfern_core::project::{Mutation, Project};
use foilfern_core::types::ElementName;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::error::{ErrorKind, ToolError, ToolResult};
use crate::outcome::ToolOutput;
use crate::session::Session;
use crate::{resolve, tools::write::write};

#[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum JobPreset {
    Step,
    SchematicPdf,
    BoardAssemblyPdf,
    BoardRenderingPng,
}

impl JobPreset {
    fn create(self) -> OutputJob {
        match self {
            Self::Step => OutputJob::new_default::<Board3DOutputJob>(),
            Self::SchematicPdf => GraphicsOutputJob::schematic_pdf(),
            Self::BoardAssemblyPdf => GraphicsOutputJob::board_assembly_pdf(),
            Self::BoardRenderingPng => {
                let mut job = GraphicsOutputJob::board_rendering_pdf();
                job.set_name(ElementName::new("Board Rendering PNG").expect("valid constant"));
                if let OutputJobKind::Graphics(settings) = job.kind_mut() {
                    settings.output_path = "{{PROJECT}}_{{VERSION}}_Rendering.png".into();
                }
                job
            }
        }
    }

    fn matches(self, job: &OutputJob) -> bool {
        match (self, job.kind()) {
            (Self::Step, OutputJobKind::Board3D(_)) => true,
            (preset, OutputJobKind::Graphics(graphics)) => {
                let expected = match preset {
                    Self::SchematicPdf => GraphicsContentType::Schematic,
                    Self::BoardAssemblyPdf => GraphicsContentType::Board,
                    Self::BoardRenderingPng => GraphicsContentType::BoardRendering,
                    Self::Step => return false,
                };
                !graphics.content.is_empty()
                    && graphics
                        .content
                        .iter()
                        .all(|content| content.content_type == expected)
            }
            _ => false,
        }
    }
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct JobsListArgs {
    /// Exact job name or UUID; omit to list all jobs.
    pub job: Option<String>,
    /// Include complete native settings for inspection (default false).
    #[serde(default)]
    pub include_settings: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct JobsConfigureArgs {
    /// Existing job name or UUID. Omit to create/upsert by name.
    pub job: Option<String>,
    /// Required for a new job; asserts the content type when updating.
    pub preset: Option<JobPreset>,
    /// New name; for creation defaults to the preset's native name.
    pub name: Option<String>,
    /// Output filename with native placeholders; keep the appropriate extension.
    pub output_path: Option<String>,
    /// Select one board by name, index or UUID; omitted selections stay intact.
    pub board: Option<String>,
    pub expected_revision: Option<u64>,
}

pub(super) fn resolve_job<'a>(project: &'a Project, key: &str) -> ToolResult<&'a OutputJob> {
    let matches: Vec<_> = project
        .output_jobs()
        .iter()
        .filter(|job| {
            job.name().as_str() == key.trim()
                || resolve::parse_uuid(key).is_some_and(|uuid| uuid == job.uuid())
        })
        .collect();
    match matches.as_slice() {
        [job] => Ok(*job),
        [] => Err(ToolError::not_found(format!(
            "There is no output job {key:?} (see jobs_list)."
        ))),
        _ => Err(ToolError::new(
            ErrorKind::Conflict,
            "Output job name is ambiguous; select its UUID.",
        )),
    }
}

fn output_path(job: &OutputJob) -> Option<&str> {
    match job.kind() {
        OutputJobKind::Board3D(settings) => Some(&settings.output_path),
        OutputJobKind::Graphics(settings) => Some(&settings.output_path),
        _ => None,
    }
}

fn job_json(job: &OutputJob, settings: bool) -> Value {
    let mut value =
        json!({"uuid":job.uuid(),"name":job.name().as_str(),"type":super::job_type(job.kind())});
    if let Some(path) = output_path(job) {
        value["output_path"] = json!(path);
    }
    if settings {
        value["settings"] = json!(job.kind());
    }
    value
}

pub fn list(session: &Session, args: JobsListArgs) -> ToolResult<ToolOutput> {
    let project = session.project()?.project();
    let jobs: Vec<_> = match args.job {
        Some(key) => vec![job_json(resolve_job(project, &key)?, args.include_settings)],
        None => project
            .output_jobs()
            .iter()
            .map(|job| job_json(job, args.include_settings))
            .collect(),
    };
    ToolOutput::new(
        format!("{} output job(s).", jobs.len()),
        json!({"jobs":jobs}),
    )
}

pub fn configure(session: &mut Session, args: JobsConfigureArgs) -> ToolResult<ToolOutput> {
    let project = session.project()?.project();
    super::super::write::check_revision(project, args.expected_revision)?;
    let template = args.preset.map(JobPreset::create);
    let name = args
        .name
        .as_deref()
        .map(ElementName::new)
        .transpose()
        .map_err(|error| ToolError::invalid(format!("Invalid output job name: {error}")))?;
    let existing = if let Some(key) = &args.job {
        Some(resolve_job(project, key)?)
    } else {
        let key = name
            .as_ref()
            .or_else(|| template.as_ref().map(OutputJob::name))
            .ok_or_else(|| {
                ToolError::invalid("Supply an existing job, or a preset for creation.")
            })?;
        let matches: Vec<_> = project
            .output_jobs()
            .iter()
            .filter(|job| job.name() == key)
            .collect();
        match matches.as_slice() {
            [] => None,
            [job] => Some(*job),
            _ => {
                return Err(ToolError::new(
                    ErrorKind::Conflict,
                    "Output job name is ambiguous; select its UUID.",
                ));
            }
        }
    };
    let mut job = existing
        .cloned()
        .or(template)
        .ok_or_else(|| ToolError::invalid("A new output job requires a preset."))?;
    if args.preset.is_some_and(|preset| !preset.matches(&job)) {
        return Err(ToolError::new(
            ErrorKind::Conflict,
            "Existing job content differs from the preset; choose another name or update it without a preset.",
        ));
    }
    if let Some(name) = name {
        job.set_name(name);
    }
    if project
        .output_jobs()
        .iter()
        .any(|other| other.uuid() != job.uuid() && other.name() == job.name())
    {
        return Err(ToolError::new(
            ErrorKind::Conflict,
            "Another output job already has this name.",
        ));
    }
    if let Some(path) = &args.output_path {
        let extension = std::path::Path::new(path)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        let valid = match job.kind() {
            OutputJobKind::Board3D(_) => ["step", "stp"].contains(&extension.as_str()),
            OutputJobKind::Graphics(_) => {
                ["pdf", "svg", "png", "jpg", "jpeg", "bmp"].contains(&extension.as_str())
            }
            _ => false,
        };
        if !valid {
            return Err(ToolError::invalid(
                "Output extension is unsupported for this job type.",
            ));
        }
        match job.kind_mut() {
            OutputJobKind::Board3D(settings) => settings.output_path = path.clone(),
            OutputJobKind::Graphics(settings) => settings.output_path = path.clone(),
            _ => {
                return Err(ToolError::invalid(
                    "Output-path editing is supported for STEP and graphics jobs.",
                ));
            }
        }
    }
    if let Some(key) = &args.board {
        let (_, id, _) = resolve::board(project, Some(key))?;
        match job.kind_mut() {
            OutputJobKind::Board3D(settings) => settings.boards = ObjectSet::custom([id]),
            OutputJobKind::Graphics(settings) => {
                if settings
                    .content
                    .iter()
                    .any(|content| content.content_type == GraphicsContentType::Schematic)
                {
                    return Err(ToolError::invalid(
                        "Board selection does not apply to schematic content.",
                    ));
                }
                for content in &mut settings.content {
                    content.boards = ObjectSet::custom([Some(id)]);
                }
            }
            _ => {
                return Err(ToolError::invalid(
                    "Board selection is supported for STEP and board graphics jobs.",
                ));
            }
        }
    }
    let changed = existing != Some(&job);
    let jobs: OutputJobList = project
        .output_jobs()
        .iter()
        .map(|other| {
            if other.uuid() == job.uuid() {
                job.clone()
            } else {
                other.clone()
            }
        })
        .chain(existing.is_none().then_some(job.clone()))
        .collect();
    let result = job_json(&job, false);
    let done = write(session, "jobs_configure", args.expected_revision, |tx| {
        if changed {
            tx.apply(Mutation::SetOutputJobs(jobs))?;
        }
        Ok(())
    })?;
    done.output(
        session.project()?,
        if changed {
            "Configured output job."
        } else {
            "Output job unchanged."
        },
        json!({"job":result,"changed":changed}),
    )
}
