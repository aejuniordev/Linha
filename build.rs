//! Embute o "target triple" no executável, para o atualizador saber qual binário baixar.

fn main() {
    let target = std::env::var("TARGET").unwrap_or_default();
    println!("cargo:rustc-env=LINHA_TARGET={target}");
    println!("cargo:rerun-if-changed=build.rs");
}
