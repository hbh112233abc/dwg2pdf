//! 图纸目录表格提取。
//!
//! 目录表是「表头行 + 数据行」的网格。本模块：
//!   ① 以归一化后等于「序号」的文本定位表头行，并要求表头同时含
//!      「图纸名称」与「图号/图幅」（否则视为非目录表，如设备表/图例表）；
//!   ② 表头行内文本按 x 升序即各列；
//!   ③ 表头下方文本按 y 聚类为数据行，取顶部连续的一段（遇大间距即停）；
//!   ④ 列边界由数据自身的 x 聚类得出（表头居中、数据左对齐，二者 x 不等）。

use crate::flatten::{texts_in, Prim, TextItem};
use crate::title::norm;

#[derive(Clone, Debug, Default)]
pub struct Toc {
    /// 列名（已归一化）
    pub headers: Vec<String>,
    /// 数据行，每行长度 == headers.len()
    pub rows: Vec<Vec<String>>,
}

impl Toc {
    pub fn col(&self, name: &str) -> Option<usize> {
        let n = norm(name);
        self.headers.iter().position(|h| *h == n)
    }
}

/// 把 x 升序排列的浮点数按间隔 > gap 切分为若干簇
fn cluster_1d(mut xs: Vec<f64>, gap: f64) -> Vec<(f64, f64)> {
    if xs.is_empty() {
        return Vec::new();
    }
    xs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mut out: Vec<(f64, f64)> = Vec::new();
    let mut lo = xs[0];
    let mut hi = xs[0];
    for &x in &xs[1..] {
        if x - hi > gap {
            out.push((lo, hi));
            lo = x;
        }
        hi = x;
    }
    out.push((lo, hi));
    out
}

/// 从给定包围盒（通常是目录页图框）中提取目录表
pub fn extract(prims: &[Prim], bbox: (f64, f64, f64, f64)) -> Option<Toc> {
    let items = texts_in(prims, bbox);

    // ① 表头种子
    let seed = items.iter().find(|t| norm(&t.s) == "序号")?;
    let h = seed.h.max(1.0);
    let row_tol = h * 0.9;

    // ② 表头行列
    let mut hdr: Vec<&TextItem> = items
        .iter()
        .filter(|t| (t.y - seed.y).abs() <= row_tol)
        .collect();
    hdr.sort_by(|a, b| a.x.partial_cmp(&b.x).unwrap_or(std::cmp::Ordering::Equal));
    let headers: Vec<(String, f64)> = hdr.iter().map(|t| (norm(&t.s), t.x)).collect();
    if headers.len() < 3 || headers.len() > 8 {
        return None;
    }
    // 必须是目录表：含「图纸名称」且含「图号」或「图幅」
    let names: Vec<&str> = headers.iter().map(|(n, _)| n.as_str()).collect();
    if !names.contains(&"图纸名称") {
        return None;
    }
    if !names.contains(&"图号") && !names.contains(&"图幅") {
        return None;
    }

    // ③ 数据行（表头下方），按 y 降序、取顶部连续段
    let mut data: Vec<&TextItem> = items.iter().filter(|t| t.y < seed.y - row_tol).collect();
    data.sort_by(|a, b| b.y.partial_cmp(&a.y).unwrap_or(std::cmp::Ordering::Equal));

    let mut rowgroups: Vec<Vec<&TextItem>> = Vec::new();
    let mut cur: Vec<&TextItem> = Vec::new();
    let mut cy = 0.0f64;
    for t in data {
        if cur.is_empty() {
            cy = t.y;
            cur.push(t);
            continue;
        }
        if (t.y - cy).abs() <= row_tol {
            cur.push(t);
        } else {
            // 与上一行间距过大 → 目录表结束
            if (cy - t.y).abs() > h * 6.0 {
                break;
            }
            rowgroups.push(std::mem::take(&mut cur));
            cur = vec![t];
            cy = t.y;
        }
    }
    if !cur.is_empty() {
        rowgroups.push(cur);
    }
    if rowgroups.is_empty() {
        return None;
    }

    // ④ 列边界：由数据 x 聚类（间隔 > 2 倍字高分列；表头居中、数据左对齐）
    let mut xs: Vec<f64> = Vec::new();
    for g in &rowgroups {
        for t in g {
            xs.push(t.x);
        }
    }
    let clusters = cluster_1d(xs, (h * 2.0).max(500.0));

    // 数据簇 → 表头列：按 x 递增贪心就近，强制列序严格递增
    let mut bound: Vec<usize> = Vec::new();
    let mut last: Option<usize> = None;
    for (lo, hi) in &clusters {
        let c = (lo + hi) / 2.0;
        let start = last.map_or(0, |l| l + 1);
        let mut bi = start;
        let mut bd = f64::INFINITY;
        for (i, (_, hx)) in headers.iter().enumerate().skip(start) {
            let d = (c - hx).abs();
            if d < bd {
                bd = d;
                bi = i;
            }
        }
        bound.push(bi);
        last = Some(bi);
    }

    // 每个数据 x → 其所属数据簇 → 表头列
    let col_of = |x: f64| -> usize {
        for (ci, (lo, hi)) in clusters.iter().enumerate() {
            if x >= lo - 1.0 && x <= hi + 1.0 {
                return bound[ci];
            }
        }
        // 落在簇间空隙：取最近簇
        let mut bi = 0;
        let mut bd = f64::INFINITY;
        for (ci, (lo, hi)) in clusters.iter().enumerate() {
            let c = (lo + hi) / 2.0;
            let d = (x - c).abs();
            if d < bd {
                bd = d;
                bi = ci;
            }
        }
        bound[bi]
    };

    let mut rows: Vec<Vec<String>> = Vec::new();
    for g in &rowgroups {
        let mut out = vec![String::new(); headers.len()];
        for t in g {
            let ci = col_of(t.x);
            let s = t.s.trim();
            if s.is_empty() {
                continue;
            }
            if out[ci].is_empty() || s.chars().count() > out[ci].chars().count() {
                out[ci] = s.to_string();
            }
        }
        if out.iter().filter(|c| !c.is_empty()).count() >= 2 {
            rows.push(out);
        }
    }

    Some(Toc {
        headers: headers.into_iter().map(|(h, _)| h).collect(),
        rows,
    })
}
