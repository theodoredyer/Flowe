fn main() {
    println!("cargo:rerun-if-changed=assets/fleow.ico");
    let mut res = winresource::WindowsResource::new();
    res.set_icon("assets/fleow.ico"); // resource id 1, used for the window + taskbar too
    // Common Controls v6 = modern-looking buttons/list instead of Windows 95 ones.
    res.set_manifest(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <dependency><dependentAssembly>
    <assemblyIdentity type="win32" name="Microsoft.Windows.Common-Controls" version="6.0.0.0"
      processorArchitecture="*" publicKeyToken="6595b64144ccf1df" language="*"/>
  </dependentAssembly></dependency>
</assembly>"#,
    );
    res.compile().expect("embed icon/manifest (needs rc.exe from the Windows SDK)");
}
