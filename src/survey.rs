//! Survey-table parsing and planar boundary geometry.
//! Bearings are measured from true north toward east or west.

#[derive(Debug, Clone)]
pub struct Vertex {
    pub x: f64,
    pub y: f64,
    /// DXF bulge for the segment that starts at this vertex.
    pub bulge: f64,
}

#[derive(Debug, Clone)]
pub struct Boundary {
    pub vertices: Vec<Vertex>,
    pub closed: bool,
    pub closure_gap: f64,
    pub area: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BearingFormat {
    /// Spaced quadrant bearing with zero-padded minutes/seconds (e.g. `S 90° 00' 00" E`).
    Spaced,
    /// Compact quadrant bearing for schedules/tables (e.g. `S68°49'18"E`).
    Compact,
}

/// Formats a planar vector `(dx, dy)` as a quadrant bearing in DMS.
pub fn format_quadrant_bearing(dx: f64, dy: f64, format: BearingFormat) -> String {
    let ns = if dy < 0.0 { 'S' } else { 'N' };
    let ew = if dx < 0.0 { 'W' } else { 'E' };
    let angle = dx.abs().atan2(dy.abs()).to_degrees();
    let total_sec = (angle * 3600.0).round() as u64;
    let deg = total_sec / 3600;
    let min = (total_sec % 3600) / 60;
    let sec = total_sec % 60;
    match format {
        BearingFormat::Spaced => format!("{ns} {deg}° {min:02}' {sec:02}\" {ew}"),
        BearingFormat::Compact => format!("{ns}{deg}°{min}'{sec}\"{ew}"),
    }
}

/// Builds a `Boundary` relative to `points[0]` from planar 3D vertices and bulges.
pub fn boundary_from_points(points: &[[f64; 3]], bulges: &[f64], closed: bool) -> Boundary {
    if points.is_empty() {
        return Boundary {
            vertices: Vec::new(),
            closed: false,
            closure_gap: 0.0,
            area: None,
        };
    }
    let origin = points[0];
    let vertices: Vec<Vertex> = points
        .iter()
        .enumerate()
        .map(|(i, p)| Vertex {
            x: p[0] - origin[0],
            y: p[1] - origin[1],
            bulge: bulges.get(i).copied().unwrap_or(0.0),
        })
        .collect();
    let closure_gap = if closed {
        0.0
    } else {
        vertices.last().map(|last| last.x.hypot(last.y)).unwrap_or(0.0)
    };
    let area = if closed && vertices.len() >= 3 {
        Some(signed_area(&vertices).abs())
    } else {
        None
    };
    Boundary {
        vertices,
        closed,
        closure_gap,
        area,
    }
}

enum Row {
    Segment {
        distance: f64,
        dx: f64,
        dy: f64,
        bulge: f64,
    },
    Close,
}

pub fn from_csv(contents: &str) -> Result<Boundary, String> {
    let mut reader = csv::ReaderBuilder::new()
        .trim(csv::Trim::All)
        .flexible(true)
        .from_reader(contents.as_bytes());
    let headers = reader.headers().map_err(|e| e.to_string())?;
    let expected = ["distance", "n/s", "bearing", "e/w", "type", "radius", "l/r"];
    for (column, name) in expected.iter().enumerate() {
        let actual = headers
            .get(column)
            .unwrap_or("")
            .trim_start_matches('\u{feff}')
            .to_ascii_lowercase();
        if actual != *name {
            return Err(format!(
                "Column {}: expected '{}', found '{}'.",
                column + 1,
                name,
                actual
            ));
        }
    }

    let mut rows = Vec::new();
    for (index, result) in reader.records().enumerate() {
        let record = result.map_err(|e| format!("Row {}: {e}", index + 2))?;
        if record.iter().all(|field| field.trim().is_empty()) {
            continue;
        }
        let field = |n: usize| record.get(n).unwrap_or("").trim();
        let kind = field(4).to_uppercase();
        if kind == "CLOSE" || kind == "CERRAR" {
            rows.push(Row::Close);
            continue;
        }
        if kind != "LINE" && kind != "LINEA" && kind != "LÍNEA"
            && kind != "ARC" && kind != "ARCO" && !kind.is_empty()
        {
            return Err(format!("Row {}: unknown type '{}'.", index + 2, field(4)));
        }

        let distance = parse_positive(field(0), "distance", index + 2)?;
        let degrees = parse_bearing(field(2))
            .map_err(|msg| format!("Row {}: {msg}", index + 2))?;
        let ns = match field(1).to_ascii_uppercase().as_str() {
            "N" => 1.0,
            "S" => -1.0,
            _ => return Err(format!("Row {}: N/S must be N or S.", index + 2)),
        };
        let ew = match field(3).to_ascii_uppercase().as_str() {
            "E" => 1.0,
            "W" | "O" => -1.0,
            _ => return Err(format!("Row {}: E/W must be E or W.", index + 2)),
        };
        let radians = degrees.to_radians();
        let dx = ew * distance * radians.sin();
        let dy = ns * distance * radians.cos();
        let bulge = if kind == "ARC" || kind == "ARCO" {
            let radius = parse_positive(field(5), "radius", index + 2)?;
            if radius <= distance / 2.0 {
                return Err(format!(
                    "Row {}: the radius must be greater than half the chord length.",
                    index + 2
                ));
            }
            let theta = 2.0 * (distance / (2.0 * radius)).asin();
            let sign = match field(6).to_ascii_uppercase().as_str() {
                "R" | "D" => 1.0,
                "L" | "I" => -1.0,
                _ => return Err(format!("Row {}: L/R must be L or R.", index + 2)),
            };
            sign * (theta / 4.0).tan()
        } else {
            0.0
        };
        rows.push(Row::Segment {
            distance,
            dx,
            dy,
            bulge,
        });
    }
    if rows.is_empty() {
        return Err("The table contains no segments.".to_string());
    }

    let mut vertices = vec![Vertex {
        x: 0.0,
        y: 0.0,
        bulge: 0.0,
    }];
    let mut request_close = false;
    for (index, row) in rows.iter().enumerate() {
        match row {
            Row::Close => {
                if index != rows.len() - 1 || request_close {
                    return Err("The Close row must be the last row.".to_string());
                }
                request_close = true;
            }
            Row::Segment {
                distance,
                dx,
                dy,
                bulge,
            } => {
                let current = vertices.last_mut().expect("start vertex exists");
                current.bulge = *bulge;
                let x = current.x + dx;
                let y = current.y + dy;
                if !x.is_finite() || !y.is_finite() || *distance > 1.0e12 {
                    return Err("Coordinates are outside the supported range.".to_string());
                }
                vertices.push(Vertex { x, y, bulge: 0.0 });
            }
        }
    }
    if vertices.len() < 2 {
        return Err("The table contains no segments.".to_string());
    }

    let end = vertices.last().expect("end vertex exists");
    let gap = end.x.hypot(end.y);
    let total: f64 = rows
        .iter()
        .map(|row| match row {
            Row::Segment { distance, .. } => *distance,
            Row::Close => 0.0,
        })
        .sum();
    let tolerance = 1.0e-7 * total.max(1.0);
    let naturally_closed = gap <= tolerance;
    let closed = request_close || naturally_closed;
    if naturally_closed {
        // The final vertex repeats the first. The bulge of the preceding
        // vertex still describes the final surveyed segment.
        vertices.pop();
    }
    if closed && vertices.len() < 3 {
        return Err("A closed property line requires at least three vertices.".to_string());
    }
    let area = if closed {
        Some(signed_area(&vertices).abs())
    } else {
        None
    };
    Ok(Boundary {
        vertices,
        closed,
        closure_gap: gap,
        area,
    })
}

fn parse_positive(value: &str, name: &str, row: usize) -> Result<f64, String> {
    let parsed: f64 = value
        .parse()
        .map_err(|_| format!("Row {row}: {name} must be a decimal number."))?;
    if !parsed.is_finite() || parsed <= 0.0 {
        return Err(format!("Row {row}: {name} must be greater than zero."));
    }
    Ok(parsed)
}

fn parse_bearing(value: &str) -> Result<f64, String> {
    let cleaned: String = value
        .chars()
        .map(|c| match c {
            '°' | 'º' | '\'' | '"' | ':' | '′' | '″' => ' ',
            other => other,
        })
        .collect();
    let parts: Vec<&str> = cleaned.split_whitespace().collect();
    if parts.is_empty() || parts.len() > 3 {
        return Err("Invalid bearing; use DMS or decimal degrees.".to_string());
    }
    let mut values = Vec::new();
    for part in parts {
        let number: f64 = part
            .parse()
            .map_err(|_| "The bearing must contain numeric degrees, minutes, and seconds.")?;
        if !number.is_finite() {
            return Err("The bearing must be finite.".to_string());
        }
        values.push(number);
    }
    let degrees = values[0];
    let minutes = *values.get(1).unwrap_or(&0.0);
    let seconds = *values.get(2).unwrap_or(&0.0);
    if !(0.0..60.0).contains(&minutes) || !(0.0..60.0).contains(&seconds)
        || degrees < 0.0
    {
        return Err("Minutes and seconds must be between 0 and 60.".to_string());
    }
    let bearing = degrees + minutes / 60.0 + seconds / 3600.0;
    if bearing > 90.0 + 1.0e-10 {
        return Err("A quadrant bearing must be between 0 and 90 degrees.".to_string());
    }
    Ok(bearing.min(90.0))
}

pub fn signed_area(vertices: &[Vertex]) -> f64 {
    let mut twice_area = 0.0;
    let mut arc_area = 0.0;
    for i in 0..vertices.len() {
        let a = &vertices[i];
        let b = &vertices[(i + 1) % vertices.len()];
        twice_area += a.x * b.y - b.x * a.y;
        if a.bulge.abs() > 1.0e-12 {
            let chord = (b.x - a.x).hypot(b.y - a.y);
            if chord > 1.0e-12 {
                let theta = 4.0 * a.bulge.atan();
                let sin_half = (theta / 2.0).sin().abs();
                if sin_half > 1.0e-12 {
                    let radius = chord / (2.0 * sin_half);
                    arc_area += radius * radius * (theta - theta.sin()) / 2.0;
                }
            }
        }
    }
    twice_area / 2.0 + arc_area
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rectangle_from_quadrant_bearings() {
        let csv = "Distance,N/S,Bearing,E/W,Type,Radius,L/R\n\
                   20,N,90,E,Line,,\n\
                   15,S,0,E,Line,,\n\
                   20,S,90,W,Line,,\n\
                   15,N,0,E,Line,,\n";
        let shape = from_csv(csv).unwrap();
        assert!(shape.closed);
        assert_eq!(shape.vertices.len(), 4);
        assert!((shape.area.unwrap() - 300.0).abs() < 1.0e-8);
    }

    #[test]
    fn closing_row_adds_last_side() {
        let csv = "Distance,N/S,Bearing,E/W,Type,Radius,L/R\n\
                   20,N,90,E,Line,,\n\
                   15,S,0,E,Line,,\n\
                   20,S,90,W,Line,,\n\
                   ,,,,Close,,\n";
        let shape = from_csv(csv).unwrap();
        assert!(shape.closed);
        assert!((shape.closure_gap - 15.0).abs() < 1.0e-8);
        assert!((shape.area.unwrap() - 300.0).abs() < 1.0e-8);
    }

    #[test]
    fn arc_radius_and_side() {
        let csv = "Distance,N/S,Bearing,E/W,Type,Radius,L/R\n\
                   10,N,90,E,Arc,10,L\n\
                   10,S,90,W,Line,,\n\
                   1,N,0,E,Line,,\n";
        // A superfluous final side leaves an open loop; the arc still
        // generates a negative bulge for a left-of-chord curve.
        let shape = from_csv(csv).unwrap();
        assert!(!shape.closed);
        assert!(shape.vertices[0].bulge < 0.0);

        let invalid = csv.replace("Arc,10,L", "Arc,5,L");
        assert!(from_csv(&invalid).is_err());
    }

    #[test]
    fn dms_bearing() {
        assert!((parse_bearing("82° 30' 00\"").unwrap() - 82.5).abs() < 1.0e-10);
        assert!(parse_bearing("91°").is_err());
    }

    #[test]
    fn utf8_spanish_types_bom_and_optional_fields() {
        let csv = "\u{feff}Distance,N/S,Bearing,E/W,Type,Radius,L/R\n\
                   20,N,90,E,Línea,,\n\
                   15,S,0,E,línea,,\n\
                   20,S,90,O,,,\n\
                   ,,,,Cerrar,,\n";
        let shape = from_csv(csv).unwrap();
        assert!(shape.closed);
        assert_eq!(shape.vertices.len(), 4);
        assert!((shape.area.unwrap() - 300.0).abs() < 1.0e-8);
    }

    #[test]
    fn tiny_bulge_signed_area_never_nan() {
        let vertices = vec![
            Vertex { x: 0.0, y: 0.0, bulge: 1.0e-20 },
            Vertex { x: 10.0, y: 0.0, bulge: 0.0 },
            Vertex { x: 10.0, y: 10.0, bulge: 0.0 },
            Vertex { x: 0.0, y: 10.0, bulge: 0.0 },
        ];
        let area = signed_area(&vertices);
        assert!(area.is_finite());
        assert!((area - 100.0).abs() < 1.0e-8);
    }
}


