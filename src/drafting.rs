use ocs_plugin_api::host::acadrust::entities::mtext::AttachmentPoint;
use ocs_plugin_api::host::acadrust::entities::table::{
    CellRange, CellStyle, CellStylePropertyFlags,
};
use ocs_plugin_api::host::acadrust::entities::{
    Circle, Insert, Line, LwPolyline, MText, Solid, Table, TableBuilder, Text,
};
use ocs_plugin_api::host::acadrust::objects::CellAlignment;
use ocs_plugin_api::host::acadrust::xdata::{ExtendedDataRecord, XDataValue};
use ocs_plugin_api::host::acadrust::{Color, LineWeight, Vector2, Vector3};
use ocs_plugin_api::host::{EntityType, Handle, HostApi, HostSettingValue, LayerConfig, PreviewWire, TableOperation};

use crate::survey::{self, BearingFormat, Boundary};

pub const XDATA_APP: &str = "OPENCAD_PROPERTY_LINES";
pub const PROPERTY_LINE_LINETYPE: &str = "PROPERTY_LINE";
pub const DEFAULT_PROPERTY_LINE_LAYER: &str = "Property Line";
pub const NORTH_BLOCK_NAME: &str = "OCS_NORTH_ARROW";

const TAG_KIND_SEGMENT: &str = "tag";
const TAG_KIND_STATION: &str = "station";
const TAG_KIND_TABLE: &str = "table";
const TAG_KIND_AREA: &str = "area";
const TAG_KIND_NORTH: &str = "north";

#[derive(Debug, Clone)]
pub struct SegmentGeometry {
    pub station: String,
    pub bulge: f64,
    pub chord_length: f64,
    pub arc_length: f64,
    pub dx: f64,
    pub dy: f64,
    pub midpoint: [f64; 3],
    pub outward_normal: [f64; 2],
    pub tangent_angle: f64,
}

impl SegmentGeometry {
    pub fn display_distance(&self) -> f64 {
        if self.bulge.abs() > 1.0e-12 {
            self.arc_length
        } else {
            self.chord_length
        }
    }

    pub fn format_bearing(&self, format: BearingFormat) -> String {
        survey::format_quadrant_bearing(self.dx, self.dy, format)
    }
}

pub fn active_layer_name(host: &dyn HostApi) -> String {
    match host.system_variable("CLAYER") {
        Some(HostSettingValue::Text(layer)) if !layer.trim().is_empty() => layer,
        _ => "0".to_string(),
    }
}

pub fn find_existing_property_layer_name<'a>(
    names: impl IntoIterator<Item = &'a str>,
) -> Option<&'a str> {
    let candidates = [
        "Property Line",
        "Linea de Propiedad",
        "Línea de Propiedad",
        "Property Lines",
        "Lineas de Propiedad",
        "Líneas de Propiedad",
    ];
    let existing: Vec<&'a str> = names.into_iter().collect();
    for preferred in candidates {
        if let Some(&found) = existing
            .iter()
            .find(|name| name.eq_ignore_ascii_case(preferred))
        {
            return Some(found);
        }
    }
    None
}

/// Ensures the `PROPERTY_LINE` linetype and a dedicated property line layer
/// (`Property Line` or an existing Spanish equivalent) exist in the drawing.
pub fn ensure_property_line_layer(host: &mut dyn HostApi) -> String {
    if !host.document().line_types.contains(PROPERTY_LINE_LINETYPE) {
        let _ = host.table_operation(TableOperation::LinetypeCreate {
            name: PROPERTY_LINE_LINETYPE.to_string(),
            description: "Property Line ____  __  __  ____".to_string(),
            pattern: vec![1.8, -0.45, 0.65, -0.45, 0.65, -0.45],
        });
    }

    let existing_name = find_existing_property_layer_name(
        host.document().layers.iter().map(|layer| layer.name.as_str()),
    )
    .map(str::to_string);

    let has_linetype = host.document().line_types.contains(PROPERTY_LINE_LINETYPE);
    if let Some(name) = existing_name {
        if has_linetype {
            let needs_linetype = host
                .document()
                .layers
                .get(&name)
                .is_some_and(|layer| {
                    layer.line_type.trim().is_empty()
                        || layer.line_type.eq_ignore_ascii_case("Continuous")
                });
            if needs_linetype {
                let _ = host.table_operation(TableOperation::LayerModify {
                    config: LayerConfig {
                        name: name.clone(),
                        linetype: Some(PROPERTY_LINE_LINETYPE.to_string()),
                        ..Default::default()
                    },
                });
            }
        }
        return name;
    }

    let config = LayerConfig {
        name: DEFAULT_PROPERTY_LINE_LAYER.to_string(),
        color: Some(Color::from_index(7)),
        linetype: has_linetype.then(|| PROPERTY_LINE_LINETYPE.to_string()),
        lineweight: Some(LineWeight::Value(30)),
        ..Default::default()
    };

    if host.add_layer(config).is_some()
        || host.document().layers.contains(DEFAULT_PROPERTY_LINE_LAYER)
    {
        return DEFAULT_PROPERTY_LINE_LAYER.to_string();
    }

    active_layer_name(host)
}

pub fn make_property_polyline(
    origin: [f64; 3],
    boundary: &Boundary,
    layer: &str,
    table_csv: &str,
) -> LwPolyline {
    let mut polyline = LwPolyline::new();
    polyline.common.layer = layer.to_string();
    polyline.common.linetype = "ByLayer".to_string();
    polyline.common.color = Color::ByLayer;
    polyline.common.line_weight = LineWeight::ByLayer;
    polyline.elevation = origin[2];
    polyline.is_closed = boundary.closed;
    for vertex in &boundary.vertices {
        polyline.add_point_with_bulge(
            Vector2::new(origin[0] + vertex.x, origin[1] + vertex.y),
            vertex.bulge,
        );
    }
    polyline
        .common
        .extended_data
        .upsert_record(table_record(table_csv));
    polyline
}

pub fn table_record(csv: &str) -> ExtendedDataRecord {
    let mut record = ExtendedDataRecord::new(XDATA_APP);
    record.add_value(XDataValue::String("v1".to_string()));
    let mut chunk = String::new();
    for ch in csv.chars() {
        if !chunk.is_empty() && chunk.len() + ch.len_utf8() > 240 {
            record.add_value(XDataValue::String(std::mem::take(&mut chunk)));
        }
        chunk.push(ch);
    }
    if !chunk.is_empty() {
        record.add_value(XDataValue::String(chunk));
    }
    record
}

pub fn read_table(record: Option<&ExtendedDataRecord>) -> Option<String> {
    let record = record?;
    let mut values = record.values.iter();
    match values.next()? {
        XDataValue::String(version) if version == "v1" => {}
        _ => return None,
    }
    let csv = values
        .map(|value| match value {
            XDataValue::String(text) => Some(text.as_str()),
            _ => None,
        })
        .collect::<Option<Vec<_>>>()?;
    Some(csv.concat())
}

fn annotation_record(kind: &str, owner: Handle) -> ExtendedDataRecord {
    let mut record = ExtendedDataRecord::new(XDATA_APP);
    record.add_value(XDataValue::String(format!("ann:{kind}:{owner:?}")));
    record
}

pub fn entity_belongs_to_annotation(
    entity: &EntityType,
    owner: Handle,
    kinds: &[&str],
) -> bool {
    let Some(record) = entity.common().extended_data.get_record(XDATA_APP) else {
        return false;
    };
    let Some(XDataValue::String(tag)) = record.values.first() else {
        return false;
    };
    kinds
        .iter()
        .any(|kind| *tag == format!("ann:{kind}:{owner:?}"))
}

pub fn remove_existing_annotations(
    host: &mut dyn HostApi,
    owner: Handle,
    kinds: &[&str],
) -> usize {
    let to_remove: Vec<Handle> = host
        .document()
        .entities()
        .filter(|entity| entity_belongs_to_annotation(entity, owner, kinds))
        .map(|entity| entity.common().handle)
        .collect();
    let mut removed = 0;
    for handle in to_remove {
        if host.remove_entity(handle) {
            removed += 1;
        }
    }
    removed
}

pub fn extract_polyline(entity: &EntityType) -> Result<(Vec<[f64; 3]>, Vec<f64>, bool), String> {
    if let EntityType::LwPolyline(polyline) = entity {
        if polyline.constant_width.abs() > 1.0e-9
            || polyline
                .vertices
                .iter()
                .any(|v| v.start_width.abs() > 1.0e-9 || v.end_width.abs() > 1.0e-9)
        {
            return Err(
                "Polylines with segment widths cannot be converted without changing their appearance."
                    .to_string(),
            );
        }
    }
    if let EntityType::Polyline2D(polyline) = entity {
        if polyline.start_width.abs() > 1.0e-9
            || polyline.end_width.abs() > 1.0e-9
            || polyline
                .vertices
                .iter()
                .any(|v| v.start_width.abs() > 1.0e-9 || v.end_width.abs() > 1.0e-9)
        {
            return Err(
                "Polylines with segment widths cannot be converted without changing their appearance."
                    .to_string(),
            );
        }
    }
    let (points, bulges, closed): (Vec<[f64; 3]>, Vec<f64>, bool) = match entity {
        EntityType::LwPolyline(polyline) => (
            polyline
                .vertices
                .iter()
                .map(|v| [v.location.x, v.location.y, polyline.elevation])
                .collect(),
            polyline.vertices.iter().map(|v| v.bulge).collect(),
            polyline.is_closed,
        ),
        EntityType::Polyline2D(polyline) => {
            if polyline.normal.x.abs() > 1.0e-9
                || polyline.normal.y.abs() > 1.0e-9
                || (polyline.normal.z - 1.0).abs() > 1.0e-9
            {
                return Err(
                    "Only polylines in the drawing XY plane can be edited as property lines."
                        .to_string(),
                );
            }
            (
                polyline
                    .vertices
                    .iter()
                    .map(|v| [v.location.x, v.location.y, polyline.elevation])
                    .collect(),
                polyline.vertices.iter().map(|v| v.bulge).collect(),
                polyline.is_closed(),
            )
        }
        EntityType::Polyline(polyline) => (
            polyline
                .vertices
                .iter()
                .map(|v| [v.location.x, v.location.y, v.location.z])
                .collect(),
            vec![0.0; polyline.vertices.len()],
            polyline.is_closed(),
        ),
        EntityType::Polyline3D(polyline) => (
            polyline
                .vertices
                .iter()
                .map(|v| [v.position.x, v.position.y, v.position.z])
                .collect(),
            vec![0.0; polyline.vertices.len()],
            polyline.is_closed(),
        ),
        _ => return Err("Select a 2D or planar polyline.".to_string()),
    };
    if points.len() < 2 || (closed && points.len() < 3) {
        return Err("The selected polyline does not contain enough vertices.".to_string());
    }
    if points.iter().flatten().any(|value| !value.is_finite())
        || bulges.iter().any(|value| !value.is_finite())
    {
        return Err("The selected polyline contains invalid coordinates.".to_string());
    }
    let z0 = points[0][2];
    if points.iter().any(|point| (point[2] - z0).abs() > 1.0e-7) {
        return Err("Only planar polylines can be used as property lines.".to_string());
    }
    Ok((points, bulges, closed))
}

pub fn extract_segments(points: &[[f64; 3]], bulges: &[f64], closed: bool) -> Vec<SegmentGeometry> {
    if points.len() < 2 {
        return Vec::new();
    }
    let vertices: Vec<survey::Vertex> = points
        .iter()
        .enumerate()
        .map(|(i, p)| survey::Vertex {
            x: p[0],
            y: p[1],
            bulge: bulges.get(i).copied().unwrap_or(0.0),
        })
        .collect();
    let area = if closed && vertices.len() >= 3 {
        survey::signed_area(&vertices)
    } else {
        -1.0
    };
    let ccw = area >= 0.0;

    let count = points.len();
    let seg_count = if closed { count } else { count - 1 };
    let mut segments = Vec::with_capacity(seg_count);
    for i in 0..seg_count {
        let next = (i + 1) % count;
        let a = points[i];
        let b = points[next];
        let dx = b[0] - a[0];
        let dy = b[1] - a[1];
        let chord = dx.hypot(dy);
        if chord <= 1.0e-9 {
            continue;
        }
        let bulge = bulges.get(i).copied().unwrap_or(0.0);
        let arc_length = if bulge.abs() > 1.0e-12 {
            let theta = 4.0 * bulge.abs().atan();
            let sin_half = (theta / 2.0).sin().abs();
            if sin_half > 1.0e-12 {
                (chord / (2.0 * sin_half)) * theta
            } else {
                chord
            }
        } else {
            chord
        };
        // Arc sagitta along right perpendicular (dy, -dx) weighted by bulge
        let mid_x = (a[0] + b[0]) * 0.5 + 0.5 * bulge * dy;
        let mid_y = (a[1] + b[1]) * 0.5 - 0.5 * bulge * dx;
        let outward_normal = if closed {
            if ccw {
                [dy / chord, -dx / chord]
            } else {
                [-dy / chord, dx / chord]
            }
        } else {
            [-dy / chord, dx / chord]
        };
        let station_start = i + 1;
        let station_end = if closed && next == 0 { 1 } else { next + 1 };
        segments.push(SegmentGeometry {
            station: format!("{station_start}-{station_end}"),
            bulge,
            chord_length: chord,
            arc_length,
            dx,
            dy,
            midpoint: [mid_x, mid_y, a[2]],
            outward_normal,
            tangent_angle: dy.atan2(dx),
        });
    }
    segments
}

pub fn default_annotation_height(points: &[[f64; 3]]) -> f64 {
    if points.len() < 2 {
        return 0.5;
    }
    let mut min_x = f64::INFINITY;
    let mut min_y = f64::INFINITY;
    let mut max_x = f64::NEG_INFINITY;
    let mut max_y = f64::NEG_INFINITY;
    for p in points {
        min_x = min_x.min(p[0]);
        min_y = min_y.min(p[1]);
        max_x = max_x.max(p[0]);
        max_y = max_y.max(p[1]);
    }
    let diag = (max_x - min_x).hypot(max_y - min_y);
    let raw = (diag * 0.015).clamp(0.25, 25.0);
    (raw * 20.0).round() / 20.0
}

/// Computes a readable CAD text angle in `(-pi/2, pi/2]` and chooses whether the
/// MText baseline sits bottom-center or top-center relative to the outward side.
pub fn readable_label_orientation(
    tangent_angle: f64,
    outward_normal: [f64; 2],
) -> (f64, AttachmentPoint) {
    let mut rot = tangent_angle.rem_euclid(std::f64::consts::TAU);
    if rot > std::f64::consts::PI {
        rot -= std::f64::consts::TAU;
    }
    let half_pi = std::f64::consts::FRAC_PI_2;
    if rot > half_pi + 1.0e-6 {
        rot -= std::f64::consts::PI;
    } else if rot <= -half_pi + 1.0e-6 {
        rot += std::f64::consts::PI;
    }
    let text_up = [-rot.sin(), rot.cos()];
    let dot = outward_normal[0] * text_up[0] + outward_normal[1] * text_up[1];
    let attach = if dot >= 0.0 {
        AttachmentPoint::BottomCenter
    } else {
        AttachmentPoint::TopCenter
    };
    (rot, attach)
}

pub fn build_segment_tag_entities(
    owner: Handle,
    segments: &[SegmentGeometry],
    layer: &str,
    text_height: f64,
    decimals: usize,
    unit_suffix: &str,
    include_tick: bool,
) -> Vec<EntityType> {
    let mut entities = Vec::with_capacity(segments.len() * if include_tick { 2 } else { 1 });
    let tick_len = text_height * 0.9;
    let text_gap = if include_tick {
        tick_len + text_height * 0.35
    } else {
        text_height * 0.45
    };
    let suffix = if unit_suffix.is_empty() {
        String::new()
    } else if unit_suffix.starts_with(' ') {
        unit_suffix.to_string()
    } else {
        format!(" {unit_suffix}")
    };

    for seg in segments {
        let [nx, ny] = seg.outward_normal;
        if include_tick {
            let tick_end = [
                seg.midpoint[0] + nx * tick_len,
                seg.midpoint[1] + ny * tick_len,
                seg.midpoint[2],
            ];
            let mut tick = Line::from_coords(
                seg.midpoint[0],
                seg.midpoint[1],
                seg.midpoint[2],
                tick_end[0],
                tick_end[1],
                tick_end[2],
            );
            tick.common.layer = layer.to_string();
            tick.common.linetype = "Continuous".to_string();
            tick.common
                .extended_data
                .upsert_record(annotation_record(TAG_KIND_SEGMENT, owner));
            entities.push(EntityType::Line(tick));
        }

        let bearing = seg.format_bearing(BearingFormat::Spaced);
        let dist = format!("{:.decimals$}{suffix}", seg.display_distance());
        let label = format!("{bearing}\\P{dist}");
        let (rotation, attachment) = readable_label_orientation(seg.tangent_angle, [nx, ny]);
        let pos = Vector3::new(
            seg.midpoint[0] + nx * text_gap,
            seg.midpoint[1] + ny * text_gap,
            seg.midpoint[2],
        );
        let mut mtext = MText::with_value(&label, pos).with_height(text_height);
        mtext.common.layer = layer.to_string();
        mtext.common.linetype = "Continuous".to_string();
        mtext.rotation = rotation;
        mtext.attachment_point = attachment;
        mtext.rectangle_width = 0.0;
        mtext
            .common
            .extended_data
            .upsert_record(annotation_record(TAG_KIND_SEGMENT, owner));
        entities.push(EntityType::MText(Box::new(mtext)));
    }
    entities
}

pub fn build_vertex_station_labels(
    owner: Handle,
    points: &[[f64; 3]],
    closed: bool,
    segments: &[SegmentGeometry],
    layer: &str,
    text_height: f64,
) -> Vec<EntityType> {
    if points.len() < 2 {
        return Vec::new();
    }
    let count = points.len();
    let mut entities = Vec::with_capacity(count);
    let offset = text_height * 0.9;

    for i in 0..count {
        let prev_normal = if i > 0 {
            segments.get(i - 1).map(|s| s.outward_normal)
        } else if closed {
            segments.last().map(|s| s.outward_normal)
        } else {
            None
        };
        let next_normal = if i < segments.len() {
            Some(segments[i].outward_normal)
        } else {
            None
        };
        let dir = match (prev_normal, next_normal) {
            (Some(a), Some(b)) => {
                let sx = a[0] + b[0];
                let sy = a[1] + b[1];
                let len = sx.hypot(sy);
                if len > 1.0e-6 {
                    [sx / len, sy / len]
                } else {
                    a
                }
            }
            (Some(a), None) => a,
            (None, Some(b)) => b,
            (None, None) => [1.0, 1.0],
        };
        let mut label = Text::with_value(
            &(i + 1).to_string(),
            Vector3::new(
                points[i][0] + dir[0] * offset,
                points[i][1] + dir[1] * offset,
                points[i][2],
            ),
        )
        .with_height(text_height * 0.9);
        label.common.layer = layer.to_string();
        label.common.linetype = "Continuous".to_string();
        label
            .common
            .extended_data
            .upsert_record(annotation_record(TAG_KIND_STATION, owner));
        entities.push(EntityType::Text(label));
    }
    entities
}

pub fn effective_table_title(title: &str) -> &str {
    let trimmed = title.trim();
    if trimmed.is_empty() {
        "Rumbos y Distancias"
    } else {
        trimmed
    }
}

pub fn table_layout_metrics(
    segments: &[SegmentGeometry],
    title: &str,
    text_height: f64,
    decimals: usize,
    compact_bearing: bool,
) -> (Vec<f64>, Vec<f64>) {
    let bearing_fmt = if compact_bearing {
        BearingFormat::Compact
    } else {
        BearingFormat::Spaced
    };
    let mut max_station_chars = "Estación".chars().count();
    let mut max_dist_chars = "Distancia".chars().count();
    let mut max_bearing_chars = "Rumbo".chars().count();
    for seg in segments {
        max_station_chars = max_station_chars.max(seg.station.chars().count());
        let d = format!("{:.decimals$}", seg.display_distance());
        max_dist_chars = max_dist_chars.max(d.chars().count());
        let b = seg.format_bearing(bearing_fmt);
        max_bearing_chars = max_bearing_chars.max(b.chars().count());
    }

    let h_margin = text_height * 0.65;
    let char_w = text_height * 0.92;
    let mut col0 = (max_station_chars as f64 * char_w + 2.0 * h_margin).max(text_height * 9.0);
    let mut col1 = (max_dist_chars as f64 * char_w + 2.0 * h_margin).max(text_height * 10.0);
    let mut col2 = (max_bearing_chars as f64 * char_w + 2.0 * h_margin).max(text_height * 13.5);

    let title_text = effective_table_title(title);
    let title_min =
        title_text.chars().count() as f64 * (text_height * 1.15 * 0.95) + 2.0 * h_margin;
    let total = col0 + col1 + col2;
    if title_min > total {
        let extra = (title_min - total) / 3.0;
        col0 += extra;
        col1 += extra;
        col2 += extra;
    }

    let title_h = text_height * 2.4;
    let header_h = text_height * 2.0;
    let data_h = text_height * 1.85;
    let mut row_heights = Vec::with_capacity(segments.len() + 2);
    row_heights.push(title_h);
    row_heights.push(header_h);
    for _ in segments {
        row_heights.push(data_h);
    }

    (vec![col0, col1, col2], row_heights)
}

pub fn table_preview_wires(
    insertion: [f64; 3],
    col_widths: &[f64],
    row_heights: &[f64],
) -> Vec<PreviewWire> {
    let total_w: f64 = col_widths.iter().sum();
    let total_h: f64 = row_heights.iter().sum();
    let x0 = insertion[0];
    let y0 = insertion[1];
    let z = insertion[2];
    let mut wires = vec![
        PreviewWire::line([x0, y0, z], [x0 + total_w, y0, z], None),
        PreviewWire::line([x0 + total_w, y0, z], [x0 + total_w, y0 - total_h, z], None),
        PreviewWire::line([x0 + total_w, y0 - total_h, z], [x0, y0 - total_h, z], None),
        PreviewWire::line([x0, y0 - total_h, z], [x0, y0, z], None),
    ];
    let mut y_acc = y0;
    for h in row_heights.iter().take(2) {
        y_acc -= *h;
        wires.push(PreviewWire::line(
            [x0, y_acc, z],
            [x0 + total_w, y_acc, z],
            None,
        ));
    }
    wires
}

fn cell_style_with(text_height: f64, alignment: CellAlignment, margin: f64) -> CellStyle {
    let flags = CellStylePropertyFlags::ALIGNMENT
        | CellStylePropertyFlags::TEXT_HEIGHT
        | CellStylePropertyFlags::MARGIN_LEFT
        | CellStylePropertyFlags::MARGIN_RIGHT
        | CellStylePropertyFlags::MARGIN_TOP
        | CellStylePropertyFlags::MARGIN_BOTTOM;
    let mut style = CellStyle::new();
    style.property_flags = flags;
    style.override_flags = flags.bits() as i32;
    style.margin_override_flags = 1;
    style.text_height = text_height;
    style.alignment = alignment as i32;
    style.margin_left = margin;
    style.margin_right = margin;
    style.margin_top = margin * 0.6;
    style.margin_bottom = margin * 0.6;
    style
}

fn set_styled_cell(
    table: &mut Table,
    row: usize,
    col: usize,
    text: &str,
    text_height: f64,
    alignment: CellAlignment,
    margin: f64,
) {
    table.set_cell_text(row, col, text);
    if let Some(cell) = table.cell_mut(row, col) {
        cell.style = Some(cell_style_with(text_height, alignment, margin));
        let content_flags = (CellStylePropertyFlags::ALIGNMENT
            | CellStylePropertyFlags::TEXT_HEIGHT)
            .bits() as i32;
        let override_flags = (CellStylePropertyFlags::ALIGNMENT
            | CellStylePropertyFlags::TEXT_HEIGHT
            | CellStylePropertyFlags::DATA_TYPE)
            .bits() as i32;
        for content in &mut cell.contents {
            content.text_height = text_height;
            content.alignment = alignment as i32;
            content.format_value_data_type = 4;
            content.format_property_flags |= content_flags;
            content.format_override_flags |= override_flags;
        }
    }
}

pub fn build_bearings_table(
    owner: Option<Handle>,
    segments: &[SegmentGeometry],
    title: &str,
    insertion: [f64; 3],
    layer: &str,
    text_height: f64,
    decimals: usize,
    compact_bearing: bool,
    table_style_handle: Option<Handle>,
) -> Table {
    let total_rows = segments.len() + 2;
    let (col_widths, row_heights) =
        table_layout_metrics(segments, title, text_height, decimals, compact_bearing);
    let mut table = TableBuilder::new(total_rows, 3)
        .at(Vector3::new(insertion[0], insertion[1], insertion[2]))
        .row_height(text_height * 1.85)
        .column_width(col_widths[0])
        .build();
    table.common.layer = layer.to_string();
    table.common.linetype = "Continuous".to_string();
    table.table_style_handle = table_style_handle;
    if let Some(owner_handle) = owner {
        table
            .common
            .extended_data
            .upsert_record(annotation_record(TAG_KIND_TABLE, owner_handle));
    }

    let margin = text_height * 0.45;
    table.base_style = Some(cell_style_with(
        text_height,
        CellAlignment::MiddleCenter,
        margin,
    ));

    for (col, width) in table.columns.iter_mut().zip(col_widths.iter()) {
        col.width = *width;
    }
    for (idx, (row, height)) in table.rows.iter_mut().zip(row_heights.iter()).enumerate() {
        row.height = *height;
        let row_text_height = match idx {
            0 => text_height * 1.15,
            1 => text_height * 1.05,
            _ => text_height,
        };
        row.style = Some(cell_style_with(
            row_text_height,
            CellAlignment::MiddleCenter,
            margin,
        ));
    }

    let title_text = effective_table_title(title);
    set_styled_cell(
        &mut table,
        0,
        0,
        title_text,
        text_height * 1.15,
        CellAlignment::MiddleCenter,
        margin,
    );
    table.merge_cells(CellRange::new(0, 0, 0, 2));

    let headers = ["Estación", "Distancia", "Rumbo"];
    for (col_idx, header) in headers.iter().enumerate() {
        set_styled_cell(
            &mut table,
            1,
            col_idx,
            header,
            text_height * 1.05,
            CellAlignment::MiddleCenter,
            margin,
        );
    }

    let bearing_fmt = if compact_bearing {
        BearingFormat::Compact
    } else {
        BearingFormat::Spaced
    };
    for (idx, seg) in segments.iter().enumerate() {
        let row_idx = idx + 2;
        set_styled_cell(
            &mut table,
            row_idx,
            0,
            &seg.station,
            text_height,
            CellAlignment::MiddleCenter,
            margin,
        );
        set_styled_cell(
            &mut table,
            row_idx,
            1,
            &format!("{:.decimals$}", seg.display_distance()),
            text_height,
            CellAlignment::MiddleRight,
            margin,
        );
        set_styled_cell(
            &mut table,
            row_idx,
            2,
            &seg.format_bearing(bearing_fmt),
            text_height,
            CellAlignment::MiddleLeft,
            margin,
        );
    }

    table
}

pub fn cleanup_kinds_for_tags() -> &'static [&'static str] {
    &[TAG_KIND_SEGMENT]
}

pub fn cleanup_kinds_for_stations() -> &'static [&'static str] {
    &[TAG_KIND_STATION]
}

pub fn cleanup_kinds_for_area() -> &'static [&'static str] {
    &[TAG_KIND_AREA]
}

pub fn parcel_centroid(points: &[[f64; 3]]) -> Option<[f64; 3]> {
    if points.is_empty() {
        return None;
    }
    if points.len() < 3 {
        let n = points.len() as f64;
        let sum = points
            .iter()
            .fold([0.0; 3], |acc, p| [acc[0] + p[0], acc[1] + p[1], acc[2] + p[2]]);
        return Some([sum[0] / n, sum[1] / n, sum[2] / n]);
    }
    let origin = points[0];
    let z = points.iter().map(|p| p[2]).sum::<f64>() / points.len() as f64;
    let mut twice_area = 0.0;
    let mut cx = 0.0;
    let mut cy = 0.0;
    for i in 0..points.len() {
        let a = points[i];
        let b = points[(i + 1) % points.len()];
        let ax = a[0] - origin[0];
        let ay = a[1] - origin[1];
        let bx = b[0] - origin[0];
        let by = b[1] - origin[1];
        let cross = ax * by - bx * ay;
        twice_area += cross;
        cx += (ax + bx) * cross;
        cy += (ay + by) * cross;
    }
    if twice_area.abs() > 1.0e-9 {
        let factor = 1.0 / (3.0 * twice_area);
        Some([origin[0] + cx * factor, origin[1] + cy * factor, z])
    } else {
        let (mut min_x, mut max_x) = (f64::INFINITY, f64::NEG_INFINITY);
        let (mut min_y, mut max_y) = (f64::INFINITY, f64::NEG_INFINITY);
        for p in points {
            min_x = min_x.min(p[0]);
            max_x = max_x.max(p[0]);
            min_y = min_y.min(p[1]);
            max_y = max_y.max(p[1]);
        }
        Some([(min_x + max_x) * 0.5, (min_y + max_y) * 0.5, z])
    }
}

pub fn format_parcel_area(area_m2: f64, decimals: usize, prefix: &str, unit: &str) -> String {
    let number = format!("{area_m2:.decimals$}");
    let prefix = prefix.trim();
    let unit = unit.trim();
    let value = if unit.is_empty() {
        number
    } else {
        format!("{number} {unit}")
    };
    if prefix.is_empty() {
        value
    } else {
        format!("{prefix} {value}")
    }
}

pub fn area_preview_wires(insertion: [f64; 3], text: &str, text_height: f64) -> Vec<PreviewWire> {
    let half_w = (text.chars().count().max(6) as f64 * text_height * 0.85) * 0.5;
    let half_h = text_height * 0.9;
    let [x, y, z] = insertion;
    let cross = text_height * 0.45;
    vec![
        PreviewWire::line([x - half_w, y - half_h, z], [x + half_w, y - half_h, z], None),
        PreviewWire::line([x + half_w, y - half_h, z], [x + half_w, y + half_h, z], None),
        PreviewWire::line([x + half_w, y + half_h, z], [x - half_w, y + half_h, z], None),
        PreviewWire::line([x - half_w, y + half_h, z], [x - half_w, y - half_h, z], None),
        PreviewWire::line([x - cross, y, z], [x + cross, y, z], None),
        PreviewWire::line([x, y - cross, z], [x, y + cross, z], None),
    ]
}

pub fn build_area_label_entity(
    owner: Handle,
    insertion: [f64; 3],
    layer: &str,
    text: &str,
    text_height: f64,
) -> EntityType {
    let mut mtext = MText::with_value(text, Vector3::new(insertion[0], insertion[1], insertion[2]))
        .with_height(text_height);
    mtext.common.layer = layer.to_string();
    mtext.common.linetype = "Continuous".to_string();
    mtext.attachment_point = AttachmentPoint::MiddleCenter;
    mtext.rectangle_width = 0.0;
    mtext
        .common
        .extended_data
        .upsert_record(annotation_record(TAG_KIND_AREA, owner));
    EntityType::MText(Box::new(mtext))
}

fn block_line(start: [f64; 2], end: [f64; 2]) -> EntityType {
    let mut line = Line::from_points(
        Vector3::new(start[0], start[1], 0.0),
        Vector3::new(end[0], end[1], 0.0),
    );
    line.common.layer = "0".to_string();
    line.common.color = Color::ByBlock;
    line.common.linetype = "Continuous".to_string();
    EntityType::Line(line)
}

pub fn ensure_north_arrow_block(host: &mut dyn HostApi) -> Result<String, String> {
    if host.document().block_records.contains(NORTH_BLOCK_NAME) {
        return Ok(NORTH_BLOCK_NAME.to_string());
    }

    let mut ring = Circle::from_center_radius(Vector3::ZERO, 0.72);
    ring.common.layer = "0".to_string();
    ring.common.color = Color::ByBlock;
    ring.common.linetype = "Continuous".to_string();
    let seed = host.add_entity(EntityType::Circle(ring));
    if seed.is_null() {
        return Err("Could not create seed geometry for the North block.".to_string());
    }

    host.table_operation(TableOperation::BlockCreate {
        name: NORTH_BLOCK_NAME.to_string(),
        entities: vec![seed],
        base_point: [0.0, 0.0, 0.0],
        erase_originals: true,
        description: Some("North arrow block (Norte) for property lines".to_string()),
    })
    .map_err(|err| format!("Could not create North block: {err}"))?;

    let mut right_wing = Solid::triangle(
        Vector3::new(0.0, 0.95, 0.0),
        Vector3::new(0.38, -0.35, 0.0),
        Vector3::new(0.0, -0.18, 0.0),
    );
    right_wing.common.layer = "0".to_string();
    right_wing.common.color = Color::ByBlock;
    right_wing.common.linetype = "Continuous".to_string();

    let mut north_label = MText::with_value("N", Vector3::new(0.0, 1.12, 0.0)).with_height(0.42);
    north_label.common.layer = "0".to_string();
    north_label.common.color = Color::ByBlock;
    north_label.common.linetype = "Continuous".to_string();
    north_label.attachment_point = AttachmentPoint::BottomCenter;
    north_label.rectangle_width = 0.0;

    let extras = [
        EntityType::Solid(right_wing),
        block_line([0.0, 0.95], [-0.38, -0.35]),
        block_line([-0.38, -0.35], [0.0, -0.18]),
        block_line([0.0, 0.95], [0.0, -0.92]),
        block_line([-0.92, 0.0], [-0.56, 0.0]),
        block_line([0.56, 0.0], [0.92, 0.0]),
        EntityType::MText(Box::new(north_label)),
    ];
    for entity in extras {
        host.table_operation(TableOperation::BlockEntityAdd {
            block: NORTH_BLOCK_NAME.to_string(),
            entity,
        })
        .map_err(|err| format!("Could not populate North block geometry: {err}"))?;
    }

    Ok(NORTH_BLOCK_NAME.to_string())
}

pub fn north_preview_wires(insertion: [f64; 3], scale: f64, rotation_deg: f64) -> Vec<PreviewWire> {
    let rad = rotation_deg.to_radians();
    let (sin_r, cos_r) = rad.sin_cos();
    let xform = |lx: f64, ly: f64| -> [f64; 3] {
        let sx = lx * scale;
        let sy = ly * scale;
        [
            insertion[0] + sx * cos_r - sy * sin_r,
            insertion[1] + sx * sin_r + sy * cos_r,
            insertion[2],
        ]
    };

    let mut wires = Vec::new();
    let ring_steps = 16usize;
    let ring_r = 0.72;
    for i in 0..ring_steps {
        let a0 = (i as f64 / ring_steps as f64) * std::f64::consts::TAU;
        let a1 = ((i + 1) as f64 / ring_steps as f64) * std::f64::consts::TAU;
        wires.push(PreviewWire::line(
            xform(ring_r * a0.cos(), ring_r * a0.sin()),
            xform(ring_r * a1.cos(), ring_r * a1.sin()),
            None,
        ));
    }

    let arrow_pts = [
        [0.0, 0.95],
        [0.38, -0.35],
        [0.0, -0.18],
        [-0.38, -0.35],
    ];
    for i in 0..arrow_pts.len() {
        let a = arrow_pts[i];
        let b = arrow_pts[(i + 1) % arrow_pts.len()];
        wires.push(PreviewWire::line(xform(a[0], a[1]), xform(b[0], b[1]), None));
    }
    wires.push(PreviewWire::line(xform(0.0, 0.95), xform(0.0, -0.92), None));
    wires.push(PreviewWire::line(xform(-0.92, 0.0), xform(-0.56, 0.0), None));
    wires.push(PreviewWire::line(xform(0.56, 0.0), xform(0.92, 0.0), None));

    // Tiny 'N' glyph above tip
    wires.push(PreviewWire::line(xform(-0.14, 1.14), xform(-0.14, 1.50), None));
    wires.push(PreviewWire::line(xform(-0.14, 1.50), xform(0.14, 1.14), None));
    wires.push(PreviewWire::line(xform(0.14, 1.14), xform(0.14, 1.50), None));
    wires
}

pub fn build_north_insert(
    insertion: [f64; 3],
    layer: &str,
    scale: f64,
    rotation_deg: f64,
) -> EntityType {
    let mut insert = Insert::new(
        NORTH_BLOCK_NAME,
        Vector3::new(insertion[0], insertion[1], insertion[2]),
    );
    insert.common.layer = layer.to_string();
    insert.common.linetype = "Continuous".to_string();
    insert.set_x_scale(scale);
    insert.set_y_scale(scale);
    insert.set_z_scale(scale);
    insert.rotation = rotation_deg.to_radians();
    insert
        .common
        .extended_data
        .upsert_record(annotation_record(TAG_KIND_NORTH, Handle::NULL));
    EntityType::Insert(insert)
}
