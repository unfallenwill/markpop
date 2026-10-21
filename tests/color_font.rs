//! COLR v1 features that system fonts may not exercise: sweep gradients,
//! clip boxes, composite modes and variation deltas.
//!
//! The fonts here are built from scratch, so each test pins down exactly one
//! part of the specification.

use markpop::color::Color;
use markpop::font::{Extend, Face, Paint};

// ---------------------------------------------------------------------------
// A minimal TrueType font builder
// ---------------------------------------------------------------------------

/// A simple glyph as a list of closed contours of integer points.
type Glyph = Vec<Vec<(i16, i16)>>;

fn triangle() -> Glyph {
    vec![vec![(100, 0), (900, 0), (500, 800)]]
}

fn square() -> Glyph {
    vec![vec![(100, 100), (900, 100), (900, 900), (100, 900)]]
}

fn glyph_bytes(glyph: &Glyph) -> Vec<u8> {
    let mut out = Vec::new();
    let points: Vec<(i16, i16)> = glyph.iter().flatten().copied().collect();
    let (mut xmin, mut ymin, mut xmax, mut ymax) = (i16::MAX, i16::MAX, i16::MIN, i16::MIN);
    for (x, y) in &points {
        xmin = xmin.min(*x);
        ymin = ymin.min(*y);
        xmax = xmax.max(*x);
        ymax = ymax.max(*y);
    }
    out.extend_from_slice(&(glyph.len() as i16).to_be_bytes());
    for v in [xmin, ymin, xmax, ymax] {
        out.extend_from_slice(&v.to_be_bytes());
    }
    let mut end = 0u16;
    for contour in glyph {
        end += contour.len() as u16;
        out.extend_from_slice(&(end - 1).to_be_bytes());
    }
    out.extend_from_slice(&0u16.to_be_bytes()); // no instructions
    // Every point is on-curve and stored as a 16-bit delta.
    out.extend(std::iter::repeat_n(0x01u8, points.len()));
    let mut last = 0i16;
    for (x, _) in &points {
        out.extend_from_slice(&(x - last).to_be_bytes());
        last = *x;
    }
    let mut last = 0i16;
    for (_, y) in &points {
        out.extend_from_slice(&(y - last).to_be_bytes());
        last = *y;
    }
    out
}

fn glyf_and_loca(glyphs: &[Glyph]) -> (Vec<u8>, Vec<u8>) {
    let mut glyf = Vec::new();
    let mut loca = Vec::new();
    for glyph in glyphs {
        loca.extend_from_slice(&((glyf.len() / 2) as u16).to_be_bytes());
        // An empty glyph shares its offset with the next one: no outline.
        if glyph.is_empty() {
            continue;
        }
        glyf.extend_from_slice(&glyph_bytes(glyph));
        // Keep every glyph 4-byte aligned, as loca assumes padding.
        while glyf.len() % 4 != 0 {
            glyf.push(0);
        }
    }
    // Empty glyph range at the end.
    loca.extend_from_slice(&((glyf.len() / 2) as u16).to_be_bytes());
    (glyf, loca)
}

fn cmap_table() -> Vec<u8> {
    // Format 4 with one segment mapping 'A'..'B' to glyphs 1 and 2.
    let mut sub = Vec::new();
    sub.extend_from_slice(&4u16.to_be_bytes());
    let seg_count = 2u16;
    sub.extend_from_slice(&(16 + 4 * seg_count).to_be_bytes()); // length
    sub.extend_from_slice(&0u16.to_be_bytes()); // language
    sub.extend_from_slice(&(seg_count * 2).to_be_bytes());
    sub.extend_from_slice(&4u16.to_be_bytes()); // searchRange
    sub.extend_from_slice(&1u16.to_be_bytes()); // entrySelector
    sub.extend_from_slice(&0u16.to_be_bytes()); // rangeShift
    sub.extend_from_slice(&0x0042u16.to_be_bytes()); // endCode
    sub.extend_from_slice(&0xFFFFu16.to_be_bytes());
    sub.extend_from_slice(&0u16.to_be_bytes()); // reservedPad
    sub.extend_from_slice(&0x0041u16.to_be_bytes()); // startCode
    sub.extend_from_slice(&0xFFFFu16.to_be_bytes());
    // 'A' + (-64) = 1, 'B' + (-64) = 2.
    sub.extend_from_slice(&(-64i16).to_be_bytes());
    sub.extend_from_slice(&1i16.to_be_bytes());
    sub.extend_from_slice(&0u16.to_be_bytes());
    sub.extend_from_slice(&0u16.to_be_bytes());

    let mut cmap = Vec::new();
    cmap.extend_from_slice(&0u16.to_be_bytes()); // version
    cmap.extend_from_slice(&1u16.to_be_bytes()); // numTables
    cmap.extend_from_slice(&0u16.to_be_bytes()); // platform (Unicode)
    cmap.extend_from_slice(&3u16.to_be_bytes()); // encoding (BMP)
    cmap.extend_from_slice(&12u32.to_be_bytes()); // offset
    cmap.extend_from_slice(&sub);
    cmap
}

fn head_table() -> Vec<u8> {
    let mut head = vec![0u8; 54];
    head[0..4].copy_from_slice(&0x0001_0000u32.to_be_bytes());
    head[12..16].copy_from_slice(&0x5F0F_3CF5u32.to_be_bytes());
    head[18..20].copy_from_slice(&1000u16.to_be_bytes()); // unitsPerEm
    head[50..52].copy_from_slice(&0i16.to_be_bytes()); // short loca
    head
}

fn maxp_table(num_glyphs: u16) -> Vec<u8> {
    let mut maxp = vec![0u8; 6];
    maxp[0..4].copy_from_slice(&0x0001_0000u32.to_be_bytes());
    maxp[4..6].copy_from_slice(&num_glyphs.to_be_bytes());
    maxp
}

fn hhea_table() -> Vec<u8> {
    let mut hhea = vec![0u8; 36];
    hhea[0..4].copy_from_slice(&0x0001_0000u32.to_be_bytes());
    hhea[34..36].copy_from_slice(&2u16.to_be_bytes()); // numberOfHMetrics
    hhea
}

fn hmtx_table(advances: &[u16]) -> Vec<u8> {
    let mut hmtx = Vec::new();
    for advance in advances {
        hmtx.extend_from_slice(&advance.to_be_bytes());
        hmtx.extend_from_slice(&0i16.to_be_bytes());
    }
    hmtx
}

/// Assembles a font file from raw tables.
#[derive(Default)]
struct FontBuilder {
    tables: Vec<([u8; 4], Vec<u8>)>,
}

impl FontBuilder {
    fn with_base(glyphs: &[Glyph]) -> FontBuilder {
        let (glyf, loca) = glyf_and_loca(glyphs);
        let advances: Vec<u16> = std::iter::repeat_n(900u16, glyphs.len()).collect();
        FontBuilder {
            tables: vec![
                (*b"head", head_table()),
                (*b"maxp", maxp_table(glyphs.len() as u16)),
                (*b"hhea", hhea_table()),
                (*b"hmtx", hmtx_table(&advances)),
                (*b"cmap", cmap_table()),
                (*b"loca", loca),
                (*b"glyf", glyf),
            ],
        }
    }

    fn table(mut self, tag: &[u8; 4], data: Vec<u8>) -> FontBuilder {
        self.tables.push((*tag, data));
        self
    }

    fn build(&self) -> Vec<u8> {
        let count = self.tables.len() as u16;
        let mut out = Vec::new();
        out.extend_from_slice(&0x0001_0000u32.to_be_bytes());
        out.extend_from_slice(&count.to_be_bytes());
        out.extend_from_slice(&0u16.to_be_bytes()); // searchRange
        out.extend_from_slice(&0u16.to_be_bytes()); // entrySelector
        out.extend_from_slice(&0u16.to_be_bytes()); // rangeShift
        let mut offset = 12 + 16 * count as usize;
        let mut body = Vec::new();
        for (tag, data) in &self.tables {
            out.extend_from_slice(tag);
            out.extend_from_slice(&0u32.to_be_bytes()); // checksum
            out.extend_from_slice(&(offset as u32).to_be_bytes());
            out.extend_from_slice(&(data.len() as u32).to_be_bytes());
            body.extend_from_slice(data);
            while body.len() % 4 != 0 {
                body.push(0);
            }
            offset += data.len();
            offset = (offset + 3) & !3;
        }
        out.extend_from_slice(&body);
        out
    }
}

// ---------------------------------------------------------------------------
// COLR / CPAL builders
// ---------------------------------------------------------------------------

/// CPAL v0 with two opaque colours: red (index 0) and blue (index 1).
fn cpal_table() -> Vec<u8> {
    let mut cpal = Vec::new();
    cpal.extend_from_slice(&0u16.to_be_bytes()); // version
    cpal.extend_from_slice(&2u16.to_be_bytes()); // numPaletteEntries
    cpal.extend_from_slice(&1u16.to_be_bytes()); // numPalettes
    cpal.extend_from_slice(&2u16.to_be_bytes()); // numColorRecords
    cpal.extend_from_slice(&14u32.to_be_bytes()); // colorRecordsArrayOffset
    cpal.extend_from_slice(&0u16.to_be_bytes()); // colorRecordIndices[0]
    // BGRA records: red, blue.
    cpal.extend_from_slice(&[0, 0, 255, 255]);
    cpal.extend_from_slice(&[255, 0, 0, 255]);
    cpal
}

/// Places paint tables, patching forward offsets once children exist.
#[derive(Default)]
struct PaintArea {
    buf: Vec<u8>,
}

impl PaintArea {
    fn start(&mut self, bytes: &[u8]) -> usize {
        let at = self.buf.len();
        self.buf.extend_from_slice(bytes);
        at
    }

    /// Patches a 24-bit offset field; offsets are relative to the table that
    /// contains the field.
    fn link(&mut self, table: usize, field: usize, target: usize) {
        let delta = target - table;
        let at = table + field;
        self.buf[at] = (delta >> 16) as u8;
        self.buf[at + 1] = (delta >> 8) as u8;
        self.buf[at + 2] = delta as u8;
    }
}

/// Colour line: `stops` of `(offset, palette index, alpha)`.
fn color_line(extend: u8, stops: &[(f32, u16, f32)], var: bool) -> Vec<u8> {
    let mut out = vec![extend];
    out.extend_from_slice(&(stops.len() as u16).to_be_bytes());
    for (offset, index, alpha) in stops {
        out.extend_from_slice(&((offset * 16384.0) as i16).to_be_bytes());
        out.extend_from_slice(&index.to_be_bytes());
        out.extend_from_slice(&((alpha * 16384.0) as i16).to_be_bytes());
        if var {
            // varIndexBase: only the first stop varies, alpha at +1.
            out.extend_from_slice(&0u32.to_be_bytes());
        }
    }
    out
}

fn solid(index: u16, alpha: f32) -> Vec<u8> {
    let mut out = vec![2u8];
    out.extend_from_slice(&index.to_be_bytes());
    out.extend_from_slice(&((alpha * 16384.0) as i16).to_be_bytes());
    out
}

fn glyph_paint(glyph: u16) -> Vec<u8> {
    let mut out = vec![10u8, 0, 0, 0];
    out.extend_from_slice(&glyph.to_be_bytes());
    out
}

fn sweep_paint(center: (i16, i16), start: f32, end: f32) -> Vec<u8> {
    let mut out = vec![8u8, 0, 0, 0];
    out.extend_from_slice(&center.0.to_be_bytes());
    out.extend_from_slice(&center.1.to_be_bytes());
    out.extend_from_slice(&(((start / 180.0 - 1.0) * 16384.0) as i16).to_be_bytes());
    out.extend_from_slice(&(((end / 180.0 - 1.0) * 16384.0) as i16).to_be_bytes());
    out
}

fn linear_paint(p0: (i16, i16), p1: (i16, i16), p2: (i16, i16)) -> Vec<u8> {
    let mut out = vec![4u8, 0, 0, 0];
    for v in [p0.0, p0.1, p1.0, p1.1, p2.0, p2.1] {
        out.extend_from_slice(&v.to_be_bytes());
    }
    out
}

fn composite_paint(mode: u8) -> Vec<u8> {
    vec![32u8, 0, 0, 0, mode, 0, 0, 0]
}

/// Builds a COLR v1 table from one finished paint area plus optional extras.
struct ColrBuilder {
    paint: PaintArea,
    /// `(glyph, paint offset inside the paint area)` for each base record.
    bases: Vec<(u16, usize)>,
    /// `(start, end, clip box)` for clip records.
    clips: Vec<(u16, u16, [i16; 4])>,
    /// Item variation store bytes and an optional index map.
    var_store: Option<Vec<u8>>,
}

impl ColrBuilder {
    fn new(paint: PaintArea, bases: &[(u16, usize)]) -> ColrBuilder {
        ColrBuilder {
            paint,
            bases: bases.to_vec(),
            clips: Vec::new(),
            var_store: None,
        }
    }

    fn clip(mut self, start: u16, end: u16, bounds: [i16; 4]) -> ColrBuilder {
        self.clips.push((start, end, bounds));
        self
    }

    fn variations(mut self, store: Vec<u8>) -> ColrBuilder {
        self.var_store = Some(store);
        self
    }

    fn build(self) -> Vec<u8> {
        // Layout: header | BaseGlyphList | ClipList | varStore | paint area.
        let header = 34usize;
        let base_list = header;
        let base_len = 4 + self.bases.len() * 6;
        let clip_list = base_list + base_len;
        // Clip records, followed by the clip boxes they point at.
        let clip_len = if self.clips.is_empty() {
            0
        } else {
            5 + self.clips.len() * 7 + self.clips.len() * 9
        };
        let var_store_at = clip_list + clip_len;
        let var_len = self.var_store.as_ref().map(Vec::len).unwrap_or(0);
        let paints = var_store_at + var_len;

        let mut out = vec![0u8; header];
        out[0..2].copy_from_slice(&1u16.to_be_bytes()); // version 1
        out[2..4].copy_from_slice(&0u16.to_be_bytes()); // numBaseGlyphRecords
        out[12..14].copy_from_slice(&0u16.to_be_bytes()); // numLayerRecords
        out[14..18].copy_from_slice(&(base_list as u32).to_be_bytes());
        if !self.clips.is_empty() {
            out[22..26].copy_from_slice(&(clip_list as u32).to_be_bytes());
        }
        out[30..34].copy_from_slice(&(var_store_at as u32).to_be_bytes());

        out.extend_from_slice(&(self.bases.len() as u32).to_be_bytes());
        for (glyph, offset) in &self.bases {
            out.extend_from_slice(&glyph.to_be_bytes());
            // Paint offsets are relative to the BaseGlyphList start.
            let delta = paints - base_list + offset;
            out.extend_from_slice(&(delta as u32).to_be_bytes());
        }

        if !self.clips.is_empty() {
            out.push(1); // ClipList format
            out.extend_from_slice(&(self.clips.len() as u32).to_be_bytes());
            let records_end = 5 + self.clips.len() * 7;
            for (i, (start, end, _)) in self.clips.iter().enumerate() {
                out.extend_from_slice(&start.to_be_bytes());
                out.extend_from_slice(&end.to_be_bytes());
                // Clip box offsets are relative to the ClipList start.
                let offset = records_end + i * 9;
                out.push((offset >> 16) as u8);
                out.push((offset >> 8) as u8);
                out.push(offset as u8);
            }
            for (_, _, bounds) in &self.clips {
                out.push(1); // ClipBox format 1
                for v in bounds {
                    out.extend_from_slice(&v.to_be_bytes());
                }
            }
        }

        if let Some(store) = &self.var_store {
            out.extend_from_slice(store);
        }
        out.extend_from_slice(&self.paint.buf);
        out
    }
}

fn face(tables: FontBuilder, colr: Vec<u8>) -> Face {
    let font = tables
        .table(b"COLR", colr)
        .table(b"CPAL", cpal_table())
        .build();
    Face::from_bytes(font, 0).expect("synthetic font parses")
}

/// Face with an empty `.notdef`, a triangle at glyph 1 and a square at 2.
fn shape_glyphs() -> Vec<Glyph> {
    vec![Vec::new(), triangle(), square()]
}

fn triangle_face(colr: ColrBuilder) -> Face {
    face(FontBuilder::with_base(&shape_glyphs()), colr.build())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn sweep_gradient_parses_angles_and_centre() {
    let mut paint = PaintArea::default();
    let glyph_at = paint.start(&glyph_paint(1));
    let sweep_at = paint.start(&sweep_paint((500, 400), 90.0, 450.0));
    paint.link(glyph_at, 1, sweep_at);
    let line = color_line(0, &[(0.0, 0, 1.0), (1.0, 1, 1.0)], false);
    let line_at = paint.start(&line);
    paint.link(sweep_at, 1, line_at);

    let face = triangle_face(ColrBuilder::new(paint, &[(1, glyph_at)]));
    let Some(Paint::Glyph { paint, .. }) = face.color_paint(1) else {
        panic!("expected a glyph paint");
    };
    let Paint::Sweep {
        center,
        start,
        end,
        stops,
        extend,
    } = *paint
    else {
        panic!("expected a sweep gradient");
    };
    assert_eq!(center, [500.0, 400.0]);
    assert!((start.to_degrees() - 90.0).abs() < 0.1, "{start}");
    assert!((end.to_degrees() - 450.0).abs() < 0.1, "{end}");
    assert_eq!(extend, Extend::Pad);
    assert_eq!(stops.len(), 2);
    assert_eq!(stops[0].color, Color::rgba(255, 0, 0, 255));
    assert_eq!(stops[1].color, Color::rgba(0, 0, 255, 255));
}

#[test]
fn linear_gradient_keeps_the_rotation_point() {
    let mut paint = PaintArea::default();
    let glyph_at = paint.start(&glyph_paint(1));
    // A gradient whose colour line runs along x but whose projection
    // direction is rotated 45 degrees: p2 is not perpendicular to p0-p1.
    let grad_at = paint.start(&linear_paint((0, 0), (1000, 0), (500, 500)));
    paint.link(glyph_at, 1, grad_at);
    let line = color_line(0, &[(0.0, 0, 1.0), (1.0, 1, 1.0)], false);
    let line_at = paint.start(&line);
    paint.link(grad_at, 1, line_at);

    let face = triangle_face(ColrBuilder::new(paint, &[(1, glyph_at)]));
    let Some(Paint::Glyph { paint, .. }) = face.color_paint(1) else {
        panic!("expected a glyph paint");
    };
    let Paint::Linear { p0, p1, p2, .. } = *paint else {
        panic!("expected a linear gradient");
    };
    assert_eq!(p0, [0.0, 0.0]);
    assert_eq!(p1, [1000.0, 0.0]);
    assert_eq!(p2, [500.0, 500.0]);
}

#[test]
fn clip_list_yields_a_clip_box() {
    let mut paint = PaintArea::default();
    let glyph_at = paint.start(&glyph_paint(1));
    let fill_at = paint.start(&solid(0, 1.0));
    paint.link(glyph_at, 1, fill_at);

    let face =
        triangle_face(ColrBuilder::new(paint, &[(1, glyph_at)]).clip(1, 2, [120, 130, 880, 770]));
    assert_eq!(face.color_clip(1), Some([120.0, 130.0, 880.0, 770.0]));
    // The record covers glyphs 1..=2, so both share the box.
    assert_eq!(face.color_clip(2), Some([120.0, 130.0, 880.0, 770.0]));
    // A glyph outside every record has none.
    assert_eq!(face.color_clip(3), None);
}

#[test]
fn clip_boxes_round_outwards() {
    let mut paint = PaintArea::default();
    let glyph_at = paint.start(&glyph_paint(1));
    let fill_at = paint.start(&solid(0, 1.0));
    paint.link(glyph_at, 1, fill_at);
    // Values are integers already; the box must stay inclusive.
    let face = triangle_face(ColrBuilder::new(paint, &[(1, glyph_at)]).clip(1, 1, [-5, -7, 9, 11]));
    assert_eq!(face.color_clip(1), Some([-5.0, -7.0, 9.0, 11.0]));
}

#[test]
fn composite_mode_is_parsed() {
    let mut paint = PaintArea::default();
    let composite_at = paint.start(&composite_paint(11)); // XOR
    let source_at = paint.start(&glyph_paint(1));
    paint.link(composite_at, 1, source_at);
    let backdrop_at = paint.start(&glyph_paint(2));
    paint.link(composite_at, 5, backdrop_at);
    let solid_at = paint.start(&solid(1, 1.0));
    paint.link(source_at, 1, solid_at);
    let solid2_at = paint.start(&solid(0, 1.0));
    paint.link(backdrop_at, 1, solid2_at);

    let face = triangle_face(ColrBuilder::new(paint, &[(1, composite_at)]));
    let Some(Paint::Composite {
        mode,
        source,
        backdrop,
    }) = face.color_paint(1)
    else {
        panic!("expected a composite paint");
    };
    assert_eq!(mode, 11);
    assert!(matches!(*source, Paint::Glyph { glyph: 1, .. }));
    assert!(matches!(*backdrop, Paint::Glyph { glyph: 2, .. }));
}

#[test]
fn unknown_composite_modes_fall_back_to_clear() {
    let mut paint = PaintArea::default();
    let composite_at = paint.start(&composite_paint(200));
    let source_at = paint.start(&glyph_paint(1));
    paint.link(composite_at, 1, source_at);
    let backdrop_at = paint.start(&glyph_paint(2));
    paint.link(composite_at, 5, backdrop_at);
    let solid_at = paint.start(&solid(0, 1.0));
    paint.link(source_at, 1, solid_at);
    paint.link(backdrop_at, 1, solid_at);

    let face = triangle_face(ColrBuilder::new(paint, &[(1, composite_at)]));
    match face.color_paint(1) {
        Some(Paint::Composite { mode, .. }) => assert_eq!(mode, 0),
        other => panic!("expected a composite paint, got {other:?}"),
    }
}

/// A one-axis `fvar` table: `wght` 100..900, default 400.
fn fvar_table() -> Vec<u8> {
    let mut fvar = Vec::new();
    fvar.extend_from_slice(&1u16.to_be_bytes()); // majorVersion
    fvar.extend_from_slice(&0u16.to_be_bytes()); // minorVersion
    fvar.extend_from_slice(&16u16.to_be_bytes()); // axesArrayOffset
    fvar.extend_from_slice(&2u16.to_be_bytes()); // reserved
    fvar.extend_from_slice(&1u16.to_be_bytes()); // axisCount
    fvar.extend_from_slice(&20u16.to_be_bytes()); // axisSize
    fvar.extend_from_slice(&0u16.to_be_bytes()); // instanceCount
    fvar.extend_from_slice(&0u16.to_be_bytes()); // instanceSize
    fvar.extend_from_slice(b"wght");
    for v in [100.0f32, 400.0, 900.0] {
        fvar.extend_from_slice(&((v * 65536.0) as i32).to_be_bytes());
    }
    fvar.extend_from_slice(&0u16.to_be_bytes()); // flags
    fvar.extend_from_slice(&256u16.to_be_bytes()); // axisNameID
    fvar
}

/// An `ItemVariationStore` with one axis, one region peaking at +1 and one
/// item whose delta is `delta` (an F2DOT14 numerator).
fn item_variation_store(delta: i16) -> Vec<u8> {
    // Header: format, regionListOffset, itemVariationDataCount, offsets[1].
    let mut store = Vec::new();
    store.extend_from_slice(&1u16.to_be_bytes());
    store.extend_from_slice(&12u32.to_be_bytes()); // region list at 12
    store.extend_from_slice(&1u16.to_be_bytes());
    store.extend_from_slice(&22u32.to_be_bytes()); // subtable at 22

    // VariationRegionList: axisCount, regionCount, one region.
    store.extend_from_slice(&1u16.to_be_bytes());
    store.extend_from_slice(&1u16.to_be_bytes());
    store.extend_from_slice(&0i16.to_be_bytes()); // startCoord
    store.extend_from_slice(&16384i16.to_be_bytes()); // peakCoord +1
    store.extend_from_slice(&16384i16.to_be_bytes()); // endCoord +1

    // ItemVariationData: itemCount, wordDeltaCount, regionIndexCount, index.
    store.extend_from_slice(&1u16.to_be_bytes());
    store.extend_from_slice(&1u16.to_be_bytes()); // one word delta
    store.extend_from_slice(&1u16.to_be_bytes());
    store.extend_from_slice(&0u16.to_be_bytes()); // region 0
    store.extend_from_slice(&delta.to_be_bytes());
    store
}

#[test]
fn variable_solid_alpha_follows_the_axis() {
    let mut paint = PaintArea::default();
    let glyph_at = paint.start(&glyph_paint(1));
    let solid_at = paint.start(&var_solid_paint());
    paint.link(glyph_at, 1, solid_at);
    let colr = ColrBuilder::new(paint, &[(1, glyph_at)])
        .variations(item_variation_store(8192)) // +0.5 alpha
        .build();
    let font = FontBuilder::with_base(&shape_glyphs())
        .table(b"fvar", fvar_table())
        .table(b"COLR", colr)
        .table(b"CPAL", cpal_table())
        .build();
    let mut face = Face::from_bytes(font, 0).expect("synthetic variable font parses");

    assert_eq!(face.axes().len(), 1);
    assert_eq!(face.axes()[0].tag, "wght");
    assert_eq!(face.axes()[0].default, 400.0);
    assert_eq!(solid_alpha(&face, 1), 0, "default instance has no delta");

    face.set_variations(&[("wght".to_string(), 900.0)]);
    assert_eq!(face.axis_values(), &[900.0]);
    assert_eq!(
        solid_alpha(&face, 1),
        128,
        "half of 255 at the +1 region peak"
    );

    // Out-of-range values are clamped to the axis.
    face.set_variations(&[("wght".to_string(), 5000.0)]);
    assert_eq!(face.axis_values(), &[900.0]);
    assert_eq!(solid_alpha(&face, 1), 128);

    // And back to the default instance.
    face.set_variations(&[("wght".to_string(), 400.0)]);
    assert_eq!(solid_alpha(&face, 1), 0);
}

fn var_solid_paint() -> Vec<u8> {
    let mut out = vec![3u8];
    out.extend_from_slice(&0u16.to_be_bytes());
    out.extend_from_slice(&0i16.to_be_bytes());
    out.extend_from_slice(&0u32.to_be_bytes());
    out
}

fn solid_alpha(face: &Face, glyph: u16) -> u8 {
    match face.color_paint(glyph) {
        Some(Paint::Glyph { paint, .. }) => match *paint {
            Paint::Solid { color } => color.a,
            other => panic!("expected a solid fill, got {other:?}"),
        },
        other => panic!("expected a glyph paint, got {other:?}"),
    }
}

#[test]
fn variable_outlines_use_gvar_and_hvar() {
    // Needs a system variable font; skipped when none is installed.
    let candidates = [
        "C:/Windows/Fonts/SegUIVar.ttf",
        "C:/Windows/Fonts/CascadiaCode.ttf",
    ];
    let Some(mut face) = candidates
        .iter()
        .find_map(|path| Face::load(std::path::Path::new(path)))
    else {
        eprintln!("skipping: no variable font installed");
        return;
    };
    assert!(!face.axes().is_empty(), "expected fvar axes");
    let axis = face.axes()[0].clone();
    let tag = axis.tag.clone();
    let gid = face.glyph_id('H');
    assert_ne!(gid, 0);

    let light_outline = face.outline(gid, 1.0);
    let light_advance = face.advance(gid);
    face.set_variations(&[(tag.clone(), axis.max)]);
    let heavy_outline = face.outline(gid, 1.0);
    let heavy_advance = face.advance(gid);

    assert_ne!(
        light_outline, heavy_outline,
        "gvar should move the outline at the axis extreme"
    );
    assert!(
        heavy_advance > light_advance,
        "a heavier instance should be wider: {light_advance} -> {heavy_advance}"
    );

    // Back to the default instance: identical to the untouched face.
    face.set_variations(&[(tag.clone(), axis.default)]);
    assert_eq!(face.outline(gid, 1.0), light_outline);
    assert_eq!(face.advance(gid), light_advance);

    // Unknown tags are ignored, known ones clamp to the axis range.
    face.set_variations(&[("zzzz".to_string(), 1.0)]);
    assert_eq!(face.axis_values()[0], axis.default);
    face.set_variations(&[(tag, axis.max + 1000.0)]);
    assert_eq!(face.axis_values()[0], axis.max);

    // ---------------------------------------------------------------------------
}

// End-to-end rendering through the public rasterizer
// ---------------------------------------------------------------------------

use markpop::font::LoadedFonts;
use markpop::raster::Rasterizer;
use markpop::scene::{Draw, FontKind, Scene, TextDraw};

/// Renders one character of `face` at `size` pixels per em.
fn render_glyph(face: Face, ch: char, size: f32) -> markpop::raster::Bitmap {
    let mut face = face;
    // A static instance keeps the geometry of the test font predictable.
    face.set_variations(&[]);
    let fonts = LoadedFonts {
        sans: vec![face.clone()],
        mono: vec![face],
    };
    let scene = Scene {
        width: size * 1.4,
        height: size * 1.4,
        background: None,
        draws: vec![Draw::Text(TextDraw {
            x: size * 0.2,
            y: size * 1.1,
            text: ch.to_string(),
            size,
            family: FontKind::Sans,
            color: Color::BLACK,
            bold: false,
            italic: false,
            underline: false,
            strike: false,
            letter_spacing: 0.0,
        })],
    };
    Rasterizer::new(1.0).rasterize(&scene, &fonts)
}

/// Distinct opaque colours in a bitmap.
fn palette(bmp: &markpop::raster::Bitmap) -> std::collections::HashSet<(u8, u8, u8)> {
    bmp.data
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|px| px[3] > 127)
        .map(|px| (px[0], px[1], px[2]))
        .collect()
}

#[test]
fn sweep_filled_glyph_paints_a_colour_wheel() {
    let mut paint = PaintArea::default();
    let glyph_at = paint.start(&glyph_paint(1));
    let sweep_at = paint.start(&sweep_paint((500, 400), 0.0, 360.0));
    paint.link(glyph_at, 1, sweep_at);
    // Four stops around the circle so the wheel has recognisable quadrants.
    let line = color_line(0, &[(0.0, 0, 1.0), (0.5, 1, 1.0), (1.0, 0, 1.0)], false);
    let line_at = paint.start(&line);
    paint.link(sweep_at, 1, line_at);

    let face = triangle_face(ColrBuilder::new(paint, &[(1, glyph_at)]));
    let bmp = render_glyph(face, 'A', 160.0);
    let colors = palette(&bmp);
    assert!(
        colors.len() > 20,
        "a sweep gradient should produce many shades, got {}",
        colors.len()
    );
}

#[test]
fn clip_box_hides_the_parts_outside_it() {
    let mut paint = PaintArea::default();
    let glyph_at = paint.start(&glyph_paint(1));
    let solid_at = paint.start(&solid(0, 1.0));
    paint.link(glyph_at, 1, solid_at);

    // The triangle spans y = 0..800 in font units; the clip keeps the top
    // quarter only (y > 600) so the base of the triangle must vanish.
    let clipped =
        triangle_face(ColrBuilder::new(paint, &[(1, glyph_at)]).clip(1, 1, [0, 600, 1000, 800]));
    let bmp = render_glyph(clipped, 'A', 160.0);
    let opaque_rows: Vec<u32> = (0..bmp.height)
        .filter(|y| (0..bmp.width).any(|x| bmp.pixel(x, *y)[3] > 127))
        .collect();
    assert!(
        !opaque_rows.is_empty(),
        "the clipped apex should still paint"
    );
    let tallest = *opaque_rows.first().unwrap();
    // The apex is only a few pixels tall; everything below the clip is gone.
    assert!(
        opaque_rows.len() < bmp.height as usize / 2,
        "clip box did not cut the glyph: {} of {} rows painted",
        opaque_rows.len(),
        bmp.height
    );
    assert!(tallest > 0);
}

#[test]
fn composite_paint_modes_reach_the_rasterizer() {
    // Two copies of the same glyph, one shifted, combined with XOR: the
    // overlap must be punched out of the rendered result.
    let mut paint = PaintArea::default();
    let composite_at = paint.start(&composite_paint(11)); // XOR
    // Shift the backdrop with a PaintTranslate (format 14: dx = 400, dy = 0).
    let mut translate = vec![14u8, 0, 0, 0];
    translate.extend_from_slice(&400i16.to_be_bytes());
    translate.extend_from_slice(&0i16.to_be_bytes());
    let translate_at = paint.start(&translate);
    let source_at = paint.start(&glyph_paint(1));
    let backdrop_at = paint.start(&glyph_paint(1));
    paint.link(composite_at, 1, source_at);
    paint.link(composite_at, 5, translate_at);
    paint.link(translate_at, 1, backdrop_at);
    let red_at = paint.start(&solid(0, 1.0));
    paint.link(source_at, 1, red_at);
    let blue_at = paint.start(&solid(1, 1.0));
    paint.link(backdrop_at, 1, blue_at);

    let face = triangle_face(ColrBuilder::new(paint, &[(1, composite_at)]));
    let bmp = render_glyph(face, 'A', 160.0);
    let colors = palette(&bmp);
    assert!(!colors.is_empty(), "the non-overlapping parts must paint");
    // Each copy keeps its own colour where it does not overlap.
    assert!(
        colors
            .iter()
            .any(|(r, g, b)| *r > 150 && *g < 100 && *b < 100),
        "expected red pixels: {colors:?}"
    );
    assert!(
        colors
            .iter()
            .any(|(b, g, r)| *b > 150 && *r < 100 && *g < 100),
        "expected blue pixels: {colors:?}"
    );
    // The overlap is cleared, so some interior pixel is fully transparent.
    let mut transparent_inside = 0;
    for y in (bmp.height / 4)..(3 * bmp.height / 4) {
        for x in (bmp.width / 4)..(3 * bmp.width / 4) {
            if bmp.pixel(x, y)[3] == 0 {
                transparent_inside += 1;
            }
        }
    }
    assert!(
        transparent_inside > 0,
        "XOR should clear the overlapping area"
    );
}

#[test]
#[ignore = "writes a preview image for manual inspection"]
fn dump_sweep_preview() {
    let mut paint = PaintArea::default();
    let glyph_at = paint.start(&glyph_paint(1));
    let sweep_at = paint.start(&sweep_paint((500, 400), 0.0, 360.0));
    paint.link(glyph_at, 1, sweep_at);
    let line = color_line(0, &[(0.0, 0, 1.0), (0.5, 1, 1.0), (1.0, 0, 1.0)], false);
    let line_at = paint.start(&line);
    paint.link(sweep_at, 1, line_at);
    let face = triangle_face(ColrBuilder::new(paint, &[(1, glyph_at)]));
    let bmp = render_glyph(face, 'A', 400.0);
    let png = markpop::encode::png::encode(bmp.width, bmp.height, &bmp.data);
    let path = std::env::temp_dir().join("markpop-sweep.png");
    std::fs::write(&path, png).unwrap();
    eprintln!("wrote {}", path.display());
}
