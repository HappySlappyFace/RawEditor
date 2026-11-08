//! ONNX Runtime sessions. The environment is built on the first session,
//! so merely linking ONNX Runtime costs nothing. It then lives for the
//! process (it is small; each session owns its own thread pool, which goes
//! with the session).

use std::path::Path;
use std::sync::OnceLock;

use ort::session::builder::GraphOptimizationLevel;
use ort::session::Session;

fn threads() -> usize {
    std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4)
}

/// A CPU session for one model file. Blocking (graph optimisation takes a
/// moment on large models) — call from `spawn_blocking`.
pub fn session(path: &Path) -> Result<Session, String> {
    static ENV: OnceLock<Result<ort::environment::Environment, String>> = OnceLock::new();
    let env = ENV
        .get_or_init(|| ort::init().with_name("raw-editor").build().map_err(|e| format!("ONNX Runtime: {e}")))
        .as_ref()
        .map_err(Clone::clone)?;
    let fail = |e: ort::Error| format!("could not load {}: {e}", path.display());
    let mut builder = Session::builder(env)
        .map_err(fail)?
        .with_optimization_level(GraphOptimizationLevel::Level3)
        .map_err(|e| fail(e.into()))?
        .with_intra_threads(threads())
        .map_err(|e| fail(e.into()))?;
    builder.commit_from_file(path).map_err(fail)
}

/// A float tensor from a shape and its row-major data.
pub fn tensor(shape: &[usize], data: Vec<f32>) -> Result<ort::value::Tensor<f32>, String> {
    let shape: Vec<i64> = shape.iter().map(|&d| d as i64).collect();
    ort::value::Tensor::from_array((shape, data)).map_err(|e| format!("tensor: {e}"))
}

/// An output's shape and data, copied out of the session's buffers.
pub fn extract(outputs: &ort::session::SessionOutputs, name: &str) -> Result<(Vec<usize>, Vec<f32>), String> {
    let value = outputs.get(name).ok_or_else(|| format!("model has no output `{name}`"))?;
    let (shape, data) = value.try_extract_tensor::<f32>().map_err(|e| format!("output `{name}`: {e}"))?;
    Ok((shape.iter().map(|&d| d as usize).collect(), data.to_vec()))
}
