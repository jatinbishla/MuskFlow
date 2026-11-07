//! Linux: find sherpa-onnx's shared libraries (copied next to the binary by its build
//! script) through an `$ORIGIN` rpath. Its own `rustc-link-arg` only reaches its own crate.
fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("linux") {
        println!("cargo:rustc-link-arg-bins=-Wl,-rpath,$ORIGIN");
    }
}
