fn main() {
  sensors_sidecar();
  // Shimmy always runs elevated (PLAN.md). SHIMMY_NO_ADMIN=1 builds without the admin manifest, for tests and
  // non-elevated development runs only.
  println!("cargo:rerun-if-env-changed=SHIMMY_NO_ADMIN");
  let mut attrs = tauri_build::Attributes::new();
  if std::env::var_os("SHIMMY_NO_ADMIN").is_none() {
    attrs = attrs.windows_attributes(tauri_build::WindowsAttributes::new().app_manifest(include_str!("app.manifest")));
  }
  tauri_build::try_build(attrs).expect("failed to run build script");
}

/// The sensor sidecar is C#, compiled with the csc.exe that ships with .NET Framework 4, so no SDK is needed (PLAN.md,
/// Phase 8). The exe sits next to LibreHardwareMonitorLib.dll and its dependencies in resources/sensors.
fn sensors_sidecar() {
  println!("cargo:rerun-if-changed=resources/sensors/shimmy-sensors.cs");
  let fw = r"C:\Windows\Microsoft.NET\Framework64\v4.0.30319";
  let status = std::process::Command::new(format!(r"{fw}\csc.exe"))
    .current_dir("resources/sensors")
    .args(["-nologo", "-target:winexe", "-platform:x64", "-optimize+", "-out:shimmy-sensors.exe", "-r:LibreHardwareMonitorLib.dll"])
    .arg(format!(r"-r:{fw}\netstandard.dll"))
    .arg("shimmy-sensors.cs")
    .status()
    .expect("could not run csc.exe from .NET Framework 4");
  assert!(status.success(), "the sensor sidecar did not compile");
}
