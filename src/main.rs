//! Minimal reproduction: skrifa (fontations) hinting vs FreeType for one glyph.
//!
//! The report we want to settle: with `Target::Smooth { mode: Lcd,
//! preserve_linear_metrics: true }` (which skrifa maps to the TrueType
//! *backward compatibility* mode) the hinted outline of SimSun's `M` ends up
//! below the baseline, while other hinting targets and FreeType keep it on the
//! baseline. If FreeType shows the same spike in its LCD/backward-compatible
//! mode, the font's hinting is at fault; if FreeType stays on the baseline,
//! skrifa's backward-compatibility interpreter path is at fault.
//!
//! Run with:
//!   cargo run --release
//!   cargo run --release -- "C:/Windows/Fonts/simsun.ttc" M 42.6

use std::ffi::CString;

use skrifa::instance::{LocationRef, Size};
use skrifa::outline::{
    DrawSettings, Engine, HintingInstance, HintingOptions, OutlinePen, SmoothMode, Target,
};
use skrifa::MetadataProvider;

/// Collects the flattened outline points of a glyph.
#[derive(Default)]
struct PointPen {
    points: Vec<(f32, f32)>,
    start: (f32, f32),
}

impl OutlinePen for PointPen {
    fn move_to(&mut self, x: f32, y: f32) {
        self.start = (x, y);
        self.points.push((x, y));
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.points.push((x, y));
    }
    fn quad_to(&mut self, cx: f32, cy: f32, x: f32, y: f32) {
        self.points.push((cx, cy));
        self.points.push((x, y));
    }
    fn curve_to(&mut self, cx0: f32, cy0: f32, cx1: f32, cy1: f32, x: f32, y: f32) {
        self.points.push((cx0, cy0));
        self.points.push((cx1, cy1));
        self.points.push((x, y));
    }
    fn close(&mut self) {
        self.points.push(self.start);
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let font_path = args
        .get(1)
        .cloned()
        .unwrap_or_else(|| "C:/Windows/Fonts/simsun.ttc".to_owned());
    let character = args
        .get(2)
        .and_then(|value| value.chars().next())
        .unwrap_or('M');
    let size: f32 = args
        .get(3)
        .and_then(|value| value.parse().ok())
        .unwrap_or(42.6);

    let data = std::fs::read(&font_path)?;
    println!("font: {font_path}");
    println!("glyph: {character:?}, size: {size} ppem");

    let skrifa = SkrifaResults::measure(&data, character, size)?;
    println!("\n--- skrifa (fontations) ---");
    for row in &skrifa.rows {
        println!(
            "{:<34} min_y = {:>8.2}  points below baseline: {}",
            row.label, row.min_y, row.below
        );
        if row.label.contains("backward") {
            for (x, y) in row.offending.iter() {
                println!("    offending point: ({x:.2}, {y:.2})");
            }
        }
    }

    let freetype = FreeTypeResults::measure(&font_path, character, size)?;
    println!("\n--- FreeType ---");
    for row in &freetype.rows {
        println!(
            "{:<34} min_y = {:>8.2}  points below baseline: {}",
            row.label, row.min_y, row.below
        );
    }

    let skrifa_bc = skrifa
        .rows
        .iter()
        .find(|row| row.label.contains("backward"))
        .map(|row| row.min_y)
        .unwrap_or(f32::NAN);
    let freetype_lcd = freetype
        .rows
        .iter()
        .find(|row| row.label.contains("LCD"))
        .map(|row| row.min_y)
        .unwrap_or(f32::NAN);
    println!("\n--- verdict ---");
    println!("skrifa backward-compatibility mode: min_y = {skrifa_bc:.2}");
    println!("FreeType LCD target:                 min_y = {freetype_lcd:.2}");
    if freetype_lcd < -0.5 {
        println!("=> FreeType reproduces the spike: the font's hinting is not backward-compatible-safe.");
    } else {
        println!("=> FreeType keeps the glyph on the baseline: skrifa's backward-compatibility path diverges.");
    }
    Ok(())
}

struct Row {
    label: &'static str,
    min_y: f32,
    below: usize,
    offending: Vec<(f32, f32)>,
}

struct SkrifaResults {
    rows: Vec<Row>,
}

impl SkrifaResults {
    fn measure(
        data: &[u8],
        character: char,
        size: f32,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let font = skrifa::FontRef::from_index(data, 0)?;
        let glyph_id = font.charmap().map(character).ok_or("glyph not found")?;
        let outlines = font.outline_glyphs();
        let glyph = outlines.get(glyph_id).ok_or("no outline for glyph")?;

        let mut rows = Vec::new();
        let mut unhinted = PointPen::default();
        glyph.draw(
            DrawSettings::unhinted(Size::new(size), LocationRef::default()),
            &mut unhinted,
        )?;
        rows.push(row_from_pen("unhinted", &unhinted));

        for (label, engine, target) in [
            (
                "hinted Normal target",
                Engine::Interpreter,
                Target::Smooth {
                    mode: SmoothMode::Normal,
                    symmetric_rendering: false,
                    preserve_linear_metrics: false,
                },
            ),
            (
                "hinted backward-compatibility",
                Engine::Interpreter,
                Target::Smooth {
                    mode: SmoothMode::Lcd,
                    symmetric_rendering: false,
                    preserve_linear_metrics: true,
                },
            ),
            ("hinted Mono target", Engine::Interpreter, Target::Mono),
        ] {
            let options = HintingOptions { engine, target };
            let instance = HintingInstance::new(
                &outlines,
                Size::new(size),
                LocationRef::default(),
                options,
            )?;
            let mut pen = PointPen::default();
            glyph.draw(DrawSettings::hinted(&instance, false), &mut pen)?;
            rows.push(row_from_pen(label, &pen));
        }
        Ok(Self { rows })
    }
}

fn row_from_pen(label: &'static str, pen: &PointPen) -> Row {
    let min_y = pen
        .points
        .iter()
        .map(|point| point.1)
        .fold(f32::MAX, f32::min);
    let offending: Vec<(f32, f32)> = pen
        .points
        .iter()
        .copied()
        .filter(|point| point.1 < -0.5)
        .collect();
    Row {
        label,
        min_y,
        below: offending.len(),
        offending,
    }
}

struct FreeTypeResults {
    rows: Vec<Row>,
}

impl FreeTypeResults {
    fn measure(
        font_path: &str,
        character: char,
        size: f32,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let path = CString::new(font_path)?;
        let mut rows = Vec::new();
        for (label, flags) in [
            (
                "no hinting",
                freetype_sys::FT_LOAD_NO_BITMAP | freetype_sys::FT_LOAD_NO_HINTING,
            ),
            (
                "target Normal",
                freetype_sys::FT_LOAD_NO_BITMAP | freetype_sys::FT_LOAD_TARGET_NORMAL,
            ),
            (
                "target LCD",
                freetype_sys::FT_LOAD_NO_BITMAP | freetype_sys::FT_LOAD_TARGET_LCD,
            ),
            (
                "target LCD_V",
                freetype_sys::FT_LOAD_NO_BITMAP | freetype_sys::FT_LOAD_TARGET_LCD_V,
            ),
            (
                "target Mono",
                freetype_sys::FT_LOAD_NO_BITMAP | freetype_sys::FT_LOAD_TARGET_MONO,
            ),
        ] {
            let points = unsafe { freetype_outline(&path, character, size, flags)? };
            let min_y = points.iter().map(|point| point.1).fold(f32::MAX, f32::min);
            let below = points.iter().filter(|point| point.1 < -0.5).count();
            rows.push(Row {
                label,
                min_y,
                below,
                offending: Vec::new(),
            });
        }
        Ok(Self { rows })
    }
}

/// Loads a glyph through FreeType and returns its outline points in pixels.
unsafe fn freetype_outline(
    path: &CString,
    character: char,
    size: f32,
    flags: i32,
) -> Result<Vec<(f32, f32)>, Box<dyn std::error::Error>> {
    use freetype_sys as ft;

    let mut library: ft::FT_Library = std::ptr::null_mut();
    if ft::FT_Init_FreeType(&mut library) != 0 {
        return Err("FT_Init_FreeType failed".into());
    }
    let mut face: ft::FT_Face = std::ptr::null_mut();
    if ft::FT_New_Face(library, path.as_ptr(), 0, &mut face) != 0 {
        ft::FT_Done_FreeType(library);
        return Err("FT_New_Face failed".into());
    }
    let size_26_6 = (f64::from(size) * 64.0).round() as ft::FT_F26Dot6;
    let error = ft::FT_Set_Char_Size(face, 0, size_26_6, 72, 72);
    if error != 0 {
        ft::FT_Done_Face(face);
        ft::FT_Done_FreeType(library);
        return Err(format!("FT_Set_Char_Size failed: {error}").into());
    }
    let glyph_index = ft::FT_Get_Char_Index(face, character as u32 as ft::FT_ULong);
    if glyph_index == 0 {
        ft::FT_Done_Face(face);
        ft::FT_Done_FreeType(library);
        return Err("glyph not found".into());
    }
    let error = ft::FT_Load_Glyph(face, glyph_index, flags);
    if error != 0 {
        ft::FT_Done_Face(face);
        ft::FT_Done_FreeType(library);
        return Err(format!("FT_Load_Glyph failed: {error}").into());
    }

    let slot = (*face).glyph;
    let outline = &(*slot).outline;
    let mut points = Vec::with_capacity(outline.n_points.max(0) as usize);
    for index in 0..outline.n_points.max(0) as usize {
        let point = *outline.points.add(index);
        points.push((point.x as f32 / 64.0, point.y as f32 / 64.0));
    }

    ft::FT_Done_Face(face);
    ft::FT_Done_FreeType(library);
    Ok(points)
}
