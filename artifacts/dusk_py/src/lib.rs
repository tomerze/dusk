use pyo3::prelude::*;

#[pymodule]
fn dusk(module: &Bound<'_, PyModule>) -> PyResult<()> {
    dusk_base::link_anchors();
    dusk_py::register_module(module)
}
