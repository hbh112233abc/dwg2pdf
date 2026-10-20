//! DWG 探针(probe)：分析图纸结构，为图框识别与字段映射提供依据。
//!
//! 用法:
//!   dwg2pdf dump  <file.dwg>           全局概览
//!   dwg2pdf blocks <file.dwg>          按块名聚合的 INSERT 统计(尺寸/数量)
//!   dwg2pdf layer <file.dwg> <图层名>   列出该图层上的实体
//!   dwg2pdf block <file.dwg> <块名>     列出块定义内部实体

use std::collections::HashMap;
use std::process::ExitCode;

use acadrust::entities::EntityType;
use acadrust::{CadDocument, DwgReader};

use dwg2pdf::geom::{entity_world_bbox, insert_world_bbox, BBox};
use dwg2pdf::{api, flatten, frames, geom, title, toc};

// ---------------------------------------------------------------------------
// dump
// ---------------------------------------------------------------------------
fn dump(path: &str) -> acadrust::Result<()> {
    let doc = load(path)?;
    println!("=========== 文件概览 ===========");
    println!("文件: {}", path);
    println!("版本: {:?}", doc.version);
    println!("实体总数: {}", doc.entity_count());
    println!("块记录数: {}", doc.block_records.len());
    println!("图层数: {}", doc.layers.len());

    let mut hist: HashMap<&'static str, usize> = HashMap::new();
    for e in doc.model_space_entities() {
        *hist.entry(e.as_entity().entity_type()).or_insert(0) += 1;
    }
    let mut hist_vec: Vec<_> = hist.into_iter().collect();
    hist_vec.sort_by(|a, b| b.1.cmp(&a.1));
    println!("\n=== 实体类型分布 ===");
    for (t, n) in hist_vec {
        println!("  {:<20} {}", t, n);
    }

    println!("\n=== 图层 ===");
    for l in doc.layers.iter() {
        println!("  {}", l.name);
    }

    println!("\n=== 布局(Layout objects) ===");
    for obj in doc.objects.values() {
        if let acadrust::objects::ObjectType::Layout(lay) = obj {
            println!(
                "  名称={:<12} 图纸={} 页宽={:.1} 页高={:.1} extents=({:.1},{:.1})-({:.1},{:.1})",
                lay.name,
                if lay.paper_size.is_empty() { "-" } else { &lay.paper_size },
                lay.paper_width,
                lay.paper_height,
                lay.min_extents.0,
                lay.min_extents.1,
                lay.max_extents.0,
                lay.max_extents.1,
            );
        }
    }

    println!("\n=== 块定义(含属性) ===");
    let mut brs: Vec<_> = doc.block_records.iter().filter(|b| b.flags.has_attributes).collect();
    brs.sort_by(|a, b| a.name.cmp(&b.name));
    for br in brs {
        println!(
            "  {:<28} 匿名={} 图元数={} 被引用={}",
            br.name,
            br.flags.anonymous,
            br.entity_handles.len(),
            br.insert_handles.len(),
        );
    }

    // ATTRIB (模型空间直接存在的)
    println!("\n=== 模型空间 ATTRIB 实体 ===");
    let mut n_attr = 0;
    for e in doc.model_space_entities() {
        if let EntityType::AttributeEntity(a) = e {
            println!("  tag={:<20} value={:<24} 位置=({:.1},{:.1})", a.tag, a.value, a.insertion_point.x, a.insertion_point.y);
            n_attr += 1;
            if n_attr > 100 {
                println!("  ...");
                break;
            }
        }
    }
    if n_attr == 0 {
        println!("  (无)");
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// texts：列出模型空间 TEXT/MTEXT(可用于定位图签字段)
//   dwg2pdf texts <file> [x0 y0 x1 y1]
// ---------------------------------------------------------------------------
fn texts(path: &str, filter: Option<(f64, f64, f64, f64)>) -> acadrust::Result<()> {
    let doc = load(path)?;
    let mut rows: Vec<(f64, f64, String)> = Vec::new();
    for e in doc.model_space_entities() {
        let (x, y, val, extra) = match e {
            EntityType::Text(t) => (
                t.insertion_point.x,
                t.insertion_point.y,
                t.value.clone(),
                format!("高={:.0} 旋转={:.1}°", t.height, t.rotation.to_degrees()),
            ),
            EntityType::MText(m) => {
                let plain = m.value.replace('\n', "\\n");
                (
                    m.insertion_point.x,
                    m.insertion_point.y,
                    plain,
                    format!("高={:.0} MTEXT", m.height),
                )
            }
            _ => continue,
        };
        if let Some((x0, y0, x1, y1)) = filter {
            if x < x0 || x > x1 || y < y0 || y > y1 {
                continue;
            }
        }
        rows.push((y, x, format!("({:.0},{:.0}) {} 图层={} {:?}", x, y, extra, e.common().layer, val)));
    }
    rows.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal).then(a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal)));
    println!("=== TEXT/MTEXT 共 {} 条 ===", rows.len());
    for (_, _, s) in rows.iter().take(400) {
        println!("{}", s);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// frames：检测图框
// ---------------------------------------------------------------------------
fn frames_cmd(path: &str) -> acadrust::Result<()> {
    let doc = load(path)?;
    let fs = frames::detect_frames(&doc);
    println!("=== 检测到 {} 个图框 ===", fs.len());
    for (i, f) in fs.iter().enumerate() {
        println!(
            "  #{:<2} 图幅={:<3} 页面={:.1}x{:.1}mm 比例1单位={:.3}mm 范围=({:.0},{:.0})-({:.0},{:.0}) 图层={}",
            i + 1,
            f.sheet,
            f.page_w_mm,
            f.page_h_mm,
            1.0 / f.units_per_mm,
            f.bbox.min_x, f.bbox.min_y, f.bbox.max_x, f.bbox.max_y,
            f.layer
        );
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// big：列出模型空间中尺寸很大的图元(可能的图框边框)
// ---------------------------------------------------------------------------
fn big(path: &str, min_dim: f64) -> acadrust::Result<()> {
    let doc = load(path)?;
    println!("=== 大尺寸图元 (任一维 > {:.0}) ===", min_dim);
    let mut rows: Vec<(f64, String)> = Vec::new();
    for e in doc.model_space_entities() {
        let bb = match e {
            EntityType::Insert(_) => continue,
            _ => entity_world_bbox(e, &geom::T2::identity()),
        };
        if !bb.valid() {
            continue;
        }
        if bb.w() > min_dim || bb.h() > min_dim {
            let t = e.as_entity().entity_type();
            rows.push((
                bb.w().max(bb.h()),
                format!(
                    "{:<14} 图层={:<16} 范围=({:.0},{:.0})-({:.0},{:.0}) 共 {:.0}x{:.0}",
                    t, e.common().layer, bb.min_x, bb.min_y, bb.max_x, bb.max_y, bb.w(), bb.h()
                ),
            ));
        }
    }
    rows.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    for (_, s) in rows.iter().take(80) {
        println!("{}", s);
    }
    println!("(共 {} 个)", rows.len());
    Ok(())
}

// ---------------------------------------------------------------------------
// layers：按图层统计实体数量与范围
// ---------------------------------------------------------------------------
fn layers(path: &str) -> acadrust::Result<()> {
    let doc = load(path)?;
    let mut agg: HashMap<String, (usize, BBox)> = HashMap::new();
    let mut all = BBox::empty();
    for e in doc.model_space_entities() {
        let lay = e.common().layer.clone();
        let bb = match e {
            EntityType::Insert(ins) => insert_world_bbox(&doc, ins),
            _ => entity_world_bbox(e, &geom::T2::identity()),
        };
        let ent = agg.entry(lay).or_insert((0, BBox::empty()));
        ent.0 += 1;
        ent.1.merge(&bb);
        all.merge(&bb);
    }
    println!("== 模型空间总范围 == ({:.0},{:.0})-({:.0},{:.0}) 共 {:.0} x {:.0}", all.min_x, all.min_y, all.max_x, all.max_y, all.w(), all.h());
    let mut rows: Vec<_> = agg.into_iter().collect();
    rows.sort_by(|a, b| b.1 .0.cmp(&a.1 .0));
    println!("\n{:<28} {:>6}  {:<28}", "图层", "数量", "范围(WxH)@左下");
    for (lay, (n, bb)) in rows {
        println!(
            "{:<28} {:>6}  {:.0}x{:.0} @({:.0},{:.0})",
            lay, n,
            if bb.valid() { bb.w() } else { 0.0 },
            if bb.valid() { bb.h() } else { 0.0 },
            bb.min_x, bb.min_y,
        );
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// blocks：按块名聚合 INSERT
// ---------------------------------------------------------------------------
fn blocks(path: &str) -> acadrust::Result<()> {
    let doc = load(path)?;
    #[derive(Default)]
    struct Agg {
        count: usize,
        layers: HashMap<String, usize>,
        min_w: f64,
        max_w: f64,
        min_h: f64,
        max_h: f64,
        has_attr: bool,
        sample_point: (f64, f64),
    }
    let mut agg: HashMap<String, Agg> = HashMap::new();
    for e in doc.model_space_entities() {
        if let EntityType::Insert(ins) = e {
            let bb = insert_world_bbox(&doc, ins);
            let (w, h) = if bb.valid() { (bb.w(), bb.h()) } else { (0.0, 0.0) };
            let a = agg.entry(ins.block_name.clone()).or_insert_with(|| Agg {
                count: 0,
                layers: HashMap::new(),
                min_w: f64::INFINITY,
                max_w: 0.0,
                min_h: f64::INFINITY,
                max_h: 0.0,
                has_attr: false,
                sample_point: (ins.insert_point.x, ins.insert_point.y),
            });
            a.count += 1;
            *a.layers.entry(ins.common.layer.clone()).or_insert(0) += 1;
            if w > 0.0 {
                a.min_w = a.min_w.min(w);
                a.max_w = a.max_w.max(w);
            }
            if h > 0.0 {
                a.min_h = a.min_h.min(h);
                a.max_h = a.max_h.max(h);
            }
            if !ins.attributes.is_empty() {
                a.has_attr = true;
            }
        }
    }
    let mut rows: Vec<_> = agg.into_iter().collect();
    rows.sort_by(|a, b| {
        let sa = a.1.max_w * a.1.max_h;
        let sb = b.1.max_w * b.1.max_h;
        sb.partial_cmp(&sa).unwrap_or(std::cmp::Ordering::Equal)
    });
    println!("块名 | 数量 | 图层 | 外框尺寸范围(WxH) | 属性 | 样例插入点");
    for (name, a) in rows {
        let mut layers: Vec<_> = a.layers.iter().collect();
        layers.sort_by(|x, y| y.1.cmp(x.1));
        let lay = layers
            .iter()
            .take(3)
            .map(|(k, v)| format!("{}x{}", k, v))
            .collect::<Vec<_>>()
            .join(",");
        println!(
            "{:<26} | {:>4} | {:<28} | {:.0}-{:.0} x {:.0}-{:.0} | {} | ({:.0},{:.0})",
            name,
            a.count,
            lay,
            if a.min_w.is_finite() { a.min_w } else { 0.0 },
            a.max_w,
            if a.min_h.is_finite() { a.min_h } else { 0.0 },
            a.max_h,
            if a.has_attr { "Y" } else { "-" },
            a.sample_point.0,
            a.sample_point.1,
        );
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// layer：列出某图层上的实体
// ---------------------------------------------------------------------------
fn layer(path: &str, name: &str) -> acadrust::Result<()> {
    let doc = load(path)?;
    let target = name.to_lowercase();
    println!("=== 图层 {} 上的实体 ===", name);
    let mut types: HashMap<&'static str, usize> = HashMap::new();
    let mut bb_all = BBox::empty();
    let mut inserts: Vec<String> = Vec::new();
    let mut texts: Vec<String> = Vec::new();
    for e in doc.model_space_entities() {
        if e.common().layer.to_lowercase() != target {
            continue;
        }
        let t = e.as_entity().entity_type();
        *types.entry(t).or_insert(0) += 1;
        match e {
            EntityType::Insert(ins) => {
                let bb = insert_world_bbox(&doc, ins);
                if bb.valid() {
                    bb_all.add((bb.min_x, bb.min_y));
                    bb_all.add((bb.max_x, bb.max_y));
                }
                inserts.push(format!(
                    "  INSERT 块={:<24} @({:.1},{:.1}) 外框={:.1}x{:.1}",
                    ins.block_name, ins.insert_point.x, ins.insert_point.y,
                    if bb.valid() { bb.w() } else { 0.0 },
                    if bb.valid() { bb.h() } else { 0.0 },
                ));
            }
            EntityType::Text(t) => {
                let bb = entity_world_bbox(e, &geom::T2::identity());
                if bb.valid() {
                    bb_all.add((bb.min_x, bb.min_y));
                    bb_all.add((bb.max_x, bb.max_y));
                }
                texts.push(format!(
                    "  TEXT   位置=({:.1},{:.1}) 高={:.1} 文本={:?}",
                    t.insertion_point.x, t.insertion_point.y, t.height, t.value
                ));
            }
            _ => {
                let bb = entity_world_bbox(e, &geom::T2::identity());
                if bb.valid() {
                    bb_all.add((bb.min_x, bb.min_y));
                    bb_all.add((bb.max_x, bb.max_y));
                }
            }
        }
    }
    println!("\n-- 类型统计 --");
    let mut tv: Vec<_> = types.into_iter().collect();
    tv.sort_by(|a, b| b.1.cmp(&a.1));
    for (t, n) in tv {
        println!("  {:<16} {}", t, n);
    }
    if bb_all.valid() {
        println!(
            "\n-- 图层总范围 -- ({:.1},{:.1})-({:.1},{:.1})  共 {:.1} x {:.1}",
            bb_all.min_x, bb_all.min_y, bb_all.max_x, bb_all.max_y, bb_all.w(), bb_all.h()
        );
    }
    println!("\n-- INSERT --");
    for s in inserts.iter().take(60) {
        println!("{}", s);
    }
    println!("\n-- TEXT --");
    for s in texts.iter().take(120) {
        println!("{}", s);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// block：列出块定义内部实体
// ---------------------------------------------------------------------------
fn block(path: &str, name: &str) -> acadrust::Result<()> {
    let doc = load(path)?;
    println!("=== 块定义 {} 内部实体 ===", name);
    let mut types: HashMap<&'static str, usize> = HashMap::new();
    let mut bb = BBox::empty();
    let mut lines = Vec::new();
    for e in doc.entities_in_block(name) {
        let t = e.as_entity().entity_type();
        *types.entry(t).or_insert(0) += 1;
        let ebb = entity_world_bbox(e, &geom::T2::identity());
        if ebb.valid() {
            bb.add((ebb.min_x, ebb.min_y));
            bb.add((ebb.max_x, ebb.max_y));
        }
        if let EntityType::Text(t2) = e {
            lines.push(format!("  TEXT 位置=({:.1},{:.1}) 高={:.1} {:?}", t2.insertion_point.x, t2.insertion_point.y, t2.height, t2.value));
        }
        if let EntityType::AttributeDefinition(a) = e {
            lines.push(format!("  ATTDEF tag={} prompt={:?} default={:?}", a.tag, a.prompt, a.default_value));
        }
    }
    let mut tv: Vec<_> = types.into_iter().collect();
    tv.sort_by(|a, b| b.1.cmp(&a.1));
    println!("-- 类型统计 --");
    for (t, n) in tv {
        println!("  {:<16} {}", t, n);
    }
    if bb.valid() {
        println!("-- 本地范围 -- ({:.1},{:.1})-({:.1},{:.1}) 共 {:.1} x {:.1}", bb.min_x, bb.min_y, bb.max_x, bb.max_y, bb.w(), bb.h());
    }
    for l in lines.iter().take(200) {
        println!("{}", l);
    }
    Ok(())
}

fn load(path: &str) -> acadrust::Result<CadDocument> {
    DwgReader::from_file(path)?.read()
}

// ---------------------------------------------------------------------------
// 渲染/输出
// ---------------------------------------------------------------------------
/// 扁平化文本探针：列出展开块后的所有文本（世界坐标）
fn ftxts(path: &str, filter: Option<(f64, f64, f64, f64)>) -> Result<(), String> {
    let doc = load(path).map_err(|e| e.to_string())?;
    let prims = flatten::flatten_model(&doc);
    let mut rows: Vec<(f64, f64, String)> = Vec::new();
    for p in &prims {
        if let flatten::PrimKind::Text { x, y, h, s, .. } = &p.kind {
            if let Some((x0, y0, x1, y1)) = filter {
                if *x < x0 || *x > x1 || *y < y0 || *y > y1 {
                    continue;
                }
            }
            rows.push((
                *y,
                *x,
                format!("({:.1},{:.1}) 高={:.1} 图层={} {:?}", x, y, h, p.layer, s),
            ));
        }
    }
    rows.sort_by(|a, b| {
        b.0.partial_cmp(&a.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
    });
    println!("=== 扁平化 TEXT 共 {} 条 ===", rows.len());
    for (_, _, s) in rows.iter().take(500) {
        println!("{}", s);
    }
    Ok(())
}

/// 信息命令：逐图框提取图签
fn info_cmd(path: &str) -> Result<(), String> {
    let doc = load(path).map_err(|e| e.to_string())?;
    let fs = frames::detect_frames(&doc);
    let prims = flatten::flatten_model(&doc);
    println!("=== {} 个图框的图签信息 ===", fs.len());
    for (i, f) in fs.iter().enumerate() {
        let bb = (f.bbox.min_x, f.bbox.min_y, f.bbox.max_x, f.bbox.max_y);
        // 目录页（含目录表）不作为图签解析
        if toc::extract(&prims, bb).is_some() {
            println!("#{:<2} {} => 【图纸目录页】", i + 1, f.label());
            continue;
        }
        let si = title::extract(&prims, bb, &f.sheet);
        println!("#{:<2} {} => {}", i + 1, f.label(), si.summary());
        for (k, v) in &si.fields {
            println!("       {:<10} = {}", k, v);
        }
    }
    Ok(())
}

/// 目录命令：在任一图框内查找图纸目录表格
fn toc_cmd(path: &str) -> Result<(), String> {
    let doc = load(path).map_err(|e| e.to_string())?;
    let fs = frames::detect_frames(&doc);
    let prims = flatten::flatten_model(&doc);
    let mut found = false;
    for (i, f) in fs.iter().enumerate() {
        let bb = (f.bbox.min_x, f.bbox.min_y, f.bbox.max_x, f.bbox.max_y);
        if let Some(t) = toc::extract(&prims, bb) {
            found = true;
            println!("=== 图纸目录（图框 #{}, {}）===", i + 1, f.sheet);
            println!("列: {}", t.headers.join(" | "));
            for r in &t.rows {
                println!("  {}", r.join(" | "));
            }
            if let (Some(no), Some(nm)) = (t.col("图号"), t.col("图纸名称")) {
                println!("-- 图号 → 图名 --");
                for r in &t.rows {
                    println!("  {}  {}", r[no], r[nm]);
                }
            }
        }
    }
    if !found {
        println!("未在任一图框内找到目录表");
    }
    Ok(())
}

/// 导出命令：按图框逐页导出 PDF（页尺寸=图幅）
fn export(path: &str, out: &str, font: Option<&str>) -> Result<(), String> {
    let s = api::export_file(path, out, font)?;
    println!("已导出 {} 页 → {}（{} 字节）", s.pages, out, s.bytes);
    for (i, p) in s.details.iter().enumerate() {
        println!(
            "  第{}页 {} {:.1}x{:.1}mm 图元={}",
            i + 1,
            p.sheet,
            p.page_w_mm,
            p.page_h_mm,
            p.prims
        );
    }
    Ok(())
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let cmd = args.get(1).map(|s| s.as_str()).unwrap_or("dump");
    let path = args.get(2).map(|s| s.as_str()).unwrap_or("tests/test.dwg");
    let arg3 = args.get(3).map(|s| s.as_str()).unwrap_or("");
    let num = |i: usize| args.get(i).and_then(|s| s.parse::<f64>().ok());

    let result: Result<(), String> = match cmd {
        "dump" => dump(path).map_err(|e| e.to_string()),
        "blocks" => blocks(path).map_err(|e| e.to_string()),
        "layers" => layers(path).map_err(|e| e.to_string()),
        "big" => big(path, arg3.parse().unwrap_or(40000.0)).map_err(|e| e.to_string()),
        "frames" => frames_cmd(path).map_err(|e| e.to_string()),
        "layer" => layer(path, arg3).map_err(|e| e.to_string()),
        "block" => block(path, arg3).map_err(|e| e.to_string()),
        "texts" => {
            let f = match (num(3), num(4), num(5), num(6)) {
                (Some(a), Some(b), Some(c), Some(d)) => Some((a, b, c, d)),
                _ => None,
            };
            texts(path, f).map_err(|e| e.to_string())
        }
        "info" => info_cmd(path),
        "ftxts" => {
            let f = match (num(3), num(4), num(5), num(6)) {
                (Some(a), Some(b), Some(c), Some(d)) => Some((a, b, c, d)),
                _ => None,
            };
            ftxts(path, f)
        }
        "toc" => toc_cmd(path),
        "export" => {
            let out = if arg3.is_empty() { "out.pdf" } else { arg3 };
            let font = args.get(4).map(|s| s.as_str()).filter(|s| !s.is_empty());
            export(path, out, font)
        }
        "font" => match api::default_font() {
            Some(f) => {
                println!("中文字体: {}", f);
                Ok(())
            }
            None => Err(format!(
                "未找到可用的中文字体，请设置环境变量 {} 指定字体文件",
                api::FONT_ENV
            )),
        },
        other => {
            eprintln!("未知命令: {}", other);
            eprintln!(
                "用法: dwg2pdf <dump|blocks|layers|big|frames|layer|block|texts|ftxts|info|toc|export|font> <file.dwg> [arg]"
            );
            eprintln!("提示: 设置环境变量 DWG2PDF_DEBUG=1 可输出图框/比例推断的调试日志");
            return ExitCode::FAILURE;
        }
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("错误: {}", e);
            ExitCode::FAILURE
        }
    }
}
