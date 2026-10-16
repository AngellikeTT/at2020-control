<#
 Runs the whole test suite across thread counts and CPU affinity masks, several times
 each, to expose flaky, racy or deadlocking tests.

   powershell -ExecutionPolicy Bypass -File scripts\test-matrix.ps1 [-Runs 5]

 Masks: 0x1 (one P-core), 0x3, 0xF, all logical CPUs, and E-cores only (detected with
 GetSystemCpuSetInformation; skipped on CPUs without efficiency classes).
 Writes test-matrix\<cell>.txt per run and test-matrix\matrix.csv / matrix.md.
#>
param([int]$Runs = 5, [int[]]$Threads = @(), [string[]]$Masks = @())
$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root
if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) { $env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH" }

$logical = [Environment]::ProcessorCount
if ($Threads.Count -eq 0) { $Threads = @(1, 2, 4, $logical) | Select-Object -Unique }

# E-cores: the lowest efficiency class, when the CPU has more than one.
Add-Type @'
using System; using System.Runtime.InteropServices;
public static class CpuSetInfo {
  [DllImport("kernel32.dll", SetLastError=true)]
  public static extern bool GetSystemCpuSetInformation(IntPtr info, uint len, out uint ret, IntPtr proc, uint flags);
}
'@
$len = 0; [void][CpuSetInfo]::GetSystemCpuSetInformation([IntPtr]::Zero, 0, [ref]$len, [IntPtr]::Zero, 0)
$buf = [Runtime.InteropServices.Marshal]::AllocHGlobal([int]$len)
[void][CpuSetInfo]::GetSystemCpuSetInformation($buf, $len, [ref]$len, [IntPtr]::Zero, 0)
$sets = @(); $off = 0
while ($off -lt $len) {
    $size = [Runtime.InteropServices.Marshal]::ReadInt32($buf, $off)
    $sets += [pscustomobject]@{ LP = [Runtime.InteropServices.Marshal]::ReadByte($buf, $off + 14); Eff = [Runtime.InteropServices.Marshal]::ReadByte($buf, $off + 18) }
    $off += $size
}
[Runtime.InteropServices.Marshal]::FreeHGlobal($buf)
$all = [UInt64]0; foreach ($s in $sets) { $all = $all -bor ([UInt64]1 -shl $s.LP) }
$classes = $sets | Select-Object -ExpandProperty Eff -Unique
$cells = [ordered]@{ "0x1" = "1"; "0x3" = "3"; "0xF" = "F"; "all" = ('{0:X}' -f $all) }
if (($classes | Measure-Object).Count -gt 1) {
    $minEff = ($classes | Measure-Object -Minimum).Minimum
    $e = [UInt64]0; foreach ($s in $sets | Where-Object { $_.Eff -eq $minEff }) { $e = $e -bor ([UInt64]1 -shl $s.LP) }
    $cells["E-cores"] = ('{0:X}' -f $e)
}
if ($Masks.Count -gt 0) { $sel = [ordered]@{}; foreach ($k in $Masks) { $sel[$k] = $cells[$k] }; $cells = $sel }

cargo test --release --workspace --no-run 2>&1 | Out-Null
if ($LASTEXITCODE -ne 0) { throw "build failed" }

New-Item -ItemType Directory -Force test-matrix | Out-Null
$csv = "test-matrix\matrix.csv"
"mask,hex,threads,run,result,seconds,passed,failed,failures" | Out-File $csv -Encoding utf8
foreach ($m in $cells.Keys) {
    foreach ($n in $Threads) {
        for ($r = 1; $r -le $Runs; $r++) {
            $name = ("{0}_t{1}_r{2}" -f $m, $n, $r) -replace '[^A-Za-z0-9_-]', ''
            $out = Join-Path $root "test-matrix\$name.txt"
            $t0 = Get-Date
            # start /affinity applies the mask to cargo and every test binary it spawns.
            $cmd = "cargo test --release --workspace -- --test-threads=$n > `"$out`" 2>&1"
            cmd /c "start `"`" /wait /affinity $($cells[$m]) cmd /c `"$cmd`""
            $secs = [math]::Round(((Get-Date) - $t0).TotalSeconds, 1)
            $text = Get-Content $out -Raw
            $passed = ([regex]::Matches($text, 'test result: \w+\. (\d+) passed') | ForEach-Object { [int]$_.Groups[1].Value } | Measure-Object -Sum).Sum
            $failed = ([regex]::Matches($text, '(\d+) failed;') | ForEach-Object { [int]$_.Groups[1].Value } | Measure-Object -Sum).Sum
            $fails = ([regex]::Matches($text, '(?m)^test (\S+) \.\.\. FAILED') | ForEach-Object { $_.Groups[1].Value }) -join ' '
            $res = if ($text -match 'os error 4551') { 'BLOCKED' } elseif ($failed -gt 0 -or $text -match 'error: test failed') { 'FAIL' } elseif ($passed -gt 0) { 'PASS' } else { 'ERROR' }
            "$m,$($cells[$m]),$n,$r,$res,$secs,$passed,$failed,$fails" | Out-File $csv -Append -Encoding utf8
            Write-Host ("{0,-8} t={1,-2} run {2}: {3} ({4} passed, {5} failed, {6}s) {7}" -f $m, $n, $r, $res, $passed, $failed, $secs, $fails)
        }
    }
}

# Markdown matrix: one cell per (mask, threads) with the per-run results.
$rows = Import-Csv $csv
$md = @("| affinity | " + (($Threads | ForEach-Object { "threads=$_" }) -join " | ") + " |",
        "|---|" + (($Threads | ForEach-Object { "---" }) -join "|") + "|")
foreach ($m in $cells.Keys) {
    $line = "| $m (0x$($cells[$m])) |"
    foreach ($n in $Threads) {
        $rs = $rows | Where-Object { $_.mask -eq $m -and $_.threads -eq "$n" } | Sort-Object { [int]$_.run }
        $line += " " + (($rs | ForEach-Object { if ($_.result -eq 'PASS') { 'P' } else { $_.result.Substring(0, 1) } }) -join '') + " |"
    }
    $md += $line
}
$md | Out-File "test-matrix\matrix.md" -Encoding utf8
$md
