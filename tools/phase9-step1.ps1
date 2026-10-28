# Phase 9 Step 1: reversible firmware test. Fan modes via ATK DEVS 0x00110019, then boost off via PERFBOOSTMODE.
# Always puts the fan mode back to Standard (0) and boost back to the saved AC/DC values.
$ErrorActionPreference = 'Stop'
Add-Type -TypeDefinition @'
using System; using System.Runtime.InteropServices;
public static class Atk {
  [DllImport("kernel32.dll", SetLastError=true, CharSet=CharSet.Unicode)] static extern IntPtr CreateFileW(string n, uint a, uint s, IntPtr sa, uint c, uint f, IntPtr t);
  [DllImport("kernel32.dll", SetLastError=true)] static extern bool DeviceIoControl(IntPtr h, uint code, byte[] i, uint il, byte[] o, uint ol, out uint got, IntPtr ov);
  [DllImport("kernel32.dll")] static extern bool CloseHandle(IntPtr h);
  static long Call(uint fn, uint dev, uint val, bool withVal) {
    IntPtr h = CreateFileW(@"\\.\ATKACPI", 0xC0000000, 3, IntPtr.Zero, 3, 0x80, IntPtr.Zero);
    if (h == new IntPtr(-1)) return -1;
    byte[] i = new byte[16]; BitConverter.GetBytes(fn).CopyTo(i,0); BitConverter.GetBytes(8u).CopyTo(i,4); BitConverter.GetBytes(dev).CopyTo(i,8);
    if (withVal) BitConverter.GetBytes(val).CopyTo(i,12);
    byte[] o = new byte[16]; uint got;
    bool ok = DeviceIoControl(h, 0x0022240C, i, 16, o, 16, out got, IntPtr.Zero); CloseHandle(h);
    return ok ? (long)BitConverter.ToUInt32(o,0) : -1L;
  }
  public static long Get(uint dev) { return Call(0x53545344, dev, 0, false); }
  public static long Set(uint dev, uint v) { return Call(0x53564544, dev, v, true); }
}
'@
$MODE = [uint32]0x00110019; $FAN = [uint32]0x00110013
$sidecar = "C:\Users\Studio RedPix\Desktop\Shimmy Optimizer\app\src-tauri\resources\sensors\shimmy-sensors.exe"
$SUB = 'SUB_PROCESSOR'; $BOOST = 'PERFBOOSTMODE'

function Sensors {
  $p = New-Object System.Diagnostics.Process
  $p.StartInfo.FileName = $sidecar; $p.StartInfo.Arguments = '1500'
  $p.StartInfo.UseShellExecute = $false; $p.StartInfo.RedirectStandardOutput = $true; $p.StartInfo.CreateNoWindow = $true
  [void]$p.Start(); [void]$p.StandardOutput.ReadLine(); $line = $p.StandardOutput.ReadLine(); $p.Kill()
  $rows = $line | ConvertFrom-Json | Where-Object { $_.type -eq 'Cpu' }
  $pick = { param($k, $n) ($rows | Where-Object { $_.kind -eq $k -and $_.sensor -like $n } | Select-Object -First 1).value }
  $clocks = $rows | Where-Object { $_.kind -eq 'Clock' -and $_.sensor -like 'Core #*' } | ForEach-Object { $_.value }
  [pscustomobject]@{
    TempC = & $pick 'Temperature' 'Core (Tctl*'
    PkgW = & $pick 'Power' 'Package'
    AvgMHz = if ($clocks) { [math]::Round(($clocks | Measure-Object -Average).Average) } else { $null }
    FanRPM = (([Atk]::Get($FAN)) -band 0xFFFF) * 100
  }
}
function Load([int]$secs) {
  $jobs = 1..16 | ForEach-Object { Start-Job { $e = (Get-Date).AddSeconds($using:secs); while ((Get-Date) -lt $e) { $x = 1; for ($i = 0; $i -lt 100000; $i++) { $x = $x * 1.0000001 } } } }
  Start-Sleep -Seconds ($secs - 6)
  $s = Sensors
  $jobs | Stop-Job -PassThru | Remove-Job -Force
  $s
}
function BoostNow { $q = powercfg /qh SCHEME_CURRENT $SUB $BOOST | Select-String 'Current (AC|DC)'; $q | ForEach-Object { [Convert]::ToInt32(($_ -split ': ')[1], 16) } }

$origBoost = BoostNow
"Start: mode DSTS = 0x{0:X8}, boost AC/DC = {1}" -f [Atk]::Get($MODE), ($origBoost -join '/')
"Idle: " + (Sensors | Out-String).Trim()
try {
  foreach ($m in 2, 1, 0) {
    $r = [Atk]::Set($MODE, [uint32]$m)
    Start-Sleep -Seconds 3
    $back = [Atk]::Get($MODE)
    $s = Load 45
    "Mode {0}: DEVS answered {1}, read back 0x{2:X8} | {3:N1} C, {4:N1} W, {5} MHz, fan {6} RPM" -f $m, $r, $back, $s.TempC, $s.PkgW, $s.AvgMHz, $s.FanRPM
    Start-Sleep -Seconds 20
  }
  powercfg /setacvalueindex SCHEME_CURRENT $SUB $BOOST 0; powercfg /setdcvalueindex SCHEME_CURRENT $SUB $BOOST 0; powercfg /setactive SCHEME_CURRENT
  $s = Load 45
  "Boost off (now {0}): {1:N1} C, {2:N1} W, {3} MHz, fan {4} RPM" -f ((BoostNow) -join '/'), $s.TempC, $s.PkgW, $s.AvgMHz, $s.FanRPM
}
finally {
  [void][Atk]::Set($MODE, 0)
  powercfg /setacvalueindex SCHEME_CURRENT $SUB $BOOST $origBoost[0]; powercfg /setdcvalueindex SCHEME_CURRENT $SUB $BOOST $origBoost[1]; powercfg /setactive SCHEME_CURRENT
  Get-Job | Remove-Job -Force -ErrorAction SilentlyContinue
  "Restored: mode DSTS = 0x{0:X8}, boost AC/DC = {1}" -f [Atk]::Get($MODE), ((BoostNow) -join '/')
}
