//! Minimal reproduction: skrifa (fontations) hinting vs FreeType for one glyph.
//!
//! The report we want to settle: with `Target::Smooth { mode: Lcd,
//! preserve_linear_metrics: true }` (which skrifa maps to the TrueType
//! *backward compatibility* mode) the hinted outline of SimSun's `M` ends up
//! below the baseline, while other hinting targets and FreeType keep it on the
//! baseline.
//!
//! The program prints the lowest outline point of every hinting configuration and
//! writes `comparison.png`: four panels (skrifa Normal, skrifa backward
//! compatibility, FreeType Normal, FreeType LCD), each drawing the glyph with
//! the x axis (baseline, y = 0) and the y axis (origin, x = 0).
//!
//! Run with:
//!   cargo run --release
//!   cargo run --release -- "C:/Windows/Fonts/simsun.ttc" M 100 comparison.png

use std::ffi::CString;

use skrifa::instance::{LocationRef, Size};
use skrifa::outline::{
    DrawSettings, Engine, HintingInstance, HintingOptions, OutlinePen, SmoothMode, Target,
};
use skrifa::MetadataProvider;
use tiny_skia::{FillRule, Paint, Path as SkiaPath, PathBuilder, Pixmap, Rect, Transform};

const PANEL_WIDTH: u32 = 380;
const PANEL_HEIGHT: u32 = 320;
const PANEL_GAP: u32 = 12;
const ORIGIN_X: f32 = 56.0;
const BASELINE_Y: f32 = 232.0;
const SCALE: f32 = 2.2;

/// One glyph outline, collected for drawing and measuring.
struct Outline {
    points: Vec<(f32, f32)>,
    path: SkiaPath,
}

/// Builds a `tiny-skia` path from skrifa outline callbacks.
#[derive(Default)]
struct SkiaPen {
    builder: PathBuilder,
    points: Vec<(f32, f32)>,
    start: (f32, f32),
}

impl SkiaPen {
    fn new() -> Self {
        Self {
            builder: PathBuilder::new(),
            points: Vec::new(),
            start: (0.0, 0.0),
        }
    }

    fn finish(self) -> Option<Outline> {
        let path = self.builder.finish()?;
        Some(Outline {
            points: self.points,
            path,
        })
    }
}

impl OutlinePen for SkiaPen {
    fn move_to(&mut self, x: f32, y: f32) {
        self.start = (x, y);
        self.points.push((x, y));
        self.builder.move_to(x, y);
    }

    fn line_to(&mut self, x: f32, y: f32) {
        self.points.push((x, y));
        self.builder.line_to(x, y);
    }

    fn quad_to(&mut self, cx: f32, cy: f32, x: f32, y: f32) {
        self.points.push((cx, cy));
        self.points.push((x, y));
        self.builder.quad_to(cx, cy, x, y);
    }

    fn curve_to(&mut self, cx0: f32, cy0: f32, cx1: f32, cy1: f32, x: f32, y: f32) {
        self.points.push((cx0, cy0));
        self.points.push((cx1, cy1));
        self.points.push((x, y));
        self.builder.cubic_to(cx0, cy0, cx1, cy1, x, y);
    }

    fn close(&mut self) {
        self.points.push(self.start);
        self.builder.close();
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
        .unwrap_or(100.0);
    let image_path = args
        .get(4)
        .cloned()
        .unwrap_or_else(|| "comparison.png".to_owned());

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

    let panels = [
        (
            "skrifa Normal",
            skrifa.outline("Normal").cloned(),
            skrifa.min_y("Normal"),
        ),
        (
            "skrifa backward-compat",
            skrifa.outline("backward").cloned(),
            skrifa.min_y("backward"),
        ),
        (
            "FreeType Normal",
            freetype.outline("Normal").cloned(),
            freetype.min_y("Normal"),
        ),
        (
            "FreeType LCD",
            freetype.outline("LCD").cloned(),
            freetype.min_y("LCD"),
        ),
    ];
    render_comparison(&font_path, &panels, &image_path)?;
    println!("\nwrote {image_path}");
    Ok(())
}

struct Row {
    label: &'static str,
    min_y: f32,
    below: usize,
    outline: Outline,
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
        let mut unhinted = SkiaPen::new();
        glyph.draw(
            DrawSettings::unhinted(Size::new(size), LocationRef::default()),
            &mut unhinted,
        )?;
        rows.push(row_from_outline(
            "unhinted",
            unhinted.finish().ok_or("empty outline")?,
        ));

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
            let instance =
                HintingInstance::new(&outlines, Size::new(size), LocationRef::default(), options)?;
            let mut pen = SkiaPen::new();
            glyph.draw(DrawSettings::hinted(&instance, false), &mut pen)?;
            rows.push(row_from_outline(label, pen.finish().ok_or("empty outline")?));
        }
        Ok(Self { rows })
    }

    fn outline(&self, needle: &str) -> Option<&SkiaPath> {
        self.rows
            .iter()
            .find(|row| row.label.contains(needle))
            .map(|row| &row.outline.path)
    }

    fn min_y(&self, needle: &str) -> f32 {
        self.rows
            .iter()
            .find(|row| row.label.contains(needle))
            .map(|row| row.min_y)
            .unwrap_or(f32::NAN)
    }
}

fn row_from_outline(label: &'static str, outline: Outline) -> Row {
    let min_y = outline
        .points
        .iter()
        .map(|point| point.1)
        .fold(f32::MAX, f32::min);
    let below = outline
        .points
        .iter()
        .filter(|point| point.1 < -0.5)
        .count();
    Row {
        label,
        min_y,
        below,
        outline,
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
            let outline = unsafe { freetype_outline(&path, character, size, flags)? };
            rows.push(row_from_outline(label, outline));
        }
        Ok(Self { rows })
    }

    fn outline(&self, needle: &str) -> Option<&SkiaPath> {
        self.rows
            .iter()
            .find(|row| row.label.contains(needle))
            .map(|row| &row.outline.path)
    }

    fn min_y(&self, needle: &str) -> f32 {
        self.rows
            .iter()
            .find(|row| row.label.contains(needle))
            .map(|row| row.min_y)
            .unwrap_or(f32::NAN)
    }
}

/// Loads a glyph through FreeType and converts its outline into a `tiny-skia` path.
unsafe fn freetype_outline(
    path: &CString,
    character: char,
    size: f32,
    flags: i32,
) -> Result<Outline, Box<dyn std::error::Error>> {
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
    let result = outline_to_path(outline);

    ft::FT_Done_Face(face);
    ft::FT_Done_FreeType(library);
    result.ok_or_else(|| "empty FreeType outline".into())
}

/// Converts a FreeType outline (points, tags and contour ends) into a path.
unsafe fn outline_to_path(outline: &freetype_sys::FT_Outline) -> Option<Outline> {
    use freetype_sys as ft;

    const TAG_ON: u8 = 1;
    const TAG_CUBIC: u8 = 2;

    let count = outline.n_points.max(0) as usize;
    let contours = outline.n_contours.max(0) as usize;
    if count == 0 || contours == 0 {
        return None;
    }
    let tags = std::slice::from_raw_parts(outline.tags as *const u8, count);
    let raw_points = std::slice::from_raw_parts(outline.points as *const ft::FT_Vector, count);
    let ends = std::slice::from_raw_parts(outline.contours as *const i16, contours);

    let points: Vec<(f32, f32)> = raw_points
        .iter()
        .map(|point| (point.x as f32 / 64.0, point.y as f32 / 64.0))
        .collect();

    let mut builder = PathBuilder::new();
    let mut collected = Vec::with_capacity(count);
    let mut start_index = 0usize;
    for end in ends {
        let end = *end as usize;
        if end < start_index || end >= count {
            return None;
        }
        let contour_points = &points[start_index..=end];
        let contour_tags = &tags[start_index..=end];
        append_contour(
            &mut builder,
            contour_points,
            contour_tags,
            TAG_ON,
            TAG_CUBIC,
            &mut collected,
        );
        start_index = end + 1;
    }
    let path = builder.finish()?;
    Some(Outline {
        points: collected,
        path,
    })
}

fn append_contour(
    builder: &mut PathBuilder,
    points: &[(f32, f32)],
    tags: &[u8],
    tag_on: u8,
    tag_cubic: u8,
    collected: &mut Vec<(f32, f32)>,
) {
    let length = points.len();
    if length == 0 {
        return;
    }
    let on_curve = |index: usize| tags[index] & 3 == tag_on;
    let cubic = |index: usize| tags[index] & 3 == tag_cubic;

    let start = (0..length).find(|index| on_curve(*index));
    let first_point;
    let mut index;
    if let Some(first) = start {
        first_point = points[first];
        index = (first + 1) % length;
    } else {
        // All points are off-curve: the contour starts at the midpoint of the first two.
        first_point = midpoint(points[0], points[1 % length]);
        index = 0;
    }
    collected.push(first_point);
    builder.move_to(first_point.0, first_point.1);

    let mut guard = 0;
    while guard < length * 2 {
        guard += 1;
        if start.is_some_and(|first| index == first) {
            break;
        }
        let point = points[index];
        if on_curve(index) {
            collected.push(point);
            builder.line_to(point.0, point.1);
            index = (index + 1) % length;
            continue;
        }
        if cubic(index) {
            let control1 = point;
            let control2 = points[(index + 1) % length];
            let end = points[(index + 2) % length];
            collected.push(control1);
            collected.push(control2);
            collected.push(end);
            builder.cubic_to(control1.0, control1.1, control2.0, control2.1, end.0, end.1);
            index = (index + 3) % length;
            continue;
        }
        // Conic (TrueType quadratic): use the midpoint rule for consecutive off-curve points.
        let next = (index + 1) % length;
        if on_curve(next) {
            collected.push(point);
            collected.push(points[next]);
            builder.quad_to(point.0, point.1, points[next].0, points[next].1);
            index = (next + 1) % length;
        } else {
            let implied = midpoint(point, points[next]);
            collected.push(point);
            collected.push(implied);
            builder.quad_to(point.0, point.1, implied.0, implied.1);
            index = next;
        }
    }
    builder.close();
}

fn midpoint(a: (f32, f32), b: (f32, f32)) -> (f32, f32) {
    ((a.0 + b.0) * 0.5, (a.1 + b.1) * 0.5)
}

/// Draws the four panels into one image, each with the glyph and both axes.
fn render_comparison(
    font_path: &str,
    panels: &[(&str, Option<SkiaPath>, f32)],
    output: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let label_face = LabelFace::new(font_path)?;
    let panel_count = panels.len() as u32;
    let width = panel_count * PANEL_WIDTH + panel_count.saturating_sub(1) * PANEL_GAP;
    let mut canvas = Pixmap::new(width, PANEL_HEIGHT).ok_or("canvas allocation failed")?;
    canvas.fill(tiny_skia::Color::from_rgba8(255, 255, 255, 255));

    for (index, (title, path, min_y)) in panels.iter().enumerate() {
        let offset_x = index as u32 * (PANEL_WIDTH + PANEL_GAP);
        let mut panel = Pixmap::new(PANEL_WIDTH, PANEL_HEIGHT).ok_or("panel allocation failed")?;
        panel.fill(tiny_skia::Color::from_rgba8(255, 255, 255, 255));

        // Axes: x axis is the baseline (y = 0), y axis is the glyph origin (x = 0).
        let mut axis = Paint::default();
        axis.anti_alias = false;
        axis.set_color_rgba8(220, 60, 60, 255);
        panel.fill_rect(
            Rect::from_xywh(0.0, BASELINE_Y, PANEL_WIDTH as f32, 1.0).unwrap(),
            &axis,
            Transform::identity(),
            None,
        );
        axis.set_color_rgba8(60, 110, 220, 255);
        panel.fill_rect(
            Rect::from_xywh(ORIGIN_X, 0.0, 1.0, PANEL_HEIGHT as f32).unwrap(),
            &axis,
            Transform::identity(),
            None,
        );

        // Lowest outline point: dashed magenta line when the outline dips below the baseline.
        if min_y.is_finite() && *min_y < -0.5 {
            let y = BASELINE_Y + (-min_y) * SCALE;
            axis.set_color_rgba8(190, 40, 190, 220);
            let mut x = 0.0f32;
            while x < PANEL_WIDTH as f32 {
                panel.fill_rect(
                    Rect::from_xywh(x, y, 5.0, 1.0).unwrap(),
                    &axis,
                    Transform::identity(),
                    None,
                );
                x += 9.0;
            }
        }

        if let Some(path) = path {
            let transform = Transform::from_translate(ORIGIN_X, BASELINE_Y).pre_scale(SCALE, -SCALE);
            if let Some(transformed) = path.clone().transform(transform) {
                let mut fill = Paint::default();
                fill.anti_alias = true;
                fill.set_color_rgba8(32, 36, 54, 255);
                panel.fill_path(
                    &transformed,
                    &fill,
                    FillRule::Winding,
                    Transform::identity(),
                    None,
                );
            }
        }

        let label = format!("{title}  min_y {min_y:.2}");
        label_face.draw(&mut panel, 10, 20, &label, 13, (30, 30, 30));
        label_face.draw(&mut panel, 10, 38, "red: x axis (baseline)", 11, (170, 70, 70));
        label_face.draw(&mut panel, 10, 54, "blue: y axis (origin)", 11, (70, 90, 170));

        canvas.draw_pixmap(
            offset_x as i32,
            0,
            panel.as_ref(),
            &tiny_skia::PixmapPaint::default(),
            Transform::identity(),
            None,
        );
    }

    canvas.save_png(output)?;
    Ok(())
}

/// Renders small labels with FreeType.
struct LabelFace {
    library: freetype_sys::FT_Library,
    face: freetype_sys::FT_Face,
}

impl LabelFace {
    fn new(font_path: &str) -> Result<Self, Box<dyn std::error::Error>> {
        use freetype_sys as ft;
        let path = CString::new(font_path)?;
        let mut library: ft::FT_Library = std::ptr::null_mut();
        let mut face: ft::FT_Face = std::ptr::null_mut();
        unsafe {
            if ft::FT_Init_FreeType(&mut library) != 0 {
                return Err("FT_Init_FreeType failed".into());
            }
            if ft::FT_New_Face(library, path.as_ptr(), 0, &mut face) != 0 {
                ft::FT_Done_FreeType(library);
                return Err("FT_New_Face failed".into());
            }
        }
        Ok(Self { library, face })
    }

    fn draw(
        &self,
        pixmap: &mut Pixmap,
        x: i32,
        baseline: i32,
        text: &str,
        px: u32,
        color: (u8, u8, u8),
    ) {
        use freetype_sys as ft;
        unsafe {
            if ft::FT_Set_Pixel_Sizes(self.face, 0, px) != 0 {
                return;
            }
            let pixmap_width = pixmap.width() as i32;
            let pixmap_height = pixmap.height() as i32;
            let mut pen = x;
            for character in text.chars() {
                let code = character as u32 as ft::FT_ULong;
                if ft::FT_Load_Char(self.face, code, ft::FT_LOAD_RENDER) != 0 {
                    continue;
                }
                let slot = (*self.face).glyph;
                let bitmap = &(*slot).bitmap;
                let width = bitmap.width as i32;
                let rows = bitmap.rows as i32;
                let pitch = bitmap.pitch;
                if width == 0 || rows == 0 || bitmap.buffer.is_null() {
                    pen += (*slot).advance.x >> 6;
                    continue;
                }
                let left = pen + (*slot).bitmap_left;
                let top = baseline - (*slot).bitmap_top;
                let stride = pitch.unsigned_abs() as usize;
                                let pixels = pixmap.data_mut();
                for row in 0..rows {
                    for column in 0..width {
                        let coverage = *bitmap.buffer.add(row as usize * stride + column as usize);
                        if coverage == 0 {
                            continue;
                        }
                        let target_x = left + column;
                        let target_y = top + row;
                        if target_x < 0
                            || target_y < 0
                            || target_x >= pixmap_width
                            || target_y >= pixmap_height
                        {
                            continue;
                        }
                        let index =
                            (target_y as usize * pixmap_width as usize + target_x as usize) * 4;
                        let alpha = coverage as f32 / 255.0;
                        for channel in 0..3 {
                            let source = [color.0, color.1, color.2][channel] as f32;
                            let destination = pixels[index + channel] as f32;
                            pixels[index + channel] =
                                (destination * (1.0 - alpha) + source * alpha).round() as u8;
                        }
                        pixels[index + 3] = 255;
                    }
                }
                pen += (*slot).advance.x >> 6;
            }
        }
    }
}

impl Drop for LabelFace {
    fn drop(&mut self) {
        unsafe {
            freetype_sys::FT_Done_Face(self.face);
            freetype_sys::FT_Done_FreeType(self.library);
        }
    }
}
