# dwg2pdf

纯 Rust 实现的 DWG 工程图纸解析与导出工具：**识别图框 → 提取图签/图纸目录 → 按图框逐页导出 PDF**（每页尺寸严格等于对应图幅）。

无需 AutoCAD、Ghostscript 等外部二进制，DWG 解析基于 [acadrust](https://crates.io/crates/acadrust)，PDF 由内置的矢量生成器直接产出并内嵌中文字体。

## 特性

- **图框识别**：从模型空间自动找出图纸图框，判定 A0/A1/A2/A3/A4 图幅，支持**加长图框**（`A1+1/4`、`A0+1/8` 等）与**自定义非标图幅**。
- **比例自动推断**：不依赖固定绘图单位，按短边与 10 的整数幂约定自动推断 `units_per_mm`。
- **图签提取**：按「标签—值」网格从图框内文本关联出图名、图号、比例、类别、日期等字段。
- **图纸目录提取**：识别目录表格，按数据坐标聚类得到列边界，输出表头与数据行。
- **逐图框出图**：单个 DWG 多图框时，一框一页，页尺寸 = 图幅（mm）。
- **中文 PDF**：走 CIDFontType2 + Identity-H 内嵌 TrueType 字体，中文/符号正常显示。
- **两种调用方式**：命令行（CLI）与 Python（PyO3 + maturin）。

## 环境要求

- Rust（edition 2024，建议 1.85+）
- Python 3.8+（仅 Python 调用时需要）
- 一个可用的中文字体文件（默认自动探测 `C:\Windows\Fonts\simhei.ttf` 等）

## 快速开始（命令行）

```bash
cargo build --release

# 识别图框
target/release/dwg2pdf frames tests/test.dwg

# 提取图签信息
target/release/dwg2pdf info tests/test.dwg

# 提取图纸目录
target/release/dwg2pdf toc tests/test.dwg

# 导出 PDF（默认 out.pdf），每页对应一个图框
target/release/dwg2pdf export tests/test.dwg out.pdf
```

导出示例输出：

```
已导出 6 页 → out.pdf（15264179 字节）
  第1页 A4  210.0x297.0mm 图元=111
  第2页 A1  841.0x594.0mm 图元=8929
  第3页 A1  841.0x594.0mm 图元=11416
  第4页 A1  841.0x594.0mm 图元=22378
  第5页 A0 1189.0x841.0mm 图元=24212
  第6页 A0 1189.0x841.0mm 图元=9530
```

### CLI 命令一览

| 命令 | 说明 |
| --- | --- |
| `dump <file.dwg>` | 文件全局概览：版本、实体类型分布、图层、布局、块定义 |
| `blocks <file.dwg>` | 按块名聚合的 INSERT 统计 |
| `layers <file.dwg>` | 列出图层 |
| `big <file.dwg> [min]` | 列出大于 `min`（默认 40000）尺寸的实体 |
| `frames <file.dwg>` | 识别图框并判定图幅/比例 |
| `layer <file.dwg> <图层名>` | 列出指定图层上的实体 |
| `block <file.dwg> <块名>` | 列出块定义内部实体 |
| `texts <file.dwg> [x0 y0 x1 y1]` | 列出模型空间 TEXT/MTEXT（可带矩形范围过滤） |
| `ftxts <file.dwg> [x0 y0 x1 y1]` | 展开块后列出所有文本（世界坐标） |
| `info <file.dwg>` | 逐图框提取图签信息（目录页自动标记） |
| `toc <file.dwg>` | 在任一图框内查找并打印图纸目录表 |
| `export <file.dwg> [out.pdf] [font]` | 按图框逐页导出 PDF（可选指定字体文件） |
| `font` | 打印自动探测到的中文字体路径 |

## Python 调用（PyO3 + maturin）

Python 绑定由 `python` feature 控制，使用 maturin 构建：

```bash
python -m venv .venv
.venv\Scripts\activate            # Windows
pip install maturin

# 开发安装（构建并安装到当前虚拟环境）
maturin develop --release
```

```python
import dwg2pdf

print(dwg2pdf.__version__)        # 版本号
print(dwg2pdf.DEFAULT_FONT)       # 自动探测到的中文字体路径

# 1) 图框识别 -> list[dict]
frames = dwg2pdf.detect_frames("tests/test.dwg")
for f in frames:
    print(f["sheet"], f["page_w_mm"], f["page_h_mm"], f["units_per_mm"], f["layer"], f["bbox"])

# 2) 图签信息 -> list[dict]（目录页 is_toc_page=True，fields 为「标签->值」字典）
for si in dwg2pdf.extract_info("tests/test.dwg"):
    print(si["sheet"], si["is_toc_page"], si["drawing_no"], si["title"], si["fields"])

# 3) 图纸目录 -> dict{headers, rows} 或 None
toc = dwg2pdf.extract_toc("tests/test.dwg")
print(toc["headers"] if toc else None)
print(toc["rows"] if toc else None)

# 4) 逐图框导出 PDF -> dict 摘要
summary = dwg2pdf.export_pdf("tests/test.dwg", "out.pdf", font=None)
print(summary["pages"], summary["bytes"], summary["out_path"])
for p in summary["details"]:
    print(p["sheet"], p["page_w_mm"], p["page_h_mm"], p["prims"])
```

### Python API

| 函数 | 返回 |
| --- | --- |
| `detect_frames(path)` | `list[dict]`：`sheet / layer / page_w_mm / page_h_mm / units_per_mm / bbox` |
| `extract_info(path)` | `list[dict]`：图签字段 + `is_toc_page` + `fields`(dict) |
| `extract_toc(path)` | `dict{headers, rows}` 或 `None` |
| `export_pdf(dwg_path, out_path, font=None)` | `dict`：`pages / bytes / out_path / details` |

`font` 传 `None` 时使用内置字体探测结果（`DEFAULT_FONT`）。

## 工作原理

整体流水线：`DWG → 图框识别 → 矢量扁平化 → 图签/目录提取 → PDF`

1. **解析**：`acadrust::DwgReader` 读取 DWG，读取模型空间实体时使用 `doc.model_space_entities()`，避免误取块定义内部实体。
2. **图框识别**（`frames.rs`）：收集闭合 LWPOLYLINE / Polyline2D 候选矩形，按尺寸门槛、长宽比（1.0–4.2）过滤，剔除含于其它图框的内框；再判定图幅与比例。
3. **矢量扁平化**（`flatten.rs`）：把实体（LINE/ARC/CIRCLE/SOLID/HATCH/TEXT/MTEXT/INSERT……）展开为统一图元，递归展开块引用并套用平移/缩放/旋转矩阵。
4. **图签提取**（`title.rs`）：归一化文本后精确匹配标签，取同行右侧文本作为值，支持值跨两行拼接，跳过英文字段名。
5. **目录提取**（`toc.rs`）：以「序号」列定位表头，数据 x 坐标聚类得出列边界，强制列序严格递增。
6. **PDF 生成**（`pdf.rs`）：自研 PDF 生成器，Type0/CIDFontType2/Identity-H 内嵌 TrueType 字体，按图元绘制矢量图形与文本。

### 图幅判定规则

- 用**短边**锁定基准幅面：`units_per_mm = 短边 / 标准短边`，须接近 10 的整数幂（`score < 0.15`）。A 系列各幅面长宽比相同，只有短边能区分 A0/A1/…。
- 长边不得短于该幅面标准长边；长边超出则为**加长**，加长量须接近标准长边的 1/8 整数倍，记为 `A1+1/4` 形式。
- 匹配失败时，若在疑似图框图层上，则遍历 10 的整数幂候选推断比例（短边落在 180–1500mm 且最接近标准短边）。

## 项目结构

```
src/
  main.rs      命令行入口与各探针子命令
  lib.rs       库入口（rlib + cdylib）
  api.rs       高层 API（CLI 与 Python 共用）
  pybind.rs    PyO3 绑定（python feature）
  geom.rs      2D 仿射变换、包围盒、块包围盒
  frames.rs    图框识别与图幅/比例判定
  flatten.rs   实体 → 矢量图元扁平化
  title.rs     图签信息提取
  toc.rs       图纸目录表提取
  pdf.rs       自研 PDF 生成器
pyproject.toml maturin 构建配置
tests/test.dwg 示例图纸
```

## 中文字体

导出 PDF 会内嵌一个系统中文字体（CIDFontType2 / Identity-H），按以下优先级自动探测，**支持 Windows / macOS / Linux**：

1. 环境变量 `DWG2PDF_FONT` 指定的字体文件（最高优先级）；
2. 各平台系统字体目录中按 CJK 关键字命中的字体：
   - Windows：`%SystemRoot%\Fonts`、`C:\Windows\Fonts`
   - macOS：`/System/Library/Fonts`、`/Library/Fonts`、`~/Library/Fonts`
   - Linux：`/usr/share/fonts`、`/usr/local/share/fonts`、`~/.local/share/fonts`、`~/.fonts`
3. 上述目录中任意可解析且含常用汉字的字体。

候选会做「可解析 + 含常用汉字」校验，避免选中不含 CJK 的字体导致中文空白；Windows 优先黑体系列（`simhei`/`msyh`/…），macOS 优先苹方（`PingFang`），Linux 优先思源/文泉驿/Noto。

```bash
# 查看当前探测到的字体
dwg2pdf font

# 显式指定字体
DWG2PDF_FONT=/usr/share/fonts/truetype/wqy/wqy-zenhei.ttc dwg2pdf export tests/test.dwg out.pdf
# 或直接作为参数
dwg2pdf export tests/test.dwg out.pdf /path/to/font.ttf
```

Python 侧：`dwg2pdf.DEFAULT_FONT` 为探测结果，`export_pdf(..., font=None)` 传 `None` 时使用该结果。

## 调试

设置环境变量 `DWG2PDF_DEBUG=1` 输出图框/比例推断的调试日志（stderr）：

```bash
DWG2PDF_DEBUG=1 target/release/dwg2pdf frames tests/test.dwg
```

## 测试

```bash
cargo test
```

## 已知限制

- 未内置字体，依赖系统存在中文字体（自动探测，可用 `DWG2PDF_FONT` 指定）；PDF 以 `FontFile2` 内嵌 TrueType 轮廓，对 CFF/OTF 轮廓字体或 `.ttc` 字体集合支持有限，建议提供 `.ttf`。
- 图框识别基于闭合多段线矩形，对以其他方式（如独立 LINE 拼合、视口边界）绘制的图框可能不识别。
- `doc.entities()` 含块定义内部实体，务必使用 `doc.model_space_entities()`。
