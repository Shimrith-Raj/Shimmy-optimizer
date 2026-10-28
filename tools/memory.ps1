# Dev helper: memory of a running Shimmy and everything it started (window engine, sensor sidecar).
# Usage: memory.ps1 [-Target target-dev]   ("installed" measures the copy in Program Files)
param([string]$Target = 'target-dev')
$all = Get-CimInstance Win32_Process
$roots = $all | Where-Object { $_.Name -eq 'shimmy-optimizer.exe' -and ($Target -eq 'installed' -and $_.ExecutablePath -eq $null -or $_.ExecutablePath -like "*\$Target\*") }
if ($Target -eq 'installed') { $roots = $all | Where-Object { $_.Name -eq 'shimmy-optimizer.exe' -and ($_.ExecutablePath -eq $null -or $_.ExecutablePath -like '*\Program Files\*') } }
$ids = @($roots.ProcessId | Where-Object { $_ })
do {
  $kids = @($all | Where-Object { $ids -contains $_.ParentProcessId -and $ids -notcontains $_.ProcessId })
  $ids += @($kids.ProcessId | Where-Object { $_ })
} while ($kids.Count)
$rows = foreach ($id in $ids) {
  $p = Get-Process -Id $id -ErrorAction SilentlyContinue
  if (-not $p) { continue }
  $c = $all | Where-Object ProcessId -eq $id
  $type = if ($c.CommandLine -match '--type=(\S+)') { $matches[1] } else { '' }
  [pscustomobject]@{ Pid = $id; Name = $c.Name; Type = $type; WS_MB = [math]::Round($p.WorkingSet64 / 1MB); Private_MB = [math]::Round($p.PrivateMemorySize64 / 1MB) }
}
$rows | Format-Table -AutoSize | Out-String
"Total: $(($rows | Measure-Object WS_MB -Sum).Sum) MB in RAM (working set), $(($rows | Measure-Object Private_MB -Sum).Sum) MB private"
