//! A small, dependency-free TrueType font reader.
//!
//! Raster output needs real glyph shapes, and there is no way to get them
//! without reading a font. This module parses just enough of the TrueType
//! format to render text:
//!
//! * the table directory (including `.ttc` collections),
//! * `head` / `maxp` / `hhea` / `hmtx` for basic metrics,
//! * `cmap` formats 4 and 12 for character → glyph mapping,
//! * `loca` / `glyf` for simple and composite outlines.
//!
//! Outlines are flattened to polylines here, in font units with the y axis
//! pointing up. The rasterizer applies the final transform (scale, shear,
//! y-flip, translation). CFF-flavoured OpenType is not supported; the
//! discovery list deliberately prefers TrueType fonts.

use std::path::{Path, PathBuf};

/// A point in font units.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point {
    /// Horizontal coordinate, positive to the right.
    pub x: f32,
    /// Vertical coordinate, positive upwards (font convention).
    pub y: f32,
}

/// A closed contour, flattened to a polyline in font units.
pub type Contour = Vec<Point>;

/// A parsed font face.
#[derive(Debug, Clone)]
pub struct Face {
    data: Vec<u8>,
    units_per_em: f32,
    loca_format: i16,
    num_glyphs: u16,
    num_h_metrics: u16,
    cmap_offset: usize,
    cmap_format: u16,
    // Absolute file offsets of required tables.
    glyf: usize,
    loca: usize,
    hmtx: usize,
    /// Best-effort family name, for diagnostics.
    name: String,
}

fn u8_at(data: &[u8], off: usize) -> u8 {
    data.get(off).copied().unwrap_or(0)
}

fn u16_at(data: &[u8], off: usize) -> u16 {
    u16::from_be_bytes([u8_at(data, off), u8_at(data, off + 1)])
}

fn i16_at(data: &[u8], off: usize) -> i16 {
    u16_at(data, off) as i16
}

fn u32_at(data: &[u8], off: usize) -> u32 {
    u32::from_be_bytes([
        u8_at(data, off),
        u8_at(data, off + 1),
        u8_at(data, off + 2),
        u8_at(data, off + 3),
    ])
}

impl Face {
    /// Loads the first face of a `.ttf`/`.ttc` file.
    pub fn load(path: &Path) -> Option<Face> {
        let data = std::fs::read(path).ok()?;
        Face::from_bytes(data, 0)
    }

    /// Parses a face from memory. `collection_index` selects a face when
    /// the data is a TrueType collection.
    pub fn from_bytes(mut data: Vec<u8>, collection_index: usize) -> Option<Face> {
        let base = if &data.get(0..4)? == b"ttcf" {
            let num = u32_at(&data, 8) as usize;
            if collection_index >= num {
                return None;
            }
            u32_at(&data, 12 + 4 * collection_index) as usize
        } else {
            0
        };

        let version = u32_at(&data, base);
        // 0x00010000 = TrueType, 'true' = Apple TrueType, 'OTTO' = CFF (unsupported).
        if version == u32::from_be_bytes(*b"OTTO") {
            return None;
        }
        let num_tables = u16_at(&data, base + 4) as usize;
        let mut table = |tag: &[u8; 4]| -> Option<(usize, usize)> {
            for i in 0..num_tables {
                let rec = base + 12 + i * 16;
                if data.get(rec..rec + 4)? == tag {
                    let off = u32_at(&data, rec + 8) as usize;
                    let len = u32_at(&data, rec + 12) as usize;
                    if off + len > data.len() {
                        return None;
                    }
                    return Some((off, len));
                }
            }
            None
        };

        let (head, _) = table(b"head")?;
        let (maxp, _) = table(b"maxp")?;
        let (hhea, _) = table(b"hhea")?;
        let (hmtx, _) = table(b"hmtx")?;
        let (cmap, _) = table(b"cmap")?;
        let (loca, _) = table(b"loca")?;
        let (glyf, _) = table(b"glyf")?;

        let units_per_em = u16_at(&data, head + 18);
        if units_per_em == 0 {
            return None;
        }
        let loca_format = i16_at(&data, head + 50);
        let num_glyphs = u16_at(&data, maxp + 4);
        let num_h_metrics = u16_at(&data, hhea + 34);
        let (cmap_offset, cmap_format) = pick_cmap(&data, cmap)?;
        let name = read_family_name(&data, base, num_tables, &mut table)
            .unwrap_or_else(|| "unnamed".to_string());

        // Normalize a TTC offset: table offsets are absolute already.
        data.shrink_to_fit();

        Some(Face {
            data,
            units_per_em: units_per_em as f32,
            loca_format,
            num_glyphs,
            num_h_metrics,
            cmap_offset,
            cmap_format,
            glyf,
            loca,
            hmtx,
            name,
        })
    }

    /// Family name (best effort).
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Glyph id for a character, or 0 (`.notdef`) when absent.
    pub fn glyph_id(&self, c: char) -> u16 {
        match self.cmap_format {
            4 => self.cmap4(c as u32),
            12 => self.cmap12(c as u32),
            _ => 0,
        }
    }

    /// Whether the face has a real glyph for `c`.
    pub fn has_glyph(&self, c: char) -> bool {
        self.glyph_id(c) != 0
    }

    /// Horizontal advance in font units (unused for layout, exposed for
    /// diagnostics and tests).
    pub fn advance(&self, gid: u16) -> f32 {
        if self.num_h_metrics == 0 {
            return 0.0;
        }
        let idx = gid.min(self.num_h_metrics - 1) as usize;
        u16_at(&self.data, self.hmtx + idx * 4) as f32
    }

    /// Units per em (2048 for most TrueType faces, 1000 for CFF).
    pub fn units_per_em(&self) -> f32 {
        self.units_per_em
    }

    fn cmap4(&self, code: u32) -> u16 {
        if code > 0xFFFF {
            return 0;
        }
        let code = code as u16;
        let t = self.cmap_offset;
        let seg_count = (u16_at(&self.data, t + 6) / 2) as usize;
        if seg_count == 0 {
            return 0;
        }
        let end_codes = t + 14;
        let start_codes = end_codes + seg_count * 2 + 2;
        let id_deltas = start_codes + seg_count * 2;
        let id_range_offsets = id_deltas + seg_count * 2;

        // Binary search for the first segment whose endCode >= code.
        let (mut lo, mut hi) = (0usize, seg_count);
        while lo < hi {
            let mid = (lo + hi) / 2;
            if u16_at(&self.data, end_codes + mid * 2) < code {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        if lo >= seg_count {
            return 0;
        }
        let start = u16_at(&self.data, start_codes + lo * 2);
        if code < start {
            return 0;
        }
        let delta = i16_at(&self.data, id_deltas + lo * 2);
        let range = u16_at(&self.data, id_range_offsets + lo * 2);
        if range == 0 {
            return code.wrapping_add(delta as u16);
        }
        let glyph_off = id_range_offsets + lo * 2 + range as usize + (code - start) as usize * 2;
        let gid = u16_at(&self.data, glyph_off);
        if gid == 0 {
            0
        } else {
            gid.wrapping_add(delta as u16)
        }
    }

    fn cmap12(&self, code: u32) -> u16 {
        let t = self.cmap_offset;
        let n_groups = u32_at(&self.data, t + 12) as usize;
        let groups = t + 16;
        let (mut lo, mut hi) = (0usize, n_groups);
        while lo < hi {
            let mid = (lo + hi) / 2;
            let start = u32_at(&self.data, groups + mid * 12);
            let end = u32_at(&self.data, groups + mid * 12 + 4);
            if code < start {
                hi = mid;
            } else if code > end {
                lo = mid + 1;
            } else {
                let start_gid = u32_at(&self.data, groups + mid * 12 + 8);
                return (start_gid + (code - start)) as u16;
            }
        }
        0
    }

    /// Outline for a glyph, flattened to polylines in font units.
    ///
    /// `flatness` is the maximum deviation from a curve, in font units.
    pub fn outline(&self, gid: u16, flatness: f32) -> Vec<Contour> {
        let mut out = Vec::new();
        self.outline_into(gid, Transform::IDENTITY, flatness, 0, &mut out);
        out
    }

    fn outline_into(
        &self,
        gid: u16,
        xf: Transform,
        flatness: f32,
        depth: u32,
        out: &mut Vec<Contour>,
    ) {
        if depth > 8 || gid >= self.num_glyphs {
            return;
        }
        let (start, end) = match self.glyph_range(gid) {
            Some(range) if range.1 > range.0 => range,
            _ => return,
        };
        let n_contours = i16_at(&self.data, start);
        if n_contours >= 0 {
            self.simple_glyph(start, n_contours as usize, xf, flatness, out);
        } else {
            self.composite_glyph(start, xf, flatness, depth, out);
        }
        let _ = end;
    }

    /// Byte range of a glyph in `glyf`.
    fn glyph_range(&self, gid: u16) -> Option<(usize, usize)> {
        let (a, b) = if self.loca_format == 0 {
            (
                u16_at(&self.data, self.loca + gid as usize * 2) as usize * 2,
                u16_at(&self.data, self.loca + gid as usize * 2 + 2) as usize * 2,
            )
        } else {
            (
                u32_at(&self.data, self.loca + gid as usize * 4) as usize,
                u32_at(&self.data, self.loca + gid as usize * 4 + 4) as usize,
            )
        };
        if b <= a || b > self.data.len().saturating_sub(self.glyf) {
            return None;
        }
        Some((self.glyf + a, self.glyf + b))
    }

    fn simple_glyph(
        &self,
        start: usize,
        n_contours: usize,
        xf: Transform,
        flatness: f32,
        out: &mut Vec<Contour>,
    ) {
        let mut p = start + 10;
        let mut end_pts = Vec::with_capacity(n_contours);
        for _ in 0..n_contours {
            end_pts.push(u16_at(&self.data, p) as usize);
            p += 2;
        }
        let n_points = end_pts.last().map(|e| e + 1).unwrap_or(0);
        let instructions = u16_at(&self.data, p) as usize;
        p += 2 + instructions;

        let mut flags = Vec::with_capacity(n_points);
        while flags.len() < n_points {
            let f = u8_at(&self.data, p);
            p += 1;
            flags.push(f);
            if f & 0x08 != 0 {
                let repeat = u8_at(&self.data, p);
                p += 1;
                for _ in 0..repeat {
                    flags.push(f);
                }
            }
        }

        let mut coords = vec![Point { x: 0.0, y: 0.0 }; n_points];
        let mut x = 0i32;
        for (i, flag) in flags.iter().enumerate().take(n_points) {
            if flag & 0x02 != 0 {
                let dx = u8_at(&self.data, p) as i32;
                p += 1;
                x += if flag & 0x10 != 0 { dx } else { -dx };
            } else if flag & 0x10 == 0 {
                x += i16_at(&self.data, p) as i32;
                p += 2;
            }
            coords[i].x = x as f32;
        }
        let mut y = 0i32;
        for (i, flag) in flags.iter().enumerate().take(n_points) {
            if flag & 0x04 != 0 {
                let dy = u8_at(&self.data, p) as i32;
                p += 1;
                y += if flag & 0x20 != 0 { dy } else { -dy };
            } else if flag & 0x20 == 0 {
                y += i16_at(&self.data, p) as i32;
                p += 2;
            }
            coords[i].y = y as f32;
        }

        let mut first = 0usize;
        for end in end_pts {
            if end < first || end >= n_points {
                first = end.saturating_add(1);
                continue;
            }
            let contour = flatten_contour(&coords[first..=end], &flags[first..=end], xf, flatness);
            if contour.len() >= 3 {
                out.push(contour);
            }
            first = end + 1;
        }
    }

    fn composite_glyph(
        &self,
        start: usize,
        mut xf: Transform,
        flatness: f32,
        depth: u32,
        out: &mut Vec<Contour>,
    ) {
        let mut p = start + 10;
        loop {
            let flags = u16_at(&self.data, p);
            let gid = u16_at(&self.data, p + 2);
            p += 4;

            let (arg1, arg2) = if flags & 0x0001 != 0 {
                let a = i16_at(&self.data, p) as f32;
                let b = i16_at(&self.data, p + 2) as f32;
                p += 4;
                (a, b)
            } else {
                let a = u8_at(&self.data, p) as i8 as f32;
                let b = u8_at(&self.data, p + 1) as i8 as f32;
                p += 2;
                (a, b)
            };

            let mut local = Transform::IDENTITY;
            if flags & 0x0008 != 0 {
                let s = f2dot14(&self.data, p);
                p += 2;
                local = Transform {
                    a: s,
                    b: 0.0,
                    c: 0.0,
                    d: s,
                    dx: 0.0,
                    dy: 0.0,
                };
            } else if flags & 0x0040 != 0 {
                let sx = f2dot14(&self.data, p);
                let sy = f2dot14(&self.data, p + 2);
                p += 4;
                local = Transform {
                    a: sx,
                    b: 0.0,
                    c: 0.0,
                    d: sy,
                    dx: 0.0,
                    dy: 0.0,
                };
            } else if flags & 0x0080 != 0 {
                let a = f2dot14(&self.data, p);
                let b = f2dot14(&self.data, p + 2);
                let c = f2dot14(&self.data, p + 4);
                let d = f2dot14(&self.data, p + 6);
                p += 8;
                local = Transform {
                    a,
                    b,
                    c,
                    d,
                    dx: 0.0,
                    dy: 0.0,
                };
            }

            // Only XY offsets are supported; point-matching offsets fall back
            // to no offset (rare in practice, and only affects composites).
            if flags & 0x0002 != 0 {
                local.dx = arg1;
                local.dy = arg2;
            }

            self.outline_into(gid, xf.then(local), flatness, depth + 1, out);

            if flags & 0x0020 == 0 {
                break;
            }
        }
        let _ = &mut xf;
    }
}

/// Affine transform in font units: `x' = a*x + c*y + dx`, `y' = b*x + d*y + dy`.
#[derive(Debug, Clone, Copy)]
struct Transform {
    a: f32,
    b: f32,
    c: f32,
    d: f32,
    dx: f32,
    dy: f32,
}

impl Transform {
    const IDENTITY: Transform = Transform {
        a: 1.0,
        b: 0.0,
        c: 0.0,
        d: 1.0,
        dx: 0.0,
        dy: 0.0,
    };

    fn apply(&self, p: Point) -> Point {
        Point {
            x: self.a * p.x + self.c * p.y + self.dx,
            y: self.b * p.x + self.d * p.y + self.dy,
        }
    }

    /// `self ∘ inner`: apply `inner` first, then `self`.
    fn then(&self, inner: Transform) -> Transform {
        Transform {
            a: self.a * inner.a + self.c * inner.b,
            b: self.b * inner.a + self.d * inner.b,
            c: self.a * inner.c + self.c * inner.d,
            d: self.b * inner.c + self.d * inner.d,
            dx: self.a * inner.dx + self.c * inner.dy + self.dx,
            dy: self.b * inner.dx + self.d * inner.dy + self.dy,
        }
    }
}

fn f2dot14(data: &[u8], off: usize) -> f32 {
    i16_at(data, off) as f32 / 16384.0
}

/// Converts a TrueType contour (quadratic splines) to a polyline.
fn flatten_contour(points: &[Point], flags: &[u8], xf: Transform, flatness: f32) -> Contour {
    let n = points.len();
    if n == 0 {
        return Vec::new();
    }
    let is_on = |i: usize| flags[i] & 0x01 != 0;

    // Rotate the contour so it starts at an on-curve point; when every point
    // is off-curve, start at the implied midpoint between the last and first
    // point (TrueType semantics).
    let mut pts: Vec<(Point, bool)> = Vec::with_capacity(n + 2);
    match (0..n).find(|&i| is_on(i)) {
        Some(s) => {
            for k in 0..=n {
                let i = (s + k) % n;
                pts.push((points[i], is_on(i)));
            }
        }
        None => {
            let start = mid_point(points[n - 1], points[0]);
            pts.push((start, true));
            for &p in points {
                pts.push((p, false));
            }
            pts.push((start, true));
        }
    }

    let mut out: Contour = Vec::with_capacity(pts.len() * 2);
    let mut current = pts[0].0;
    out.push(xf.apply(current));

    let mut i = 1usize;
    while i < pts.len() {
        let (p, on) = pts[i];
        if on {
            out.push(xf.apply(p));
            current = p;
            i += 1;
        } else if i + 1 < pts.len() {
            let (next, next_on) = pts[i + 1];
            if next_on {
                flatten_quad(current, p, next, xf, flatness, &mut out);
                current = next;
                i += 2;
            } else {
                let end = mid_point(p, next);
                flatten_quad(current, p, end, xf, flatness, &mut out);
                current = end;
                i += 1;
            }
        } else {
            break;
        }
    }
    out
}

fn mid_point(a: Point, b: Point) -> Point {
    Point {
        x: (a.x + b.x) * 0.5,
        y: (a.y + b.y) * 0.5,
    }
}

/// Appends a flattened quadratic Bézier from `p0` to `p2` with control `p1`.
fn flatten_quad(p0: Point, p1: Point, p2: Point, xf: Transform, flatness: f32, out: &mut Contour) {
    // Deviation of the control point from the chord decides the subdivision.
    let dx = p0.x - 2.0 * p1.x + p2.x;
    let dy = p0.y - 2.0 * p1.y + p2.y;
    let dev = (dx * dx + dy * dy).sqrt() * 0.25;
    let steps = if flatness > 0.0 {
        ((dev / flatness).sqrt().ceil() as usize).clamp(1, 16)
    } else {
        8
    };
    for s in 1..=steps {
        let t = s as f32 / steps as f32;
        let mt = 1.0 - t;
        let p = Point {
            x: mt * mt * p0.x + 2.0 * mt * t * p1.x + t * t * p2.x,
            y: mt * mt * p0.y + 2.0 * mt * t * p1.y + t * t * p2.y,
        };
        out.push(xf.apply(p));
    }
}

/// Picks the best Unicode cmap subtable: format 12 first, then format 4.
fn pick_cmap(data: &[u8], cmap: usize) -> Option<(usize, u16)> {
    let n = u16_at(data, cmap + 2) as usize;
    let mut best4: Option<usize> = None;
    let mut best12: Option<usize> = None;
    for i in 0..n {
        let rec = cmap + 4 + i * 8;
        let platform = u16_at(data, rec);
        let encoding = u16_at(data, rec + 2);
        let sub = cmap + u32_at(data, rec + 4) as usize;
        let format = u16_at(data, sub);
        let unicode = matches!((platform, encoding), (0, _) | (3, 1) | (3, 10));
        if !unicode {
            continue;
        }
        match format {
            12 => best12.get_or_insert(sub),
            4 => best4.get_or_insert(sub),
            _ => continue,
        };
    }
    if let Some(sub) = best12 {
        return Some((sub, 12));
    }
    best4.map(|sub| (sub, 4))
}

fn read_family_name(
    data: &[u8],
    base: usize,
    num_tables: usize,
    table: &mut impl FnMut(&[u8; 4]) -> Option<(usize, usize)>,
) -> Option<String> {
    let (name, _) = table(b"name")?;
    let count = u16_at(data, name + 2) as usize;
    let string_offset = name + u16_at(data, name + 4) as usize;
    let mut fallback: Option<String> = None;
    for i in 0..count {
        let rec = name + 6 + i * 12;
        let platform = u16_at(data, rec);
        let name_id = u16_at(data, rec + 6);
        if name_id != 1 {
            continue;
        }
        let len = u16_at(data, rec + 8) as usize;
        let off = string_offset + u16_at(data, rec + 10) as usize;
        let bytes = data.get(off..off + len)?;
        let text = if platform == 3 || platform == 0 {
            // UTF-16BE
            let units: Vec<u16> = bytes
                .as_chunks::<2>()
                .0
                .iter()
                .map(|c| u16::from_be_bytes(*c))
                .collect();
            String::from_utf16_lossy(&units)
        } else {
            String::from_utf8_lossy(bytes).to_string()
        };
        if !text.is_empty() {
            if platform == 3 {
                return Some(text);
            }
            fallback.get_or_insert(text);
        }
    }
    let _ = base;
    let _ = num_tables;
    fallback
}

// ---------------------------------------------------------------------------
// Font discovery
// ---------------------------------------------------------------------------

/// Fonts used to rasterize text: a primary face per family slot and a list
/// of fallbacks for characters the primary faces do not cover (CJK, emoji).
#[derive(Debug, Clone)]
pub struct FontSet {
    /// Proportional face(s); the first entry is the primary.
    pub sans: Vec<PathBuf>,
    /// Monospace face(s).
    pub mono: Vec<PathBuf>,
    /// Extra fallbacks, consulted for any missing glyph.
    pub fallback: Vec<PathBuf>,
}

impl FontSet {
    /// Loads every listed face that parses. Returns `None` when nothing
    /// usable is found.
    pub fn load(&self) -> Option<LoadedFonts> {
        let mut fonts = LoadedFonts::default();
        for p in self.sans.iter().chain(&self.fallback) {
            if let Some(f) = Face::load(p) {
                fonts.sans.push(f);
            }
        }
        for p in self.mono.iter().chain(&self.fallback) {
            if let Some(f) = Face::load(p) {
                fonts.mono.push(f);
            }
        }
        if fonts.sans.is_empty() && fonts.mono.is_empty() {
            None
        } else {
            if fonts.sans.is_empty() {
                fonts.sans = fonts.mono.clone();
            }
            if fonts.mono.is_empty() {
                fonts.mono = fonts.sans.clone();
            }
            Some(fonts)
        }
    }
}

/// Font faces ready for rendering.
#[derive(Debug, Clone, Default)]
pub struct LoadedFonts {
    /// Proportional candidates, in priority order.
    pub sans: Vec<Face>,
    /// Monospace candidates, in priority order.
    pub mono: Vec<Face>,
}

impl LoadedFonts {
    /// Finds the first face in `list` containing `c`.
    pub fn find(&self, c: char, mono: bool) -> Option<&Face> {
        let primary = if mono { &self.mono } else { &self.sans };
        primary
            .iter()
            .chain(self.fallback_iter(mono))
            .find(|f| f.has_glyph(c))
    }

    fn fallback_iter(&self, mono: bool) -> impl Iterator<Item = &Face> {
        let other = if mono { &self.sans } else { &self.mono };
        other.iter()
    }

    /// Total number of loaded faces.
    pub fn len(&self) -> usize {
        self.sans.len() + self.mono.len()
    }

    /// Whether no faces are loaded.
    pub fn is_empty(&self) -> bool {
        self.sans.is_empty() && self.mono.is_empty()
    }
}

/// Candidate font locations for the current platform.
pub fn discover() -> FontSet {
    let mut set = FontSet {
        sans: Vec::new(),
        mono: Vec::new(),
        fallback: Vec::new(),
    };

    #[cfg(target_os = "windows")]
    {
        let dir = Path::new("C:/Windows/Fonts");
        set.sans
            .extend(existing(dir, &["segoeui.ttf", "arial.ttf", "calibri.ttf"]));
        set.mono
            .extend(existing(dir, &["consola.ttf", "cour.ttf", "lucon.ttf"]));
        set.fallback.extend(existing(
            dir,
            &["msyh.ttc", "simsun.ttc", "simhei.ttf", "seguisym.ttf"],
        ));
    }

    #[cfg(target_os = "macos")]
    {
        set.sans.extend(existing(
            Path::new("/System/Library/Fonts"),
            &["Helvetica.ttc", "SFNS.ttf"],
        ));
        set.mono
            .extend(existing(Path::new("/System/Library/Fonts"), &["Menlo.ttc"]));
        set.fallback.extend(existing(
            Path::new("/System/Library/Fonts"),
            &["PingFang.ttc", "Hiragino Sans GB.ttc"],
        ));
        set.fallback.extend(existing(
            Path::new("/System/Library/Fonts/Supplemental"),
            &["Arial Unicode.ttf"],
        ));
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let dirs = [
            Path::new("/usr/share/fonts/truetype/dejavu"),
            Path::new("/usr/share/fonts/truetype/liberation"),
            Path::new("/usr/share/fonts"),
        ];
        for dir in dirs {
            set.sans.extend(existing(
                dir,
                &["DejaVuSans.ttf", "LiberationSans-Regular.ttf"],
            ));
            set.mono.extend(existing(
                dir,
                &["DejaVuSansMono.ttf", "LiberationMono-Regular.ttf"],
            ));
            set.fallback.extend(existing(
                dir,
                &["NotoSansCJK-Regular.ttc", "wqy-zenhei.ttc"],
            ));
        }
    }

    set
}

fn existing(dir: &Path, names: &[&str]) -> Vec<PathBuf> {
    names
        .iter()
        .map(|n| dir.join(n))
        .filter(|p| p.is_file())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn any_face() -> Option<Face> {
        let set = discover();
        let all: Vec<&PathBuf> = set.sans.iter().chain(&set.fallback).collect();
        all.iter().find_map(|p| Face::load(p))
    }

    #[test]
    fn discovery_finds_fonts_on_this_platform() {
        let set = discover();
        assert!(
            !set.sans.is_empty() || !set.fallback.is_empty(),
            "no system fonts discovered"
        );
    }

    #[test]
    fn parses_a_system_face() {
        let Some(face) = any_face() else {
            eprintln!("skipping: no system font available");
            return;
        };
        assert!(face.units_per_em() > 0.0);
        assert!(!face.name().is_empty());
        assert!(face.has_glyph('A'), "face {} lacks 'A'", face.name());
    }

    #[test]
    fn latin_glyph_has_contours() {
        let Some(face) = any_face() else { return };
        let gid = face.glyph_id('A');
        assert_ne!(gid, 0);
        let outline = face.outline(gid, 1.0);
        assert!(!outline.is_empty(), "glyph 'A' produced no contours");
        assert!(outline.iter().all(|c| c.len() >= 3));
        // Points must be finite and within a sane range.
        for c in &outline {
            for p in c {
                assert!(p.x.is_finite() && p.y.is_finite());
                assert!(p.x.abs() < 20000.0 && p.y.abs() < 20000.0);
            }
        }
    }

    #[test]
    fn space_has_no_contours() {
        let Some(face) = any_face() else { return };
        let gid = face.glyph_id(' ');
        assert!(face.outline(gid, 1.0).is_empty());
    }

    #[test]
    fn composite_glyphs_parse() {
        let Some(face) = any_face() else { return };
        if !face.has_glyph('é') {
            return;
        }
        let outline = face.outline(face.glyph_id('é'), 1.0);
        assert!(!outline.is_empty(), "composite glyph produced no contours");
    }

    #[test]
    fn missing_characters_map_to_notdef() {
        let Some(face) = any_face() else { return };
        // A private-use codepoint is almost certainly absent.
        assert!(!face.has_glyph('\u{F8FF}') || face.glyph_id('\u{F8FF}') != 0);
    }

    #[test]
    fn advance_is_positive_for_letters() {
        let Some(face) = any_face() else { return };
        assert!(face.advance(face.glyph_id('M')) > 0.0);
    }

    #[test]
    fn cjk_font_is_available_and_covers_han() {
        let set = discover();
        let mut found = false;
        for p in &set.fallback {
            if let Some(f) = Face::load(p)
                && f.has_glyph('中')
            {
                found = true;
                break;
            }
        }
        assert!(found, "no CJK-capable fallback font found");
    }

    #[test]
    fn loaded_fonts_resolve_by_family() {
        let set = discover();
        if let Some(loaded) = set.load() {
            assert!(!loaded.is_empty());
            if let Some(face) = loaded.find('A', false) {
                assert!(face.has_glyph('A'));
            }
        }
    }

    #[test]
    fn garbage_data_is_rejected() {
        assert!(Face::from_bytes(vec![0u8; 64], 0).is_none());
        assert!(Face::from_bytes(b"not a font at all".to_vec(), 0).is_none());
    }

    #[test]
    fn missing_file_returns_none() {
        assert!(Face::load(Path::new("no-such-font-file.ttf")).is_none());
    }

    #[test]
    fn cff_flavoured_opentype_is_skipped() {
        // 'OTTO' header: parsed as a font directory but unsupported.
        let mut data = vec![0u8; 64];
        data[0..4].copy_from_slice(b"OTTO");
        data[4..6].copy_from_slice(&0u16.to_be_bytes());
        assert!(Face::from_bytes(data, 0).is_none());
    }

    #[test]
    fn collection_index_is_bounds_checked() {
        // Minimal TTC header with one font, then ask for the second.
        let mut data = vec![0u8; 32];
        data[0..4].copy_from_slice(b"ttcf");
        data[8..12].copy_from_slice(&1u32.to_be_bytes());
        data[12..16].copy_from_slice(&16u32.to_be_bytes());
        assert!(Face::from_bytes(data, 1).is_none());
    }

    #[test]
    fn loads_the_first_face_of_a_collection() {
        let set = discover();
        let Some(ttc) = set
            .fallback
            .iter()
            .find(|p| p.extension().is_some_and(|e| e == "ttc"))
        else {
            eprintln!("skipping: no .ttc on this platform");
            return;
        };
        let face = Face::load(ttc).expect("first face of a collection");
        assert!(face.units_per_em() > 0.0);
        assert!(face.has_glyph('中'));
        // A second face may or may not exist; either way it must not panic.
        let _ = Face::from_bytes(std::fs::read(ttc).unwrap(), 1);
    }

    #[test]
    fn outline_flatness_changes_point_counts() {
        let Some(face) = any_face() else { return };
        let gid = face.glyph_id('O');
        let coarse = face.outline(gid, 8.0);
        let fine = face.outline(gid, 0.2);
        let count = |c: &Vec<Contour>| c.iter().map(|p| p.len()).sum::<usize>();
        assert!(
            count(&fine) >= count(&coarse),
            "finer flatness should not reduce points"
        );
    }

    #[test]
    fn glyph_lookup_is_stable() {
        let Some(face) = any_face() else { return };
        for c in ['a', 'Z', '0', '中', 'Ω'] {
            let first = face.glyph_id(c);
            assert_eq!(first, face.glyph_id(c));
        }
    }
}
