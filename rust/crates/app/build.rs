fn main() {
    // Only when building for Windows; the icon and version details go into the exe's resources.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/icon.ico")
            .set("ProductName", "Swiftcord")
            .set("FileDescription", "Swiftcord")
            .set("LegalCopyright", "GPL-3.0-or-later");
        if let Err(e) = res.compile() {
            println!("cargo:warning=couldn't embed the icon: {e}");
        }
        compile_shaders();
    }
    println!("cargo:rerun-if-changed=assets/icon.ico");
}

/// Compiles the renderer's HLSL to bytecode now, so the app never loads a shader compiler.
#[cfg(windows)]
fn compile_shaders() {
    use windows::Win32::Graphics::Direct3D::Fxc::{D3DCOMPILE_OPTIMIZATION_LEVEL3, D3DCompile};
    use windows::Win32::Graphics::Direct3D::ID3DBlob;
    use windows::core::s;

    const SOURCE: &str = "src/platform/egui.hlsl";
    println!("cargo:rerun-if-changed={SOURCE}");
    let hlsl = std::fs::read(SOURCE).expect("shader source");
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let blob_bytes = |b: &ID3DBlob| unsafe { std::slice::from_raw_parts(b.GetBufferPointer() as *const u8, b.GetBufferSize()).to_vec() };
    for (entry, target, file) in [(s!("vs_main"), s!("vs_4_0"), "egui_vs.cso"), (s!("ps_main"), s!("ps_4_0"), "egui_ps.cso")] {
        let (mut code, mut errors) = (None, None);
        let result = unsafe {
            D3DCompile(
                hlsl.as_ptr().cast(),
                hlsl.len(),
                s!("egui.hlsl"),
                None,
                None,
                entry,
                target,
                D3DCOMPILE_OPTIMIZATION_LEVEL3,
                0,
                &mut code,
                Some(&mut errors),
            )
        };
        if let Err(e) = result {
            let detail = errors.as_ref().map(|b| String::from_utf8_lossy(&blob_bytes(b)).into_owned()).unwrap_or_default();
            panic!("compiling {file} failed: {e}\n{detail}");
        }
        std::fs::write(out.join(file), blob_bytes(code.as_ref().unwrap())).expect("write shader");
    }
}

#[cfg(not(windows))]
fn compile_shaders() {
    panic!("the shaders are compiled with Windows' D3DCompile, so the app has to be built on Windows");
}
