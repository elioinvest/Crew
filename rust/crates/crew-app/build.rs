//! Puts Crew's icon and version into the Windows executable. GPUI gives the window resource 1 as its
//! icon, and Explorer shows the rest under Properties > Details.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let manifest = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let icon = format!("{manifest}/../../../build/icon.ico").replace('\\', "/");
    println!("cargo:rerun-if-changed={icon}");
    let version = std::env::var("CARGO_PKG_VERSION").unwrap();
    let number = |key: &str| std::env::var(key).unwrap();
    let numeric = format!("{},{},{},0", number("CARGO_PKG_VERSION_MAJOR"), number("CARGO_PKG_VERSION_MINOR"), number("CARGO_PKG_VERSION_PATCH"));
    let rc = format!(
        r#"1 ICON "{icon}"
1 VERSIONINFO
FILEVERSION {numeric}
PRODUCTVERSION {numeric}
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904B0"
    BEGIN
      VALUE "CompanyName", "Crew"
      VALUE "FileDescription", "Crew"
      VALUE "FileVersion", "{version}"
      VALUE "InternalName", "crew"
      VALUE "OriginalFilename", "crew.exe"
      VALUE "ProductName", "Crew"
      VALUE "ProductVersion", "{version}"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x0409, 0x04B0
  END
END
"#
    );
    let path = std::path::Path::new(&std::env::var("OUT_DIR").unwrap()).join("crew.rc");
    std::fs::write(&path, rc).unwrap();
    embed_resource::compile(&path, embed_resource::NONE).manifest_required().unwrap();
}
