# Property Lines for Open CAD Studio

Plugin for **Open CAD Studio `v2026.45` (API 16)** to create, sketch, convert, annotate, and schedule property boundaries using survey distances, quadrant bearings, and arcs.

## Features

- **Property Line (`PL_CREATE`)**: Chooser dialog to create property boundaries by **Distances & Bearings** (`PL_TABLE_MODE`), **Interactive Sketch** (`PL_SKETCH`), **2-Corner Rectangle** (`PL_SKETCH_RECT`), or **Pick Lines** (`PL_PICK_LINES`).
- **Pick Lines (`PL_PICK_LINES`)**: Click an existing polyline (or pick connected lines/arcs and press `Enter`) to convert geometry into a Property Line on the `Property Line` layer with `PROPERTY_LINE` linetype and stored survey XDATA.
- **Sketch Mode (`PL_SKETCH`, `PL_SKETCH_RECT`)**: Live viewport preview showing segment distance and quadrant bearing, snapping/closing smoothly back to the start point or on `Enter`.
- **Edit Table (`PL_EDIT`)**: Opens any property boundary polyline in the survey table editor to modify bearings, distances, or arc parameters while keeping XDATA synchronized.
- **Tag Segments (`PL_TAG`)**: Annotates boundary edges on the outward side with rotated `MText` bearing + distance labels and optional perpendicular midpoint tick marks.
- **Area Label (`PL_AREA`)**: Computes parcel area in `m²` and places a customizable `MText` area annotation (`Área: ... m²`) with cursor preview or automatic centroid placement.
- **Data Table (`PL_TABLE`)**: Inserts a formatted **"Rumbos y Distancias"** (`Estación | Distancia | Rumbo`) CAD `Table` entity with live placement preview and optional station vertex numbers (`1, 2, 3...`).
- **North Arrow Block (`PL_NORTH`)**: Creates and inserts a CAD North arrow block (`OCS_NORTH_ARROW`) with configurable scale, rotation, and live viewport preview.
- **Import CSV (`PL_IMPORT`)**: Imports survey tables directly from `.csv` files (see [examples/](examples/)).
- **Persistent XDATA**: Survey definitions are stored in `OPENCAD_PROPERTY_LINES` XDATA on the polyline so drawings remain editable across save/reopen cycles.

## Installation (Prebuilt Windows Binary)

1. Download the latest release archive (`property-lines-v*-windows-x86_64.zip`) from the **Releases** page.
2. Extract the files into your Open CAD Studio plugins directory:
   - Windows: `%APPDATA%\OpenCADStudio\plugins\opencad.property_lines\`
3. Ensure that folder contains:
   - `opencad.property_lines-windows-x86_64.dll`
   - `plugin.toml`
4. Restart **Open CAD Studio** (`v2026.45` or compatible API 16 build). The **Property Lines** tab will appear in the ribbon.

## CSV Format

```csv
Distance,N/S,Bearing,E/W,Type,Radius,L/R
20,N,90,E,Line,,
15,S,0,E,Line,,
20,S,90,W,Line,,
15,N,0,E,Line,,
```

- Distances and arc radii use drawing units.
- Bearings accept decimal degrees or degrees/minutes/seconds (`0`–`90`).
- Quadrants: `N`/`S` and `E`/`W` (`+Y` = North, `+X` = East).
- `Type` accepts `Line` or `Arc` (`L/R` sets left/right arc curvature; radius must exceed half the chord).
- Append `,,,,Close,,` as the final row to close the polygon automatically.

## Build from Source

Requires Rust (`1.98.1`, see `rust-toolchain.toml`) and Windows MSVC build tools (Visual Studio 2022 + Windows SDK):

```powershell
cargo test --locked
cargo build --release --locked
```

Then copy `target\release\ocs_property_lines.dll` as `opencad.property_lines-windows-x86_64.dll`, alongside `plugin.toml`, into `%APPDATA%\OpenCADStudio\plugins\opencad.property_lines\`.

## License

Licensed under **GPL-3.0-only**. See [LICENSE](LICENSE).

## References

- [Open CAD Studio Plugin Template](https://github.com/HakanSeven12/OpenCADStudio/tree/v2026.45/docs/plugin-template-v2)
- [Open CAD Studio Plugin Architecture & API](https://github.com/HakanSeven12/OpenCADStudio/blob/v2026.45/docs/plugin-architecture.md)
- [API 16 Host Contract](https://github.com/HakanSeven12/OpenCADStudio/blob/v2026.45/crates/ocs_plugin_api/src/host.rs)
