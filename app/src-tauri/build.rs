fn main() {
    // unicorn (QEMU) uses 16-byte atomics that need libatomic on Linux.
    // rustc-link-arg reaches every target (bin, examples, tests); rustc-link-lib reached only the bin.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("linux") {
        println!("cargo:rustc-link-arg=-latomic");
    }
    tauri_build::build()
}
