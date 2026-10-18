# CPU consommé par l'app Kobi sous Windows : kobi.exe et ses processus
# WebView2 (msedgewebview2.exe : navigateur, rendu, GPU logiciel…), mesuré
# sur le temps processeur cumulé de tout l'arbre.
#
#   powershell -File scripts/cpu-windows.ps1 [-Seconds 60] [-Warmup 5] [-AppPid 1234]
#
# Le pourcentage est donné pour la machine entière (comme le gestionnaire des
# tâches) et pour un cœur. Sans -AppPid, prend le premier kobi.exe trouvé.

param(
  [int]$Seconds = 60,
  [int]$Warmup = 5,
  [int]$AppPid = 0
)

function Get-Tree([int]$Root) {
  $all = Get-CimInstance Win32_Process | Select-Object ProcessId, ParentProcessId, Name
  $set = @{ $Root = $true }
  do {
    $grew = $false
    foreach ($p in $all) {
      if (-not $set.ContainsKey([int]$p.ProcessId) -and $set.ContainsKey([int]$p.ParentProcessId)) {
        $set[[int]$p.ProcessId] = $true
        $grew = $true
      }
    }
  } while ($grew)
  return $set.Keys
}

function Get-CpuSeconds([int[]]$Ids) {
  # Temps noyau + utilisateur lus par WMI : lisibles aussi pour les processus
  # de rendu de WebView2, qui tournent en bac à sable.
  $total = 0.0
  $byName = @{}
  $filter = ($Ids | ForEach-Object { "ProcessId=$_" }) -join ' OR '
  foreach ($p in Get-CimInstance Win32_Process -Filter $filter) {
    $s = ($p.KernelModeTime + $p.UserModeTime) / 1e7
    $total += $s
    $byName[$p.Name] = ($byName[$p.Name] + $s)
  }
  return @{ Total = $total; ByName = $byName }
}

if ($AppPid -eq 0) {
  $k = Get-Process -Name kobi -ErrorAction SilentlyContinue | Select-Object -First 1
  if (-not $k) { Write-Error 'kobi.exe ne tourne pas'; exit 1 }
  $AppPid = $k.Id
}
Start-Sleep -Seconds $Warmup
$ids = Get-Tree $AppPid
$a = Get-CpuSeconds $ids
$t0 = Get-Date
Start-Sleep -Seconds $Seconds
$b = Get-CpuSeconds $ids
$wall = ((Get-Date) - $t0).TotalSeconds
$cores = [Environment]::ProcessorCount
$core = 100 * ($b.Total - $a.Total) / $wall
$detail = ($b.ByName.Keys | ForEach-Object { '{0} {1:N2} %' -f $_, (100 * ($b.ByName[$_] - $a.ByName[$_]) / $wall / $cores) }) -join ', '
'| {0:N0} s | {1} processus | {2:N2} % (machine, {3} cœurs logiques) | {4:N2} % d''un cœur | {5} |' -f $wall, $ids.Count, ($core / $cores), $cores, $core, $detail
