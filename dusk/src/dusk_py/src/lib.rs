use pyo3::prelude::*;

/// A Python module implemented in Rust.
#[pymodule]
mod dusk {
    use pyo3::prelude::*;

    #[pyclass]
    pub struct Dusk {
        #[pyo3(get, set)]
        pub address: String,
        #[pyo3(get, set)]
        pub port: u16,
    }

    #[pymethods]
    impl Dusk {
        #[new]
        fn new() -> Self {
            Dusk {
                address: String::from("127.0.0.1"),
                port: 9090,
            }
        }

        fn push(&mut self, v: usize) {
            self.port += v as u16;
        }
    }
}
