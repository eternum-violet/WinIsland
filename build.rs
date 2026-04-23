use std::env;
use std::fs;
use std::path::Path;

fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").unwrap() == "windows" {
        let mut res = winres::WindowsResource::new();

        let icon_path = "resources/icon-dark.ico";
        if std::path::Path::new(icon_path).exists() {
            res.set_icon(icon_path);
        } else {
            println!("cargo:warning=Icon file not found: {}, executable will use default icon", icon_path);
        }

        res.set("CompanyName", "Eatgrapes");
        res.set("FileDescription", "WinIsland");
        res.set("ProductName", "WinIsland");
        res.set("LegalCopyright", "Copyright (c) Eatgrapes");

        let manifest = r#"
<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <trustInfo xmlns="urn:schemas-microsoft-com:asm.v3">
    <security>
      <requestedPrivileges>
        <requestedExecutionLevel level="asInvoker" uiAccess="false"/>
      </requestedPrivileges>
    </security>
  </trustInfo>
</assembly>
"#;
        res.set_manifest(manifest);
        res.compile().unwrap();
    }

    // Copy hook-scripts to target directory
    let out_dir = env::var("OUT_DIR").unwrap();
    let target_dir = Path::new(&out_dir)
        .parent().unwrap()
        .parent().unwrap()
        .parent().unwrap();

    let hook_scripts_src = Path::new("hook-scripts");
    let hook_scripts_dst = target_dir.join("hook-scripts");

    if hook_scripts_src.exists() {
        let _ = fs::create_dir_all(&hook_scripts_dst);
        if let Ok(entries) = fs::read_dir(hook_scripts_src) {
            for entry in entries.flatten() {
                let src_path = entry.path();
                if let Some(file_name) = src_path.file_name() {
                    let dst_path = hook_scripts_dst.join(file_name);
                    let _ = fs::copy(&src_path, &dst_path);
                    println!("cargo:warning=Copied {} to {}", src_path.display(), dst_path.display());
                }
            }
        }
    }
}
