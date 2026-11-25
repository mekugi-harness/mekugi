fn main() {
    println!("cargo:rerun-if-changed=macos_interpose.c");
    #[cfg(target_os = "macos")]
    {
        let out_dir = std::env::var("OUT_DIR").expect("OUT_DIR");
        let out_dylib = std::path::Path::new(&out_dir).join("libmekugi_write_observer.dylib");
        let status = std::process::Command::new("cc")
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
