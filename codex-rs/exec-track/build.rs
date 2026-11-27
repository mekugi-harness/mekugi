fn main() {
    println!("cargo:rerun-if-changed=macos_interpose.c");
    #[cfg(target_os = "macos")]
    {
        let out_dir = std::env::var("OUT_DIR").expect("OUT_DIR");
        let out_dylib = std::path::Path::new(&out_dir).join("libmekugi_write_observer.dylib");
        let mut cc = std::process::Command::new("cc");
        if std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() == Ok("aarch64") {
            // Descendants inherit the interposer and may run as either arm64 or arm64e.
            cc.args(["-arch", "arm64", "-arch", "arm64e"]);
        }
        let status = cc
            .args([
                "-dynamiclib",
                "-O2",
                "-Wall",
                "-Wextra",
                "-o",
                out_dylib.to_str().expect("utf-8 dylib path"),
                "macos_interpose.c",
            ])
            .status()
            .expect("invoke cc to build libmekugi_write_observer.dylib");
        assert!(
            status.success(),
            "failed to compile macos_interpose.c (exit: {status})"
        );
    }
}
