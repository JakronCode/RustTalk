//! The one ONNX Runtime environment of the process (ort allows a single one,
//! ever), shared by the natural voices and background removal.

use std::sync::OnceLock;

use anyhow::Result;
use ort::environment::Environment;

pub fn env() -> Result<Environment> {
    static ENV: OnceLock<std::result::Result<Environment, String>> = OnceLock::new();
    ENV.get_or_init(|| {
        ort::init()
            .with_name("rusttalk")
            .build()
            .map_err(|e| e.to_string())
    })
    .clone()
    .map_err(|e| anyhow::anyhow!("ONNX Runtime: {e}"))
}
