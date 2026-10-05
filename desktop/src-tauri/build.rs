// 把 cargo 的 TARGET triple 传给主代码（tools.rs 的 sidecar 命名用）
fn main() {
    let target = std::env::var("TARGET").unwrap_or_default();
    println!("cargo:rustc-env=MAESTRO_TARGET={target}");
    tauri_build::build()
}
