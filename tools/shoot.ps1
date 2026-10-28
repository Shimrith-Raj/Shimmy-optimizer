# Dev helper: launches a Shimmy build, screenshots the window, optionally clicks points first.
# Usage: shoot.ps1 -Out <folder> [-Clicks "memory:118,231"] [-Wait 7] [-Target target-dev]
# Click points are window-relative physical pixels. Dev builds (target-dev, identifier com.shimmy.optimizer.dev) run
# beside the real, elevated Shimmy without touching it.
param([string]$Out, [string[]]$Clicks = @(), [int]$Wait = 7, [string]$Target = 'target-dev')
$exe = Join-Path $PSScriptRoot "..\app\src-tauri\$Target\release\shimmy-optimizer.exe"
Add-Type -AssemblyName System.Drawing
Add-Type -Namespace W -Name U -MemberDefinition @'
[DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
[DllImport("user32.dll")] public static extern bool SetForegroundWindow(System.IntPtr h);
[DllImport("user32.dll")] public static extern bool GetWindowRect(System.IntPtr h, out RECT r);
[DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
[DllImport("user32.dll")] public static extern void mouse_event(int f, int x, int y, int d, int e);
public struct RECT { public int L, T, R, B; }
'@
[void][W.U]::SetProcessDPIAware()
Get-Process shimmy-optimizer -ErrorAction SilentlyContinue | Where-Object { $_.Path -like "*\$Target\*" } | Stop-Process -Force -ErrorAction SilentlyContinue
Start-Sleep -Milliseconds 500
$p = Start-Process -FilePath $exe -PassThru
Start-Sleep -Seconds $Wait
$p.Refresh(); $h = $p.MainWindowHandle
function Shot($name) {
  [void][W.U]::SetForegroundWindow($h); Start-Sleep -Milliseconds 600
  $r = New-Object W.U+RECT; [void][W.U]::GetWindowRect($h, [ref]$r)
  $bmp = New-Object System.Drawing.Bitmap ($r.R - $r.L), ($r.B - $r.T)
  [System.Drawing.Graphics]::FromImage($bmp).CopyFromScreen($r.L, $r.T, 0, 0, $bmp.Size)
  $bmp.Save((Join-Path $Out "$name.png")); $bmp.Dispose()
  $r
}
$r = Shot 'home'
foreach ($c in $Clicks) {
  # "name:x,y" clicks then shoots; "name:x,y:secs" waits that long first; name "_" clicks without a shot.
  # "name:x,y:secs:n" turns the mouse wheel n notches there instead of clicking (negative scrolls down).
  $name, $xy, $secs, $wheel = $c -split ':'; $x, $y = $xy -split ','
  [void][W.U]::SetCursorPos($r.L + [int]$x, $r.T + [int]$y)
  if ($wheel) { [W.U]::mouse_event(0x800, 0, 0, 120 * [int]$wheel, 0) }
  else { [W.U]::mouse_event(2, 0, 0, 0, 0); [W.U]::mouse_event(4, 0, 0, 0, 0) }
  Start-Sleep -Seconds ($(if ($secs) { [int]$secs } else { 2 }))
  if ($name -ne '_') { [void](Shot $name) }
}
"pid $($p.Id)"
