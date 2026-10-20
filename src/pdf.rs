//! 自研 PDF 生成器（无外部二进制依赖）。
//!
//! - 逐图框一页，页面尺寸 = 图框实际几何尺寸（mm → PDF 点）
//! - 内嵌 TrueType 中文字体（Identity-H / CIDFontType2），支持 CJK
//! - 图元：折线描边、多边形填充、文本

use std::collections::BTreeMap;

use ttf_parser::Face;

use crate::flatten::{Prim, PrimKind};

const MM_TO_PT: f64 = 72.0 / 25.4;

/// 一页（已换算为 PDF 点坐标的图元）
pub struct Page {
    pub w_pt: f64,
    pub h_pt: f64,
    /// 原始图元（世界坐标，图纸单位），由 build 内部转换
    pub prims: Vec<Prim>,
    /// 世界坐标 → 页面点的映射参数： px = (x - ox) * k, py = (y - oy) * k
    pub ox: f64,
    pub oy: f64,
    pub k: f64,
}

// ---------------------------------------------------------------------------
// PDF 对象容器
// ---------------------------------------------------------------------------
struct Pdf {
    objs: Vec<Vec<u8>>,
}

impl Pdf {
    fn new() -> Self {
        Pdf { objs: Vec::new() }
    }
    fn add(&mut self, body: Vec<u8>) -> usize {
        self.objs.push(body);
        self.objs.len()
    }
    fn reserve(&mut self) -> usize {
        self.objs.push(Vec::new());
        self.objs.len()
    }
    fn set(&mut self, id: usize, body: Vec<u8>) {
        self.objs[id - 1] = body;
    }
    fn stream(&mut self, dict_extra: &str, data: &[u8]) -> usize {
        let mut b = Vec::new();
        b.extend_from_slice(format!("<< /Length {}{} >>\nstream\n", data.len(), dict_extra).as_bytes());
        b.extend_from_slice(data);
        b.extend_from_slice(b"\nendstream");
        self.add(b)
    }
    fn into_bytes(self, root: usize) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(b"%PDF-1.7\n");
        out.extend_from_slice(b"%\xE2\xE3\xCF\xD3\n");
        let mut offsets = Vec::with_capacity(self.objs.len());
        for (i, o) in self.objs.iter().enumerate() {
            offsets.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
            out.extend_from_slice(o);
            out.extend_from_slice(b"\nendobj\n");
        }
        let xref_pos = out.len();
        let n = self.objs.len();
        out.extend_from_slice(format!("xref\n0 {}\n", n + 1).as_bytes());
        out.extend_from_slice(b"0000000000 65535 f \n");
        for off in &offsets {
            out.extend_from_slice(format!("{:010} 00000 n \n", off).as_bytes());
        }
        out.extend_from_slice(
            format!("trailer\n<< /Size {} /Root {} 0 R >>\nstartxref\n{}\n%%EOF\n", n + 1, root, xref_pos)
                .as_bytes(),
        );
        out
    }
}

// ---------------------------------------------------------------------------
// 数值格式化
// ---------------------------------------------------------------------------
fn n(v: f64) -> String {
    if !v.is_finite() {
        return "0".to_string();
    }
    let r = (v * 1000.0).round() / 1000.0;
    if r == 0.0 { "0".to_string() } else { format!("{}", r) }
}

/// 世界坐标 → 页面点
fn to_pt(p: (f64, f64), ox: f64, oy: f64, k: f64) -> (f64, f64) {
    ((p.0 - ox) * k, (p.1 - oy) * k)
}

// ---------------------------------------------------------------------------
// 构建 PDF
// ---------------------------------------------------------------------------
/// 字体文件是否可解析且包含常用汉字（用于筛选可用的中文字体）
pub fn font_has_cjk(path: &str) -> bool {
    let Ok(data) = std::fs::read(path) else {
        return false;
    };
    let Ok(face) = Face::parse(&data, 0) else {
        return false;
    };
    ['中', '图', '一'].iter().all(|c| face.glyph_index(*c).is_some())
}

pub fn build(pages: &[Page], font_path: &str) -> Result<Vec<u8>, String> {
    let font_data = std::fs::read(font_path).map_err(|e| format!("读取字体失败 {}: {}", font_path, e))?;
    let face = Face::parse(&font_data, 0).map_err(|e| format!("解析字体失败: {:?}", e))?;
    let upm = face.units_per_em() as f64;
    if upm <= 0.0 {
        return Err("字体 units_per_em 非法".into());
    }

    // 收集用到的字形及其宽度（单位 em → 1000 单位）
    let mut used: BTreeMap<u16, f64> = BTreeMap::new();
    for pg in pages {
        for p in &pg.prims {
            if let PrimKind::Text { s, .. } = &p.kind {
                for ch in s.chars() {
                    if let Some(g) = face.glyph_index(ch) {
                        let adv = face.glyph_hor_advance(g).unwrap_or((upm * 0.5) as u16) as f64;
                        used.entry(g.0).or_insert(adv * 1000.0 / upm);
                    }
                }
            }
        }
    }

    // 字体对象
    let gbb = face.global_bounding_box();
    let fb = [
        gbb.x_min as f64 * 1000.0 / upm,
        gbb.y_min as f64 * 1000.0 / upm,
        gbb.x_max as f64 * 1000.0 / upm,
        gbb.y_max as f64 * 1000.0 / upm,
    ];
    let ascent = face.ascender() as f64 * 1000.0 / upm;
    let descent = face.descender() as f64 * 1000.0 / upm;
    let cap = face.capital_height().unwrap_or((upm * 0.7) as i16) as f64 * 1000.0 / upm;
    let fname = "CJKFont";

    let mut pdf = Pdf::new();
    // FontFile2（先占位，含 /Length1）
    let ffile_id = pdf.reserve();
    {
        let mut b = Vec::new();
        b.extend_from_slice(
            format!("<< /Length {} /Length1 {} >>\nstream\n", font_data.len(), font_data.len()).as_bytes(),
        );
        b.extend_from_slice(&font_data);
        b.extend_from_slice(b"\nendstream");
        pdf.set(ffile_id, b);
    }
    let fdesc_id = pdf.add(
        format!(
            "<< /Type /FontDescriptor /FontName /{} /Flags 4 /FontBBox [{} {} {} {}] /ItalicAngle 0 /Ascent {} /Descent {} /CapHeight {} /StemV 80 /FontFile2 {} 0 R >>",
            fname, n(fb[0]), n(fb[1]), n(fb[2]), n(fb[3]), n(ascent), n(descent), n(cap), ffile_id
        )
        .into_bytes(),
    );
    // /W 数组
    let mut w = String::from("[ ");
    for (gid, adv) in &used {
        w.push_str(&format!("{} [{}] ", gid, n(*adv)));
    }
    w.push(']');
    let cid_id = pdf.add(
        format!(
            "<< /Type /Font /Subtype /CIDFontType2 /BaseFont /{} /CIDSystemInfo << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >> /FontDescriptor {} 0 R /DW 1000 /W {} /CIDToGIDMap /Identity >>",
            fname, fdesc_id, w
        )
        .into_bytes(),
    );
    let type0_id = pdf.add(
        format!(
            "<< /Type /Font /Subtype /Type0 /BaseFont /{} /Encoding /Identity-H /DescendantFonts [{} 0 R] >>",
            fname, cid_id
        )
        .into_bytes(),
    );

    // 页面
    let pages_id = pdf.reserve();
    let mut kids: Vec<usize> = Vec::new();
    for pg in pages {
        let content = build_content(pg, &face, upm, cap);
        let cref = pdf.stream("", &content);
        let pid = pdf.add(
            format!(
                "<< /Type /Page /Parent {} 0 R /MediaBox [0 0 {} {}] /Resources << /Font << /F0 {} 0 R >> /ProcSet [/PDF /Text] >> /Contents {} 0 R >>",
                pages_id,
                n(pg.w_pt),
                n(pg.h_pt),
                type0_id,
                cref
            )
            .into_bytes(),
        );
        kids.push(pid);
    }
    let kids_s: String = kids.iter().map(|k| format!("{} 0 R ", k)).collect();
    pdf.set(
        pages_id,
        format!("<< /Type /Pages /Kids [ {}] /Count {} >>", kids_s, kids.len()).into_bytes(),
    );
    let catalog_id = pdf.add(
        format!("<< /Type /Catalog /Pages {} 0 R >>", pages_id).into_bytes(),
    );

    Ok(pdf.into_bytes(catalog_id))
}

/// 生成一页内容流
fn build_content(pg: &Page, face: &Face, upm: f64, cap: f64) -> Vec<u8> {
    let mut c: Vec<u8> = Vec::new();
    c.extend_from_slice(b"1 J 1 j\n"); // 圆头、圆角
    for p in &pg.prims {
        match &p.kind {
            PrimKind::Stroke { pts, closed, lw_mm } => {
                if pts.len() < 2 {
                    continue;
                }
                let lw = if *lw_mm > 0.0 { lw_mm.min(2.5) } else { 0.15 };
                c.extend_from_slice(format!("{} w 0 G\n", n(lw * MM_TO_PT)).as_bytes());
                emit_path(&mut c, pts, &pg, *closed);
                c.extend_from_slice(b"S\n");
            }
            PrimKind::Fill { pts } => {
                if pts.len() < 3 {
                    continue;
                }
                c.extend_from_slice(b"0.6 g\n");
                emit_path(&mut c, pts, &pg, true);
                c.extend_from_slice(b"f\n");
            }
            PrimKind::Text { x, y, h, rot, wf, halign, valign, s } => {
                if s.is_empty() || *h <= 0.0 {
                    continue;
                }
                emit_text(&mut c, *x, *y, *h, *rot, *wf, *halign, *valign, s, &pg, face, upm, cap);
            }
        }
    }
    c
}

fn emit_path(c: &mut Vec<u8>, pts: &[(f64, f64)], pg: &Page, closed: bool) {
    for (i, &p) in pts.iter().enumerate() {
        let (x, y) = to_pt(p, pg.ox, pg.oy, pg.k);
        if i == 0 {
            c.extend_from_slice(format!("{} {} m\n", n(x), n(y)).as_bytes());
        } else {
            c.extend_from_slice(format!("{} {} l\n", n(x), n(y)).as_bytes());
        }
    }
    if closed {
        c.extend_from_slice(b"h\n");
    }
}

#[allow(clippy::too_many_arguments)]
fn emit_text(
    c: &mut Vec<u8>,
    x: f64,
    y: f64,
    h: f64,
    rot: f64,
    wf: f64,
    halign: u8,
    valign: u8,
    s: &str,
    pg: &Page,
    face: &Face,
    upm: f64,
    cap: f64,
) {
    // 字形编码 + 宽度
    let mut glyphs: Vec<u16> = Vec::with_capacity(s.chars().count());
    let mut width_em = 0.0f64;
    for ch in s.chars() {
        if let Some(g) = face.glyph_index(ch) {
            glyphs.push(g.0);
            width_em += face.glyph_hor_advance(g).unwrap_or((upm * 0.5) as u16) as f64;
        }
    }
    if glyphs.is_empty() {
        return;
    }

    let size_pt = h * pg.k; // 字高(pt)
    let w_pt = width_em / upm * size_pt * wf;
    let cap_pt = cap / upm * size_pt;

    let (sin, cos) = (rot.sin(), rot.cos());
    // 基线锚点（世界→点）
    let (mut px, mut py) = to_pt((x, y), pg.ox, pg.oy, pg.k);
    // 水平对齐：沿文本方向平移
    let shift_h = match halign {
        1 => -w_pt / 2.0,
        2 => -w_pt,
        _ => 0.0,
    };
    // 垂直对齐：沿字高方向平移
    let shift_v = match valign {
        2 => -cap_pt / 2.0,
        3 => -cap_pt,
        _ => 0.0,
    };
    px += cos * shift_h - sin * shift_v;
    py += sin * shift_h + cos * shift_v;

    let mut hex = String::with_capacity(glyphs.len() * 4);
    for g in glyphs {
        hex.push_str(&format!("{:04X}", g));
    }
    c.extend_from_slice(
        format!(
            "BT /F0 {} Tf {} {} {} {} {} {} Tm <{}> Tj ET\n",
            n(size_pt),
            n(cos * wf),
            n(sin * wf),
            n(-sin),
            n(cos),
            n(px),
            n(py),
            hex
        )
        .as_bytes(),
    );
}
