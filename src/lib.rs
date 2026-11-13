mod drafting;
mod survey;

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};

use drafting::{
    ensure_property_line_layer, extract_polyline, extract_polyline_for_pick, make_property_polyline,
    read_table, table_record, XDATA_APP,
};
use ocs_plugin_api::dialog::{DialogControl, DialogDef, DialogResponse, DialogRow, DialogValue};
use ocs_plugin_api::host::acadrust::LwPolyline;
use ocs_plugin_api::host::{
    BuiltinPlugin, CommandStep, EntityType, Handle, HostApi, InteractiveCommand, PreviewWire,
};
use ocs_plugin_api::manifest::{ApiVersion, PluginManifest};
use ocs_plugin_api::ribbon::{CadModule, IconKind, ModuleEvent, RibbonGroup, RibbonItem, ToolDef};

const TABLE_HEADER: &str = "Distance,N/S,Bearing,E/W,Type,Radius,L/R
";
const DIALOG_TABLE_ID: &str = "segments";
const DIALOG_CLOSE_ID: &str = "close";
const DIALOG_WIDTH: f32 = 860.0;

static MANIFEST: PluginManifest = PluginManifest {
    id: "opencad.property_lines",
    name: "Property Lines",
    version: env!("CARGO_PKG_VERSION"),
    description: "Create, sketch, tag, and schedule parametric property lines",
    api_version: ApiVersion::CURRENT,
    ribbon_order: 60,
    xdata_apps: &[XDATA_APP],
    command_prefixes: &["PL_"],
};

struct PropertyLinesModule;

impl CadModule for PropertyLinesModule {
    fn id(&self) -> &'static str {
        "property_lines"
    }
    fn title(&self) -> &'static str {
        "Property Lines"
    }
    fn ribbon_groups(&self) -> &[RibbonGroup] {
        static GROUPS: OnceLock<Vec<RibbonGroup>> = OnceLock::new();
        GROUPS.get_or_init(|| {
            vec![
                RibbonGroup {
                    title: "Create",
                    tools: vec![
                        RibbonItem::LargeTool(ToolDef {
                            id: "PL_CREATE",
                            label: "Property Line",
                            icon: IconKind::Svg(include_bytes!("../icons/property_line.svg")),
                            event: ModuleEvent::Command("PL_CREATE".to_string()),
                        }),
                        RibbonItem::LargeTool(ToolDef {
                            id: "PL_TABLE_MODE",
                            label: "By Bearings",
                            icon: IconKind::Svg(include_bytes!("../icons/table_entry.svg")),
                            event: ModuleEvent::Command("PL_TABLE_MODE".to_string()),
                        }),
                        RibbonItem::LargeTool(ToolDef {
                            id: "PL_SKETCH",
                            label: "Sketch",
                            icon: IconKind::Svg(include_bytes!("../icons/sketch.svg")),
                            event: ModuleEvent::Command("PL_SKETCH".to_string()),
                        }),
                        RibbonItem::LargeTool(ToolDef {
                            id: "PL_SKETCH_RECT",
                            label: "Rectangle",
                            icon: IconKind::Svg(include_bytes!("../icons/rectangle.svg")),
                            event: ModuleEvent::Command("PL_SKETCH_RECT".to_string()),
                        }),
                        RibbonItem::LargeTool(ToolDef {
                            id: "PL_PICK_LINES",
                            label: "Pick Lines",
                            icon: IconKind::Svg(include_bytes!("../icons/pick_lines.svg")),
                            event: ModuleEvent::Command("PL_PICK_LINES".to_string()),
                        }),
                    ],
                },
                RibbonGroup {
                    title: "Annotate",
                    tools: vec![
                        RibbonItem::LargeTool(ToolDef {
                            id: "PL_TAG",
                            label: "Tag Segments",
                            icon: IconKind::Svg(include_bytes!("../icons/tag_segments.svg")),
                            event: ModuleEvent::Command("PL_TAG".to_string()),
                        }),
                        RibbonItem::LargeTool(ToolDef {
                            id: "PL_AREA",
                            label: "Area (m²)",
                            icon: IconKind::Svg(include_bytes!("../icons/area_label.svg")),
                            event: ModuleEvent::Command("PL_AREA".to_string()),
                        }),
                        RibbonItem::LargeTool(ToolDef {
                            id: "PL_TABLE",
                            label: "Data Table",
                            icon: IconKind::Svg(include_bytes!("../icons/data_table.svg")),
                            event: ModuleEvent::Command("PL_TABLE".to_string()),
                        }),
                        RibbonItem::LargeTool(ToolDef {
                            id: "PL_NORTH",
                            label: "North",
                            icon: IconKind::Svg(include_bytes!("../icons/north_arrow.svg")),
                            event: ModuleEvent::Command("PL_NORTH".to_string()),
                        }),
                    ],
                },
                RibbonGroup {
                    title: "Edit & Data",
                    tools: vec![
                        RibbonItem::LargeTool(ToolDef {
                            id: "PL_EDIT",
                            label: "Edit Table",
                            icon: IconKind::Svg(include_bytes!("../icons/edit_table.svg")),
                            event: ModuleEvent::Command("PL_EDIT".to_string()),
                        }),
                        RibbonItem::LargeTool(ToolDef {
                            id: "PL_IMPORT",
                            label: "Import CSV",
                            icon: IconKind::Svg(include_bytes!("../icons/import_csv.svg")),
                            event: ModuleEvent::PluginFileDialog {
                                command: "PL_IMPORT".to_string(),
                                title: "Select Property Line CSV".to_string(),
                                filter_name: "Property Line CSV".to_string(),
                                extensions: vec!["csv".to_string()],
                            },
                        }),
                    ],
                },
            ]
        })
    }
}

struct PendingEdit {
    handle: Handle,
    origin: [f64; 3],
    boundary: survey::Boundary,
    table_csv: String,
}

#[derive(Clone)]
struct PendingTagApply {
    handle: Handle,
    text_height: f64,
    decimals: usize,
    unit_suffix: String,
    include_tick: bool,
    replace_existing: bool,
}

#[derive(Clone)]
struct PendingTableApply {
    handle: Handle,
    insertion: [f64; 3],
    title: String,
    text_height: f64,
    decimals: usize,
    compact_bearing: bool,
    label_stations: bool,
}

#[derive(Clone)]
struct PendingAreaApply {
    handle: Handle,
    insertion: [f64; 3],
    text: String,
    text_height: f64,
    replace_existing: bool,
}

#[derive(Clone)]
struct PendingNorthApply {
    insertion: [f64; 3],
    scale: f64,
    rotation_deg: f64,
}

#[derive(Default)]
struct PropertyLinesPlugin {
    pending_edit: Arc<Mutex<Option<PendingEdit>>>,
    pending_pick: Arc<Mutex<Vec<Handle>>>,
    pending_tag: Arc<Mutex<Option<PendingTagApply>>>,
    pending_table: Arc<Mutex<Option<PendingTableApply>>>,
    pending_area: Arc<Mutex<Option<PendingAreaApply>>>,
    pending_north: Arc<Mutex<Option<PendingNorthApply>>>,
}

impl BuiltinPlugin for PropertyLinesPlugin {
    fn manifest(&self) -> &'static PluginManifest {
        &MANIFEST
    }
    fn ribbon(&self) -> Box<dyn CadModule> {
        Box::new(PropertyLinesModule)
    }

    fn dispatch(&self, host: &mut dyn HostApi, command: &str) -> bool {
        let trimmed = command.trim();
        if trimmed.eq_ignore_ascii_case("PL_CREATE") {
            let layer = ensure_property_line_layer(host);
            host.start_interactive(Box::new(ChooseCreationModeCommand::new(layer)));
            return true;
        }
        if trimmed.eq_ignore_ascii_case("PL_TABLE_MODE") {
            return self.create_from_table(host, TABLE_HEADER);
        }
        if trimmed.eq_ignore_ascii_case("PL_SKETCH") {
            let layer = ensure_property_line_layer(host);
            host.start_interactive(Box::new(SketchBoundary::new(layer)));
            return true;
        }
        if trimmed.eq_ignore_ascii_case("PL_SKETCH_RECT") {
            let layer = ensure_property_line_layer(host);
            host.start_interactive(Box::new(SketchRectBoundary::new(layer)));
            return true;
        }
        if trimmed.eq_ignore_ascii_case("PL_PICK_LINES") {
            self.start_or_apply_pick_lines(host);
            return true;
        }
        if trimmed.eq_ignore_ascii_case("PL_PICK_LINES_APPLY") {
            let handles = self.take_pending_picks();
            self.apply_pick_lines(host, &handles);
            return true;
        }
        if trimmed.eq_ignore_ascii_case("PL_IMPORT") {
            host.push_info("Choose Import CSV and select a bearings and distances table.");
            return true;
        }
        if trimmed.len() >= 10 && trimmed[..10].eq_ignore_ascii_case("PL_IMPORT ") {
            let path = trimmed[10..].trim().trim_matches('"');
            if path.is_empty() {
                host.push_info("Choose Import CSV and select a bearings and distances table.");
                return true;
            }
            return match std::fs::read_to_string(path) {
                Ok(csv) => self.create_from_table(host, &csv),
                Err(error) => {
                    host.push_error(&format!("Could not read {}: {error}", path));
                    true
                }
            };
        }
        if trimmed.eq_ignore_ascii_case("PL_EDIT") {
            self.edit_selected(host);
            return true;
        }
        if trimmed.eq_ignore_ascii_case("PL_EDIT_PICKED") {
            let handles = self.take_pending_picks();
            if let Some(&handle) = handles.first() {
                self.edit_handle(host, handle);
            }
            return true;
        }
        if trimmed.eq_ignore_ascii_case("PL_APPLY_EDIT") {
            self.apply_pending_edit(host);
            return true;
        }
        if trimmed.eq_ignore_ascii_case("PL_TAG") {
            self.tag_selected(host);
            return true;
        }
        if trimmed.eq_ignore_ascii_case("PL_TAG_PICKED") {
            let handles = self.take_pending_picks();
            if let Some(&handle) = handles.first() {
                self.start_tag_dialog(host, handle);
            }
            return true;
        }
        if trimmed.eq_ignore_ascii_case("PL_APPLY_TAG") {
            self.apply_pending_tag(host);
            return true;
        }
        if trimmed.eq_ignore_ascii_case("PL_TABLE") {
            self.schedule_table_selected(host);
            return true;
        }
        if trimmed.eq_ignore_ascii_case("PL_TABLE_PICKED") {
            let handles = self.take_pending_picks();
            if let Some(&handle) = handles.first() {
                self.start_schedule_table(host, handle);
            }
            return true;
        }
        if trimmed.eq_ignore_ascii_case("PL_APPLY_TABLE") {
            self.apply_pending_table(host);
            return true;
        }
        if trimmed.eq_ignore_ascii_case("PL_AREA") {
            self.area_label_selected(host);
            return true;
        }
        if trimmed.eq_ignore_ascii_case("PL_AREA_PICKED") {
            let handles = self.take_pending_picks();
            if let Some(&handle) = handles.first() {
                self.start_area_label(host, handle);
            }
            return true;
        }
        if trimmed.eq_ignore_ascii_case("PL_APPLY_AREA") {
            self.apply_pending_area(host);
            return true;
        }
        if trimmed.eq_ignore_ascii_case("PL_NORTH") {
            self.start_north_arrow(host);
            return true;
        }
        if trimmed.eq_ignore_ascii_case("PL_APPLY_NORTH") {
            self.apply_pending_north(host);
            return true;
        }
        false
    }
}

impl PropertyLinesPlugin {
    fn take_pending_picks(&self) -> Vec<Handle> {
        self.pending_pick
            .lock()
            .ok()
            .map(|mut slot| std::mem::take(&mut *slot))
            .unwrap_or_default()
    }

    fn create_from_table(&self, host: &mut dyn HostApi, initial: &str) -> bool {
        let layer = ensure_property_line_layer(host);
        host.start_interactive(Box::new(new_place_boundary(initial, layer)));
        true
    }

    fn edit_selected(&self, host: &mut dyn HostApi) {
        let handles = host.selection();
        match handles.as_slice() {
            [] => {
                host.start_interactive(Box::new(PickSingleBoundaryCommand::new(
                    "Select a property line polyline to edit:",
                    "PL_EDIT_PICKED",
                    Arc::clone(&self.pending_pick),
                )));
            }
            [handle] => self.edit_handle(host, *handle),
            _ => host.push_error("Select a single polyline, then click Edit Table."),
        }
    }

    fn edit_handle(&self, host: &mut dyn HostApi, handle: Handle) {
        let Some(entity) = host.document().get_entity(handle).cloned() else {
            host.push_error("The selected entity no longer exists in the drawing.");
            return;
        };
        if host
            .document()
            .layers
            .get(&entity.common().layer)
            .is_some_and(|layer| layer.is_locked())
        {
            host.push_error(&format!(
                "Layer '{}' is locked; unlock it before editing the property line.",
                entity.common().layer
            ));
            return;
        }
        let (points, bulges, was_closed) = match extract_polyline(&entity) {
            Ok(geometry) => geometry,
            Err(error) => {
                host.push_error(&error);
                return;
            }
        };
        let csv = read_table(entity.common().extended_data.get_record(XDATA_APP))
            .unwrap_or_else(|| table_from_polyline(&points, &bulges, was_closed));
        let (rows, close_polygon) = csv_to_dialog_rows(&csv);
        if let Ok(mut slot) = self.pending_edit.lock() {
            *slot = None;
        }
        host.start_interactive(Box::new(EditBoundary {
            handle,
            origin: points[0],
            rows,
            close_polygon,
            error: None,
            active: true,
            pending_edit: Arc::clone(&self.pending_edit),
        }));
    }

    fn apply_pending_edit(&self, host: &mut dyn HostApi) {
        let pending = self.pending_edit.lock().ok().and_then(|mut slot| slot.take());
        let Some(pending) = pending else {
            return;
        };
        let Some(entity) = host.document().get_entity(pending.handle).cloned() else {
            host.push_error("The selected entity no longer exists in the drawing.");
            return;
        };
        if host
            .document()
            .layers
            .get(&entity.common().layer)
            .is_some_and(|layer| layer.is_locked())
        {
            host.push_error(&format!(
                "Layer '{}' is locked; unlock it before editing the property line.",
                entity.common().layer
            ));
            return;
        }
        let mut updated = LwPolyline::new();
        updated.common = entity.common().clone();
        updated.common.raw_record = None;
        updated.common.graphic_data = None;
        updated.elevation = pending.origin[2];
        updated.is_closed = pending.boundary.closed;
        for vertex in &pending.boundary.vertices {
            updated.add_point_with_bulge(
                ocs_plugin_api::host::acadrust::Vector2::new(
                    pending.origin[0] + vertex.x,
                    pending.origin[1] + vertex.y,
                ),
                vertex.bulge,
            );
        }
        let record = table_record(&pending.table_csv);
        updated.common.extended_data.upsert_record(record.clone());
        host.begin_undo_group("Edit Property Line Table");
        let updated_ok = host.update_entity(EntityType::LwPolyline(updated));
        if updated_ok {
            host.write_record(pending.handle, record);
            host.bump_geometry();
            host.set_dirty();
            host.push_info(&format_boundary_summary(&pending.boundary));
            host.push_info("Geometry and parameter table updated.");
        } else {
            host.push_error("Open CAD Studio could not update the selected polyline.");
        }
        host.end_undo_group();
    }

    fn start_or_apply_pick_lines(&self, host: &mut dyn HostApi) {
        let handles = host.selection();
        if !handles.is_empty() {
            self.apply_pick_lines(host, &handles);
            return;
        }
        if let Ok(mut slot) = self.pending_pick.lock() {
            slot.clear();
        }
        let polyline_handles: HashSet<Handle> = host
            .document()
            .entities()
            .filter(|entity| {
                matches!(
                    entity,
                    EntityType::LwPolyline(_)
                        | EntityType::Polyline2D(_)
                        | EntityType::Polyline(_)
                        | EntityType::Polyline3D(_)
                )
            })
            .map(|entity| entity.common().handle)
            .collect();
        host.start_interactive(Box::new(PickChainCommand {
            handles: Vec::new(),
            polyline_handles,
            pending_pick: Arc::clone(&self.pending_pick),
        }));
    }

    fn apply_pick_lines(&self, host: &mut dyn HostApi, handles: &[Handle]) {
        if handles.is_empty() {
            host.push_info("No lines or polylines selected.");
            return;
        }
        let entities: Vec<EntityType> = handles
            .iter()
            .filter_map(|h| host.document().get_entity(*h).cloned())
            .collect();
        let refs: Vec<&EntityType> = entities.iter().collect();
        let (points, bulges, closed) = match drafting::chain_entities_to_polyline(&refs) {
            Ok(chain) => chain,
            Err(error) => {
                host.push_error(&error);
                return;
            }
        };

        let layer = ensure_property_line_layer(host);
        let boundary = survey::boundary_from_points(&points, &bulges, closed);
        let table_csv = table_from_polyline(&points, &bulges, closed);
        let record = table_record(&table_csv);

        if entities.len() == 1 && extract_polyline_for_pick(&entities[0]).is_ok() {
            let source = &entities[0];
            let source_layer = &source.common().layer;
            if host
                .document()
                .layers
                .get(source_layer)
                .is_some_and(|l| l.is_locked())
            {
                host.push_error(&format!(
                    "Layer '{source_layer}' is locked; unlock it before converting."
                ));
                return;
            }
            let mut updated = make_property_polyline(points[0], &boundary, &layer, &table_csv);
            if matches!(source, EntityType::LwPolyline(_)) {
                updated.common.handle = source.common().handle;
                updated.common.owner_handle = source.common().owner_handle;
                host.begin_undo_group("Convert Polyline to Property Line");
                if host.update_entity(EntityType::LwPolyline(updated)) {
                    host.write_record(source.common().handle, record);
                    host.bump_geometry();
                    host.set_dirty();
                    host.push_info(&format!(
                        "Converted to Property Line on layer '{layer}'. {}",
                        format_boundary_summary(&boundary)
                    ));
                } else {
                    host.push_error("Could not convert the selected polyline.");
                }
                host.end_undo_group();
            } else {
                host.begin_undo_group("Convert Polyline to Property Line");
                let new_handle = host.add_entity(EntityType::LwPolyline(updated));
                host.write_record(new_handle, record);
                host.remove_entity(source.common().handle);
                host.bump_geometry();
                host.set_dirty();
                host.end_undo_group();
                host.push_info(&format!(
                    "Converted to Property Line on layer '{layer}'. {}",
                    format_boundary_summary(&boundary)
                ));
            }
        } else {
            let polyline = make_property_polyline(points[0], &boundary, &layer, &table_csv);
            host.begin_undo_group("Create Property Line from Picked Lines");
            let handle = host.add_entity(EntityType::LwPolyline(polyline));
            host.write_record(handle, record);
            host.bump_geometry();
            host.set_dirty();
            host.end_undo_group();
            host.push_info(&format!(
                "Created Property Line on layer '{layer}'. {}",
                format_boundary_summary(&boundary)
            ));
        }
    }

    fn tag_selected(&self, host: &mut dyn HostApi) {
        let handles = host.selection();
        match handles.as_slice() {
            [] => {
                host.start_interactive(Box::new(PickSingleBoundaryCommand::new(
                    "Select a property line polyline to tag segments:",
                    "PL_TAG_PICKED",
                    Arc::clone(&self.pending_pick),
                )));
            }
            [handle] => self.start_tag_dialog(host, *handle),
            _ => host.push_error("Select a single polyline, then click Tag Segments."),
        }
    }

    fn start_tag_dialog(&self, host: &mut dyn HostApi, handle: Handle) {
        let Some(entity) = host.document().get_entity(handle).cloned() else {
            host.push_error("The selected entity no longer exists.");
            return;
        };
        let (points, bulges, closed) = match extract_polyline(&entity) {
            Ok(geom) => geom,
            Err(error) => {
                host.push_error(&error);
                return;
            }
        };
        let segments = drafting::extract_segments(&points, &bulges, closed);
        if segments.is_empty() {
            host.push_error("The selected polyline has no non-zero segments to tag.");
            return;
        }
        let default_height = drafting::default_annotation_height(&points);
        if let Ok(mut slot) = self.pending_tag.lock() {
            *slot = None;
        }
        host.start_interactive(Box::new(TagOptionsCommand {
            handle,
            default_height,
            error: None,
            active: true,
            pending_tag: Arc::clone(&self.pending_tag),
        }));
    }

    fn apply_pending_tag(&self, host: &mut dyn HostApi) {
        let pending = self.pending_tag.lock().ok().and_then(|mut slot| slot.take());
        let Some(pending) = pending else {
            return;
        };
        let Some(entity) = host.document().get_entity(pending.handle).cloned() else {
            host.push_error("The target polyline no longer exists.");
            return;
        };
        let (points, bulges, closed) = match extract_polyline(&entity) {
            Ok(geom) => geom,
            Err(error) => {
                host.push_error(&error);
                return;
            }
        };
        let segments = drafting::extract_segments(&points, &bulges, closed);
        if segments.is_empty() {
            host.push_error("No segments to tag.");
            return;
        }
        let layer = if entity.common().layer.trim().is_empty() {
            ensure_property_line_layer(host)
        } else {
            entity.common().layer.clone()
        };
        let new_entities = drafting::build_segment_tag_entities(
            pending.handle,
            &segments,
            &layer,
            pending.text_height,
            pending.decimals,
            &pending.unit_suffix,
            pending.include_tick,
        );
        host.begin_undo_group("Tag Property Line Segments");
        if pending.replace_existing {
            drafting::remove_existing_annotations(
                host,
                pending.handle,
                drafting::cleanup_kinds_for_tags(),
            );
        }
        for ann in new_entities {
            host.add_entity(ann);
        }
        host.bump_geometry();
        host.set_dirty();
        host.end_undo_group();
        host.push_info(&format!(
            "Tagged {} property line segments on layer '{layer}'.",
            segments.len()
        ));
    }

    fn schedule_table_selected(&self, host: &mut dyn HostApi) {
        let handles = host.selection();
        match handles.as_slice() {
            [] => {
                host.start_interactive(Box::new(PickSingleBoundaryCommand::new(
                    "Select a property line polyline for the bearings table:",
                    "PL_TABLE_PICKED",
                    Arc::clone(&self.pending_pick),
                )));
            }
            [handle] => self.start_schedule_table(host, *handle),
            _ => host.push_error("Select a single polyline, then click Data Table."),
        }
    }

    fn start_schedule_table(&self, host: &mut dyn HostApi, handle: Handle) {
        let Some(entity) = host.document().get_entity(handle).cloned() else {
            host.push_error("The selected entity no longer exists.");
            return;
        };
        let (points, bulges, closed) = match extract_polyline(&entity) {
            Ok(geom) => geom,
            Err(error) => {
                host.push_error(&error);
                return;
            }
        };
        let segments = drafting::extract_segments(&points, &bulges, closed);
        if segments.is_empty() {
            host.push_error("The selected polyline has no segments for the table.");
            return;
        }
        let default_height = drafting::default_annotation_height(&points);
        if let Ok(mut slot) = self.pending_table.lock() {
            *slot = None;
        }
        host.start_interactive(Box::new(ScheduleTableCommand {
            handle,
            segments,
            default_height,
            error: None,
            stage: ScheduleTableStage::Dialog,
            title: "Rumbos y Distancias".to_string(),
            text_height: default_height,
            decimals: 2,
            compact_bearing: true,
            label_stations: true,
            pending_table: Arc::clone(&self.pending_table),
        }));
    }

    fn apply_pending_table(&self, host: &mut dyn HostApi) {
        let pending = self.pending_table.lock().ok().and_then(|mut slot| slot.take());
        let Some(pending) = pending else {
            return;
        };
        let Some(entity) = host.document().get_entity(pending.handle).cloned() else {
            host.push_error("The target polyline no longer exists.");
            return;
        };
        let (points, bulges, closed) = match extract_polyline(&entity) {
            Ok(geom) => geom,
            Err(error) => {
                host.push_error(&error);
                return;
            }
        };
        let segments = drafting::extract_segments(&points, &bulges, closed);
        if segments.is_empty() {
            host.push_error("No segments found.");
            return;
        }
        let layer = if entity.common().layer.trim().is_empty() {
            ensure_property_line_layer(host)
        } else {
            entity.common().layer.clone()
        };
        let table = drafting::build_bearings_table(
            Some(pending.handle),
            &segments,
            &pending.title,
            pending.insertion,
            &layer,
            pending.text_height,
            pending.decimals,
            pending.compact_bearing,
            None,
        );
        let station_labels = if pending.label_stations {
            drafting::build_vertex_station_labels(
                pending.handle,
                &points,
                closed,
                &segments,
                &layer,
                pending.text_height,
            )
        } else {
            Vec::new()
        };

        host.begin_undo_group("Insert Property Line Data Table");
        if pending.label_stations {
            drafting::remove_existing_annotations(
                host,
                pending.handle,
                drafting::cleanup_kinds_for_stations(),
            );
            for label in station_labels {
                host.add_entity(label);
            }
        }
        host.add_entity(EntityType::Table(Box::new(table)));
        host.bump_geometry();
        host.set_dirty();
        host.end_undo_group();
        host.push_info(&format!(
            "Placed '{}' ({} rows) on layer '{layer}'.",
            drafting::effective_table_title(&pending.title),
            segments.len()
        ));
    }

    fn area_label_selected(&self, host: &mut dyn HostApi) {
        let handles = host.selection();
        match handles.as_slice() {
            [] => {
                host.start_interactive(Box::new(PickSingleBoundaryCommand::new(
                    "Select a closed property line polyline to label its area (m²):",
                    "PL_AREA_PICKED",
                    Arc::clone(&self.pending_pick),
                )));
            }
            [handle] => self.start_area_label(host, *handle),
            _ => host.push_error("Select a single closed polyline, then click Area (m²)."),
        }
    }

    fn start_area_label(&self, host: &mut dyn HostApi, handle: Handle) {
        let Some(entity) = host.document().get_entity(handle).cloned() else {
            host.push_error("The selected entity no longer exists.");
            return;
        };
        let (points, bulges, closed) = match extract_polyline(&entity) {
            Ok(geom) => geom,
            Err(error) => {
                host.push_error(&error);
                return;
            }
        };
        let boundary = survey::boundary_from_points(&points, &bulges, closed);
        let endpoints_meet = points.len() >= 3
            && (points[0][0] - points.last().unwrap()[0])
                .hypot(points[0][1] - points.last().unwrap()[1])
                <= 1.0e-4;
        let area_m2 = if let Some(area) = boundary.area {
            area
        } else if endpoints_meet {
            survey::signed_area(&boundary.vertices).abs()
        } else {
            host.push_error("The selected property line must be closed to compute its area (m²).");
            return;
        };
        if !area_m2.is_finite() || area_m2 <= 1.0e-9 {
            host.push_error("The selected polyline encloses zero area.");
            return;
        }
        let centroid = drafting::parcel_centroid(&points).unwrap_or(points[0]);
        let default_height = drafting::default_annotation_height(&points) * 1.25;
        if let Ok(mut slot) = self.pending_area.lock() {
            *slot = None;
        }
        host.start_interactive(Box::new(AreaOptionsCommand {
            handle,
            stage: AreaStage::Dialog,
            area_m2,
            centroid,
            prefix: "Área:".to_string(),
            unit: "m²".to_string(),
            text_height: default_height,
            decimals: 2,
            replace_existing: true,
            error: None,
            pending_area: Arc::clone(&self.pending_area),
        }));
    }

    fn apply_pending_area(&self, host: &mut dyn HostApi) {
        let pending = self.pending_area.lock().ok().and_then(|mut slot| slot.take());
        let Some(pending) = pending else {
            return;
        };
        let Some(entity) = host.document().get_entity(pending.handle).cloned() else {
            host.push_error("The target polyline no longer exists.");
            return;
        };
        let layer = if entity.common().layer.trim().is_empty() {
            ensure_property_line_layer(host)
        } else {
            entity.common().layer.clone()
        };
        let label_entity = drafting::build_area_label_entity(
            pending.handle,
            pending.insertion,
            &layer,
            &pending.text,
            pending.text_height,
        );
        host.begin_undo_group("Label Property Line Area");
        if pending.replace_existing {
            drafting::remove_existing_annotations(
                host,
                pending.handle,
                drafting::cleanup_kinds_for_area(),
            );
        }
        host.add_entity(label_entity);
        host.bump_geometry();
        host.set_dirty();
        host.end_undo_group();
        host.push_info(&format!(
            "Placed area label '{}' on layer '{layer}'.",
            pending.text
        ));
    }

    fn start_north_arrow(&self, host: &mut dyn HostApi) {
        if let Ok(mut slot) = self.pending_north.lock() {
            *slot = None;
        }
        host.start_interactive(Box::new(NorthArrowCommand {
            stage: NorthArrowStage::Dialog,
            scale: 1.0,
            rotation_deg: 0.0,
            error: None,
            pending_north: Arc::clone(&self.pending_north),
        }));
    }

    fn apply_pending_north(&self, host: &mut dyn HostApi) {
        let pending = self
            .pending_north
            .lock()
            .ok()
            .and_then(|mut slot| slot.take());
        let Some(pending) = pending else {
            return;
        };
        let layer = ensure_property_line_layer(host);
        let block_name = match drafting::ensure_north_arrow_block(host) {
            Ok(name) => name,
            Err(err) => {
                host.push_error(&err);
                return;
            }
        };
        let insert = drafting::build_north_insert(
            pending.insertion,
            &layer,
            pending.scale,
            pending.rotation_deg,
        );
        host.begin_undo_group("Insert Property Line North Arrow");
        host.add_entity(insert);
        host.bump_geometry();
        host.set_dirty();
        host.end_undo_group();
        host.push_info(&format!(
            "Inserted North arrow block '{block_name}' on layer '{layer}' (scale {:.2}, rotation {:.1}°).",
            pending.scale, pending.rotation_deg
        ));
    }
}

fn table_from_polyline(points: &[[f64; 3]], bulges: &[f64], closed: bool) -> String {
    let mut csv = String::from(TABLE_HEADER);
    let segment_count = if closed { points.len() } else { points.len() - 1 };
    for index in 0..segment_count {
        let next = (index + 1) % points.len();
        let dx = points[next][0] - points[index][0];
        let dy = points[next][1] - points[index][1];
        let chord = dx.hypot(dy);
        if chord <= 1.0e-9 { continue; }
        let angle = dx.abs().atan2(dy.abs()).to_degrees();
        let hundredths = (angle * 360_000.0).round() as u64;
        let degrees = hundredths / 360_000;
        let minutes = (hundredths % 360_000) / 6_000;
        let seconds = (hundredths % 6_000) as f64 / 100.0;
        let ns = if dy < 0.0 { "S" } else { "N" };
        let ew = if dx < 0.0 { "W" } else { "E" };
        let bulge = bulges.get(index).copied().unwrap_or(0.0);
        let (kind, radius, side) = if bulge.abs() > 1.0e-12 {
            let included = 4.0 * bulge.abs().atan();
            let radius = chord / (2.0 * (included / 2.0).sin());
            ("Arc", format!("{radius:.12}"), if bulge > 0.0 { "R" } else { "L" })
        } else {
            ("Line", String::new(), "R")
        };
        csv.push_str(&format!(
            "{chord:.12},{ns},{degrees}° {minutes:02}' {seconds:05.2}\",{ew},{kind},{radius},{side}
"
        ));
    }
    csv
}

fn format_boundary_summary(boundary: &survey::Boundary) -> String {
    let detail = if boundary.closed {
        format!("closed; area {:.3} square units", boundary.area.unwrap_or(0.0))
    } else {
        format!("open; endpoint gap {:.3} units", boundary.closure_gap)
    };
    let segments = if boundary.closed {
        boundary.vertices.len()
    } else {
        boundary.vertices.len() - 1
    };
    format!("Property line ready: {segments} segments; {detail}.")
}

fn starter_dialog_row() -> Vec<String> {
    vec![
        String::new(),
        "N".to_string(),
        "0".to_string(),
        "0".to_string(),
        "0".to_string(),
        "E".to_string(),
        "Line".to_string(),
        String::new(),
        "R".to_string(),
    ]
}

fn bearing_to_dms_cells(value: &str) -> [String; 3] {
    let cleaned: String = value
        .chars()
        .map(|c| match c {
            '°' | 'º' | '\'' | '"' | ':' | '′' | '″' => ' ',
            other => other,
        })
        .collect();
    let parts: Vec<&str> = cleaned.split_whitespace().collect();
    let deg = parts.first().copied().unwrap_or("0");
    let min = parts.get(1).copied().unwrap_or("0");
    let sec = parts.get(2).copied().unwrap_or("0");
    [
        if deg.is_empty() { "0" } else { deg }.to_string(),
        if min.is_empty() { "0" } else { min }.to_string(),
        if sec.is_empty() { "0" } else { sec }.to_string(),
    ]
}

fn csv_to_dialog_rows(csv_text: &str) -> (Vec<Vec<String>>, bool) {
    let trimmed = csv_text.trim_start_matches('\u{feff}');
    let mut reader = csv::ReaderBuilder::new()
        .trim(csv::Trim::All)
        .flexible(true)
        .from_reader(trimmed.as_bytes());
    let mut rows = Vec::new();
    let mut close_polygon = false;
    for result in reader.records() {
        let Ok(record) = result else {
            continue;
        };
        if record.iter().all(|field| field.trim().is_empty()) {
            continue;
        }
        let field = |idx: usize| record.get(idx).unwrap_or("").trim();
        let kind_upper = field(4).to_uppercase();
        if kind_upper == "CLOSE" || kind_upper == "CERRAR" {
            close_polygon = true;
            continue;
        }
        let distance = field(0).to_string();
        let ns = if field(1).eq_ignore_ascii_case("S") {
            "S".to_string()
        } else {
            "N".to_string()
        };
        let [deg, min, sec] = bearing_to_dms_cells(field(2));
        let ew_upper = field(3).to_uppercase();
        let ew = if ew_upper == "W" || ew_upper == "O" {
            "W".to_string()
        } else {
            "E".to_string()
        };
        let kind = if kind_upper == "ARC" || kind_upper == "ARCO" {
            "Arc".to_string()
        } else {
            "Line".to_string()
        };
        let radius = field(5).to_string();
        let lr_upper = field(6).to_uppercase();
        let lr = if lr_upper == "L" || lr_upper == "I" {
            "L".to_string()
        } else {
            "R".to_string()
        };
        rows.push(vec![distance, ns, deg, min, sec, ew, kind, radius, lr]);
    }
    if rows.is_empty() {
        rows.push(starter_dialog_row());
    }
    (rows, close_polygon)
}

fn csv_escape_field(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}

fn dialog_rows_to_boundary(
    rows: &[Vec<String>],
    close_polygon: bool,
) -> Result<(String, survey::Boundary), String> {
    let mut csv = String::from(TABLE_HEADER);
    let mut segment_count = 0usize;

    for (index, raw_row) in rows.iter().enumerate() {
        let cell = |col: usize| raw_row.get(col).map(|s| s.trim()).unwrap_or("");
        let distance = cell(0);
        let ns_raw = cell(1);
        let deg_raw = cell(2);
        let min_raw = cell(3);
        let sec_raw = cell(4);
        let ew_raw = cell(5);
        let kind_raw = cell(6);
        let radius_raw = cell(7);
        let lr_raw = cell(8);

        let all_default_or_blank = distance.is_empty()
            && radius_raw.is_empty()
            && (deg_raw.is_empty() || deg_raw == "0")
            && (min_raw.is_empty() || min_raw == "0")
            && (sec_raw.is_empty() || sec_raw == "0");
        if all_default_or_blank {
            continue;
        }

        let row_num = index + 1;
        let dist_num: f64 = distance
            .parse()
            .map_err(|_| format!("Row {row_num}: Distance must be a positive number."))?;
        if !dist_num.is_finite() || dist_num <= 0.0 {
            return Err(format!("Row {row_num}: Distance must be greater than zero."));
        }

        let ns = match ns_raw.to_uppercase().as_str() {
            "" | "N" => "N",
            "S" => "S",
            other => return Err(format!("Row {row_num}: N/S must be N or S (found '{other}').")),
        };
        let ew = match ew_raw.to_uppercase().as_str() {
            "" | "E" => "E",
            "W" | "O" => "W",
            other => {
                return Err(format!(
                    "Row {row_num}: E/W must be E, W, or O (found '{other}')."
                ))
            }
        };
        let kind = match kind_raw.to_uppercase().as_str() {
            "" | "LINE" | "LINEA" | "LÍNEA" => "Line",
            "ARC" | "ARCO" => "Arc",
            other => {
                return Err(format!(
                    "Row {row_num}: Type must be Line or Arc (found '{other}')."
                ))
            }
        };
        let lr = match lr_raw.to_uppercase().as_str() {
            "" | "R" | "D" => "R",
            "L" | "I" => "L",
            other => {
                return Err(format!(
                    "Row {row_num}: L/R must be R or L (found '{other}')."
                ))
            }
        };
        let deg = if deg_raw.is_empty() { "0" } else { deg_raw };
        let min = if min_raw.is_empty() { "0" } else { min_raw };
        let sec = if sec_raw.is_empty() { "0" } else { sec_raw };
        let bearing = format!("{deg}° {min}' {sec}\"");
        let radius = if kind == "Arc" { radius_raw } else { "" };

        let line = [distance, ns, &bearing, ew, kind, radius, lr]
            .into_iter()
            .map(csv_escape_field)
            .collect::<Vec<_>>()
            .join(",");
        csv.push_str(&line);
        csv.push('\n');
        segment_count += 1;
    }

    if segment_count == 0 {
        return Err("Enter at least one property-line segment.".to_string());
    }
    if close_polygon {
        csv.push_str(",,,,Close,,\n");
    }

    let boundary = survey::from_csv(&csv)?;
    Ok((csv, boundary))
}

fn extract_dialog_table_and_close(
    values: &HashMap<String, DialogValue>,
    fallback_rows: &[Vec<String>],
    fallback_close: bool,
) -> (Vec<Vec<String>>, bool) {
    let rows = match values.get(DIALOG_TABLE_ID) {
        Some(DialogValue::Table(rows)) => rows.clone(),
        _ => fallback_rows.to_vec(),
    };
    let close_polygon = match values.get(DIALOG_CLOSE_ID) {
        Some(DialogValue::Bool(checked)) => *checked,
        _ => fallback_close,
    };
    (rows, close_polygon)
}

fn build_table_dialog(
    title: &str,
    rows: &[Vec<String>],
    close_polygon: bool,
    error: Option<&str>,
) -> DialogDef {
    let mut dialog = DialogDef::new(title)
        .width(DIALOG_WIDTH)
        .submit_label("Accept")
        .cancel_label("Cancel");
    if let Some(err) = error {
        dialog = dialog.with_row(DialogRow::single(DialogControl::InfoText {
            text: format!("Invalid input: {err}"),
            align_h: None,
        }));
    }
    dialog = dialog
        .with_row(DialogRow::single(DialogControl::Table {
            id: DIALOG_TABLE_ID.to_string(),
            label: Some("Survey Segments".to_string()),
            headers: vec![
                "Distance".to_string(),
                "N/S".to_string(),
                "Deg".to_string(),
                "Min".to_string(),
                "Sec".to_string(),
                "E/W".to_string(),
                "Type".to_string(),
                "Radius".to_string(),
                "L/R".to_string(),
            ],
            rows: rows.to_vec(),
            allow_add: true,
            allow_delete: true,
        }))
        .with_row(DialogRow::single(DialogControl::Checkbox {
            id: DIALOG_CLOSE_ID.to_string(),
            label: "Add a final segment to close the polygon".to_string(),
            default: close_polygon,
        }))
        .with_row(DialogRow::single(DialogControl::InfoText {
            text: "Bearings: DMS 0°–90° from true North (+Y) toward E/W (O = W). Type: Line or Arc (enter Radius and R/L; empty cells default to N, 0° 0' 0\", E, Line, R)."
                .to_string(),
            align_h: None,
        }));
    dialog
}

fn arc_preview_wire(from: [f64; 3], to: [f64; 3], bulge: f64) -> PreviewWire {
    let dx = to[0] - from[0];
    let dy = to[1] - from[1];
    let chord = dx.hypot(dy);
    if chord <= 1.0e-9 || bulge.abs() <= 1.0e-12 {
        return PreviewWire::line(from, to, None);
    }
    let theta = 4.0 * bulge.atan();
    let offset = chord * (1.0 - bulge * bulge) / (4.0 * bulge);
    let cx = (from[0] + to[0]) * 0.5 + (-dy / chord) * offset;
    let cy = (from[1] + to[1]) * 0.5 + (dx / chord) * offset;
    let radius = (from[0] - cx).hypot(from[1] - cy);
    let a_from = (from[1] - cy).atan2(from[0] - cx);
    let a_to = (to[1] - cy).atan2(to[0] - cx);
    let (start_angle_rad, end_angle_rad) = if bulge > 0.0 {
        (a_from, a_from + theta)
    } else {
        (a_to, a_to + theta.abs())
    };
    PreviewWire::arc([cx, cy, from[2]], radius, start_angle_rad, end_angle_rad, None)
}

fn preview_wires_for_boundary(boundary: &survey::Boundary, origin: [f64; 3]) -> Vec<PreviewWire> {
    let count = boundary.vertices.len();
    if count < 2 {
        return Vec::new();
    }
    let segments = if boundary.closed { count } else { count - 1 };
    let mut wires = Vec::with_capacity(segments);
    for i in 0..segments {
        let a = &boundary.vertices[i];
        let b = &boundary.vertices[(i + 1) % count];
        let from = [origin[0] + a.x, origin[1] + a.y, origin[2]];
        let to = [origin[0] + b.x, origin[1] + b.y, origin[2]];
        wires.push(arc_preview_wire(from, to, a.bulge));
    }
    wires
}

enum PlacementState {
    Editing {
        rows: Vec<Vec<String>>,
        close_polygon: bool,
        error: Option<String>,
    },
    Ready {
        boundary: survey::Boundary,
        table_csv: String,
    },
    Canceled,
}

fn new_place_boundary(initial: &str, layer: String) -> PlaceBoundary {
    let (rows, close_polygon) = csv_to_dialog_rows(initial);
    PlaceBoundary {
        state: PlacementState::Editing {
            rows,
            close_polygon,
            error: None,
        },
        layer,
    }
}

struct PlaceBoundary {
    state: PlacementState,
    layer: String,
}

impl InteractiveCommand for PlaceBoundary {
    fn prompt(&self) -> String {
        match &self.state {
            PlacementState::Editing { .. } => {
                "Define the property line table in the dialog (Esc to cancel):".to_string()
            }
            PlacementState::Ready { boundary, .. } => format!(
                "{} Click the starting point:",
                format_boundary_summary(boundary)
            ),
            PlacementState::Canceled => "Table canceled.".to_string(),
        }
    }

    fn modal_dialog(&self) -> Option<DialogDef> {
        match &self.state {
            PlacementState::Editing {
                rows,
                close_polygon,
                error,
            } => Some(build_table_dialog(
                "New Property Line",
                rows,
                *close_polygon,
                error.as_deref(),
            )),
            PlacementState::Ready { .. } | PlacementState::Canceled => None,
        }
    }

    fn on_dialog_response(&mut self, response: DialogResponse) -> CommandStep {
        match response {
            DialogResponse::Cancelled => {
                self.state = PlacementState::Canceled;
                CommandStep::Cancel
            }
            DialogResponse::Submitted(values) => {
                let (fallback_rows, fallback_close) = match &self.state {
                    PlacementState::Editing {
                        rows,
                        close_polygon,
                        ..
                    } => (rows.as_slice(), *close_polygon),
                    _ => (&[][..], false),
                };
                let (rows, close_polygon) =
                    extract_dialog_table_and_close(&values, fallback_rows, fallback_close);
                match dialog_rows_to_boundary(&rows, close_polygon) {
                    Ok((table_csv, boundary)) => {
                        self.state = PlacementState::Ready {
                            boundary,
                            table_csv,
                        };
                        CommandStep::NeedPoint
                    }
                    Err(err) => {
                        self.state = PlacementState::Editing {
                            rows,
                            close_polygon,
                            error: Some(err),
                        };
                        CommandStep::NeedPoint
                    }
                }
            }
        }
    }

    fn on_cursor_move(&mut self, point: [f64; 3]) -> Vec<PreviewWire> {
        if point.iter().any(|coordinate| !coordinate.is_finite()) {
            return Vec::new();
        }
        let PlacementState::Ready { boundary, .. } = &self.state else {
            return Vec::new();
        };
        preview_wires_for_boundary(boundary, point)
    }

    fn on_point(&mut self, point: [f64; 3]) -> CommandStep {
        if point.iter().any(|coordinate| !coordinate.is_finite()) {
            return CommandStep::Cancel;
        }
        let PlacementState::Ready {
            boundary,
            table_csv,
        } = &self.state
        else {
            return if matches!(self.state, PlacementState::Canceled) {
                CommandStep::Cancel
            } else {
                CommandStep::NeedPoint
            };
        };
        let polyline = make_property_polyline(point, boundary, &self.layer, table_csv);
        CommandStep::CommitAndEnd(EntityType::LwPolyline(polyline))
    }
}

struct EditBoundary {
    handle: Handle,
    origin: [f64; 3],
    rows: Vec<Vec<String>>,
    close_polygon: bool,
    error: Option<String>,
    active: bool,
    pending_edit: Arc<Mutex<Option<PendingEdit>>>,
}

impl InteractiveCommand for EditBoundary {
    fn prompt(&self) -> String {
        "Edit the property line table in the dialog (Esc to cancel):".to_string()
    }

    fn modal_dialog(&self) -> Option<DialogDef> {
        if self.active {
            Some(build_table_dialog(
                "Edit Property Line",
                &self.rows,
                self.close_polygon,
                self.error.as_deref(),
            ))
        } else {
            None
        }
    }

    fn on_dialog_response(&mut self, response: DialogResponse) -> CommandStep {
        match response {
            DialogResponse::Cancelled => {
                self.active = false;
                CommandStep::Cancel
            }
            DialogResponse::Submitted(values) => {
                let (rows, close_polygon) =
                    extract_dialog_table_and_close(&values, &self.rows, self.close_polygon);
                self.rows = rows;
                self.close_polygon = close_polygon;
                match dialog_rows_to_boundary(&self.rows, self.close_polygon) {
                    Ok((table_csv, boundary)) => {
                        self.error = None;
                        self.active = false;
                        if let Ok(mut slot) = self.pending_edit.lock() {
                            *slot = Some(PendingEdit {
                                handle: self.handle,
                                origin: self.origin,
                                boundary,
                                table_csv,
                            });
                        }
                        CommandStep::Dispatch("PL_APPLY_EDIT".to_string())
                    }
                    Err(err) => {
                        self.error = Some(err);
                        CommandStep::NeedPoint
                    }
                }
            }
        }
    }

    fn on_point(&mut self, _point: [f64; 3]) -> CommandStep {
        CommandStep::NeedPoint
    }
}
enum CreationModeState {
    Choosing,
    Table(PlaceBoundary),
    Sketch(SketchBoundary),
    Rectangle(SketchRectBoundary),
    Done,
}

struct ChooseCreationModeCommand {
    layer: String,
    state: CreationModeState,
}

impl ChooseCreationModeCommand {
    fn new(layer: String) -> Self {
        Self {
            layer,
            state: CreationModeState::Choosing,
        }
    }
}

impl InteractiveCommand for ChooseCreationModeCommand {
    fn prompt(&self) -> String {
        match &self.state {
            CreationModeState::Choosing => {
                "Choose how to create the property line (Esc to cancel):".to_string()
            }
            CreationModeState::Table(cmd) => cmd.prompt(),
            CreationModeState::Sketch(cmd) => cmd.prompt(),
            CreationModeState::Rectangle(cmd) => cmd.prompt(),
            CreationModeState::Done => "Done.".to_string(),
        }
    }

    fn modal_dialog(&self) -> Option<DialogDef> {
        match &self.state {
            CreationModeState::Choosing => Some(
                DialogDef::new("Create Property Line")
                    .width(460.0)
                    .submit_label("Continue")
                    .cancel_label("Cancel")
                    .with_control(DialogControl::InfoText {
                        text: "How would you like to create the property lines?".to_string(),
                        align_h: None,
                    })
                    .with_control(DialogControl::Choice {
                        id: "mode".to_string(),
                        label: "Creation method".to_string(),
                        options: vec![
                            "Create by entering distances and bearings".to_string(),
                            "Create by sketching".to_string(),
                            "Create rectangle (2 corners)".to_string(),
                            "Pick existing lines or polylines".to_string(),
                        ],
                        default_index: 0,
                    }),
            ),
            CreationModeState::Table(cmd) => cmd.modal_dialog(),
            CreationModeState::Sketch(cmd) => cmd.modal_dialog(),
            CreationModeState::Rectangle(cmd) => cmd.modal_dialog(),
            CreationModeState::Done => None,
        }
    }

    fn on_dialog_response(&mut self, response: DialogResponse) -> CommandStep {
        match &mut self.state {
            CreationModeState::Choosing => match response {
                DialogResponse::Cancelled => {
                    self.state = CreationModeState::Done;
                    CommandStep::Cancel
                }
                DialogResponse::Submitted(values) => {
                    let idx = match values.get("mode") {
                        Some(DialogValue::Choice { index, .. }) => *index,
                        _ => 0,
                    };
                    match idx {
                        0 => {
                            self.state = CreationModeState::Table(new_place_boundary(
                                TABLE_HEADER,
                                self.layer.clone(),
                            ));
                            CommandStep::NeedPoint
                        }
                        1 => {
                            self.state = CreationModeState::Sketch(SketchBoundary::new(
                                self.layer.clone(),
                            ));
                            CommandStep::NeedPoint
                        }
                        2 => {
                            self.state = CreationModeState::Rectangle(SketchRectBoundary::new(
                                self.layer.clone(),
                            ));
                            CommandStep::NeedPoint
                        }
                        _ => {
                            self.state = CreationModeState::Done;
                            CommandStep::Dispatch("PL_PICK_LINES".to_string())
                        }
                    }
                }
            },
            CreationModeState::Table(cmd) => cmd.on_dialog_response(response),
            CreationModeState::Sketch(cmd) => cmd.on_dialog_response(response),
            CreationModeState::Rectangle(cmd) => cmd.on_dialog_response(response),
            CreationModeState::Done => CommandStep::Cancel,
        }
    }

    fn on_cursor_move(&mut self, point: [f64; 3]) -> Vec<PreviewWire> {
        match &mut self.state {
            CreationModeState::Table(cmd) => cmd.on_cursor_move(point),
            CreationModeState::Sketch(cmd) => cmd.on_cursor_move(point),
            CreationModeState::Rectangle(cmd) => cmd.on_cursor_move(point),
            CreationModeState::Choosing | CreationModeState::Done => Vec::new(),
        }
    }

    fn on_point(&mut self, point: [f64; 3]) -> CommandStep {
        match &mut self.state {
            CreationModeState::Table(cmd) => cmd.on_point(point),
            CreationModeState::Sketch(cmd) => cmd.on_point(point),
            CreationModeState::Rectangle(cmd) => cmd.on_point(point),
            CreationModeState::Choosing | CreationModeState::Done => CommandStep::NeedPoint,
        }
    }

    fn on_enter(&mut self) -> CommandStep {
        match &mut self.state {
            CreationModeState::Table(cmd) => cmd.on_enter(),
            CreationModeState::Sketch(cmd) => cmd.on_enter(),
            CreationModeState::Rectangle(cmd) => cmd.on_enter(),
            CreationModeState::Choosing => CommandStep::NeedPoint,
            CreationModeState::Done => CommandStep::Done,
        }
    }
}

struct SketchBoundary {
    vertices: Vec<[f64; 3]>,
    cursor: Option<[f64; 3]>,
    layer: String,
}

impl SketchBoundary {
    fn new(layer: String) -> Self {
        Self {
            vertices: Vec::new(),
            cursor: None,
            layer,
        }
    }

    fn build_polyline_step(&self, closed: bool) -> CommandStep {
        if self.vertices.len() < 2 || (closed && self.vertices.len() < 3) {
            return CommandStep::Cancel;
        }
        let bulges = vec![0.0; self.vertices.len()];
        let boundary = survey::boundary_from_points(&self.vertices, &bulges, closed);
        let csv = table_from_polyline(&self.vertices, &bulges, closed);
        let polyline = make_property_polyline(self.vertices[0], &boundary, &self.layer, &csv);
        CommandStep::CommitAndEnd(EntityType::LwPolyline(polyline))
    }
}

impl InteractiveCommand for SketchBoundary {
    fn prompt(&self) -> String {
        if self.vertices.is_empty() {
            return "Sketch Property Line: Pick first corner point (Esc to cancel):".to_string();
        }
        let last = *self.vertices.last().unwrap();
        let live_segment = self.cursor.and_then(|c| {
            let dx = c[0] - last[0];
            let dy = c[1] - last[1];
            let dist = dx.hypot(dy);
            if dist <= 1.0e-6 {
                None
            } else {
                let bearing = survey::format_quadrant_bearing(
                    dx,
                    dy,
                    survey::BearingFormat::Spaced,
                );
                Some(format!(" [{bearing}, {dist:.2}]"))
            }
        }).unwrap_or_default();

        if self.vertices.len() < 3 {
            format!(
                "Sketch Property Line ({} pts){live_segment}: Pick next point:",
                self.vertices.len()
            )
        } else {
            format!(
                "Sketch Property Line ({} pts){live_segment}: Pick next point, click Start, or Enter to close:",
                self.vertices.len()
            )
        }
    }

    fn on_cursor_move(&mut self, point: [f64; 3]) -> Vec<PreviewWire> {
        if !point.iter().all(|v| v.is_finite()) {
            return Vec::new();
        }
        let snapped = if let Some(first) = self.vertices.first() {
            [point[0], point[1], first[2]]
        } else {
            point
        };
        self.cursor = Some(snapped);
        let mut wires = Vec::new();
        for pair in self.vertices.windows(2) {
            wires.push(PreviewWire::line(pair[0], pair[1], None));
        }
        if let Some(&last) = self.vertices.last() {
            if (snapped[0] - last[0]).hypot(snapped[1] - last[1]) > 1.0e-9 {
                wires.push(PreviewWire::line(last, snapped, None));
            }
            if self.vertices.len() >= 2 {
                let first = self.vertices[0];
                if (snapped[0] - first[0]).hypot(snapped[1] - first[1]) > 1.0e-9 {
                    wires.push(PreviewWire::line(
                        snapped,
                        first,
                        Some([0.45, 0.75, 1.0, 0.65]),
                    ));
                }
            }
        }
        wires
    }

    fn on_point(&mut self, point: [f64; 3]) -> CommandStep {
        if !point.iter().all(|v| v.is_finite()) {
            return CommandStep::Cancel;
        }
        let z = self.vertices.first().map(|v| v[2]).unwrap_or(point[2]);
        let pt = [point[0], point[1], z];

        if let Some(&last) = self.vertices.last() {
            if (pt[0] - last[0]).hypot(pt[1] - last[1]) <= 1.0e-6 {
                return CommandStep::NeedPoint;
            }
        }
        if self.vertices.len() >= 3 {
            let first = self.vertices[0];
            if (pt[0] - first[0]).hypot(pt[1] - first[1]) <= 1.0e-3 {
                return self.build_polyline_step(true);
            }
        }
        self.vertices.push(pt);
        self.cursor = None;
        CommandStep::NeedPoint
    }

    fn on_enter(&mut self) -> CommandStep {
        if self.vertices.len() >= 3 {
            self.build_polyline_step(true)
        } else if self.vertices.len() == 2 {
            self.build_polyline_step(false)
        } else {
            CommandStep::Cancel
        }
    }
}

struct SketchRectBoundary {
    first: Option<[f64; 3]>,
    cursor: Option<[f64; 3]>,
    layer: String,
}

impl SketchRectBoundary {
    fn new(layer: String) -> Self {
        Self {
            first: None,
            cursor: None,
            layer,
        }
    }

    fn rect_corners(a: [f64; 3], b: [f64; 3]) -> Vec<[f64; 3]> {
        let min_x = a[0].min(b[0]);
        let max_x = a[0].max(b[0]);
        let min_y = a[1].min(b[1]);
        let max_y = a[1].max(b[1]);
        vec![
            [min_x, min_y, a[2]],
            [max_x, min_y, a[2]],
            [max_x, max_y, a[2]],
            [min_x, max_y, a[2]],
        ]
    }
}

impl InteractiveCommand for SketchRectBoundary {
    fn prompt(&self) -> String {
        let Some(first) = self.first else {
            return "Sketch Property Rectangle: Pick first corner (Esc to cancel):".to_string();
        };
        if let Some(c) = self.cursor {
            let w = (c[0] - first[0]).abs();
            let h = (c[1] - first[1]).abs();
            format!(
                "Sketch Property Rectangle [{w:.2} x {h:.2}, area {:.2}]: Pick opposite corner:",
                w * h
            )
        } else {
            "Sketch Property Rectangle: Pick opposite corner:".to_string()
        }
    }

    fn on_cursor_move(&mut self, point: [f64; 3]) -> Vec<PreviewWire> {
        if !point.iter().all(|v| v.is_finite()) {
            return Vec::new();
        }
        let Some(first) = self.first else {
            return Vec::new();
        };
        self.cursor = Some([point[0], point[1], first[2]]);
        let corners = Self::rect_corners(first, point);
        (0..4)
            .map(|i| PreviewWire::line(corners[i], corners[(i + 1) % 4], None))
            .collect()
    }

    fn on_point(&mut self, point: [f64; 3]) -> CommandStep {
        if !point.iter().all(|v| v.is_finite()) {
            return CommandStep::Cancel;
        }
        let Some(first) = self.first else {
            self.first = Some(point);
            return CommandStep::NeedPoint;
        };
        if (point[0] - first[0]).abs() <= 1.0e-6 || (point[1] - first[1]).abs() <= 1.0e-6 {
            return CommandStep::NeedPoint;
        }
        let corners = Self::rect_corners(first, point);
        let bulges = vec![0.0; 4];
        let boundary = survey::boundary_from_points(&corners, &bulges, true);
        let csv = table_from_polyline(&corners, &bulges, true);
        let polyline = make_property_polyline(corners[0], &boundary, &self.layer, &csv);
        CommandStep::CommitAndEnd(EntityType::LwPolyline(polyline))
    }
}

struct PickSingleBoundaryCommand {
    prompt_text: String,
    dispatch_cmd: &'static str,
    pending_pick: Arc<Mutex<Vec<Handle>>>,
}

impl PickSingleBoundaryCommand {
    fn new(
        prompt_text: impl Into<String>,
        dispatch_cmd: &'static str,
        pending_pick: Arc<Mutex<Vec<Handle>>>,
    ) -> Self {
        Self {
            prompt_text: prompt_text.into(),
            dispatch_cmd,
            pending_pick,
        }
    }
}

impl InteractiveCommand for PickSingleBoundaryCommand {
    fn prompt(&self) -> String {
        self.prompt_text.clone()
    }

    fn needs_object_pick(&self) -> bool {
        true
    }

    fn on_object_pick(&mut self, handle: Handle, _pt: [f64; 3]) -> CommandStep {
        if let Ok(mut slot) = self.pending_pick.lock() {
            *slot = vec![handle];
        }
        CommandStep::Dispatch(self.dispatch_cmd.to_string())
    }

    fn on_point(&mut self, _pt: [f64; 3]) -> CommandStep {
        CommandStep::NeedPoint
    }
}

struct PickChainCommand {
    handles: Vec<Handle>,
    polyline_handles: HashSet<Handle>,
    pending_pick: Arc<Mutex<Vec<Handle>>>,
}

impl InteractiveCommand for PickChainCommand {
    fn prompt(&self) -> String {
        if self.handles.is_empty() {
            "Pick a polyline or pick lines/arcs for Property Line (Enter to finish):".to_string()
        } else {
            format!(
                "Pick lines/arcs [{} selected] (pick more or press Enter to convert):",
                self.handles.len()
            )
        }
    }

    fn is_selection_gathering(&self) -> bool {
        true
    }

    fn accepts_drag_selection(&self) -> bool {
        true
    }

    fn on_selection_complete(&mut self, handles: Vec<Handle>) -> CommandStep {
        if handles.is_empty() {
            return CommandStep::NeedPoint;
        }
        if self.handles.is_empty()
            && handles.len() == 1
            && self.polyline_handles.contains(&handles[0])
        {
            if let Ok(mut slot) = self.pending_pick.lock() {
                *slot = handles;
            }
            return CommandStep::Dispatch("PL_PICK_LINES_APPLY".to_string());
        }
        for handle in handles {
            if !self.handles.contains(&handle) {
                self.handles.push(handle);
            }
        }
        CommandStep::NeedPoint
    }

    fn on_enter(&mut self) -> CommandStep {
        if self.handles.is_empty() {
            return CommandStep::Cancel;
        }
        if let Ok(mut slot) = self.pending_pick.lock() {
            *slot = std::mem::take(&mut self.handles);
        }
        CommandStep::Dispatch("PL_PICK_LINES_APPLY".to_string())
    }

    fn on_point(&mut self, _pt: [f64; 3]) -> CommandStep {
        CommandStep::NeedPoint
    }
}

struct TagOptionsCommand {
    handle: Handle,
    default_height: f64,
    error: Option<String>,
    active: bool,
    pending_tag: Arc<Mutex<Option<PendingTagApply>>>,
}

impl InteractiveCommand for TagOptionsCommand {
    fn prompt(&self) -> String {
        "Configure property line segment tags (Esc to cancel):".to_string()
    }

    fn modal_dialog(&self) -> Option<DialogDef> {
        if !self.active {
            return None;
        }
        let mut dialog = DialogDef::new("Tag Property Line Segments")
            .width(480.0)
            .submit_label("Apply Tags")
            .cancel_label("Cancel");
        if let Some(err) = &self.error {
            dialog = dialog.with_control(DialogControl::InfoText {
                text: format!("Invalid option: {err}"),
                align_h: None,
            });
        }
        dialog = dialog
            .with_row(DialogRow::new(vec![
                DialogControl::Number {
                    id: "height".to_string(),
                    label: "Text height".to_string(),
                    default: self.default_height,
                    min: Some(0.01),
                    max: Some(1000.0),
                    step: Some(0.1),
                },
                DialogControl::Text {
                    id: "unit".to_string(),
                    label: "Distance unit".to_string(),
                    default: "m".to_string(),
                },
                DialogControl::Number {
                    id: "decimals".to_string(),
                    label: "Decimals".to_string(),
                    default: 2.0,
                    min: Some(0.0),
                    max: Some(6.0),
                    step: Some(1.0),
                },
            ]))
            .with_control(DialogControl::Checkbox {
                id: "tick".to_string(),
                label: "Draw midpoint perpendicular tick line".to_string(),
                default: true,
            })
            .with_control(DialogControl::Checkbox {
                id: "replace".to_string(),
                label: "Replace existing segment tags on this boundary".to_string(),
                default: true,
            });
        Some(dialog)
    }

    fn on_dialog_response(&mut self, response: DialogResponse) -> CommandStep {
        match response {
            DialogResponse::Cancelled => {
                self.active = false;
                CommandStep::Cancel
            }
            DialogResponse::Submitted(values) => {
                let height = match values.get("height") {
                    Some(DialogValue::Number(v)) if v.is_finite() && *v > 0.0 => *v,
                    _ => {
                        self.error = Some("Text height must be positive.".to_string());
                        return CommandStep::NeedPoint;
                    }
                };
                let decimals = match values.get("decimals") {
                    Some(DialogValue::Number(v)) if v.is_finite() && *v >= 0.0 => {
                        (*v as usize).min(6)
                    }
                    _ => 2,
                };
                let unit_suffix = match values.get("unit") {
                    Some(DialogValue::Text(s)) => s.trim().to_string(),
                    _ => "m".to_string(),
                };
                let include_tick = matches!(values.get("tick"), Some(DialogValue::Bool(true)) | None);
                let replace_existing =
                    matches!(values.get("replace"), Some(DialogValue::Bool(true)) | None);

                self.error = None;
                self.active = false;
                if let Ok(mut slot) = self.pending_tag.lock() {
                    *slot = Some(PendingTagApply {
                        handle: self.handle,
                        text_height: height,
                        decimals,
                        unit_suffix,
                        include_tick,
                        replace_existing,
                    });
                }
                CommandStep::Dispatch("PL_APPLY_TAG".to_string())
            }
        }
    }

    fn on_point(&mut self, _pt: [f64; 3]) -> CommandStep {
        CommandStep::NeedPoint
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ScheduleTableStage {
    Dialog,
    PlacePoint,
    Done,
}

struct ScheduleTableCommand {
    handle: Handle,
    segments: Vec<drafting::SegmentGeometry>,
    default_height: f64,
    error: Option<String>,
    stage: ScheduleTableStage,
    title: String,
    text_height: f64,
    decimals: usize,
    compact_bearing: bool,
    label_stations: bool,
    pending_table: Arc<Mutex<Option<PendingTableApply>>>,
}

impl InteractiveCommand for ScheduleTableCommand {
    fn prompt(&self) -> String {
        match self.stage {
            ScheduleTableStage::Dialog => {
                "Configure Rumbos y Distancias table (Esc to cancel):".to_string()
            }
            ScheduleTableStage::PlacePoint => format!(
                "Click top-left insertion point for '{}' ({} rows):",
                self.title,
                self.segments.len()
            ),
            ScheduleTableStage::Done => "Table inserted.".to_string(),
        }
    }

    fn modal_dialog(&self) -> Option<DialogDef> {
        if self.stage != ScheduleTableStage::Dialog {
            return None;
        }
        let mut dialog = DialogDef::new("Bearings and Distances Table")
            .width(480.0)
            .submit_label("Place Table")
            .cancel_label("Cancel");
        if let Some(err) = &self.error {
            dialog = dialog.with_control(DialogControl::InfoText {
                text: format!("Invalid option: {err}"),
                align_h: None,
            });
        }
        dialog = dialog
            .with_control(DialogControl::Text {
                id: "title".to_string(),
                label: "Table title".to_string(),
                default: self.title.clone(),
            })
            .with_row(DialogRow::new(vec![
                DialogControl::Number {
                    id: "height".to_string(),
                    label: "Text height".to_string(),
                    default: self.default_height,
                    min: Some(0.01),
                    max: Some(1000.0),
                    step: Some(0.1),
                },
                DialogControl::Number {
                    id: "decimals".to_string(),
                    label: "Distance decimals".to_string(),
                    default: 2.0,
                    min: Some(0.0),
                    max: Some(6.0),
                    step: Some(1.0),
                },
            ]))
            .with_control(DialogControl::Checkbox {
                id: "compact".to_string(),
                label: "Compact bearing format (e.g. S68°49'18\"E)".to_string(),
                default: true,
            })
            .with_control(DialogControl::Checkbox {
                id: "stations".to_string(),
                label: "Label corner stations (1, 2, 3...) on drawing".to_string(),
                default: true,
            });
        Some(dialog)
    }

    fn on_dialog_response(&mut self, response: DialogResponse) -> CommandStep {
        match response {
            DialogResponse::Cancelled => {
                self.stage = ScheduleTableStage::Done;
                CommandStep::Cancel
            }
            DialogResponse::Submitted(values) => {
                let height = match values.get("height") {
                    Some(DialogValue::Number(v)) if v.is_finite() && *v > 0.0 => *v,
                    _ => {
                        self.error = Some("Text height must be positive.".to_string());
                        return CommandStep::NeedPoint;
                    }
                };
                self.text_height = height;
                self.decimals = match values.get("decimals") {
                    Some(DialogValue::Number(v)) if v.is_finite() && *v >= 0.0 => {
                        (*v as usize).min(6)
                    }
                    _ => 2,
                };
                if let Some(DialogValue::Text(t)) = values.get("title") {
                    let trimmed = t.trim();
                    if !trimmed.is_empty() {
                        self.title = trimmed.to_string();
                    }
                }
                self.compact_bearing =
                    matches!(values.get("compact"), Some(DialogValue::Bool(true)) | None);
                self.label_stations =
                    matches!(values.get("stations"), Some(DialogValue::Bool(true)) | None);
                self.error = None;
                self.stage = ScheduleTableStage::PlacePoint;
                CommandStep::NeedPoint
            }
        }
    }

    fn on_cursor_move(&mut self, point: [f64; 3]) -> Vec<PreviewWire> {
        if self.stage != ScheduleTableStage::PlacePoint || !point.iter().all(|v| v.is_finite()) {
            return Vec::new();
        }
        let (col_widths, row_heights) = drafting::table_layout_metrics(
            &self.segments,
            &self.title,
            self.text_height,
            self.decimals,
            self.compact_bearing,
        );
        drafting::table_preview_wires(point, &col_widths, &row_heights)
    }

    fn on_point(&mut self, point: [f64; 3]) -> CommandStep {
        if self.stage != ScheduleTableStage::PlacePoint || !point.iter().all(|v| v.is_finite()) {
            return CommandStep::NeedPoint;
        }
        self.stage = ScheduleTableStage::Done;
        if let Ok(mut slot) = self.pending_table.lock() {
            *slot = Some(PendingTableApply {
                handle: self.handle,
                insertion: point,
                title: self.title.clone(),
                text_height: self.text_height,
                decimals: self.decimals,
                compact_bearing: self.compact_bearing,
                label_stations: self.label_stations,
            });
        }
        CommandStep::Dispatch("PL_APPLY_TABLE".to_string())
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum AreaStage {
    Dialog,
    PlacePoint,
    Done,
}

struct AreaOptionsCommand {
    handle: Handle,
    stage: AreaStage,
    area_m2: f64,
    centroid: [f64; 3],
    prefix: String,
    unit: String,
    text_height: f64,
    decimals: usize,
    replace_existing: bool,
    error: Option<String>,
    pending_area: Arc<Mutex<Option<PendingAreaApply>>>,
}

impl AreaOptionsCommand {
    fn formatted_text(&self) -> String {
        drafting::format_parcel_area(self.area_m2, self.decimals, &self.prefix, &self.unit)
    }

    fn queue_apply(&self, insertion: [f64; 3]) {
        if let Ok(mut slot) = self.pending_area.lock() {
            *slot = Some(PendingAreaApply {
                handle: self.handle,
                insertion,
                text: self.formatted_text(),
                text_height: self.text_height,
                replace_existing: self.replace_existing,
            });
        }
    }
}

impl InteractiveCommand for AreaOptionsCommand {
    fn prompt(&self) -> String {
        match self.stage {
            AreaStage::Dialog => "Configure parcel area label (m²) and click Continue:".to_string(),
            AreaStage::PlacePoint => {
                "Click location in drawing to place the parcel area label:".to_string()
            }
            AreaStage::Done => String::new(),
        }
    }

    fn modal_dialog(&self) -> Option<DialogDef> {
        if self.stage != AreaStage::Dialog {
            return None;
        }
        let mut dialog = DialogDef::new("Property Line Area Label (m²)")
            .width(480.0)
            .submit_label("Place Label")
            .cancel_label("Cancel");
        if let Some(err) = &self.error {
            dialog = dialog.with_control(DialogControl::InfoText {
                text: format!("Invalid option: {err}"),
                align_h: None,
            });
        }
        dialog = dialog
            .with_control(DialogControl::InfoText {
                text: format!(
                    "Calculated parcel area: {}",
                    drafting::format_parcel_area(self.area_m2, 2, "", "m²")
                ),
                align_h: None,
            })
            .with_row(DialogRow::new(vec![
                DialogControl::Text {
                    id: "prefix".to_string(),
                    label: "Label prefix".to_string(),
                    default: self.prefix.clone(),
                },
                DialogControl::Text {
                    id: "unit".to_string(),
                    label: "Area unit".to_string(),
                    default: self.unit.clone(),
                },
            ]))
            .with_row(DialogRow::new(vec![
                DialogControl::Number {
                    id: "height".to_string(),
                    label: "Text height".to_string(),
                    default: self.text_height,
                    min: Some(0.01),
                    max: Some(1000.0),
                    step: Some(0.1),
                },
                DialogControl::Number {
                    id: "decimals".to_string(),
                    label: "Decimals".to_string(),
                    default: self.decimals as f64,
                    min: Some(0.0),
                    max: Some(6.0),
                    step: Some(1.0),
                },
            ]))
            .with_control(DialogControl::Checkbox {
                id: "center".to_string(),
                label: "Place automatically at parcel center".to_string(),
                default: false,
            })
            .with_control(DialogControl::Checkbox {
                id: "replace".to_string(),
                label: "Replace existing area label on this boundary".to_string(),
                default: self.replace_existing,
            });
        Some(dialog)
    }

    fn on_dialog_response(&mut self, response: DialogResponse) -> CommandStep {
        match response {
            DialogResponse::Cancelled => {
                self.stage = AreaStage::Done;
                CommandStep::Cancel
            }
            DialogResponse::Submitted(values) => {
                let height = match values.get("height") {
                    Some(DialogValue::Number(v)) if v.is_finite() && *v > 0.0 => *v,
                    _ => {
                        self.error = Some("Text height must be positive.".to_string());
                        return CommandStep::NeedPoint;
                    }
                };
                self.text_height = height;
                self.decimals = match values.get("decimals") {
                    Some(DialogValue::Number(v)) if v.is_finite() && *v >= 0.0 => {
                        (*v as usize).min(6)
                    }
                    _ => 2,
                };
                if let Some(DialogValue::Text(p)) = values.get("prefix") {
                    self.prefix = p.trim().to_string();
                }
                if let Some(DialogValue::Text(u)) = values.get("unit") {
                    self.unit = u.trim().to_string();
                }
                self.replace_existing =
                    matches!(values.get("replace"), Some(DialogValue::Bool(true)) | None);
                self.error = None;
                let auto_center = matches!(values.get("center"), Some(DialogValue::Bool(true)));
                if auto_center {
                    self.stage = AreaStage::Done;
                    self.queue_apply(self.centroid);
                    CommandStep::Dispatch("PL_APPLY_AREA".to_string())
                } else {
                    self.stage = AreaStage::PlacePoint;
                    CommandStep::NeedPoint
                }
            }
        }
    }

    fn on_cursor_move(&mut self, point: [f64; 3]) -> Vec<PreviewWire> {
        if self.stage != AreaStage::PlacePoint || !point.iter().all(|v| v.is_finite()) {
            return Vec::new();
        }
        let text = self.formatted_text();
        drafting::area_preview_wires(point, &text, self.text_height)
    }

    fn on_point(&mut self, point: [f64; 3]) -> CommandStep {
        if self.stage != AreaStage::PlacePoint || !point.iter().all(|v| v.is_finite()) {
            return CommandStep::NeedPoint;
        }
        self.stage = AreaStage::Done;
        self.queue_apply(point);
        CommandStep::Dispatch("PL_APPLY_AREA".to_string())
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum NorthArrowStage {
    Dialog,
    PlacePoint,
    Done,
}

struct NorthArrowCommand {
    stage: NorthArrowStage,
    scale: f64,
    rotation_deg: f64,
    error: Option<String>,
    pending_north: Arc<Mutex<Option<PendingNorthApply>>>,
}

impl InteractiveCommand for NorthArrowCommand {
    fn prompt(&self) -> String {
        match self.stage {
            NorthArrowStage::Dialog => {
                "Configure North Arrow block options (Esc to cancel):".to_string()
            }
            NorthArrowStage::PlacePoint => {
                "Click location in drawing to place the North Arrow block:".to_string()
            }
            NorthArrowStage::Done => String::new(),
        }
    }

    fn modal_dialog(&self) -> Option<DialogDef> {
        if self.stage != NorthArrowStage::Dialog {
            return None;
        }
        let mut dialog = DialogDef::new("Property Line North Arrow")
            .width(440.0)
            .submit_label("Place North")
            .cancel_label("Cancel");
        if let Some(err) = &self.error {
            dialog = dialog.with_control(DialogControl::InfoText {
                text: format!("Invalid option: {err}"),
                align_h: None,
            });
        }
        dialog = dialog
            .with_control(DialogControl::InfoText {
                text: format!(
                    "Inserts CAD block '{}' on the Property Line layer. Rotation 0° points North (+Y).",
                    drafting::NORTH_BLOCK_NAME
                ),
                align_h: None,
            })
            .with_row(DialogRow::new(vec![
                DialogControl::Number {
                    id: "scale".to_string(),
                    label: "Block scale factor".to_string(),
                    default: self.scale,
                    min: Some(0.05),
                    max: Some(1000.0),
                    step: Some(0.25),
                },
                DialogControl::Number {
                    id: "rotation".to_string(),
                    label: "Rotation (degrees, CCW)".to_string(),
                    default: self.rotation_deg,
                    min: Some(-360.0),
                    max: Some(360.0),
                    step: Some(5.0),
                },
            ]));
        Some(dialog)
    }

    fn on_dialog_response(&mut self, response: DialogResponse) -> CommandStep {
        match response {
            DialogResponse::Cancelled => {
                self.stage = NorthArrowStage::Done;
                CommandStep::Cancel
            }
            DialogResponse::Submitted(values) => {
                let scale = match values.get("scale") {
                    Some(DialogValue::Number(v)) if v.is_finite() && *v > 0.0 => *v,
                    _ => {
                        self.error = Some("Scale factor must be positive.".to_string());
                        return CommandStep::NeedPoint;
                    }
                };
                let rotation_deg = match values.get("rotation") {
                    Some(DialogValue::Number(v)) if v.is_finite() => *v,
                    _ => {
                        self.error = Some("Rotation must be a valid angle in degrees.".to_string());
                        return CommandStep::NeedPoint;
                    }
                };
                self.scale = scale;
                self.rotation_deg = rotation_deg;
                self.error = None;
                self.stage = NorthArrowStage::PlacePoint;
                CommandStep::NeedPoint
            }
        }
    }

    fn on_cursor_move(&mut self, point: [f64; 3]) -> Vec<PreviewWire> {
        if self.stage != NorthArrowStage::PlacePoint || !point.iter().all(|v| v.is_finite()) {
            return Vec::new();
        }
        drafting::north_preview_wires(point, self.scale, self.rotation_deg)
    }

    fn on_point(&mut self, point: [f64; 3]) -> CommandStep {
        if self.stage != NorthArrowStage::PlacePoint || !point.iter().all(|v| v.is_finite()) {
            return CommandStep::NeedPoint;
        }
        self.stage = NorthArrowStage::Done;
        if let Ok(mut slot) = self.pending_north.lock() {
            *slot = Some(PendingNorthApply {
                insertion: point,
                scale: self.scale,
                rotation_deg: self.rotation_deg,
            });
        }
        CommandStep::Dispatch("PL_APPLY_NORTH".to_string())
    }
}

ocs_plugin_api::export_plugin!(PropertyLinesPlugin::default());


#[cfg(test)]
mod existing_polyline_tests {
    use super::*;
    use ocs_plugin_api::host::acadrust::entities::mtext::AttachmentPoint;
    use ocs_plugin_api::host::acadrust::entities::{Arc as CadArc, Line};
    use ocs_plugin_api::host::acadrust::xdata::XDataValue;
    use ocs_plugin_api::host::acadrust::{Vector2, Vector3};

    #[test]
    fn imports_closed_existing_polyline_as_survey_table() {
        let mut polyline = LwPolyline::new();
        polyline.is_closed = true;
        polyline.add_point_with_bulge(Vector2::new(0.0, 0.0), 0.0);
        polyline.add_point_with_bulge(Vector2::new(10.0, 0.0), 0.0);
        polyline.add_point_with_bulge(Vector2::new(10.0, 5.0), 0.0);
        polyline.add_point_with_bulge(Vector2::new(0.0, 5.0), 0.0);
        let entity = EntityType::LwPolyline(polyline);

        let (points, bulges, closed) = extract_polyline(&entity).unwrap();
        let table = table_from_polyline(&points, &bulges, closed);
        let boundary = survey::from_csv(&table).unwrap();
        assert!(boundary.closed);
        assert_eq!(boundary.vertices.len(), 4);
        assert!((boundary.area.unwrap() - 50.0).abs() < 1.0e-8);
    }

    #[test]
    fn preserves_existing_arc_bulge_through_table_conversion() {
        let mut polyline = LwPolyline::new();
        polyline.is_closed = true;
        polyline.add_point_with_bulge(Vector2::new(0.0, 0.0), 0.25);
        polyline.add_point_with_bulge(Vector2::new(10.0, 0.0), 0.0);
        polyline.add_point_with_bulge(Vector2::new(5.0, 5.0), 0.0);
        let entity = EntityType::LwPolyline(polyline);

        let (points, bulges, closed) = extract_polyline(&entity).unwrap();
        let table = table_from_polyline(&points, &bulges, closed);
        let boundary = survey::from_csv(&table).unwrap();
        assert!(boundary.closed);
        assert!((boundary.vertices[0].bulge - 0.25).abs() < 1.0e-10);
    }

    #[test]
    fn skips_near_zero_chords_when_converting_polyline() {
        let mut polyline = LwPolyline::new();
        polyline.is_closed = true;
        polyline.add_point_with_bulge(Vector2::new(0.0, 0.0), 0.0);
        polyline.add_point_with_bulge(Vector2::new(1.0e-13, 0.0), 0.0);
        polyline.add_point_with_bulge(Vector2::new(10.0, 0.0), 0.0);
        polyline.add_point_with_bulge(Vector2::new(10.0, 5.0), 0.0);
        polyline.add_point_with_bulge(Vector2::new(0.0, 5.0), 0.0);
        let entity = EntityType::LwPolyline(polyline);

        let (points, bulges, closed) = extract_polyline(&entity).unwrap();
        let table = table_from_polyline(&points, &bulges, closed);
        let boundary = survey::from_csv(&table).unwrap();
        assert!(boundary.closed);
        assert_eq!(boundary.vertices.len(), 4);
        assert!((boundary.area.unwrap() - 50.0).abs() < 1.0e-8);
    }

    #[test]
    fn table_record_chunks_by_utf8_byte_length() {
        let csv = "20,N,45° 00' 00\",E,Line,,\n".repeat(25);
        let record = table_record(&csv);
        for value in &record.values {
            if let XDataValue::String(text) = value {
                assert!(text.len() <= 240);
            }
        }
        assert_eq!(read_table(Some(&record)).as_deref(), Some(csv.as_str()));
    }

    #[test]
    fn preview_wires_cover_lines_and_arcs() {
        let csv = "Distance,N/S,Bearing,E/W,Type,Radius,L/R\n\
                   10,N,90,E,Arc,10,L\n\
                   10,S,90,W,Line,,\n\
                   1,N,0,E,Line,,\n";
        let boundary = survey::from_csv(csv).unwrap();
        let wires = preview_wires_for_boundary(&boundary, [5.0, 6.0, 1.0]);
        assert_eq!(wires.len(), 3);
        assert!(matches!(wires[0], PreviewWire::Arc { .. }));
        assert!(matches!(wires[1], PreviewWire::Polyline { .. }));
        assert!(matches!(wires[2], PreviewWire::Polyline { .. }));
    }

    #[test]
    fn csv_and_native_dialog_rows_roundtrip_including_spanish_fields() {
        let csv = "\u{feff}Distance,N/S,Bearing,E/W,Type,Radius,L/R\n\
                   20,N,82° 30' 15\",E,Línea,,\n\
                   15,S,0,O,Arco,10,I\n\
                   ,,,,Cerrar,,\n";
        let (rows, close_polygon) = csv_to_dialog_rows(csv);
        assert!(close_polygon);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0], vec!["20", "N", "82", "30", "15", "E", "Line", "", "R"]);
        assert_eq!(rows[1], vec!["15", "S", "0", "0", "0", "W", "Arc", "10", "L"]);

        let (generated_csv, boundary) = dialog_rows_to_boundary(&rows, close_polygon).unwrap();
        assert!(boundary.closed);
        assert_eq!(boundary.vertices.len(), 3);
        let parsed = survey::from_csv(&generated_csv).unwrap();
        assert!(parsed.closed);
    }

    #[test]
    fn edit_boundary_dialog_dispatches_apply_when_valid() {
        let pending = Arc::new(Mutex::new(None));
        let (rows, close_polygon) = csv_to_dialog_rows(
            "Distance,N/S,Bearing,E/W,Type,Radius,L/R\n\
             10,N,90,E,Line,,\n\
             10,S,0,E,Line,,\n\
             10,S,90,W,Line,,\n",
        );
        let mut cmd = EditBoundary {
            handle: Handle::new(42),
            origin: [1.0, 2.0, 0.0],
            rows,
            close_polygon,
            error: None,
            active: true,
            pending_edit: Arc::clone(&pending),
        };
        assert!(cmd.modal_dialog().is_some());

        let mut values = HashMap::new();
        values.insert(
            DIALOG_TABLE_ID.to_string(),
            DialogValue::Table(cmd.rows.clone()),
        );
        values.insert(DIALOG_CLOSE_ID.to_string(), DialogValue::Bool(true));

        let step = cmd.on_dialog_response(DialogResponse::Submitted(values));
        assert!(matches!(step, CommandStep::Dispatch(ref s) if s == "PL_APPLY_EDIT"));
        assert!(cmd.modal_dialog().is_none());
        let saved = pending.lock().unwrap().take().expect("pending edit saved");
        assert_eq!(saved.handle, Handle::new(42));
        assert!(saved.boundary.closed);
    }

    #[test]
    fn place_boundary_dialog_preserves_rows_on_error_and_commits_when_valid() {
        let (rows, close_polygon) = csv_to_dialog_rows(TABLE_HEADER);
        assert_eq!(rows.len(), 1);
        assert!(!close_polygon);

        let mut cmd = PlaceBoundary {
            state: PlacementState::Editing {
                rows,
                close_polygon,
                error: None,
            },
            layer: "PROP".to_string(),
        };
        assert!(cmd.modal_dialog().is_some());

        // Submit an invalid arc row (radius too small); dialog must stay open with user's row.
        let mut bad_values = HashMap::new();
        bad_values.insert(
            DIALOG_TABLE_ID.to_string(),
            DialogValue::Table(vec![vec![
                "20".into(), "N".into(), "90".into(), "".into(), "".into(), "E".into(), "Arc".into(), "5".into(), "".into(),
            ]]),
        );
        bad_values.insert(DIALOG_CLOSE_ID.to_string(), DialogValue::Bool(true));
        let step = cmd.on_dialog_response(DialogResponse::Submitted(bad_values));
        assert!(matches!(step, CommandStep::NeedPoint));
        let reopened = cmd.modal_dialog().expect("dialog should reopen on validation error");
        assert!(reopened.rows.len() >= 3);

        // Fix the row to a valid rectangle and submit.
        let mut good_values = HashMap::new();
        good_values.insert(
            DIALOG_TABLE_ID.to_string(),
            DialogValue::Table(vec![
                vec!["20".into(), "N".into(), "90".into(), "".into(), "".into(), "E".into(), "".into(), "".into(), "".into()],
                vec!["15".into(), "S".into(), "".into(), "".into(), "".into(), "E".into(), "Line".into(), "".into(), "".into()],
                vec!["20".into(), "S".into(), "90".into(), "".into(), "".into(), "O".into(), "Línea".into(), "".into(), "".into()],
            ]),
        );
        good_values.insert(DIALOG_CLOSE_ID.to_string(), DialogValue::Bool(true));
        let step = cmd.on_dialog_response(DialogResponse::Submitted(good_values));
        assert!(matches!(step, CommandStep::NeedPoint));
        assert!(cmd.modal_dialog().is_none());
        assert_eq!(cmd.on_cursor_move([0.0, 0.0, 0.0]).len(), 4);

        let commit = cmd.on_point([10.0, 20.0, 0.0]);
        let CommandStep::CommitAndEnd(EntityType::LwPolyline(poly)) = commit else {
            panic!("expected committed LwPolyline");
        };
        assert_eq!(poly.common.layer, "PROP");
        assert!(poly.is_closed);
        assert_eq!(poly.vertices.len(), 4);
    }

    #[test]
    fn layer_name_detection_matches_english_and_spanish_names() {
        assert_eq!(
            drafting::find_existing_property_layer_name(["0", "Defpoints", "Linea de Propiedad"]),
            Some("Linea de Propiedad")
        );
        assert_eq!(
            drafting::find_existing_property_layer_name(["0", "Property Line"]),
            Some("Property Line")
        );
        assert_eq!(
            drafting::find_existing_property_layer_name(["0", "Línea de Propiedad"]),
            Some("Línea de Propiedad")
        );
        assert_eq!(
            drafting::find_existing_property_layer_name(["0", "Walls"]),
            None
        );
    }

    #[test]
    fn sketch_mode_previews_and_closes_on_start_or_enter() {
        let mut sketch = SketchBoundary::new("Property Line".to_string());
        assert!(matches!(sketch.on_point([0.0, 0.0, 0.0]), CommandStep::NeedPoint));
        assert!(matches!(sketch.on_point([13.5, 0.0, 0.0]), CommandStep::NeedPoint));
        assert_eq!(sketch.on_cursor_move([13.5, -18.7, 0.0]).len(), 3);
        assert!(matches!(sketch.on_point([13.5, -18.7, 0.0]), CommandStep::NeedPoint));
        assert!(matches!(sketch.on_point([0.0, -18.7, 0.0]), CommandStep::NeedPoint));

        // Clicking back on start snaps and closes polygon with survey XDATA
        let commit = sketch.on_point([0.0, 0.0, 0.0]);
        let CommandStep::CommitAndEnd(EntityType::LwPolyline(poly)) = commit else {
            panic!("expected closed property polyline on start click");
        };
        assert_eq!(poly.common.layer, "Property Line");
        assert!(poly.is_closed);
        assert_eq!(poly.vertices.len(), 4);
        assert!(read_table(poly.common.extended_data.get_record(XDATA_APP)).is_some());

        // Enter also closes when >= 3 vertices
        let mut sketch2 = SketchBoundary::new("Property Line".to_string());
        sketch2.on_point([0.0, 0.0, 0.0]);
        sketch2.on_point([10.0, 0.0, 0.0]);
        sketch2.on_point([10.0, 10.0, 0.0]);
        let CommandStep::CommitAndEnd(EntityType::LwPolyline(poly2)) = sketch2.on_enter() else {
            panic!("expected closed polygon on Enter");
        };
        assert!(poly2.is_closed);
        assert_eq!(poly2.vertices.len(), 3);
    }

    #[test]
    fn sketch_rect_creates_closed_rectangle_with_xdata() {
        let mut rect = SketchRectBoundary::new("Property Line".to_string());
        assert!(matches!(rect.on_point([0.0, 0.0, 0.0]), CommandStep::NeedPoint));
        assert_eq!(rect.on_cursor_move([13.5, -18.7, 0.0]).len(), 4);
        let CommandStep::CommitAndEnd(EntityType::LwPolyline(poly)) =
            rect.on_point([13.5, -18.7, 0.0])
        else {
            panic!("expected rectangle polyline");
        };
        assert_eq!(poly.common.layer, "Property Line");
        assert!(poly.is_closed);
        assert_eq!(poly.vertices.len(), 4);
        assert!(read_table(poly.common.extended_data.get_record(XDATA_APP)).is_some());
    }

    #[test]
    fn chains_mixed_ordered_and_reversed_lines_and_arc_into_closed_boundary() {
        let e1 = EntityType::Line(Line::from_coords(0.0, 0.0, 0.0, 10.0, 0.0, 0.0));
        // Reversed second line
        let e2 = EntityType::Line(Line::from_coords(10.0, 10.0, 0.0, 10.0, 0.0, 0.0));
        // Quarter-circle arc from (10,10) to (0,10)? Center (5,10), radius 5: angles 0..PI
        let arc = CadArc::from_center_radius_angles(
            Vector3::new(5.0, 10.0, 0.0),
            5.0,
            0.0,
            std::f64::consts::PI,
        );
        let e3 = EntityType::Arc(arc);
        let e4 = EntityType::Line(Line::from_coords(0.0, 10.0, 0.0, 0.0, 0.0, 0.0));

        let (points, bulges, closed) =
            drafting::chain_entities_to_polyline(&[&e1, &e2, &e3, &e4]).unwrap();
        assert!(closed);
        assert_eq!(points.len(), 4);
        assert!((bulges[2] - 1.0).abs() < 1.0e-8);
    }

    #[test]
    fn formats_bearings_tags_and_bearings_table_correctly() {
        assert_eq!(
            survey::format_quadrant_bearing(13.5, 0.0, survey::BearingFormat::Spaced),
            "N 90° 00' 00\" E"
        );
        assert_eq!(
            survey::format_quadrant_bearing(13.5, -1.0e-6, survey::BearingFormat::Spaced),
            "S 90° 00' 00\" E"
        );
        let theta = (68.0 + 49.0 / 60.0 + 18.0 / 3600.0_f64).to_radians();
        assert_eq!(
            survey::format_quadrant_bearing(
                11.21 * theta.sin(),
                -11.21 * theta.cos(),
                survey::BearingFormat::Compact,
            ),
            "S68°49'18\"E"
        );

        // Closed CCW lot 13.5 x 18.7
        let pts = vec![
            [0.0, 0.0, 0.0],
            [13.5, 0.0, 0.0],
            [13.5, 18.7, 0.0],
            [0.0, 18.7, 0.0],
        ];
        let bulges = vec![0.0; 4];
        let segments = drafting::extract_segments(&pts, &bulges, true);
        assert_eq!(segments.len(), 4);
        // Bottom edge outward normal is (0, -1), top edge is (0, 1), right edge is (1, 0)
        assert!((segments[0].outward_normal[1] + 1.0).abs() < 1.0e-9);
        assert!((segments[1].outward_normal[0] - 1.0).abs() < 1.0e-9);
        assert!((segments[2].outward_normal[1] - 1.0).abs() < 1.0e-9);

        let (_rot, attach_top) = drafting::readable_label_orientation(
            segments[2].tangent_angle,
            segments[2].outward_normal,
        );
        assert_eq!(attach_top, AttachmentPoint::BottomCenter);

        let owner = Handle::new(99);
        let tag_entities = drafting::build_segment_tag_entities(
            owner,
            &segments,
            "Property Line",
            0.5,
            2,
            "m",
            true,
        );
        assert_eq!(tag_entities.len(), 8);
        assert!(tag_entities
            .iter()
            .all(|e| drafting::entity_belongs_to_annotation(e, owner, drafting::cleanup_kinds_for_tags())));
        let EntityType::Line(first_tick) = &tag_entities[0] else {
            panic!("expected continuous tick line");
        };
        assert_eq!(first_tick.common.linetype, "Continuous");

        let table = drafting::build_bearings_table(
            Some(owner),
            &segments,
            "Rumbos y Distancias",
            [25.0, 18.7, 0.0],
            "Property Line",
            0.5,
            2,
            true,
            None,
        );
        assert_eq!(table.rows.len(), 6);
        assert_eq!(table.columns.len(), 3);
        assert_eq!(table.cell_text(0, 0), Some("Rumbos y Distancias"));
        assert_eq!(table.cell_text(1, 0), Some("Estación"));
        assert_eq!(table.cell_text(1, 1), Some("Distancia"));
        assert_eq!(table.cell_text(1, 2), Some("Rumbo"));
        assert_eq!(table.cell_text(2, 0), Some("1-2"));
        assert_eq!(table.cell_text(2, 1), Some("13.50"));
        assert_eq!(table.cell_text(5, 0), Some("4-1"));
        assert_eq!(table.merged_ranges.len(), 1);
        assert!(table.table_style_handle.is_none());
        assert!(table.base_style.is_some());

        let title_cell = table.cell(0, 0).expect("title cell");
        let title_style = title_cell.style.as_ref().expect("title cell style");
        assert_ne!(title_style.override_flags, 0);
        assert!((title_style.text_height - 0.5 * 1.15).abs() < 1.0e-9);
        assert!((title_cell.contents[0].text_height - 0.5 * 1.15).abs() < 1.0e-9);
        assert_ne!(title_cell.contents[0].format_property_flags, 0);
        assert_ne!(title_cell.contents[0].format_override_flags, 0);

        let data_cell = table.cell(2, 2).expect("data bearing cell");
        let data_style = data_cell.style.as_ref().expect("data cell style");
        assert!((data_style.text_height - 0.5).abs() < 1.0e-9);
        assert!((data_cell.contents[0].text_height - 0.5).abs() < 1.0e-9);
        assert!(table.columns.iter().map(|c| c.width).sum::<f64>() >= 16.0);

        let area_text = drafting::format_parcel_area(252.45, 2, "Área:", "m²");
        assert_eq!(area_text, "Área: 252.45 m²");
        let centroid = drafting::parcel_centroid(&pts).expect("centroid");
        assert!((centroid[0] - 6.75).abs() < 1.0e-6);
        assert!((centroid[1] - 9.35).abs() < 1.0e-6);
        let area_ent = drafting::build_area_label_entity(owner, centroid, "Property Line", &area_text, 0.7);
        assert!(drafting::entity_belongs_to_annotation(
            &area_ent,
            owner,
            drafting::cleanup_kinds_for_area()
        ));
    }

    #[test]
    fn creation_chooser_and_annotation_dialogs_dispatch_cleanly() {
        let mut command_ids = Vec::new();
        for group in PropertyLinesModule.ribbon_groups() {
            for item in &group.tools {
                let tool = match item {
                    RibbonItem::LargeTool(t) => t,
                    RibbonItem::Tool(_) | RibbonItem::LabeledTool(_) => {
                        panic!("expected LargeTool ribbon item for clear 32px icons");
                    }
                    _ => continue,
                };
                command_ids.push(tool.id);
                match tool.icon {
                    IconKind::Svg(bytes) => {
                        let s = std::str::from_utf8(bytes).expect("utf-8 svg");
                        assert!(s.contains("<svg"), "missing <svg for {}", tool.id);
                    }
                    IconKind::Glyph(_) => panic!("expected native SVG icon for {}", tool.id),
                }
            }
        }
        assert!(command_ids.contains(&"PL_AREA"));
        assert!(command_ids.contains(&"PL_TABLE"));

        // First run: sketch mode
        let mut chooser_sketch =
            ChooseCreationModeCommand::new(drafting::DEFAULT_PROPERTY_LINE_LAYER.to_string());
        let first_dlg = chooser_sketch.modal_dialog().expect("chooser dialog");
        assert!(matches!(
            &first_dlg.rows[1].controls[0],
            DialogControl::Choice { default_index: 0, .. }
        ));
        let mut vals = HashMap::new();
        vals.insert(
            "mode".to_string(),
            DialogValue::Choice {
                index: 1,
                text: "Create by sketching".to_string(),
            },
        );
        let step = chooser_sketch.on_dialog_response(DialogResponse::Submitted(vals));
        assert!(matches!(step, CommandStep::NeedPoint));
        assert!(chooser_sketch.modal_dialog().is_none());
        assert!(matches!(chooser_sketch.on_point([0.0, 0.0, 0.0]), CommandStep::NeedPoint));

        // Subsequent run: bearings & distances opens directly in-command without Dispatch
        let mut chooser_table =
            ChooseCreationModeCommand::new(drafting::DEFAULT_PROPERTY_LINE_LAYER.to_string());
        let mut tmode = HashMap::new();
        tmode.insert(
            "mode".to_string(),
            DialogValue::Choice {
                index: 0,
                text: "Create by entering distances and bearings".to_string(),
            },
        );
        let step_table = chooser_table.on_dialog_response(DialogResponse::Submitted(tmode));
        assert!(matches!(step_table, CommandStep::NeedPoint));
        let table_dlg = chooser_table.modal_dialog().expect("table dialog should open");
        assert_eq!(table_dlg.title, "New Property Line");
        assert_eq!(table_dlg.submit_label, "Accept");

        let pending_tag = Arc::new(Mutex::new(None));
        let mut tag_cmd = TagOptionsCommand {
            handle: Handle::new(10),
            default_height: 0.5,
            error: None,
            active: true,
            pending_tag: Arc::clone(&pending_tag),
        };
        let tag_dlg = tag_cmd.modal_dialog().expect("tag dialog");
        assert_eq!(tag_dlg.rows.len(), 3);
        assert_eq!(tag_dlg.submit_label, "Apply Tags");
        assert_eq!(tag_dlg.cancel_label, "Cancel");

        let mut tvals = HashMap::new();
        tvals.insert("height".to_string(), DialogValue::Number(0.4));
        tvals.insert("unit".to_string(), DialogValue::Text("m".to_string()));
        tvals.insert("decimals".to_string(), DialogValue::Number(2.0));
        tvals.insert("tick".to_string(), DialogValue::Bool(true));
        tvals.insert("replace".to_string(), DialogValue::Bool(true));
        let tstep = tag_cmd.on_dialog_response(DialogResponse::Submitted(tvals));
        assert!(matches!(tstep, CommandStep::Dispatch(ref s) if s == "PL_APPLY_TAG"));
        assert!(pending_tag.lock().unwrap().is_some());

        let pending_area = Arc::new(Mutex::new(None));
        let mut area_cmd = AreaOptionsCommand {
            handle: Handle::new(11),
            stage: AreaStage::Dialog,
            area_m2: 252.45,
            centroid: [6.75, 9.35, 0.0],
            prefix: "Área:".to_string(),
            unit: "m²".to_string(),
            text_height: 0.7,
            decimals: 2,
            replace_existing: true,
            error: None,
            pending_area: Arc::clone(&pending_area),
        };
        let area_dlg = area_cmd.modal_dialog().expect("area dialog");
        assert_eq!(area_dlg.title, "Property Line Area Label (m²)");
        let mut avals = HashMap::new();
        avals.insert("prefix".to_string(), DialogValue::Text("Área:".to_string()));
        avals.insert("unit".to_string(), DialogValue::Text("m²".to_string()));
        avals.insert("height".to_string(), DialogValue::Number(0.65));
        avals.insert("decimals".to_string(), DialogValue::Number(2.0));
        avals.insert("center".to_string(), DialogValue::Bool(false));
        avals.insert("replace".to_string(), DialogValue::Bool(true));
        let astep = area_cmd.on_dialog_response(DialogResponse::Submitted(avals));
        assert!(matches!(astep, CommandStep::NeedPoint));
        assert!(!area_cmd.on_cursor_move([5.0, 5.0, 0.0]).is_empty());
        let aplace = area_cmd.on_point([6.75, 9.35, 0.0]);
        assert!(matches!(aplace, CommandStep::Dispatch(ref s) if s == "PL_APPLY_AREA"));
        let queued = pending_area.lock().unwrap().take().expect("queued area apply");
        assert_eq!(queued.text, "Área: 252.45 m²");
    }

    #[test]
    fn pick_lines_handles_polylines_with_widths_and_single_click_and_north_block() {
        assert_eq!(
            drafting::effective_table_title("   "),
            "Rumbos y Distancias"
        );

        let mut wide = LwPolyline::new();
        wide.is_closed = true;
        wide.constant_width = 0.15;
        wide.add_point_with_bulge(Vector2::new(0.0, 0.0), 0.0);
        wide.add_point_with_bulge(Vector2::new(10.0, 0.0), 0.0);
        wide.add_point_with_bulge(Vector2::new(10.0, 10.0), 0.0);
        wide.add_point_with_bulge(Vector2::new(0.0, 10.0), 0.0);
        let mut wide_entity = EntityType::LwPolyline(wide);
        wide_entity.common_mut().handle = Handle::new(301);

        let (pts, _bulges, closed) =
            drafting::chain_entities_to_polyline(&[&wide_entity]).expect("pick wide polyline");
        assert!(closed);
        assert_eq!(pts.len(), 4);

        let pending_pick = Arc::new(Mutex::new(Vec::new()));
        let mut poly_set = HashSet::new();
        poly_set.insert(Handle::new(301));
        let mut pick_cmd = PickChainCommand {
            handles: Vec::new(),
            polyline_handles: poly_set,
            pending_pick: Arc::clone(&pending_pick),
        };
        let step = pick_cmd.on_selection_complete(vec![Handle::new(301)]);
        assert!(matches!(step, CommandStep::Dispatch(ref s) if s == "PL_PICK_LINES_APPLY"));
        assert_eq!(*pending_pick.lock().unwrap(), vec![Handle::new(301)]);

        let pending_north = Arc::new(Mutex::new(None));
        let mut north_cmd = NorthArrowCommand {
            stage: NorthArrowStage::Dialog,
            scale: 1.0,
            rotation_deg: 0.0,
            error: None,
            pending_north: Arc::clone(&pending_north),
        };
        let dlg = north_cmd.modal_dialog().expect("north dialog");
        assert_eq!(dlg.title, "Property Line North Arrow");
        let mut nvals = HashMap::new();
        nvals.insert("scale".to_string(), DialogValue::Number(1.5));
        nvals.insert("rotation".to_string(), DialogValue::Number(15.0));
        let nstep = north_cmd.on_dialog_response(DialogResponse::Submitted(nvals));
        assert!(matches!(nstep, CommandStep::NeedPoint));
        assert!(!north_cmd.on_cursor_move([2.0, 3.0, 0.0]).is_empty());
        let nplace = north_cmd.on_point([2.0, 3.0, 0.0]);
        assert!(matches!(nplace, CommandStep::Dispatch(ref s) if s == "PL_APPLY_NORTH"));
        let queued_north = pending_north
            .lock()
            .unwrap()
            .take()
            .expect("queued north insert");
        let insert_entity = drafting::build_north_insert(
            queued_north.insertion,
            "Property Line",
            queued_north.scale,
            queued_north.rotation_deg,
        );
        let EntityType::Insert(ins) = insert_entity else {
            panic!("expected Insert entity");
        };
        assert_eq!(ins.block_name, drafting::NORTH_BLOCK_NAME);
        assert!((ins.x_scale() - 1.5).abs() < 1.0e-9);
        assert!((ins.rotation - 15.0_f64.to_radians()).abs() < 1.0e-9);

        let groups = PropertyLinesModule.ribbon_groups();
        let has_north_svg = groups.iter().flat_map(|g| g.tools.iter()).any(|item| {
            matches!(item, RibbonItem::LargeTool(t) if t.id == "PL_NORTH" && matches!(t.icon, IconKind::Svg(_)))
        });
        assert!(has_north_svg);
    }
}
