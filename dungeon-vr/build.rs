use std::env;
use std::fs::create_dir_all;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use anyhow::{bail, Result};

const GLSL_SOURCES: &[&str] = &[
    "untextured.vert",
    "untextured.frag",
    "textured.vert",
    "textured.frag",
];

fn main() {
    let src_dir: PathBuf = ["shaders".to_string()].into_iter().collect();
    let dst_dir: PathBuf = [env::var("OUT_DIR").unwrap(), "shaders".to_string()]
        .into_iter()
        .collect();

    create_dir_all(&dst_dir).unwrap();

    for &name in GLSL_SOURCES {
        let mut src: PathBuf = src_dir.clone();
        src.push(name);
        let mut dst: PathBuf = dst_dir.clone();
        dst.push(format!("{}.spv", name));
        eprintln!("src: {:?}, dst: {:?}", src, dst);
        compile_glsl(src.to_str().unwrap(), dst.to_str().unwrap()).unwrap();
    }
}

fn compile_glsl(src: &str, dst: &str) -> Result<()> {
    println!("cargo:rerun-if-changed={}", src);

    let output = Command::new("glslangvalidator")
        .args(["-V", "-o", dst, src])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?
        .wait_with_output()?;

    if !output.status.success() || !output.stderr.is_empty() {
        bail!(
            "status={}\nstdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8(output.stdout)?,
            String::from_utf8(output.stderr)?,
        )
    }
    Ok(())
}
