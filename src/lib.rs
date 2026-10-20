//! dwg2pdf 库入口。
//!
//! 提供 DWG 解析、图框识别、图签/图纸目录提取与逐图框 PDF 导出，
//! 供命令行(`src/main.rs`)与 Python(`pybind`)复用。

pub mod api;
pub mod flatten;
pub mod frames;
pub mod geom;
pub mod pdf;
pub mod split;
pub mod title;
pub mod toc;

#[cfg(feature = "python")]
mod pybind;
