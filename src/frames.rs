//! 图框识别：从模型空间找出图框矩形，并判定图幅与比例。

use acadrust::entities::EntityType;
use acadrust::CadDocument;

use crate::geom::BBox;

/// 调试日志宏：设置环境变量 `DWG2PDF_DEBUG=1` 时输出到 stderr，便于排查
/// 图框/比例推断不准的问题；默认静默，不干扰正常输出。
macro_rules! dlog {
    ($($arg:tt)*) => {
        if std::env::var_os("DWG2PDF_DEBUG").map_or(false, |v| v != "0") {
            eprintln!($($arg)*);
        }
    };
}

/// 标准基本幅面 (短边mm, 长边mm, 名称)。
///
/// 加长幅面只改变长边，短边保持不变，因此用短边即可锁定基准幅面与比例尺。
const BASE_SHEETS: &[(f64, f64, &str)] = &[
    (841.0, 1189.0, "A0"),
    (594.0, 841.0, "A1"),
    (420.0, 594.0, "A2"),
    (297.0, 420.0, "A3"),
    (210.0, 297.0, "A4"),
];

/// 一个图框
#[derive(Clone, Debug)]
pub struct Frame {
    /// 外框包围盒(图纸单位)
    pub bbox: BBox,
    /// 图框所在图层
    pub layer: String,
    /// 匹配到的标准图幅名(如 A1)，未匹配则 "?" 
    pub sheet: String,
    /// 图纸单位 -> mm 的比例(units per mm)
    pub units_per_mm: f64,
    /// 页面尺寸(mm)
    pub page_w_mm: f64,
    pub page_h_mm: f64,
}

impl Frame {
    pub fn label(&self) -> String {
        format!(
            "{} {:.0}x{:.0}mm @({:.0},{:.0}) 图层={}",
            self.sheet, self.page_w_mm, self.page_h_mm, self.bbox.min_x, self.bbox.min_y, self.layer
        )
    }
}

/// 收集实体自身的矩形候选(闭合 LWPOLYLINE/2D 多段线)
fn candidate_rects(doc: &CadDocument) -> Vec<(BBox, String)> {
    let mut out = Vec::new();
    for e in doc.model_space_entities() {
        let (bb, closed) = match e {
            EntityType::LwPolyline(lw) => {
                let mut bb = BBox::empty();
                for v in &lw.vertices {
                    bb.add((v.location.x, v.location.y));
                }
                (bb, lw.is_closed && lw.vertices.len() >= 4 && lw.vertices.len() <= 6)
            }
            EntityType::Polyline2D(pl) => {
                let mut bb = BBox::empty();
                for v in &pl.vertices {
                    bb.add((v.location.x, v.location.y));
                }
                (bb, pl.flags.is_closed())
            }
            _ => continue,
        };
        if closed && bb.valid() {
            out.push((bb, e.common().layer.clone()));
        }
    }
    out
}

/// 匹配图幅，返回(图幅名, units_per_mm)。
///
/// 判据（依次）：
///   ① 用**短边**锁定基准幅面：`upp = 短边 / 标准短边` 必须接近 10 的整数幂
///      （A 系列各幅面长宽比相同，只有短边能区分 A0/A1/…）；
///   ② 长边不得短于该幅面标准长边（缩放后）；
///   ③ 若长边大于标准长边则为**加长**：其加长量须接近标准长边的 1/8 整数倍，
///      并记为 `A1+1/4` 形式。
fn match_sheet(w: f64, h: f64) -> Option<(String, f64)> {
    let (bw, bh) = if w >= h { (w, h) } else { (h, w) };
    dlog!("[图幅] 判定 {:.0}x{:.0} 短边={:.0}", bw, bh, bh);
    let mut best: Option<(&str, f64, f64, f64)> = None; // (名称, upp, score, 标准长边)
    for (sh, sl, name) in BASE_SHEETS {
        let upp = bh / sh; // 由短边定比例尺
        if !(0.05..5000.0).contains(&upp) {
            dlog!("[图幅] {} 短边比例尺 {:.4} 超出 [0.05,5000] → 跳过", name, upp);
            continue;
        }
        let l = upp.log10();
        let score = (l - l.round()).abs(); // 0 = 正好 10^k
        if score >= 0.15 {
            dlog!(
                "[图幅] {} 短边比例尺 {:.4}(10^{:.3}) 偏离 10 的整数幂 score={:.3}≥0.15 → 跳过",
                name, upp, l, score
            );
            continue;
        }
        let exp_long = sl * upp; // 该幅面标准长边(图纸单位)
        if bw + 1.0 < exp_long * 0.97 {
            dlog!("[图幅] {} 长边 {:.0} 短于标准长边 {:.0} → 跳过", name, bw, exp_long);
            continue; // 长边比标准还短 → 不是该幅面
        }
        // 加长时，加长量须接近标准长边 1/8 的整数倍
        if bw > exp_long * 1.005 {
            let n8 = (bw - exp_long) / (exp_long / 8.0);
            if (n8 - n8.round()).abs() > 0.12 {
                dlog!("[图幅] {} 加长量 {:.2}/8 非整倍(需 ±0.12) → 跳过", name, n8);
                continue;
            }
            dlog!(
                "[图幅] {} 候选(加长 {:.2}/8) upp={:.4} score={:.4} 标准长边={:.0}",
                name, n8, upp, score, exp_long
            );
        } else {
            dlog!(
                "[图幅] {} 候选(标准) upp={:.4} score={:.4} 标准长边={:.0}",
                name, upp, score, exp_long
            );
        }
        if best.map_or(true, |b| score < b.2) {
            best = Some((name, upp, score, exp_long));
        }
    }

    let Some((name, upp, _, exp_long)) = best else {
        dlog!("[图幅] 无任何标准/加长幅面匹配 → 需要兜底推断");
        return None;
    };
    if bw <= exp_long * 1.005 {
        dlog!("[图幅] 采用 {} (标准) upp={:.4}", name, upp);
        Some((name.to_string(), upp))
    } else {
        // 加长：以标准长边的 1/8 为步长，约简为 A1+1/4 这类记法
        let n8 = ((bw - exp_long) / (exp_long / 8.0)).round() as i64;
        let (a, b) = reduce(n8.max(1), 8);
        let label = if b == 1 {
            format!("{}+{}", name, a)
        } else {
            format!("{}+{}/{}", name, a, b)
        };
        dlog!("[图幅] 采用 {} (加长 {}/8) upp={:.4}", label, n8, upp);
        Some((label, upp))
    }
}

/// 约分
fn reduce(mut n: i64, mut d: i64) -> (i64, i64) {
    fn gcd(a: i64, b: i64) -> i64 {
        if b == 0 { a.abs() } else { gcd(b, a % b) }
    }
    let g = gcd(n, d).max(1);
    n /= g;
    d /= g;
    (n, d)
}

/// 图层名是否"像图框层"(用于兜底：无法匹配标准图幅时仍保留)
fn is_frame_layer(lay: &str) -> bool {
    let l = lay.to_ascii_lowercase();
    lay.contains("图框") || lay.contains("图幅") || lay.contains("标题栏") || lay.contains("图签")
        || l.contains("frame") || l.contains("border") || l.contains("sheet") || l.contains("title")
}

/// 兜底比例尺推断（用于无法匹配标准/加长图幅的自定义图框）。
///
/// 工程图绘图单位几乎都是「10 的整数幂 单位/mm」(1、0.1、0.01…)。在所有
/// 10^k 中，选出使**短边**落在常见纸张尺寸区间、且最接近标准短边的那个。
fn infer_upp(bh: f64) -> f64 {
    const STD_SHORT: &[f64] = &[210.0, 297.0, 420.0, 594.0, 841.0];
    dlog!("[比例] 兜底推断 短边={:.0} 单位", bh);
    let mut best = (100.0f64, f64::INFINITY);
    for k in -3..=4 {
        let upp = 10f64.powi(k);
        let short_mm = bh / upp;
        if !(180.0..=1500.0).contains(&short_mm) {
            dlog!("[比例] upp=10^{} 短边={:.1}mm 不在 [180,1500] → 跳过", k, short_mm);
            continue;
        }
        let ls = short_mm.log10();
        let score = STD_SHORT.iter().map(|s| (ls - s.log10()).abs()).fold(f64::INFINITY, f64::min);
        dlog!("[比例] upp=10^{} 短边={:.1}mm 距标准短边 score={:.4}", k, short_mm, score);
        if score < best.1 {
            best = (upp, score);
        }
    }
    dlog!(
        "[比例] 兜底推断结果 upp={} → 短边 {:.1}mm",
        best.0,
        bh / best.0
    );
    best.0
}

/// 检测图框：先取所有闭合矩形候选，过滤尺寸，再去掉被更大矩形包含的内框。
pub fn detect_frames(doc: &CadDocument) -> Vec<Frame> {
    let mut cands = candidate_rects(doc);
    dlog!("[图框] 模型空间闭合矩形候选数={}", cands.len());
    // 过滤：尺寸足够大；长宽比放宽以容纳加长图框(max ≈ A0 加长 4.06:1)
    cands.retain(|(bb, lay)| {
        let (w, h) = (bb.w(), bb.h());
        if w < 5000.0 || h < 5000.0 {
            return false; // 尺寸过小：非关键分支，不单独记录
        }
        let (bw, bh) = if w >= h { (w, h) } else { (h, w) };
        let ratio = bw / bh;
        if !(1.0..4.2).contains(&ratio) {
            dlog!("[图框] 剔除 长宽比 {:.3} 超出 [1.0,4.2] {:.0}x{:.0} 图层={}", ratio, w, h, lay);
            return false;
        }
        dlog!("[图框] 候选 {:.0}x{:.0} 长宽比={:.3} 图层={}", w, h, ratio, lay);
        true
    });
    // 按面积降序，剔除外层已包含的内框
    cands.sort_by(|a, b| {
        (b.0.w() * b.0.h()).partial_cmp(&(a.0.w() * a.0.h())).unwrap_or(std::cmp::Ordering::Equal)
    });
    let mut kept: Vec<(BBox, String)> = Vec::new();
    for (bb, lay) in cands {
        let contained = kept.iter().any(|(k, _)| {
            k.contains((bb.min_x, bb.min_y), 1.0)
                && k.contains((bb.max_x, bb.max_y), 1.0)
                && (k.w() - bb.w()).abs() > 1.0
        });
        if !contained {
            kept.push((bb, lay));
        } else {
            dlog!("[图框] 剔除 被外框包含 {:.0}x{:.0} 图层={}", bb.w(), bb.h(), lay);
        }
    }

    // 判定图幅
    let mut frames = Vec::new();
    for (bb, lay) in kept {
        let (sheet, upp) = match match_sheet(bb.w(), bb.h()) {
            Some((s, u)) => (s, u),
            None => {
                // 不匹配标准/加长图幅：仅当位于"像图框层"时保留(自定义图幅)，否则剔除
                if is_frame_layer(&lay) {
                    let u = infer_upp(bb.h().min(bb.w()));
                    dlog!("[图框] {:.0}x{:.0} 图层={} 像图框层 → 兜底推断 upp={}", bb.w(), bb.h(), lay, u);
                    ("?".to_string(), u)
                } else {
                    dlog!("[图框] 剔除 未匹配标准幅面且图层不像图框层 {:.0}x{:.0} 图层={}", bb.w(), bb.h(), lay);
                    continue;
                }
            }
        };
        let (pw, ph) = (bb.w() / upp, bb.h() / upp);
        dlog!("[图框] 采用 {:.0}x{:.0} → {} {:.1}x{:.1}mm upp={:.4}", bb.w(), bb.h(), sheet, pw, ph, upp);
        frames.push(Frame {
            bbox: bb,
            layer: lay,
            sheet,
            units_per_mm: upp,
            page_w_mm: pw,
            page_h_mm: ph,
        });
    }
    frames.sort_by(|a, b| {
        b.bbox.min_y.partial_cmp(&a.bbox.min_y).unwrap_or(std::cmp::Ordering::Equal)
            .then(a.bbox.min_x.partial_cmp(&b.bbox.min_x).unwrap_or(std::cmp::Ordering::Equal))
    });
    frames
}

#[cfg(test)]
mod tests {
    use super::match_sheet;

    /// 图纸单位下的 mm 尺寸（1 单位 = 0.01mm）
    fn u(w_mm: f64, h_mm: f64) -> (f64, f64) {
        (w_mm * 100.0, h_mm * 100.0)
    }

    fn sheet(w_mm: f64, h_mm: f64) -> Option<(String, f64)> {
        let (w, h) = u(w_mm, h_mm);
        match_sheet(w, h)
    }

    #[test]
    fn standard_sheets() {
        for (w, h, name) in [
            (1189.0, 841.0, "A0"),
            (841.0, 594.0, "A1"),
            (594.0, 420.0, "A2"),
            (420.0, 297.0, "A3"),
            (297.0, 210.0, "A4"),
        ] {
            let (got, upp) = sheet(w, h).unwrap_or_else(|| panic!("{} 未识别", name));
            assert_eq!(got, name);
            assert!((upp - 100.0).abs() < 1e-6, "{} 比例尺 {}", name, upp);
        }
    }

    #[test]
    fn portrait_sheets() {
        // 竖向摆放(宽<高)应得到同样的幅面判定
        let (got, _) = sheet(594.0, 841.0).unwrap();
        assert_eq!(got, "A1");
    }

    #[test]
    fn extended_sheets() {
        // 加长量 = 标准长边的 1/8 的整数倍
        // A1 长边 841 → 加长 1/4 = 841/4 = 210.25
        let (got, _) = sheet(841.0 + 841.0 / 4.0, 594.0).unwrap();
        assert_eq!(got, "A1+1/4");
        // A1 加长 1/2
        let (got, _) = sheet(841.0 + 841.0 / 2.0, 594.0).unwrap();
        assert_eq!(got, "A1+1/2");
        // A0 加长 1/4
        let (got, _) = sheet(1189.0 + 1189.0 / 4.0, 841.0).unwrap();
        assert_eq!(got, "A0+1/4");
        // A2 加长 1/8
        let (got, _) = sheet(594.0 + 594.0 / 8.0, 420.0).unwrap();
        assert_eq!(got, "A2+1/8");
    }

    #[test]
    fn rejects_non_standard() {
        // 长边超出标准但加长量非 1/8 整数倍 → 拒绝
        assert!(sheet(1450.0, 594.0).is_none());
        // 长边短于标准长边(短边像 A2，长边只有 500) → 拒绝
        assert!(sheet(500.0, 420.0).is_none());
    }

    // ---- 兜底比例尺推断 ----
    use super::infer_upp;

    #[test]
    fn infer_by_unit_convention() {
        // 同一张 A1(短边 594mm)在不同绘图单位下的推断
        assert_eq!(infer_upp(594.0), 1.0); // 1 单位 = 1mm
        assert_eq!(infer_upp(5940.0), 10.0); // 1 单位 = 0.1mm
        assert_eq!(infer_upp(59400.0), 100.0); // 1 单位 = 0.01mm
        assert_eq!(infer_upp(5.94), 0.01); // 1 单位 = 0.001mm? → 0.01 更贴近
    }

    #[test]
    fn infer_custom_size() {
        // 自定义 600×424mm(非标准)
        assert_eq!(infer_upp(600.0), 1.0);
        assert_eq!(infer_upp(60000.0), 100.0);
        // 大图 2000×1500mm
        assert_eq!(infer_upp(1500.0), 1.0);
        assert_eq!(infer_upp(150000.0), 100.0);
    }
}
