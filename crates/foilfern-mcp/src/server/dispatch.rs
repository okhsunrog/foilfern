//! Shared in-process dispatch. No MCP request context or peer is required.

use super::*;

macro_rules! invoke {
    ($server:ident, $arguments:ident, $name:ident, $method:ident) => {
        $server.$method().await
    };
    ($server:ident, $arguments:ident, $name:ident, $method:ident, $args:ty) => {
        match serde_json::from_value::<$args>(serde_json::Value::Object($arguments)) {
            Ok(args) => $server.$method(Parameters(args)).await,
            Err(error) => {
                $server
                    .fail(ToolError::invalid(format!(
                        "Invalid arguments for tool {}: {error}",
                        $name
                    )))
                    .await
            }
        }
    };
}

macro_rules! dispatcher {
    ($( $method:ident $( : $args:ty )? ),* $(,)?) => {
        impl FoilfernMcp {
            /// Execute an operation with identical validation for MCP and CLI.
            pub async fn dispatch(&self, name: &str, arguments: JsonObject)
                -> Result<CallToolResult, McpError>
            {
                if let Err(error) = self.check_arguments(name, Some(&arguments)) {
                    return self.fail(error).await;
                }
                CURRENT_TOOL.scope(name.to_owned(), async {
                    match name {
                        $(stringify!($method) => invoke!(self, arguments, name, $method $(, $args)?),)*
                        _ => Err(McpError::invalid_params(format!("Unknown tool {name}"), None)),
                    }
                }).await
            }
        }
    };
}

dispatcher! {
    server_info,
    workspace_open: WorkspacePathArgs,
    workspace_create: WorkspacePathArgs,
    project_create: ProjectCreateArgs,
    project_open: ProjectOpenArgs,
    project_save: ProjectSaveArgs,
    project_close: ProjectCloseArgs,
    project_summary,
    undo: UndoArgs,
    redo: UndoArgs,
    history,
    library_list,
    library_rescan,
    library_install: LibraryInstallArgs,
    library_import: LibraryImportArgs,
    part_import: PartImportArgs,
    library_search: LibrarySearchArgs,
    library_element: LibraryElementArgs,
    component_list,
    component_get: ComponentGetArgs,
    component_add: ComponentAddArgs,
    component_remove: ComponentRemoveArgs,
    component_update: ComponentUpdateArgs,
    symbol_move: SymbolMoveArgs,
    symbol_text_move: schematic_edit::SymbolTextMoveArgs,
    connect: ConnectArgs,
    disconnect: DisconnectArgs,
    net_rename: NetRenameArgs,
    net_class_set: NetClassSetArgs,
    net_list: NetListArgs,
    netlist,
    schematic_list,
    schematic_get: SchematicGetArgs,
    schematic_review: SchematicReviewArgs,
    schematic_add: SchematicAddArgs,
    schematic_tidy: SchematicTidyArgs,
    board_get: BoardGetArgs,
    board_add: BoardAddArgs,
    board_set_outline: BoardSetOutlineArgs,
    board_polygon_add: board_edit::BoardPolygonAddArgs,
    board_legend_list: board_edit::BoardLegendArgs,
    board_text_move: board_edit::BoardTextMoveArgs,
    device_place: DevicePlaceArgs,
    device_auto_place: DeviceAutoPlaceArgs,
    trace_add: TraceAddArgs,
    trace_update: TraceUpdateArgs,
    via_add: ViaAddArgs,
    trace_remove: TraceRemoveArgs,
    plane_add: PlaneAddArgs,
    plane_set_outline: board_edit::PlaneSetOutlineArgs,
    design_rules_get: BoardArgs,
    design_rules_preview: DesignRulesSetArgs,
    design_rules_set: DesignRulesSetArgs,
    planes_rebuild: BoardArgs,
    plane_diagnose: board_edit::PlaneDiagnoseArgs,
    plane_stitch_prepare: crate::tools::routing::PlaneStitchPrepareArgs,
    unrouted: BoardArgs,
    autoroute: AutorouteArgs,
    routing_prepare: crate::tools::routing::RoutingPrepareArgs,
    routing_staged_prepare: crate::tools::routing::RoutingStagedPrepareArgs,
    routing_copper_prepare: crate::tools::routing::RoutingCopperArgs,
    routing_placement_compare: crate::tools::routing::RoutingPlacementArgs,
    routing_placement_start: crate::tools::routing::RoutingPlacementArgs,
    routing_placement_status: crate::tools::routing::placement::PlacementStatusArgs,
    reference_coverage: crate::tools::ground_reference::ReferenceCoverageArgs,
    routing_compare: crate::tools::routing::RoutingCompareArgs,
    routing_region_prepare: crate::tools::routing::RoutingRegionPrepareArgs,
    routing_repair_start: crate::tools::routing::repair_search::RepairSearchArgs,
    routing_repair_status: crate::tools::routing::repair_search::RepairStatusArgs,
    routing_edit_prepare: crate::tools::routing::RoutingEditArgs,
    routing_via_prepare: crate::tools::routing::RoutingViaPrepareArgs,
    trace_path_measure: crate::tools::trace_path::TracePathArgs,
    routing_diagnose: crate::tools::routing::RoutingDiagnoseArgs,
    routing_preview: crate::tools::routing::RoutingPreviewArgs,
    routing_apply: crate::tools::routing::RoutingCandidateArgs,
    routing_discard: crate::tools::routing::RoutingCandidateArgs,
    specctra_export: SpecctraExportArgs,
    specctra_import: SpecctraImportArgs,
    erc_run: ErcArgs,
    drc_run: DrcArgs,
    export_fabrication: ExportFabricationArgs,
    export_bom: ExportBomArgs,
    export_pick_place: ExportPickPlaceArgs,
    export_netlist: ExportNetlistArgs,
    jobs_list: output::JobsListArgs,
    jobs_configure: output::JobsConfigureArgs,
    jobs_run: JobsRunArgs,
    render: RenderArgs,
    design_preview: crate::tools::design::DesignDocument,
    design_preview_status,
    design_preview_cancel: crate::tools::design::CancelArgs,
    design_apply: crate::tools::design::DesignApplyArgs,
    design_discard: crate::tools::design::DesignPreviewArgs,
    requirements_get,
    requirements_set: crate::tools::requirements::SetArgs,
    requirements_check,
    board_requirements_get,
    board_requirements_set: crate::tools::board_requirements::SetArgs,
    board_requirements_check,
    mutation_apply: MutationApplyArgs,
    mutation_schema: MutationSchemaArgs,
}
