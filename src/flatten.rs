//! 实体 → 矢量图元扁平化。
//!
//! 模型空间的实体（含 INSERT 递归展开）统一转换为两种基本图元：
//! 折线(描边/填充) 与 文本。渲染层(PDF)只处理这两种图元。

use acadrust::entities::{EntityType, Text};
use acadrust::CadDocument;

use crate::geom::{insert_transform, T2};

const TAU: f64 = std::f64::consts::TAU;

/// 水平对齐：0=左 1=中 2=右
pub type HAlign = u8;
/// 垂直对齐：0=基线 1=底 2=中 3=顶
pub type VAlign = u8;

#[derive(Clone, Debug)]
pub enum PrimKind {
    /// 折线（描边）。closed 表示闭合；lw_mm 为线宽(mm)，0 表示默认细线
    Stroke { pts: Vec<(f64, f64)>, closed: bool, lw_mm: f64 },
    /// 填充多边形
    Fill { pts: Vec<(f64, f64)> },
    /// 文本。x,y 为锚点(基线点参考)，h 为字高(图纸单位)，rot 为旋转(弧度)
    Text { x: f64, y: f64, h: f64, rot: f64, wf: f64, halign: HAlign, valign: VAlign, s: String },
}

#[derive(Clone, Debug)]
pub struct Prim {
    /// 世界包围盒 (min_x,min_y,max_x,max_y)
    pub bbox: (f64, f64, f64, f64),
    pub layer: String,
    pub kind: PrimKind,
}

impl Prim {
    pub fn valid(&self) -> bool {
        self.bbox.0 <= self.bbox.2 && self.bbox.1 <= self.bbox.3
    }
    pub fn intersects(&self, b: (f64, f64, f64, f64), pad: f64) -> bool {
        self.valid()
            && self.bbox.2 >= b.0 - pad
            && self.bbox.0 <= b.2 + pad
            && self.bbox.3 >= b.1 - pad
            && self.bbox.1 <= b.3 + pad
    }
}

// ---------------------------------------------------------------------------
// 变换辅助
// ---------------------------------------------------------------------------

/// 提取变换的 x/y 尺度与 x 轴旋转角
fn xf_scale_rot(xf: &T2) -> (f64, f64, f64) {
    let o = xf.apply((0.0, 0.0));
    let ex = xf.apply((1.0, 0.0));
    let ey = xf.apply((0.0, 1.0));
    let sx = ((ex.0 - o.0).powi(2) + (ex.1 - o.1).powi(2)).sqrt();
    let sy = ((ey.0 - o.0).powi(2) + (ey.1 - o.1).powi(2)).sqrt();
    let rot = (ex.1 - o.1).atan2(ex.0 - o.0);
    (sx, sy, rot)
}

fn map_halign(h: &acadrust::entities::TextHorizontalAlignment) -> HAlign {
    use acadrust::entities::TextHorizontalAlignment::*;
    match h {
        Center | Middle => 1,
        Right => 2,
        _ => 0,
    }
}

fn map_valign(v: &acadrust::entities::TextVerticalAlignment) -> VAlign {
    use acadrust::entities::TextVerticalAlignment::*;
    match v {
        Bottom => 1,
        Middle => 2,
        Top => 3,
        _ => 0,
    }
}

// ---------------------------------------------------------------------------
// 圆弧采样
// ---------------------------------------------------------------------------
fn arc_pts(cx: f64, cy: f64, r: f64, a0: f64, a1: f64) -> Vec<(f64, f64)> {
    let mut span = (a1 - a0) % TAU;
    if span < 0.0 {
        span += TAU;
    }
    if span < 1e-9 {
        span = TAU; // 视为整圆
    }
    let n = ((span / 0.12).ceil() as usize).clamp(8, 1024);
    (0..=n)
        .map(|i| {
            let a = a0 + span * (i as f64) / (n as f64);
            (cx + r * a.cos(), cy + r * a.sin())
        })
        .collect()
}

fn bbox_of(pts: &[(f64, f64)]) -> (f64, f64, f64, f64) {
    let mut b = (f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
    for &(x, y) in pts {
        if x.is_finite() && y.is_finite() {
            b.0 = b.0.min(x);
            b.1 = b.1.min(y);
            b.2 = b.2.max(x);
            b.3 = b.3.max(y);
        }
    }
    b
}

// ---------------------------------------------------------------------------
// 主入口
// ---------------------------------------------------------------------------
pub fn flatten_model(doc: &CadDocument) -> Vec<Prim> {
    let mut out = Vec::new();
    let id = T2::identity();
    for e in doc.model_space_entities() {
        flatten_ent(doc, e, &id, &mut out, 0);
    }
    out
}

fn flatten_ent(doc: &CadDocument, e: &EntityType, xf: &T2, out: &mut Vec<Prim>, depth: usize) {
    if depth > 12 {
        return;
    }
    let cm = e.common();
    if cm.invisible {
        return;
    }
    if cm.layer.eq_ignore_ascii_case("defpoints") {
        return;
    }
    let layer = cm.layer.clone();
    let lw = cm.line_weight.millimeters().unwrap_or(0.0);

    match e {
        EntityType::Insert(ins) => {
            let base = doc
                .block_records
                .get(&ins.block_name)
                .map(|b| (b.base_point.x, b.base_point.y))
                .unwrap_or((0.0, 0.0));
            let child = xf.mul(&insert_transform(ins, base));
            for be in doc.entities_in_block(&ins.block_name) {
                flatten_ent(doc, be, &child, out, depth + 1);
            }
            // 块属性值
            //
            // 注意：ATTRIB 的坐标/字高/旋转已经是在**所在容器(父)坐标系**中的最终值
            // （AutoCAD 存储时已把本 INSERT 的平移/缩放/旋转换算进去），因此只能
            // 再套用父级变换 xf，绝不能再叠加 child（否则会二次变换，轴号飞到远处）。
            for a in &ins.attributes {
                if a.common.invisible || a.value.is_empty() {
                    continue;
                }
                let (sx, sy, rot) = xf_scale_rot(xf);
                let (x, y) = xf.apply((a.insertion_point.x, a.insertion_point.y));
                let h = a.height * (sx * sy).sqrt();
                push_text_prim(out, &layer, x, y, h, a.rotation + rot, a.width_factor, &a.value,
                    map_halign_pub(&a.horizontal_alignment), map_valign_pub(&a.vertical_alignment));
            }
        }
        EntityType::Text(t) => {
            push_text(t, xf, out, &layer);
        }
        EntityType::MText(_) => {
            for p in e.explode() {
                flatten_ent(doc, &p, xf, out, depth + 1);
            }
        }
        EntityType::Line(l) => {
            let pts = xf2(&[(l.start.x, l.start.y), (l.end.x, l.end.y)], xf);
            out.push(Prim { bbox: bbox_of(&pts), layer, kind: PrimKind::Stroke { pts, closed: false, lw_mm: lw } });
        }
        EntityType::Arc(a) => {
            let pts = xf2(&arc_pts(a.center.x, a.center.y, a.radius, a.start_angle, a.end_angle), xf);
            out.push(Prim { bbox: bbox_of(&pts), layer, kind: PrimKind::Stroke { pts, closed: false, lw_mm: lw } });
        }
        EntityType::Circle(c) => {
            let pts = xf2(&arc_pts(c.center.x, c.center.y, c.radius, 0.0, TAU), xf);
            out.push(Prim { bbox: bbox_of(&pts), layer, kind: PrimKind::Stroke { pts, closed: true, lw_mm: lw } });
        }
        EntityType::Solid(s) => {
            let mut pts: Vec<(f64, f64)> = s.boundary_corners().iter().map(|v| (v.x, v.y)).collect();
            dedup_pts(&mut pts);
            if pts.len() >= 3 {
                let pts = xf2(&pts, xf);
                out.push(Prim { bbox: bbox_of(&pts), layer, kind: PrimKind::Fill { pts } });
            }
        }
        EntityType::Hatch(h) => {
            flatten_hatch(h, xf, out, &layer);
        }
        _ => {
            let parts = e.explode();
            for p in parts {
                flatten_ent(doc, &p, xf, out, depth + 1);
            }
        }
    }
}

fn dedup_pts(pts: &mut Vec<(f64, f64)>) {
    pts.dedup_by(|a, b| (a.0 - b.0).abs() < 1e-9 && (a.1 - b.1).abs() < 1e-9);
}

fn xf2(pts: &[(f64, f64)], xf: &T2) -> Vec<(f64, f64)> {
    pts.iter().map(|&p| xf.apply(p)).collect()
}

pub fn map_halign_pub(h: &acadrust::entities::HorizontalAlignment) -> HAlign {
    use acadrust::entities::HorizontalAlignment::*;
    match h {
        Center | Middle => 1,
        Right => 2,
        _ => 0,
    }
}

pub fn map_valign_pub(v: &acadrust::entities::VerticalAlignment) -> VAlign {
    use acadrust::entities::VerticalAlignment::*;
    match v {
        Middle => 2,
        Top => 3,
        _ => 0,
    }
}

fn push_text(t: &Text, xf: &T2, out: &mut Vec<Prim>, layer: &str) {
    if t.value.is_empty() {
        return;
    }
    let (anchor, halign, valign) = match t.alignment_point {
        Some(ap) => (ap, map_halign(&t.horizontal_alignment), map_valign(&t.vertical_alignment)),
        None => (t.insertion_point, 0u8, 0u8),
    };
    let (sx, sy, rot) = xf_scale_rot(xf);
    let (x, y) = xf.apply((anchor.x, anchor.y));
    let h = t.height * (sx * sy).sqrt();
    push_text_prim(
        out,
        layer,
        x,
        y,
        h,
        t.rotation + rot,
        t.width_factor,
        &t.value,
        halign,
        valign,
    );
}

#[allow(clippy::too_many_arguments)]
fn push_text_prim(
    out: &mut Vec<Prim>,
    layer: &str,
    x: f64,
    y: f64,
    h: f64,
    rot: f64,
    wf: f64,
    s: &str,
    halign: HAlign,
    valign: VAlign,
) {
    if s.is_empty() || !h.is_finite() || h <= 0.0 {
        return;
    }
    // 粗估文本包围盒（用于按图框裁剪）
    let n = s.chars().count() as f64;
    let w = n * h * 0.75 * if wf > 0.0 { wf } else { 1.0 };
    let (dx, dy) = if rot.abs() < 1e-6 { (w, h * 1.2) } else { (w.max(h * 1.2), w.max(h * 1.2)) };
    let bbox = (x - if halign == 1 { dx / 2.0 } else if halign == 2 { dx } else { 0.0 },
                y,
                x + if halign == 1 { dx / 2.0 } else if halign == 2 { 0.0 } else { dx },
                y + dy);
    out.push(Prim {
        bbox,
        layer: layer.to_string(),
        kind: PrimKind::Text { x, y, h, rot, wf: if wf > 0.0 { wf } else { 1.0 }, halign, valign, s: s.to_string() },
    });
}

/// HATCH：边界路径转为填充多边形（无边界视为无效）
fn flatten_hatch(h: &acadrust::entities::Hatch, xf: &T2, out: &mut Vec<Prim>, layer: &str) {
    use acadrust::entities::BoundaryEdge as BE;
    for path in &h.paths {
        let mut pts: Vec<(f64, f64)> = Vec::new();
        for edge in &path.edges {
            match edge {
                BE::Line(le) => {
                    pts.push((le.start.x, le.start.y));
                    pts.push((le.end.x, le.end.y));
                }
                BE::CircularArc(a) => {
                    let (sa, ea) = if a.counter_clockwise {
                        (a.start_angle, a.end_angle)
                    } else {
                        (a.end_angle, a.start_angle)
                    };
                    pts.extend(arc_pts(a.center.x, a.center.y, a.radius, sa, ea));
                }
                BE::Polyline(pl) => {
                    let n = pl.vertices.len();
                    let seg = if pl.is_closed { n } else { n.saturating_sub(1) };
                    for i in 0..seg {
                        let v1 = pl.vertices[i];
                        let v2 = pl.vertices[(i + 1) % n];
                        let bulge = v1.z;
                        if bulge.abs() < 1e-9 {
                            pts.push((v1.x, v1.y));
                            pts.push((v2.x, v2.y));
                        } else {
                            pts.extend(arc_from_bulge(v1.x, v1.y, v2.x, v2.y, bulge));
                        }
                    }
                }
                // 椭圆弧/样条边界在工程图中罕见，简化为端点
                BE::EllipticArc(ea) => {
                    pts.push((ea.center.x, ea.center.y));
                }
                BE::Spline(sp) => {
                    for cp in &sp.control_points {
                        pts.push((cp.x, cp.y));
                    }
                }
            }
        }
        dedup_pts(&mut pts);
        if pts.len() >= 3 {
            let pts = xf2(&pts, xf);
            out.push(Prim { bbox: bbox_of(&pts), layer: layer.to_string(), kind: PrimKind::Fill { pts } });
        }
    }
}

// ---------------------------------------------------------------------------
// 文本查询辅助
// ---------------------------------------------------------------------------

/// 图元中的文本项（世界坐标）
#[derive(Clone, Debug)]
pub struct TextItem {
    pub s: String,
    pub x: f64,
    pub y: f64,
    pub h: f64,
}

/// 取出与给定世界包围盒相交的所有文本（按 y 降序、x 升序排列）
pub fn texts_in(prims: &[Prim], bbox: (f64, f64, f64, f64)) -> Vec<TextItem> {
    let mut v: Vec<TextItem> = prims
        .iter()
        .filter(|p| p.intersects(bbox, 0.0))
        .filter_map(|p| match &p.kind {
            PrimKind::Text { x, y, h, s, .. } => Some(TextItem {
                s: s.clone(),
                x: *x,
                y: *y,
                h: *h,
            }),
            _ => None,
        })
        .collect();
    v.sort_by(|a, b| {
        b.y.partial_cmp(&a.y)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.x.partial_cmp(&b.x).unwrap_or(std::cmp::Ordering::Equal))
    });
    v
}

/// 由凸度(bulge)生成圆弧采样点
fn arc_from_bulge(x1: f64, y1: f64, x2: f64, y2: f64, bulge: f64) -> Vec<(f64, f64)> {
    let dx = x2 - x1;
    let dy = y2 - y1;
    let d = (dx * dx + dy * dy).sqrt();
    if d < 1e-12 {
        return vec![(x1, y1), (x2, y2)];
    }
    let b = bulge;
    let r = d * (1.0 + b * b) / (4.0 * b.abs());
    let perp = d * (1.0 - b * b) / (4.0 * b);
    let mx = (x1 + x2) / 2.0;
    let my = (y1 + y2) / 2.0;
    let px = -dy / d;
    let py = dx / d;
    let cx = mx + px * perp;
    let cy = my + py * perp;
    let sa = (y1 - cy).atan2(x1 - cx);
    let ea = (y2 - cy).atan2(x2 - cx);
    arc_pts(cx, cy, r, sa, ea)
}

