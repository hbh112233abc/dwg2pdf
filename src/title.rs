//! 图签(标题栏)信息提取：从图框区域内的文本中按「标签—值」关联出字段。
//!
//! 工程图图签通常是「左列标签 + 右侧值」的网格。本模块不依赖固定坐标，
//! 而是：① 归一化文本后精确匹配已知标签；② 取同行(基线接近)右侧的文本
//! 作为值，遇下一个标签或间距过大即停止。

use crate::flatten::{texts_in, Prim, TextItem};

/// 已知标签（比较前会去除所有空白，故此处不带空格）
const LABELS: &[&str] = &[
    "图名", "图号", "工程名称", "子项目名称", "工程代号", "版次", "日期",
    "设计", "校对", "审核", "审定", "制图", "专业负责", "项目负责人",
    "建设单位", "图别", "比例", "备注", "制表", "制表人", "审核人",
    "会签", "工程编号", "项目名称",
];

/// 英文字段名：仅作为「值的终止符」，不生成字段（已归一化去空白并转大写）
const EN_LABELS: &[&str] = &[
    "DRAWNNO.", "TITLE", "PROJECTNO.", "PROJECT", "DATE", "DESIGN", "DRAWING",
    "CHECK", "EXAM", "PRO.ENG.", "APPROVAL", "CHIEFDESIGNER", "CLIENT",
    "SUBPROJECT", "SCALE", "ITEM", "NO.", "REMARK",
];

/// 归一化：删除所有空白字符（含全角空格）
pub fn norm(s: &str) -> String {
    s.chars()
        .filter(|c| !c.is_whitespace() && *c != '\u{3000}')
        .collect()
}

fn is_label(s: &str) -> Option<&'static str> {
    let n = norm(s);
    LABELS.iter().copied().find(|l| n.as_str() == *l)
}

/// 是否为英文字段名（不生成字段，仅需跳过）
fn is_en_label(s: &str) -> bool {
    let n = norm(s).to_ascii_uppercase();
    EN_LABELS.contains(&n.as_str())
}

/// 单张图纸的图签信息
#[derive(Clone, Debug, Default)]
pub struct SheetInfo {
    /// 图幅（A0/A1/...）
    pub sheet: String,
    /// 图号
    pub drawing_no: String,
    /// 图名
    pub title: String,
    /// 类别（由图名推断）
    pub category: String,
    /// 专业（由图号/图名推断）
    pub discipline: String,
    /// 日期
    pub date: String,
    /// 全部「标签=值」
    pub fields: Vec<(String, String)>,
}

impl SheetInfo {
    pub fn get(&self, k: &str) -> &str {
        self.fields
            .iter()
            .find(|(a, _)| a == k)
            .map(|(_, v)| v.as_str())
            .unwrap_or("")
    }
    pub fn summary(&self) -> String {
        format!(
            "[{}] 图号={} 图名={} 类别={} 专业={} 日期={}",
            if self.sheet.is_empty() { "?" } else { &self.sheet },
            if self.drawing_no.is_empty() { "-" } else { &self.drawing_no },
            if self.title.is_empty() { "-" } else { &self.title },
            if self.category.is_empty() { "-" } else { &self.category },
            if self.discipline.is_empty() { "-" } else { &self.discipline },
            if self.date.is_empty() { "-" } else { &self.date },
        )
    }
}

/// 文本宽度粗估（图纸单位）
fn est_w(t: &TextItem) -> f64 {
    t.s.chars().count() as f64 * t.h * 0.75
}

/// 取标签右侧同一行的文本作为值
fn value_right(items: &[TextItem], label: &TextItem) -> String {
    let tol = (label.h * 0.8).max(1.0);
    let mut cands: Vec<&TextItem> = items
        .iter()
        .filter(|t| (t.y - label.y).abs() <= tol && t.x > label.x + label.h * 0.3)
        .collect();
    // 值可能跨多行（如两行图名）：先按 y 从上到下，再按 x 从左到右
    cands.sort_by(|a, b| {
        b.y.partial_cmp(&a.y)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.x.partial_cmp(&b.x).unwrap_or(std::cmp::Ordering::Equal))
    });

    let mut out = String::new();
    let mut prev_end = label.x + est_w(label);
    for t in cands {
        // 遇到下一个中文标签 → 结束当前值
        if is_label(&t.s).is_some() {
            break;
        }
        // 英文字段名（如 DRAWN NO.）位于中文标签与其值之间，跳过而不计入值
        if is_en_label(&t.s) {
            continue;
        }
        let gap = t.x - prev_end;
        let lim = if out.is_empty() { 6000.0 } else { 3000.0 };
        if gap > lim {
            break;
        }
        out.push_str(&t.s);
        prev_end = t.x + est_w(t);
    }
    out
}

/// 依据图名推断类别
pub fn categorize(title: &str) -> &'static str {
    let t = title;
    if t.is_empty() {
        ""
    } else if t.contains("目录") {
        "图纸目录"
    } else if t.contains("图例") {
        "图例"
    } else if t.contains("说明") {
        "设计说明"
    } else if t.contains("设备") || t.contains("材料表") {
        "设备材料表"
    } else if t.contains("平面") {
        "平面图"
    } else if t.contains("系统") {
        "系统图"
    } else if t.contains("剖面") {
        "剖面图"
    } else if t.contains("大样") || t.contains("详图") {
        "详图"
    } else if t.contains("立面") {
        "立面图"
    } else if t.contains("示意") {
        "示意图"
    } else {
        "图纸"
    }
}

/// 由图号/图名推断专业
pub fn discipline_of(s: &str) -> &'static str {
    for c in s.chars() {
        match c {
            '暖' => return "暖通",
            '水' | '给' => return "给排水",
            '电' | '气' => return "电气",
            '建' => return "建筑",
            '结' => return "结构",
            '总' => return "总图",
            _ => {}
        }
    }
    ""
}

/// 从图框内的所有图元提取图签信息
pub fn extract(prims: &[Prim], bbox: (f64, f64, f64, f64), sheet: &str) -> SheetInfo {
    let items = texts_in(prims, bbox);
    let mut info = SheetInfo {
        sheet: sheet.to_string(),
        ..Default::default()
    };

    for it in &items {
        let Some(lbl) = is_label(&it.s) else { continue };
        if info.fields.iter().any(|(k, _)| k == lbl) {
            continue;
        }
        let v = value_right(&items, it);
        if !v.is_empty() {
            info.fields.push((lbl.to_string(), v));
        }
    }

    info.drawing_no = info.get("图号").to_string();
    info.title = info.get("图名").to_string();
    info.date = info.get("日期").to_string();
    info.category = categorize(&info.title).to_string();
    let seed = if !info.drawing_no.is_empty() {
        info.drawing_no.clone()
    } else {
        info.title.clone()
    };
    info.discipline = discipline_of(&seed).to_string();
    info
}
