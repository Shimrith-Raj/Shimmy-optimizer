# Shimmy Optimizer Phase 0 spike. Run elevated (one UAC prompt). Writes results\<step>.json per step.
# -Only limits the run to named steps, e.g. -Only sensors,battery
param([string[]]$Only)
$ErrorActionPreference = 'Stop'
$Only = @($Only | ForEach-Object { $_ -split ',' } | Where-Object { $_ })
$here = $PSScriptRoot
$out = Join-Path $here 'results'
New-Item -ItemType Directory -Force $out | Out-Null
$R = [ordered]@{}
function Step([string]$name, [scriptblock]$body) {
  if ($Only -and $name -notin $Only) { return }
  try { $R[$name] = & $body } catch { $R[$name] = @{ error = "$_" } }
  # Saved per step, so one failure can't lose the others.
  try { ConvertTo-Json -InputObject $R[$name] -Depth 4 | Set-Content -LiteralPath "$out\$name.json" -Encoding UTF8 }
  catch { Set-Content -LiteralPath "$out\$name.json" -Value ('"could not serialise: ' + "$_" + '"') }
}

Step 'elevated' {
  ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
}

# 1. PawnIO driver (signed by namazso.eu); installed silently only if missing.
Step 'pawnio' {
  $find = { Get-CimInstance Win32_SystemDriver | Where-Object { $_.Name -match 'pawn' } | Select-Object -First 1 }
  $before = & $find
  $code = $null
  if (-not $before) {
    $p = Start-Process -FilePath "$here\vendor\pawnio\PawnIO_setup.exe" -ArgumentList '-install', '-silent' -Wait -PassThru
    $code = $p.ExitCode
  }
  $after = & $find
  @{ installedBefore = [bool]$before; setupExitCode = $code; driver = $after.Name; state = $after.State; path = $after.PathName }
}

# 2. Sensors through LibreHardwareMonitorLib (CPU temps/clocks/power, GPU, storage, battery).
Step 'sensors' {
  $dir = "$here\vendor\lhm"
  # Resolver in compiled C#: a PowerShell scriptblock resolver re-enters itself and overflowed the stack.
  Add-Type -TypeDefinition @'
using System; using System.IO; using System.Reflection;
public static class ShimmyResolver {
  static string Dir;
  public static void Install(string dir) {
    Dir = dir;
    AppDomain.CurrentDomain.AssemblyResolve += (s, e) => {
      var n = new AssemblyName(e.Name).Name;
      if (n.EndsWith(".resources")) return null;
      var f = Path.Combine(Dir, n + ".dll");
      return File.Exists(f) ? Assembly.LoadFrom(f) : null;
    };
  }
}
'@
  [ShimmyResolver]::Install($dir)
  [void][Reflection.Assembly]::LoadFrom("$dir\LibreHardwareMonitorLib.dll")
  $c = New-Object LibreHardwareMonitor.Hardware.Computer
  $c.IsCpuEnabled = $true; $c.IsGpuEnabled = $true; $c.IsMotherboardEnabled = $true
  $c.IsStorageEnabled = $true; $c.IsBatteryEnabled = $true; $c.IsMemoryEnabled = $true
  $c.Open()
  $all = { foreach ($h in $c.Hardware) { $h; $h.SubHardware } }
  foreach ($pass in 1, 2) { foreach ($h in & $all) { $h.Update() }; Start-Sleep -Milliseconds 1200 }
  $rows = foreach ($h in & $all) {
    foreach ($s in $h.Sensors) {
      [pscustomobject]@{ hw = [string]$h.Name; type = "$($h.HardwareType)"; sensor = [string]$s.Name; kind = "$($s.SensorType)"; value = if ($null -ne $s.Value) { [math]::Round([double]$s.Value, 2) } else { $null } }
    }
  }
  $c.Close()
  @($rows)
}

# 3. Battery wear and cycles.
Step 'battery' {
  $ns = 'root\wmi'
  @{
    designMWh = @(Get-CimInstance -Namespace $ns -ClassName BatteryStaticData)[0].DesignedCapacity
    fullMWh   = @(Get-CimInstance -Namespace $ns -ClassName BatteryFullChargedCapacity)[0].FullChargedCapacity
    cycles    = @(Get-CimInstance -Namespace $ns -ClassName BatteryCycleCount -ErrorAction SilentlyContinue)[0].CycleCount
    status    = @(Get-CimInstance -Namespace $ns -ClassName BatteryStatus)[0] | ForEach-Object { @{ chargeRate = $_.ChargeRate; dischargeRate = $_.DischargeRate; remainingMWh = $_.RemainingCapacity; voltage = $_.Voltage; powerOnline = $_.PowerOnline; charging = $_.Charging } }
  }
}

# 4. ASUS charge limit (ATK WMI device 0x00120057). Reads it, then writes the SAME value back to prove writes work.
Step 'asus' {
  $cls = Get-CimClass -Namespace root\wmi -ClassName AsusAtkWmi_WMNB
  $methods = $cls.CimClassMethods | ForEach-Object { "$($_.Name)(" + (($_.Parameters | ForEach-Object Name) -join ',') + ')' }
  $inst = Get-CimInstance -Namespace root\wmi -ClassName AsusAtkWmi_WMNB | Select-Object -First 1
  $read = Invoke-CimMethod -InputObject $inst -MethodName DSTS -Arguments @{ Device_ID = [uint32]0x00120057 }
  $raw = [uint32]($read.CimInstanceProperties | Where-Object { $_.Name -ne 'ReturnValue' -and $_.Value -is [ValueType] } | Select-Object -First 1).Value
  $present = ($raw -band 0x10000) -ne 0
  $limit = $raw -band 0xFF
  $write = $null
  if ($present -and $limit -ge 20 -and $limit -le 100) {
    $w = Invoke-CimMethod -InputObject $inst -MethodName DEVS -Arguments @{ Device_ID = [uint32]0x00120057; Control_status = [uint32]$limit }
    $write = ($w.CimInstanceProperties | ForEach-Object { "$($_.Name)=$($_.Value)" }) -join '; '
  }
  @{ methods = $methods; raw = ('0x{0:X8}' -f $raw); supported = $present; limitPct = $limit; writeBackSameValue = $write }
}

# 5. Drive health (SMART via Storage module).
Step 'disk' {
  @(Get-PhysicalDisk | ForEach-Object {
    $r = $_ | Get-StorageReliabilityCounter
    @{ name = $_.FriendlyName; health = "$($_.HealthStatus)"; wearPct = $r.Wear; tempC = $r.Temperature; powerOnHours = $r.PowerOnHours; readErrors = $r.ReadErrorsTotal; writeErrors = $r.WriteErrorsTotal }
  })
}

# 6. ACPI thermal zone, for comparison with the real sensor.
Step 'acpiThermal' {
  @(Get-CimInstance -Namespace root\wmi -ClassName MSAcpi_ThermalZoneTemperature | ForEach-Object { [math]::Round($_.CurrentTemperature / 10 - 273.15, 1) })
}

# 7. Silent elevated autostart: a temporary task (highest privileges, allowed on battery) launches the spike exe,
#    which must open its window, write a report and exit. The task is deleted afterwards.
Step 'task' {
  $name = 'Shimmy Spike Elevation Test'
  $exe = "$here\app\src-tauri\target\release\app.exe"
  $rep = "$out\tauri-from-task.json"
  Remove-Item -LiteralPath $rep -ErrorAction SilentlyContinue
  $action = New-ScheduledTaskAction -Execute "`"$exe`"" -Argument "--report `"$rep`""
  $principal = New-ScheduledTaskPrincipal -UserId "$env:USERDOMAIN\$env:USERNAME" -LogonType Interactive -RunLevel Highest
  $settings = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -ExecutionTimeLimit ([TimeSpan]::Zero)
  Register-ScheduledTask -TaskName $name -Action $action -Principal $principal -Settings $settings -Force | Out-Null
  Start-ScheduledTask -TaskName $name
  for ($i = 0; $i -lt 60 -and -not (Test-Path -LiteralPath $rep); $i++) { Start-Sleep -Milliseconds 500 }
  Unregister-ScheduledTask -TaskName $name -Confirm:$false
  @{ reportWritten = (Test-Path -LiteralPath $rep); taskRemoved = -not (Get-ScheduledTask -TaskName $name -ErrorAction SilentlyContinue) }
}

