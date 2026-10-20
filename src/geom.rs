//! 几何工具：2D 仿射变换、包围盒、实体关键点、块世界包围盒。

use acadrust::entities::{EntityType, Insert};
use acadrust::CadDocument;

// ---------------------------------------------------------------------------
// 2x3 仿射变换 (列主序): x' = a*x + c*y + e ; y' = b*x + d*y + f
// ---------------------------------------------------------------------------
#[derive(Clone, Copy, Debug)]
pub struct T2 {
    pub a: f64,
    pub b: f64,
    pub c: f64,
    pub d: f64,
    pub e: f64,
    pub f: f64,
}

impl T2 {
    pub fn identity() -> Self {
        T2 { a: 1.0, b: 0.0, c: 0.0, d: 1.0, e: 0.0, f: 0.0 }
    }

    pub fn apply(&self, p: (f64, f64)) -> (f64, f64) {
        (
            self.a * p.0 + self.c * p.1 + self.e,
            self.b * p.0 + self.d * p.1 + self.f,
        )
    }

    /// self ∘ other : 先 other 后 self
    pub fn mul(&self, o: &T2) -> T2 {
        T2 {
            a: self.a * o.a + self.c * o.b,
            b: self.b * o.a + self.d * o.b,
            c: self.a * o.c + self.c * o.d,
            d: self.b * o.c + self.d * o.d,
            e: self.a * o.e + self.c * o.f + self.e,
            f: self.b * o.e + self.d * o.f + self.f,
        }
    }
}

/// 由 INSERT 的插入点/缩放/旋转以及块基点构造变换: world = insert + R*S*(p - base)
pub fn insert_transform(ins: &Insert, base: (f64, f64)) -> T2 {
    let (sx, sy) = (ins.x_scale(), ins.y_scale());
    let (cs, sn) = (ins.rotation.cos(), ins.rotation.sin());
    let a = cs * sx;
    let b = sn * sx;
    let c = -sn * sy;
    let d = cs * sy;
    let e = ins.insert_point.x - (a * base.0 + c * base.1);
    let f = ins.insert_point.y - (b * base.0 + d * base.1);
    T2 { a, b, c, d, e, f }
}

// ---------------------------------------------------------------------------
// 包围盒
// ---------------------------------------------------------------------------
#[derive(Clone, Copy, Debug)]
pub struct BBox {
    pub min_x: f64,
    pub min_y: f64,
    pub max_x: f64,
    pub max_y: f64,
}

impl BBox {
    pub fn empty() -> Self {
        BBox { min_x: f64::INFINITY, min_y: f64::INFINITY, max_x: f64::NEG_INFINITY, max_y: f64::NEG_INFINITY }
    }
    pub fn add(&mut self, p: (f64, f64)) {
        if !p.0.is_finite() || !p.1.is_finite() {
            return;
        }
        self.min_x = self.min_x.min(p.0);
        self.min_y = self.min_y.min(p.1);
        self.max_x = self.max_x.max(p.0);
        self.max_y = self.max_y.max(p.1);
    }
    pub fn merge(&mut self, o: &BBox) {
        if o.valid() {
            self.add((o.min_x, o.min_y));
            self.add((o.max_x, o.max_y));
        }
    }
    pub fn valid(&self) -> bool {
        self.min_x <= self.max_x && self.min_y <= self.max_y
    }
    pub fn w(&self) -> f64 {
        self.max_x - self.min_x
    }
    pub fn h(&self) -> f64 {
        self.max_y - self.min_y
    }
    pub fn contains(&self, p: (f64, f64), pad: f64) -> bool {
        p.0 >= self.min_x - pad && p.0 <= self.max_x + pad && p.1 >= self.min_y - pad && p.1 <= self.max_y + pad
    }
}

/// 实体的本地(块坐标系)关键点集合
pub fn entity_local_points(e: &EntityType) -> Vec<(f64, f64)> {
    let mut v = Vec::new();
    match e {
        EntityType::Line(l) => {
            v.push((l.start.x, l.start.y));
            v.push((l.end.x, l.end.y));
        }
        EntityType::Point(p) => v.push((p.location.x, p.location.y)),
        EntityType::Circle(c) => {
            v.push((c.center.x - c.radius, c.center.y - c.radius));
            v.push((c.center.x + c.radius, c.center.y + c.radius));
        }
        EntityType::Arc(a) => {
            v.push((a.center.x - a.radius, a.center.y - a.radius));
            v.push((a.center.x + a.radius, a.center.y + a.radius));
            let mut ang = a.start_angle;
            let end = a.end_angle;
            let mut steps = 0;
            while steps < 64 {
                v.push((a.center.x + a.radius * ang.cos(), a.center.y + a.radius * ang.sin()));
                if ang >= end - 1e-9 {
                    break;
                }
                ang = (ang + 0.2).min(end);
                steps += 1;
            }
        }
        EntityType::LwPolyline(lw) => {
            for vtx in &lw.vertices {
                v.push((vtx.location.x, vtx.location.y));
            }
        }
        EntityType::Polyline(pl) => {
            for vtx in &pl.vertices {
                v.push((vtx.location.x, vtx.location.y));
            }
        }
        EntityType::Polyline2D(pl) => {
            for vtx in &pl.vertices {
                v.push((vtx.location.x, vtx.location.y));
            }
        }
        EntityType::Polyline3D(pl) => {
            for vtx in &pl.vertices {
                v.push((vtx.position.x, vtx.position.y));
            }
        }
        EntityType::Solid(s) => {
            v.push((s.first_corner.x, s.first_corner.y));
            v.push((s.second_corner.x, s.second_corner.y));
            v.push((s.third_corner.x, s.third_corner.y));
            v.push((s.fourth_corner.x, s.fourth_corner.y));
        }
        EntityType::Face3D(f) => {
            for c in [f.first_corner, f.second_corner, f.third_corner, f.fourth_corner] {
                v.push((c.x, c.y));
            }
        }
        EntityType::Text(t) => {
            v.push((t.insertion_point.x, t.insertion_point.y));
            if let Some(ap) = t.alignment_point {
                v.push((ap.x, ap.y));
            }
        }
        EntityType::MText(m) => {
            v.push((m.insertion_point.x, m.insertion_point.y));
        }
        EntityType::AttributeEntity(a) => {
            v.push((a.insertion_point.x, a.insertion_point.y));
        }
        EntityType::AttributeDefinition(a) => {
            v.push((a.insertion_point.x, a.insertion_point.y));
        }
        EntityType::Ellipse(el) => {
            let maj = (el.major_axis.x * el.major_axis.x + el.major_axis.y * el.major_axis.y).sqrt();
            v.push((el.center.x - maj, el.center.y - maj));
            v.push((el.center.x + maj, el.center.y + maj));
        }
        EntityType::Spline(sp) => {
            for cp in &sp.control_points {
                v.push((cp.x, cp.y));
            }
            for fp in &sp.fit_points {
                v.push((fp.x, fp.y));
            }
        }
        _ => {
            let bb = e.as_entity().bounding_box();
            v.push((bb.min.x, bb.min.y));
            v.push((bb.max.x, bb.max.y));
        }
    }
    v
}

/// 单实体在给定变换下的世界包围盒（INSERT 递归展开）
pub fn entity_world_bbox(e: &EntityType, xf: &T2) -> BBox {
    // 需要 doc 才能递归 INSERT；此处对无 doc 场景仅处理叶子几何。
    let mut bb = BBox::empty();
    for p in entity_local_points(e) {
        bb.add(xf.apply(p));
    }
    bb
}

/// 递归计算块定义在给定变换下的包围盒（返回 bbox, 叶子实体计数）
pub fn block_bbox(doc: &CadDocument, block_name: &str, xf: &T2, depth: usize) -> (BBox, usize) {
    let mut bb = BBox::empty();
    let mut count = 0usize;
    if depth > 8 {
        return (bb, count);
    }
    for e in doc.entities_in_block(block_name) {
        match e {
            EntityType::Insert(ins) => {
                let base = doc
                    .block_records
                    .get(&ins.block_name)
                    .map(|b| (b.base_point.x, b.base_point.y))
                    .unwrap_or((0.0, 0.0));
                let child = xf.mul(&insert_transform(ins, base));
                let (cbb, ccount) = block_bbox(doc, &ins.block_name, &child, depth + 1);
                bb.merge(&cbb);
                count += ccount;
            }
            _ => {
                count += 1;
                for p in entity_local_points(e) {
                    bb.add(xf.apply(p));
                }
            }
        }
    }
    (bb, count)
}

/// INSERT 的世界包围盒
pub fn insert_world_bbox(doc: &CadDocument, ins: &Insert) -> BBox {
    let base = doc
        .block_records
        .get(&ins.block_name)
        .map(|b| (b.base_point.x, b.base_point.y))
        .unwrap_or((0.0, 0.0));
    let xf = insert_transform(ins, base);
    let (bb, _) = block_bbox(doc, &ins.block_name, &xf, 0);
    bb
}
