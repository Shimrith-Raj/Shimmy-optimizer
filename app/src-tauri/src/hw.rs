//! System info page: what is inside this PC, read once from Windows' hardware inventory (CIM, through one hidden
//! PowerShell call) and the registry. Read only. Serial numbers are not read, so the Copy text is safe to paste into a
//! bug report.
use serde_json::Value;

const SCRIPT: &str = r#"
$ErrorActionPreference = 'SilentlyContinue'
$vram = @{}
Get-ChildItem 'HKLM:\SYSTEM\CurrentControlSet\Control\Class\{4d36e968-e325-11ce-bfc1-08002be10318}' | ForEach-Object {
  $p = Get-ItemProperty $_.PSPath
  if ($p.DriverDesc -and $p.'HardwareInformation.qwMemorySize') { $vram[$p.DriverDesc] = [uint64]$p.'HardwareInformation.qwMemorySize' }
}
$day = { param($d) if ($d) { $d.ToString('yyyy-MM-dd') } }
$nt = Get-ItemProperty 'HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion'
[ordered]@{
  system = Get-CimInstance Win32_ComputerSystem | Select-Object Manufacturer, Model, SystemType, TotalPhysicalMemory
  board = Get-CimInstance Win32_BaseBoard | Select-Object Manufacturer, Product, Version
  bios = Get-CimInstance Win32_BIOS | Select-Object Manufacturer, SMBIOSBIOSVersion, @{n='ReleaseDate'; e={ & $day $_.ReleaseDate }}
  cpu = @(Get-CimInstance Win32_Processor | Select-Object Name, NumberOfCores, NumberOfLogicalProcessors, MaxClockSpeed, L2CacheSize, L3CacheSize, SocketDesignation)
  ram = @(Get-CimInstance Win32_PhysicalMemory | Select-Object Manufacturer, PartNumber, Capacity, Speed, ConfiguredClockSpeed, DeviceLocator, SMBIOSMemoryType)
  slots = [int](Get-CimInstance Win32_PhysicalMemoryArray | Measure-Object MemoryDevices -Sum).Sum
  gpu = @(Get-CimInstance Win32_VideoController | Select-Object Name, DriverVersion, @{n='DriverDate'; e={ & $day $_.DriverDate }}, @{n='Memory'; e={ $vram[$_.Name] }}, CurrentHorizontalResolution, CurrentVerticalResolution, CurrentRefreshRate)
  disks = @(Get-PhysicalDisk | Select-Object FriendlyName, @{n='MediaType'; e={ "$($_.MediaType)" }}, @{n='BusType'; e={ "$($_.BusType)" }}, Size)
  os = Get-CimInstance Win32_OperatingSystem | Select-Object Caption, Version, BuildNumber, OSArchitecture, @{n='InstallDate'; e={ & $day $_.InstallDate }}, @{n='LastBoot'; e={ $_.LastBootUpTime.ToString('yyyy-MM-dd HH:mm') }}
  release = [ordered]@{ DisplayVersion = $nt.DisplayVersion; UBR = $nt.UBR }
} | ConvertTo-Json -Depth 4 -Compress
"#;

/// Everything the System info page shows. Takes a second or two, so the caller keeps the answer.
pub fn read() -> Result<Value, String> {
  let modules = [crate::clean::PS_BASE[0], crate::clean::PS_BASE[1], "CimCmdlets", "Storage"];
  let out = crate::clean::powershell(&modules, SCRIPT)
    .map_err(|e| format!("Could not ask Windows: {e}"))?;
  let text = String::from_utf8_lossy(&out.stdout);
  serde_json::from_str(text.trim()).map_err(|_| "Windows' hardware inventory gave an answer Shimmy could not read.".into())
}

#[cfg(test)]
mod tests {
  use crate::clean::ps_script;

  #[test]
  fn no_module_is_found_by_name() {
    let s = ps_script(&["CimCmdlets", "Storage"], "Get-PhysicalDisk");
    assert!(s.starts_with("$PSModuleAutoLoadingPreference='None'\n"), "auto-loading goes off before anything else: {s}");
    assert!(s.contains(r#"foreach ($m in 'CimCmdlets','Storage') { Import-Module "$PSHOME\Modules\$m" -ErrorAction Stop }"#), "{s}");
    assert!(s.ends_with("\nGet-PhysicalDisk"));
  }
}
