//! 面向上层（命令行 / Python）的高层 API。
//!
//! 把「解析 → 图框识别 → 图签/目录提取 → 逐图框 PDF 导出」串成可直接调用的
//! 函数，返回可序列化的数据结构，避免上层重复实现。

use acadrust::{CadDocument, DwgReader};

use crate::{flatten, frames, pdf, split, title, toc};

/// 毫米 → PDF 点(1/72 英寸)
pub const MM_TO_PT: f64 = 72.0 / 25.4;

/// 解析 DWG 文件
pub fn load(path: &str) -> Result<CadDocument, String> {
    DwgReader::from_file(path)
        .map_err(|e| e.to_string())?
        .read()
        .map_err(|e| e.to_string())
}

/// 显式指定字体文件的环境变量（最高优先级）
pub const FONT_ENV: &str = "DWG2PDF_FONT";

/// 各平台常见字体目录
fn font_dirs() -> Vec<std::path::PathBuf> {
    #[allow(unused_mut)]
    let mut v: Vec<std::path::PathBuf> = Vec::new();

    #[cfg(target_os = "windows")]
    {
        if let Ok(sys) = std::env::var("SystemRoot") {
            v.push(std::path::PathBuf::from(sys).join("Fonts"));
        }
        v.push(std::path::PathBuf::from("C:\\Windows\\Fonts"));
    }

    #[cfg(target_os = "macos")]
    {
        v.push(std::path::PathBuf::from("/System/Library/Fonts"));
        v.push(std::path::PathBuf::from("/Library/Fonts"));
        if let Some(h) = std::env::var_os("HOME") {
            v.push(std::path::PathBuf::from(h).join("Library/Fonts"));
        }
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        v.push(std::path::PathBuf::from("/usr/share/fonts"));
        v.push(std::path::PathBuf::from("/usr/local/share/fonts"));
        if let Some(h) = std::env::var_os("HOME") {
            let home = std::path::PathBuf::from(h);
            v.push(home.join(".local/share/fonts"));
            v.push(home.join(".fonts"));
        }
    }

    v
}

/// 常见 CJK 字体文件名的关键字（越靠前优先级越高）。
///
/// 先放各平台「系统自带且为 TrueType 轮廓」的常见中文字体（Windows 黑体系列、
/// macOS 苹方等），再放开源跨平台字体，最后是 Linux 常见字体。
const CJK_HINTS: &[&str] = &[
    // Windows
    "simhei", "msyh", "simkai", "simfang", "simsun", "dengxian",
    // macOS
    "pingfang", "hiraginosansgb", "stheiti", "songti", "heiti",
    // 开源 / 跨平台
    "sourcehansans", "sourcehanserif", "notosanscjk", "notoserifcjk", "notosanssc", "notoserifsc",
    // Linux
    "wqy", "wenquanyi", "zenhei", "microhei", "droidsansfallback", "fandol",
];

/// 递归收集字体文件（限制深度，避免大目录/软链循环）
fn collect_fonts(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>, depth: usize) {
    if depth > 4 {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for ent in rd.flatten() {
        let p = ent.path();
        if p.is_dir() {
            collect_fonts(&p, out, depth + 1);
        } else if p.is_file() {
            let ext = p
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            if matches!(ext.as_str(), "ttf" | "ttc" | "otf") {
                out.push(p);
            }
        }
    }
}

/// 选择一个可用的中文字体文件。
///
/// 优先级：① 环境变量 `DWG2PDF_FONT`；② 各平台字体目录中按 CJK 关键字命中的
/// 字体；③ 任意可解析且含常用汉字的字体。均做「可解析 + 含汉字」校验，避免选中
/// 不含 CJK 的字体导致中文空白。
pub fn default_font() -> Option<String> {
    // ① 环境变量显式指定
    if let Some(v) = std::env::var_os(FONT_ENV) {
        let p = std::path::PathBuf::from(v);
        if p.is_file() {
            return Some(p.to_string_lossy().into_owned());
        }
        eprintln!(
            "警告: 环境变量 {} 指定的字体不存在: {}",
            FONT_ENV,
            p.display()
        );
    }

    // ② 扫描平台字体目录
    let mut files = Vec::new();
    for d in font_dirs() {
        collect_fonts(&d, &mut files, 0);
    }
    files.sort();
    files.dedup();

    // 快路径：只校验文件名命中 CJK 关键字的（开销小）；TrueType(.ttf) 优先
    let mut best: Option<(usize, String)> = None;
    for f in &files {
        let name = f
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        let Some(hint) = CJK_HINTS.iter().position(|h| name.contains(h)) else {
            continue;
        };
        let ext = f
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        let ttf_bonus = if ext == "ttf" { 0 } else { 1 };
        let rank = hint * 2 + ttf_bonus;
        if best.as_ref().is_some_and(|(b, _)| *b <= rank) {
            continue;
        }
        let p = f.to_string_lossy().into_owned();
        if pdf::font_has_cjk(&p) {
            best = Some((rank, p));
        }
    }
    if let Some((_, p)) = best {
        return Some(p);
    }

    // 慢路径：遍历所有字体，取第一个含汉字的
    for f in &files {
        let p = f.to_string_lossy().into_owned();
        if pdf::font_has_cjk(&p) {
            return Some(p);
        }
    }
    None
}

/// 图框信息
#[derive(Clone, Debug)]
pub struct FrameInfo {
    /// 图幅名(A1 / A0+1/4 / ? ...)
    pub sheet: String,
    /// 所在图层
    pub layer: String,
    /// 页面宽(mm)
    pub page_w_mm: f64,
    /// 页面高(mm)
    pub page_h_mm: f64,
    /// 图纸单位 / mm
    pub units_per_mm: f64,
    /// 世界包围盒 (min_x, min_y, max_x, max_y)
    pub bbox: (f64, f64, f64, f64),
}

/// 单张图纸的图签信息
#[derive(Clone, Debug, Default)]
pub struct SheetInfo {
    pub sheet: String,
    pub drawing_no: String,
    pub title: String,
    pub category: String,
    pub discipline: String,
    pub date: String,
    /// 全部「标签 = 值」
    pub fields: Vec<(String, String)>,
    /// 是否为图纸目录页
    pub is_toc_page: bool,
}

/// 图纸目录表
#[derive(Clone, Debug, Default)]
pub struct Toc {
    pub headers: Vec<String>,
    pub rows: Vec<Vec<String>>,
}

/// 导出后单页摘要
#[derive(Clone, Debug)]
pub struct PageOut {
    pub sheet: String,
    pub page_w_mm: f64,
    pub page_h_mm: f64,
    pub prims: usize,
}

/// 导出摘要
#[derive(Clone, Debug)]
pub struct ExportSummary {
    pub pages: usize,
    pub bytes: usize,
    pub details: Vec<PageOut>,
}

/// 图框识别
pub fn detect_frames(path: &str) -> Result<Vec<FrameInfo>, String> {
    let doc = load(path)?;
    Ok(frames::detect_frames(&doc)
        .into_iter()
        .map(|f| FrameInfo {
            sheet: f.sheet,
            layer: f.layer,
            page_w_mm: f.page_w_mm,
            page_h_mm: f.page_h_mm,
            units_per_mm: f.units_per_mm,
            bbox: (f.bbox.min_x, f.bbox.min_y, f.bbox.max_x, f.bbox.max_y),
        })
        .collect())
}

/// 逐图框提取图签信息（目录页标记 is_toc_page）
pub fn extract_info(path: &str) -> Result<Vec<SheetInfo>, String> {
    let doc = load(path)?;
    let fs = frames::detect_frames(&doc);
    let prims = flatten::flatten_model(&doc);
    Ok(sheet_infos(&fs, &prims))
}

/// 图签信息提取（与 `frames::detect_frames` 顺序严格一一对应）。
///
/// 独立出来供 `extract_info` 与 `split_file` 复用，避免重复解析 DWG 与重复扁平化。
fn sheet_infos(fs: &[frames::Frame], prims: &[flatten::Prim]) -> Vec<SheetInfo> {
    let mut out = Vec::with_capacity(fs.len());
    for f in fs {
        let bb = (f.bbox.min_x, f.bbox.min_y, f.bbox.max_x, f.bbox.max_y);
        if toc::extract(prims, bb).is_some() {
            out.push(SheetInfo {
                sheet: f.sheet.clone(),
                is_toc_page: true,
                ..Default::default()
            });
            continue;
        }
        let si = title::extract(prims, bb, &f.sheet);
        out.push(SheetInfo {
            sheet: si.sheet,
            drawing_no: si.drawing_no,
            title: si.title,
            category: si.category,
            discipline: si.discipline,
            date: si.date,
            fields: si.fields,
            is_toc_page: false,
        });
    }
    out
}

/// 提取图纸目录表格（在任一图框内查找）
pub fn extract_toc(path: &str) -> Result<Option<Toc>, String> {
    let doc = load(path)?;
    let fs = frames::detect_frames(&doc);
    let prims = flatten::flatten_model(&doc);
    for f in &fs {
        let bb = (f.bbox.min_x, f.bbox.min_y, f.bbox.max_x, f.bbox.max_y);
        if let Some(t) = toc::extract(&prims, bb) {
            return Ok(Some(Toc {
                headers: t.headers,
                rows: t.rows,
            }));
        }
    }
    Ok(None)
}

/// 按图框逐页导出 PDF（页尺寸 = 图幅）；`font` 为 None 时自动选择中文字体。
pub fn export_file(path: &str, out: &str, font: Option<&str>) -> Result<ExportSummary, String> {
    let doc = load(path)?;
    let fs = frames::detect_frames(&doc);
    if fs.is_empty() {
        return Err("未检测到图框，无法导出".into());
    }
    let prims = flatten::flatten_model(&doc);
    let mut pages = Vec::with_capacity(fs.len());
    for f in &fs {
        let bb = (f.bbox.min_x, f.bbox.min_y, f.bbox.max_x, f.bbox.max_y);
        let pad = f.bbox.w().max(f.bbox.h()) * 0.02;
        let sel: Vec<flatten::Prim> = prims
            .iter()
            .filter(|p| p.intersects(bb, pad))
            .cloned()
            .collect();
        pages.push(pdf::Page {
            w_pt: f.page_w_mm * MM_TO_PT,
            h_pt: f.page_h_mm * MM_TO_PT,
            prims: sel,
            ox: f.bbox.min_x,
            oy: f.bbox.min_y,
            k: MM_TO_PT / f.units_per_mm,
        });
    }
    let font = match font {
        Some(f) => f.to_string(),
        None => default_font().ok_or_else(|| "找不到可用的中文字体".to_string())?,
    };
    let bytes = pdf::build(&pages, &font)?;
    std::fs::write(out, &bytes).map_err(|e| format!("写文件失败 {}: {}", out, e))?;
    Ok(ExportSummary {
        pages: pages.len(),
        bytes: bytes.len(),
        details: fs
            .iter()
            .enumerate()
            .map(|(i, f)| PageOut {
                sheet: f.sheet.clone(),
                page_w_mm: f.page_w_mm,
                page_h_mm: f.page_h_mm,
                prims: pages[i].prims.len(),
            })
            .collect(),
    })
}

/// 拆分后单个文件摘要
#[derive(Clone, Debug)]
pub struct SplitFile {
    /// 输出文件路径
    pub path: String,
    /// 图幅名
    pub sheet: String,
    /// 图号(取自图签，可能为空)
    pub drawing_no: String,
    /// 图名(取自图签，可能为空)
    pub title: String,
    /// 保留下来的模型空间实体数
    pub entities: usize,
    /// 文件字节数
    pub bytes: usize,
}

/// 拆分摘要
#[derive(Clone, Debug)]
pub struct SplitSummary {
    /// 识别到的图框数
    pub frames: usize,
    /// 成功写出的文件数
    pub written: usize,
    /// 写出总字节数
    pub bytes: usize,
    pub files: Vec<SplitFile>,
    /// 写失败的图框：(标注, 原因)
    pub failed: Vec<(String, String)>,
}

/// 按图框拆分为多个 DWG/DXF 文件（`format` 为 None 时默认 dwg）。
///
/// 逐个图框「克隆整份文档 → 删除框外实体 → 写出」，图层/块定义/样式全部保留；
/// 单个图框失败不中断其余图框，失败原因汇总在 [`SplitSummary::failed`]。
pub fn split_file(path: &str, out_dir: &str, format: Option<&str>) -> Result<SplitSummary, String> {
    let doc = load(path)?;
    let fs = frames::detect_frames(&doc);
    if fs.is_empty() {
        return Err("未检测到图框，无法拆分".into());
    }
    let fmt = split::OutFormat::from_arg(format.unwrap_or(""))?;

    let prims = flatten::flatten_model(&doc);
    let infos = sheet_infos(&fs, &prims);

    let dir = std::path::Path::new(out_dir);
    std::fs::create_dir_all(dir).map_err(|e| format!("创建目录失败 {}: {}", out_dir, e))?;
    let dir_abs = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
    let src_abs = std::fs::canonicalize(path).ok();

    let mut used = std::collections::HashSet::new();
    let mut files = Vec::new();
    let mut failed: Vec<(String, String)> = Vec::new();
    let mut total_bytes = 0usize;

    for (i, f) in fs.iter().enumerate() {
        let seq = i + 1;
        let info = &infos[i];
        let label = format!("#{} {}", seq, f.label());
        let stem = split::file_stem(seq, &info.drawing_no, &info.title, &f.sheet);
        let target = split::unique_path(dir, &stem, fmt.ext(), &mut used);

        // 避免覆盖源文件（同名输出与输入文件重合时跳过该框）
        if let Some(src) = &src_abs {
            if target.file_name().is_some_and(|n| dir_abs.join(n) == *src) {
                failed.push((label, "输出路径与输入文件相同，已跳过".into()));
                continue;
            }
        }

        let pad = f.bbox.w().max(f.bbox.h()) * split::PAD_RATIO;
        let (sub, _) = split::split_document(&doc, &f.bbox, pad);
        let entities = sub.model_space_entities().count();
        match split::write_output(&sub, &target, fmt) {
            Ok(bytes) => {
                total_bytes += bytes;
                files.push(SplitFile {
                    path: target.to_string_lossy().into_owned(),
                    sheet: f.sheet.clone(),
                    drawing_no: info.drawing_no.clone(),
                    title: info.title.clone(),
                    entities,
                    bytes,
                });
            }
            Err(e) => failed.push((label, e)),
        }
    }

    Ok(SplitSummary {
        frames: fs.len(),
        written: files.len(),
        bytes: total_bytes,
        files,
        failed,
    })
}

