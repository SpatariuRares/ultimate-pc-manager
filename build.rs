//! Embeds the app icon and the version details into the Windows executable,
//! so Explorer, the Start menu and the taskbar show the brand's icon.

fn main() {
    println!("cargo:rerun-if-changed=assets/brand/icon.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let mut res = winresource::WindowsResource::new();
    res.set_icon("assets/brand/icon.ico");
    res.set("ProductName", "Ultimate PC Manager");
    res.set("FileDescription", "Ultimate PC Manager");
    res.set("OriginalFilename", "Ultimate PC Manager.exe");
    res.compile().expect("could not embed the Windows icon (is the Windows SDK installed?)");
}
