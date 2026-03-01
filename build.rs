use std::env;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

fn main() {
    let manifest_dir =
        PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("missing manifest dir"));
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("missing OUT_DIR"));
    let profile = env::var("PROFILE").expect("missing PROFILE");
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();

    if target_os == "windows" {
        let sankaku_dir = manifest_dir.join("core").join("sankaku");
        let nezumi_dir = manifest_dir.join("core").join("nezumi");
        let sankaku_import_lib = sankaku_dir.join("sankaku.dll.lib");
        let nezumi_import_lib = nezumi_dir.join("nezumi.dll.lib");
        let sankaku_dll = sankaku_dir.join("sankaku.dll");
        let nezumi_dll = nezumi_dir.join("nezumi.dll");

        ensure_exists(&sankaku_import_lib);
        ensure_exists(&nezumi_import_lib);
        ensure_exists(&sankaku_dll);
        ensure_exists(&nezumi_dll);

        println!("cargo:rustc-link-search=native={}", sankaku_dir.display());
        println!("cargo:rustc-link-search=native={}", nezumi_dir.display());
        println!("cargo:rustc-link-lib=dylib=sankaku");
        println!("cargo:rustc-link-lib=dylib=nezumi");

        let profile_dir = profile_output_dir(&out_dir, &profile);
        deploy_runtime_dependencies(
            &profile_dir,
            &[(&sankaku_dll, "sankaku.dll"), (&nezumi_dll, "nezumi.dll")],
        );

        println!("cargo:rerun-if-changed={}", sankaku_import_lib.display());
        println!("cargo:rerun-if-changed={}", nezumi_import_lib.display());
        println!("cargo:rerun-if-changed={}", sankaku_dll.display());
        println!("cargo:rerun-if-changed={}", nezumi_dll.display());
    }

    println!(
        "cargo:rerun-if-changed={}",
        manifest_dir.join("build.rs").display()
    );

    slint_build::compile("ui/app-window.slint").expect("Slint build failed");
}

fn ensure_exists(path: &Path) {
    if !path.exists() {
        panic!("required artifact is missing: {}", path.display());
    }
}

fn profile_output_dir(out_dir: &Path, profile: &str) -> PathBuf {
    out_dir
        .ancestors()
        .find(|path| path.file_name() == Some(OsStr::new(profile)))
        .unwrap_or_else(|| {
            panic!(
                "failed to locate profile directory for {}",
                out_dir.display()
            )
        })
        .to_path_buf()
}

fn deploy_runtime_dependencies(profile_dir: &Path, libraries: &[(&Path, &str)]) {
    let deployment_roots = [profile_dir.to_path_buf(), profile_dir.join("deps")];

    for root in deployment_roots {
        fs::create_dir_all(&root)
            .unwrap_or_else(|error| panic!("failed to create {}: {error}", root.display()));

        for (source, file_name) in libraries {
            let destination = root.join(file_name);
            fs::copy(source, &destination).unwrap_or_else(|error| {
                panic!(
                    "failed to copy {} to {}: {error}",
                    source.display(),
                    destination.display()
                )
            });
        }
    }
}
