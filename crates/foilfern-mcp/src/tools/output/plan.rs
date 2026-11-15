//! Read-only output review: native selections/substitution and shared file naming.
use crate::error::{ToolError, ToolResult};
use foilfern_core::fileio::FilePath;
use foilfern_core::job::{OutputJob, OutputJobKind};
use foilfern_core::project::{GraphicsPageContent, Project, plan_graphics_job, plan_step_job};
use serde_json::{Value, json};
use std::collections::BTreeMap;

fn resolved(project: &Project, job: &OutputJob, directory: &FilePath) -> ToolResult<Value> {
    let files = match job.kind() {
        OutputJobKind::Graphics(settings) => {
            let plan = plan_graphics_job(project, settings).map_err(|error| ToolError::invalid(error.to_string()))?;
            let base = directory.path_to(&plan.output_path);
            let names = foilfern_graphics::GraphicsExport::output_file_names(plan.pages.len(), base.as_str())
                .map_err(|error| ToolError::invalid(error.to_string()))?;
            let pdf = base.suffix().eq_ignore_ascii_case("pdf");
            let pages = plan.pages.iter().enumerate().map(|(index,page)| {
                let mut value = json!({"number":index+1,"board":page.board_context,"assembly_variant":page.assembly_variant,
                    "mirrored":page.settings.mirror,"rotate_90":page.settings.rotate});
                match page.content {
                    GraphicsPageContent::Schematic(id) => {
                        value["kind"] = json!("schematic"); value["schematic"] = json!(id);
                        value["name"] = json!(project.schematic(id).map(|s|s.name().as_str()));
                    }
                    GraphicsPageContent::Board(id) | GraphicsPageContent::BoardRendering(id) => {
                        value["kind"] = json!(if matches!(page.content,GraphicsPageContent::BoardRendering(_)) {"board_rendering"} else {"board_drawing"});
                        value["board"] = json!(id);
                        value["name"] = json!(project.board(id).map(|b|b.name().as_str()));
                        value["view_from"] = json!(if page.settings.mirror {"bottom"} else {"top"});
                        value["layer_selection"] = json!("job_settings");
                    }
                }
                value
            }).collect::<Vec<_>>();
            names.into_iter().enumerate().map(|(index,path)| json!({"path":FilePath::new(&path).map_or(path.clone(), |file| file.to_native()),"pages":if pdf {pages.clone()} else {vec![pages[index].clone()]}})).collect::<Vec<_>>()
        }
        OutputJobKind::Board3D(settings) => plan_step_job(project, settings).map_err(|error| ToolError::invalid(error.to_string()))?.into_iter().map(|file| {
            json!({"path":directory.path_to(&file.output_path).to_native(),"kind":"step","board":file.board,
                "board_name":project.board(file.board).map(|b|b.name().as_str()),"assembly_variant":file.assembly_variant,"pages":[]})
        }).collect(),
        _ => return Ok(json!({"complete":false,"job":job.uuid(),"files":[],"reason":"planning_not_supported_for_job_type","type":super::job_type(job.kind())})),
    };
    Ok(
        json!({"complete":true,"job":job.uuid(),"output_dir":directory.to_native(),"output_count":files.len(),"files":files}),
    )
}

pub(super) fn job(project: &Project, job: &OutputJob, directory: Option<&FilePath>) -> Value {
    let default = project.current_output_dir();
    let result = directory
        .or(default.as_ref())
        .ok_or_else(|| ToolError::invalid("Project has no output directory."))
        .and_then(|directory| resolved(project, job, directory));
    result.unwrap_or_else(
        |error| json!({"complete":false,"job":job.uuid(),"files":[],"error":error.to_string()}),
    )
}

pub(super) fn batch(project: &Project, jobs: &[OutputJob], directory: Option<&FilePath>) -> Value {
    let plans = jobs
        .iter()
        .map(|item| job(project, item, directory))
        .collect::<Vec<_>>();
    let mut paths = BTreeMap::<String, Vec<Value>>::new();
    for plan in &plans {
        if let Some(files) = plan["files"].as_array() {
            for file in files {
                if let Some(path) = file["path"].as_str() {
                    paths
                        .entry(path.into())
                        .or_default()
                        .push(plan["job"].clone());
                }
            }
        }
    }
    let conflicts = paths
        .iter()
        .filter(|(_, owners)| owners.len() > 1)
        .map(|(path, owners)| json!({"path":path,"jobs":owners}))
        .collect::<Vec<_>>();
    json!({"complete":plans.iter().all(|plan| plan["complete"]==true),"output_count":paths.len(),
        "files":paths.keys().collect::<Vec<_>>(),"jobs":plans,"conflicts":conflicts,"writes":false})
}
