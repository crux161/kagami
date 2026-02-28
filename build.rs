use std::env;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    let manifest_dir =
        PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("missing manifest dir"));
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("missing OUT_DIR"));
    let profile = env::var("PROFILE").expect("missing PROFILE");
    let sankaku_dir = manifest_dir.join("dependencies").join("sankaku");
    let nezumi_dir = manifest_dir.join("dependencies").join("nezumi");
    let sankaku_lib = sankaku_dir.join("libsankaku.dylib");
    let nezumi_lib = nezumi_dir.join("libnezumi.dylib");
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();

    if target_os == "macos" {
        remediate_dylib(
            &sankaku_lib,
            "@rpath/libsankaku.dylib",
            &[(&nezumi_lib, "@rpath/libnezumi.dylib")],
        );
        remediate_dylib(
            &nezumi_lib,
            "@rpath/libnezumi.dylib",
            &[(&sankaku_lib, "@rpath/libsankaku.dylib")],
        );

        let profile_dir = profile_output_dir(&out_dir, &profile);
        deploy_runtime_dependencies(
            &profile_dir,
            &[
                (
                    &sankaku_lib,
                    Path::new("dependencies")
                        .join("sankaku")
                        .join("libsankaku.dylib"),
                ),
                (
                    &nezumi_lib,
                    Path::new("dependencies")
                        .join("nezumi")
                        .join("libnezumi.dylib"),
                ),
            ],
        );
    }

    println!("cargo:rustc-link-search=native={}", sankaku_dir.display());
    println!("cargo:rustc-link-search=native={}", nezumi_dir.display());
    println!("cargo:rustc-link-lib=dylib=sankaku");
    println!("cargo:rustc-link-lib=dylib=nezumi");
    if target_os == "macos" {
        println!("cargo:rustc-link-arg=-Wl,-rpath,@loader_path/dependencies/sankaku");
        println!("cargo:rustc-link-arg=-Wl,-rpath,@loader_path/dependencies/nezumi");
    }
    println!("cargo:rerun-if-changed={}", sankaku_lib.display());
    println!("cargo:rerun-if-changed={}", nezumi_lib.display());
    println!(
        "cargo:rerun-if-changed={}",
        manifest_dir.join("build.rs").display()
    );

    slint_build::compile("ui/app-window.slint").expect("Slint build failed");
}

fn remediate_dylib(dylib_path: &Path, install_name: &str, peers: &[(&Path, &str)]) {
    run_command(
        Command::new("install_name_tool")
            .arg("-id")
            .arg(install_name)
            .arg(dylib_path),
        "update dylib id",
    );

    for dependency in dylib_dependencies(dylib_path) {
        for (peer_path, peer_install_name) in peers {
            if matches_dependency(&dependency, peer_path) {
                run_command(
                    Command::new("install_name_tool")
                        .arg("-change")
                        .arg(&dependency)
                        .arg(peer_install_name)
                        .arg(dylib_path),
                    "rewrite dylib dependency",
                );
            }
        }
    }

    run_command(
        Command::new("strip").arg("-x").arg(dylib_path),
        "strip local symbols from dylib",
    );
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

fn deploy_runtime_dependencies(profile_dir: &Path, libraries: &[(&Path, PathBuf)]) {
    let deployment_roots = [profile_dir.to_path_buf(), profile_dir.join("deps")];

    for root in deployment_roots {
        for (source, relative_target) in libraries {
            let destination = root.join(relative_target);
            let parent = destination
                .parent()
                .unwrap_or_else(|| panic!("missing parent for {}", destination.display()));

            fs::create_dir_all(parent)
                .unwrap_or_else(|error| panic!("failed to create {}: {error}", parent.display()));
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

fn dylib_dependencies(dylib_path: &Path) -> Vec<String> {
    let output = Command::new("otool")
        .arg("-L")
        .arg(dylib_path)
        .output()
        .unwrap_or_else(|error| panic!("failed to inspect {}: {error}", dylib_path.display()));

    if !output.status.success() {
        panic!("otool -L failed for {}", dylib_path.display());
    }

    String::from_utf8(output.stdout)
        .expect("otool output was not valid UTF-8")
        .lines()
        .skip(1)
        .filter_map(|line| {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                return None;
            }
            trimmed.split_once(" (").map(|(path, _)| path.to_owned())
        })
        .collect()
}

fn matches_dependency(dependency: &str, dylib_path: &Path) -> bool {
    if dependency == dylib_path.to_string_lossy() {
        return true;
    }

    let Some(file_name) = dylib_path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };

    dependency.ends_with(file_name)
}

fn run_command(command: &mut Command, description: &str) {
    let status = command
        .status()
        .unwrap_or_else(|error| panic!("failed to {description}: {error}"));

    if !status.success() {
        panic!("{description} failed with status {status}");
    }
}
