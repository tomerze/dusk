use pyo3::prelude::*;
use std::hint::black_box;

#[pymodule]
fn dusk(module: &Bound<'_, PyModule>) -> PyResult<()> {
    // Unfortunately we need to trick the linker into including all
    // crates that register sh entries.

    black_box(dusk_program_ps::sh_entry);

    dusk_py::register_module(module)
}
