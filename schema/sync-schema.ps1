# Propagate the canonical EdgeCommons config schema into each language library.
#
# The canonical schema lives at schema/edgecommons-config-schema.json and is the SINGLE
# SOURCE OF TRUTH. Each lib embeds/loads its own copy (cargo include_str!, tsc import,
# Python package-data, Java classpath resource), so the canonical file must be copied in.
#
#   .\schema\sync-schema.ps1           # copy canonical -> all per-lib copies
#   .\schema\sync-schema.ps1 -Check    # verify copies match (CI drift gate; nonzero on drift)

param([switch]$Check)

$ErrorActionPreference = 'Stop'
$root  = Split-Path -Parent $PSScriptRoot
$canon = Join-Path $root 'schema\edgecommons-config-schema.json'

# canonical -> destination copies (TS uses a different filename)
$targets = @(
  'libs\rust\resources\edgecommons-config-schema.json',
  'libs\ts\src\config\schema.json',
  'libs\python\edgecommons\resources\edgecommons-config-schema.json',
  'libs\java\src\main\resources\edgecommons-config-schema.json',
  'libs\java\doc\edgecommons-config-schema.json'
) | ForEach-Object { Join-Path $root $_ }

if (-not (Test-Path $canon)) {
  Write-Error "canonical schema not found: $canon"
}

$canonText = Get-Content -Raw -Path $canon
$drift = $false

foreach ($dst in $targets) {
  $rel = $dst.Substring($root.Length + 1)
  if ($Check) {
    $same = (Test-Path $dst) -and ((Get-Content -Raw -Path $dst) -eq $canonText)
    if ($same) { Write-Host "ok:    $rel" }
    else       { Write-Host "DRIFT: $rel differs from canonical schema"; $drift = $true }
  } else {
    New-Item -ItemType Directory -Force -Path (Split-Path -Parent $dst) | Out-Null
    # write without BOM, preserving exact canonical bytes
    [System.IO.File]::WriteAllText($dst, $canonText)
    Write-Host "synced: $rel"
  }
}

if ($Check -and $drift) {
  Write-Host ""
  Write-Host "Config schema copies are out of sync with schema/edgecommons-config-schema.json." -ForegroundColor Red
  Write-Host "Run .\schema\sync-schema.ps1 and commit the result." -ForegroundColor Red
  exit 1
}

if ($Check) { Write-Host "All schema copies match the canonical source." } else { Write-Host "Done." }
