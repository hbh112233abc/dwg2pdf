//! PyO3 绑定：把 [crate::api] 的高层 API 暴露给 Python。
//!
//! 由 `python` feature 控制编译（maturin 构建时启用），不影响命令行构建。

use pyo3::exceptions::PyRuntimeError;
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList};

use crate::api;

fn rt<E: std::fmt::Display>(e: E) -> PyErr {
    PyRuntimeError::new_err(e.to_string())
}

/// 图框识别：返回 list[dict]
#[pyfunction]
fn detect_frames<'py>(py: Python<'py>, path: &str) -> PyResult<Bound<'py, PyList>> {
    let frames = api::detect_frames(path).map_err(rt)?;
    let list = PyList::empty(py);
    for f in frames {
        let d = PyDict::new(py);
        d.set_item("sheet", f.sheet)?;
        d.set_item("layer", f.layer)?;
        d.set_item("page_w_mm", f.page_w_mm)?;
        d.set_item("page_h_mm", f.page_h_mm)?;
        d.set_item("units_per_mm", f.units_per_mm)?;
        d.set_item("bbox", f.bbox)?;
        list.append(d)?;
    }
    Ok(list)
}

/// 逐图框提取图签：返回 list[dict]
#[pyfunction]
fn extract_info<'py>(py: Python<'py>, path: &str) -> PyResult<Bound<'py, PyList>> {
    let infos = api::extract_info(path).map_err(rt)?;
    let list = PyList::empty(py);
    for si in infos {
        let d = PyDict::new(py);
        d.set_item("sheet", si.sheet)?;
        d.set_item("drawing_no", si.drawing_no)?;
        d.set_item("title", si.title)?;
        d.set_item("category", si.category)?;
        d.set_item("discipline", si.discipline)?;
        d.set_item("date", si.date)?;
        d.set_item("is_toc_page", si.is_toc_page)?;
        let fields = PyDict::new(py);
        for (k, v) in si.fields {
            fields.set_item(k, v)?;
        }
        d.set_item("fields", fields)?;
        list.append(d)?;
    }
    Ok(list)
}

/// 提取图纸目录：返回 dict 或 None
#[pyfunction]
fn extract_toc<'py>(py: Python<'py>, path: &str) -> PyResult<Py<PyAny>> {
    match api::extract_toc(path).map_err(rt)? {
        None => Ok(py.None()),
        Some(t) => {
            let d = PyDict::new(py);
            d.set_item("headers", t.headers)?;
            d.set_item("rows", t.rows)?;
            Ok(d.into_any().unbind())
        }
    }
}

/// 按图框逐页导出 PDF：返回 dict 摘要
#[pyfunction]
#[pyo3(signature = (dwg_path, out_path, font=None))]
fn export_pdf<'py>(
    py: Python<'py>,
    dwg_path: &str,
    out_path: &str,
    font: Option<&str>,
) -> PyResult<Py<PyAny>> {
    let s = api::export_file(dwg_path, out_path, font).map_err(rt)?;
    let d = PyDict::new(py);
    d.set_item("pages", s.pages)?;
    d.set_item("bytes", s.bytes)?;
    d.set_item("out_path", out_path)?;
    let details = PyList::empty(py);
    for p in s.details {
        let pd = PyDict::new(py);
        pd.set_item("sheet", p.sheet)?;
        pd.set_item("page_w_mm", p.page_w_mm)?;
        pd.set_item("page_h_mm", p.page_h_mm)?;
        pd.set_item("prims", p.prims)?;
        details.append(pd)?;
    }
    d.set_item("details", details)?;
    Ok(d.into_any().unbind())
}

/// 按图框拆分为多个 DWG/DXF：返回 dict 摘要
#[pyfunction]
#[pyo3(signature = (dwg_path, out_dir, format=None))]
fn split_dwg<'py>(
    py: Python<'py>,
    dwg_path: &str,
    out_dir: &str,
    format: Option<&str>,
) -> PyResult<Py<PyAny>> {
    let s = api::split_file(dwg_path, out_dir, format).map_err(rt)?;
    let d = PyDict::new(py);
    d.set_item("frames", s.frames)?;
    d.set_item("written", s.written)?;
    d.set_item("bytes", s.bytes)?;
    let files = PyList::empty(py);
    for f in s.files {
        let fd = PyDict::new(py);
        fd.set_item("path", f.path)?;
        fd.set_item("sheet", f.sheet)?;
        fd.set_item("drawing_no", f.drawing_no)?;
        fd.set_item("title", f.title)?;
        fd.set_item("entities", f.entities)?;
        fd.set_item("bytes", f.bytes)?;
        files.append(fd)?;
    }
    d.set_item("files", files)?;
    let failed = PyList::empty(py);
    for (label, reason) in s.failed {
        failed.append((label, reason))?;
    }
    d.set_item("failed", failed)?;
    Ok(d.into_any().unbind())
}

#[pymodule]
fn dwg2pdf(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(detect_frames, m)?)?;
    m.add_function(wrap_pyfunction!(extract_info, m)?)?;
    m.add_function(wrap_pyfunction!(extract_toc, m)?)?;
    m.add_function(wrap_pyfunction!(export_pdf, m)?)?;
    m.add_function(wrap_pyfunction!(split_dwg, m)?)?;
    m.add("__version__", env!("CARGO_PKG_VERSION"))?;
    m.add("DEFAULT_FONT", api::default_font())?;
    Ok(())
}
