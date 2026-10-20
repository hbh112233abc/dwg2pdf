//! 按图框拆分 DWG/DXF。
//!
//! 思路：克隆整份文档 → 移除与目标图框不相交的模型空间实体 → 写出单个文件。
//! 图层、块定义、文字/标注样式、布局全部保留，出图效果与原文一致；不做坐标平移。
//! 块定义内部实体不裁剪（块定义是共享的，裁剪会破坏其他 INSERT），仅按 INSERT
//! 的世界包围盒决定是否保留该引用。

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use acadrust::entities::EntityType;
use acadrust::{CadDocument, DwgWriter, DxfWriter};

use crate::geom::{self, BBox};

/// 图框外扩比例（与 PDF 导出一致）
pub const PAD_RATIO: f64 = 0.02;

/// 输出格式
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutFormat {
    Dwg,
    Dxf,
}

impl OutFormat {
    /// 解析用户参数；空串按默认 DWG 处理
    pub fn from_arg(s: &str) -> Result<Self, String> {
        match s.to_ascii_lowercase().as_str() {
            "" | "dwg" => Ok(OutFormat::Dwg),
            "dxf" => Ok(OutFormat::Dxf),
            other => Err(format!("不支持的输出格式: {}（可选 dwg / dxf）", other)),
        }
    }

    pub fn ext(&self) -> &'static str {
        match self {
            OutFormat::Dwg => "dwg",
            OutFormat::Dxf => "dxf",
        }
    }
}

/// 模型空间单个实体的世界包围盒；无法度量时返回 None
fn entity_world_bbox(doc: &CadDocument, e: &EntityType) -> Option<BBox> {
    let bb = match e {
        // Insert::bounding_box() 是桩实现，必须递归展开块定义
        EntityType::Insert(ins) => geom::insert_world_bbox(doc, ins),
        _ => geom::entity_world_bbox(e, &geom::T2::identity()),
    };
    if bb.valid() { Some(bb) } else { None }
}

/// 实体是否归属该图框（pad 用于救回略微越界的图元）。
///
/// 判定分两步：
/// ① 包围盒与图框有**正的**重叠深度即归属——相邻图框边对边相切时重叠深度为 0，
///    不会被误收（工程图常把多张图框严丝合缝地拼在一张图上）；
///    零尺寸包围盒（TEXT/POINT 等只有一个锚点）按「锚点落在框内」处理。
/// ② 否则看包围盒中心是否落在图框外扩 pad 的范围内，用于保留引出线、索引符号等
///    略微越出图框边界的图元（pad 与 PDF 导出保持一致）。
pub fn belongs_to(a: &BBox, frame: &BBox, pad: f64) -> bool {
    if !a.valid() || !frame.valid() {
        return false;
    }
    if axis_overlap(a.min_x, a.max_x, frame.min_x, frame.max_x)
        && axis_overlap(a.min_y, a.max_y, frame.min_y, frame.max_y)
    {
        return true;
    }
    let center = ((a.min_x + a.max_x) / 2.0, (a.min_y + a.max_y) / 2.0);
    frame.contains(center, pad)
}

/// 一维：区间 a 与 b 是否有**正**的重叠；退化为单点时按「点落入 b」判定
fn axis_overlap(a0: f64, a1: f64, b0: f64, b1: f64) -> bool {
    if a1 - a0 <= 0.0 {
        a0 >= b0 && a0 <= b1
    } else {
        a1.min(b1) - a0.max(b0) > 0.0
    }
}

/// 克隆文档并移除不属于该图框的模型空间实体，返回 (新文档, 删除实体数)。
///
/// 包围盒不可度量的实体一律保留，避免因无法度量而丢内容。
pub fn split_document(doc: &CadDocument, frame: &BBox, pad: f64) -> (CadDocument, usize) {
    let mut out = doc.clone();
    let mut drop_handles = Vec::new();
    for e in doc.model_space_entities() {
        let Some(bb) = entity_world_bbox(doc, e) else {
            continue; // 保守保留
        };
        if !belongs_to(&bb, frame, pad) {
            drop_handles.push(e.common().handle);
        }
    }
    let mut removed = 0usize;
    for h in drop_handles {
        if out.remove_entity(h).is_some() {
            removed += 1;
        }
    }
    (out, removed)
}

/// Windows 文件名非法字符
const ILLEGAL_CHARS: &[char] = &['\\', '/', ':', '*', '?', '"', '<', '>', '|'];

/// Windows 保留设备名（不区分大小写）
const RESERVED_NAMES: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// 清洗文件名片段：去掉非法字符与控制字符、首尾空白与结尾点；规避保留设备名
pub fn sanitize_segment(s: &str) -> String {
    let cleaned: String = s
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .filter(|c| !ILLEGAL_CHARS.contains(c))
        .collect();
    // 折叠换行/多空格造成的空白块
    let cleaned = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    let trimmed = cleaned.trim_matches(|c| c == ' ' || c == '.');
    if trimmed.is_empty() {
        return String::new();
    }
    if RESERVED_NAMES.iter().any(|r| r.eq_ignore_ascii_case(trimmed)) {
        return format!("_{}", trimmed);
    }
    trimmed.to_string()
}

/// 组装文件名主体：`序号_图号_图名`；图号/图名缺失时省略对应段，
/// 两者都缺失则回退为图幅名。
pub fn file_stem(seq: usize, drawing_no: &str, title: &str, sheet: &str) -> String {
    let mut parts = vec![format!("{:02}", seq)];
    let no = sanitize_segment(drawing_no);
    let ti = sanitize_segment(title);
    if !no.is_empty() {
        parts.push(no);
    }
    if !ti.is_empty() {
        parts.push(ti);
    }
    if parts.len() == 1 {
        let s = sanitize_segment(sheet);
        parts.push(if s.is_empty() { "未命名".to_string() } else { s });
    }
    parts.join("_")
}

/// 在 `dir` 下生成不重复的文件路径（重名追加 `_2`、`_3`…），并登记到 `used`
pub fn unique_path(dir: &Path, stem: &str, ext: &str, used: &mut HashSet<String>) -> PathBuf {
    let mut name = format!("{}.{}", stem, ext);
    let mut n = 2usize;
    while !used.insert(name.clone()) {
        name = format!("{}_{}.{}", stem, n, ext);
        n += 1;
    }
    dir.join(name)
}

/// 写出单个文档（先写 `.part` 再原子改名），返回文件字节数
pub fn write_output(doc: &CadDocument, path: &Path, fmt: OutFormat) -> Result<usize, String> {
    let tmp = tmp_path(path);
    let res = match fmt {
        OutFormat::Dwg => DwgWriter::write_to_file(&tmp, doc).map_err(|e| e.to_string()),
        OutFormat::Dxf => DxfWriter::new(doc).write_to_file(&tmp).map_err(|e| e.to_string()),
    };
    if let Err(e) = res {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    let bytes = std::fs::metadata(&tmp).map(|m| m.len() as usize).unwrap_or(0);
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("重命名失败 {}: {}", path.display(), e)
    })?;
    Ok(bytes)
}

/// 临时文件路径：`xxx.dwg` → `xxx.dwg.part`
fn tmp_path(path: &Path) -> PathBuf {
    let mut s = path.as_os_str().to_os_string();
    s.push(".part");
    PathBuf::from(s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use acadrust::entities::{Line, LwPolyline};
    use acadrust::tables::Layer;
    use acadrust::Vector2;

    /// A4 @ 1 unit = 0.01mm → 21000 x 29700 图纸单位
    const A4_W: f64 = 21000.0;
    const A4_H: f64 = 29700.0;

    fn a4_frame(x0: f64, y0: f64) -> LwPolyline {
        let mut lw = LwPolyline::from_points(vec![
            Vector2::new(x0, y0),
            Vector2::new(x0 + A4_W, y0),
            Vector2::new(x0 + A4_W, y0 + A4_H),
            Vector2::new(x0, y0 + A4_H),
        ]);
        lw.close();
        lw
    }

    /// 两个相邻 A4 图框 + 各自框内一条线 + 远处一条线，共 5 个模型空间实体
    fn doc_with_two_frames() -> CadDocument {
        let mut doc = CadDocument::new();
        doc.layers.add(Layer::new("J-图框线")).unwrap();

        for (i, x0) in [0.0f64, 40000.0].into_iter().enumerate() {
            let mut f = a4_frame(x0, 0.0);
            f.common.layer = "J-图框线".into();
            doc.add_entity(EntityType::LwPolyline(f)).unwrap();

            let mut l = Line::from_coords(x0 + 1000.0, 1000.0, 0.0, x0 + 5000.0, 5000.0, 0.0);
            l.common.layer = format!("L{}", i + 1);
            doc.add_entity(EntityType::Line(l)).unwrap();
        }
        // 远离两个图框
        let far = Line::from_coords(200000.0, 0.0, 0.0, 201000.0, 1000.0, 0.0);
        doc.add_entity(EntityType::Line(far)).unwrap();
        doc
    }

    #[test]
    fn split_document_prunes_outside_entities() {
        let doc = doc_with_two_frames();
        let frames = crate::frames::detect_frames(&doc);
        assert_eq!(frames.len(), 2, "应识别出两个 A4 图框");
        assert_eq!(doc.model_space_entities().count(), 5);

        let layers_before = doc.layers.len();
        // 第一个图框：保留自身 + 框内线，删除另一个图框、其内线、远处线
        let (out, removed) = split_document(&doc, &frames[0].bbox, 0.0);
        assert_eq!(removed, 3);
        assert_eq!(out.model_space_entities().count(), 2);
        assert_eq!(out.layers.len(), layers_before, "图层表不应减少");
        // 剩余实体都归属第一个图框
        for e in out.model_space_entities() {
            let bb = entity_world_bbox(&out, e).unwrap();
            assert!(belongs_to(&bb, &frames[0].bbox, 0.0));
        }
        // 原文档未被修改
        assert_eq!(doc.model_space_entities().count(), 5);
    }

    /// 工程图常把多张图框严丝合缝地拼在一起，相邻图框不能被互相误收
    #[test]
    fn adjacent_frames_do_not_bleed() {
        let mut doc = CadDocument::new();
        for x0 in [0.0f64, A4_W] {
            doc.add_entity(EntityType::LwPolyline(a4_frame(x0, 0.0))).unwrap();
        }
        // 刚出左边界 300 单位（小于 pad=594）的点：应被救回
        let outside = Line::from_coords(-300.0, 1000.0, 0.0, -300.0, 2000.0, 0.0);
        doc.add_entity(EntityType::Line(outside)).unwrap();

        let frames = crate::frames::detect_frames(&doc);
        assert_eq!(frames.len(), 2);
        let pad = A4_W.max(A4_H) * PAD_RATIO;
        let (out, removed) = split_document(&doc, &frames[0].bbox, pad);
        assert_eq!(removed, 1, "只应删除右侧相邻图框");
        assert_eq!(out.model_space_entities().count(), 2);
    }

    #[test]
    fn write_output_roundtrip() {
        let doc = doc_with_two_frames();
        let frames = crate::frames::detect_frames(&doc);
        let layers_before = doc.layers.len();
        let (out, _) = split_document(&doc, &frames[0].bbox, 0.0);

        let dir = std::env::temp_dir().join(format!("dwg2pdf_split_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();

        for fmt in [OutFormat::Dwg, OutFormat::Dxf] {
            let path = dir.join(format!("one.{}", fmt.ext()));
            let bytes = write_output(&out, &path, fmt).unwrap();
            assert!(bytes > 0, "{:?} 输出不应为空", fmt);

            let back = match fmt {
                OutFormat::Dwg => crate::api::load(path.to_str().unwrap()).unwrap(),
                OutFormat::Dxf => acadrust::DxfReader::from_file(&path).unwrap().read().unwrap(),
            };
            assert_eq!(
                crate::frames::detect_frames(&back).len(),
                1,
                "{:?} 回读后应只剩原图框",
                fmt
            );
            assert_eq!(back.model_space_entities().count(), 2);
            assert_eq!(back.layers.len(), layers_before, "{:?} 图层不应丢失", fmt);
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sanitize_segment_rules() {
        assert_eq!(sanitize_segment("结施-01"), "结施-01");
        assert_eq!(sanitize_segment("A/B:C*?\"<>|"), "ABC");
        assert_eq!(sanitize_segment("  图名.  "), "图名");
        assert_eq!(sanitize_segment("a\n\tb"), "a b");
        assert_eq!(sanitize_segment("   "), "");
        assert_eq!(sanitize_segment("con"), "_con");
    }

    #[test]
    fn file_stem_composition() {
        assert_eq!(file_stem(1, "结施-01", "一层平面图", "A1"), "01_结施-01_一层平面图");
        assert_eq!(file_stem(7, "结施-01", "", "A1"), "07_结施-01");
        assert_eq!(file_stem(2, "", "一层平面图", "A4"), "02_一层平面图");
        assert_eq!(file_stem(3, "", "", "A4"), "03_A4");
        assert_eq!(file_stem(4, "", "", ""), "04_未命名");
    }

    #[test]
    fn unique_path_dedups() {
        let dir = Path::new("/tmp/x");
        let mut used = HashSet::new();
        assert_eq!(unique_path(dir, "01_a", "dwg", &mut used), dir.join("01_a.dwg"));
        assert_eq!(unique_path(dir, "01_a", "dwg", &mut used), dir.join("01_a_2.dwg"));
        assert_eq!(unique_path(dir, "01_a", "dwg", &mut used), dir.join("01_a_3.dwg"));
        // 不同扩展名互不影响
        assert_eq!(unique_path(dir, "01_a", "dxf", &mut used), dir.join("01_a.dxf"));
    }

    #[test]
    fn out_format_from_arg() {
        assert_eq!(OutFormat::from_arg("").unwrap(), OutFormat::Dwg);
        assert_eq!(OutFormat::from_arg("DWG").unwrap(), OutFormat::Dwg);
        assert_eq!(OutFormat::from_arg("dxf").unwrap(), OutFormat::Dxf);
        assert!(OutFormat::from_arg("pdf").is_err());
        assert_eq!(OutFormat::Dxf.ext(), "dxf");
    }
}
