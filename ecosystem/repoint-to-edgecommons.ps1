#!/usr/bin/env pwsh
# Repoint ggcommons package coordinates from the personal `mbreissi` GitHub owner to the
# `edgecommons` org. See ecosystem/RUNBOOK.md (Phase 1b).
#
# DRY RUN by default — prints every file that would change. Add -Apply to write.
#
# SAFE BY DESIGN: it only rewrites GitHub-OWNER references. The three rules below do NOT match:
#   - the Java groupId / package `com.mbreissi`   (independent of the GH Packages owner)
#   - the docs domain `docs.ggcommons.mbreissi.com`
#
# -KeepAddonScope leaves the PUBLIC-npm streaming addon `@mbreissi/ggstreamlog-node` on @mbreissi
# (its npm scope is independent of the GitHub owner); the library `@mbreissi/ggcommons` is renamed
# either way.

[CmdletBinding()]
param(
    [switch]$Apply,
    [switch]$KeepAddonScope
)

$ErrorActionPreference = 'Stop'
$root = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path

# Directories never touched (heavy/generated + the docs that describe this mapping).
$skipDirs = @('.git', 'node_modules', 'target', 'build', 'dist', '.venv', 'venvs', '.idea', '.pytest_cache', 'ecosystem')
# Files never touched (they document before -> after literally).
$skipFiles = @('docs\ECOSYSTEM.md')

# Only these file types, plus exact-name dotfiles / well-known names.
$exts = @('.java', '.py', '.ts', '.tsx', '.js', '.mjs', '.cjs', '.json', '.toml', '.xml',
          '.yml', '.yaml', '.md', '.mdx', '.sh', '.ps1', '.cfg', '.ini', '.gradle', '.properties')
$names = @('.npmrc', 'Dockerfile', 'recipe.yaml', 'gdk-config.json')

function Test-SkipDir($full) {
    foreach ($d in $skipDirs) {
        if ($full -match "[\\/]$([regex]::Escape($d))[\\/]") { return $true }
    }
    return $false
}

$files = Get-ChildItem -LiteralPath $root -Recurse -File | Where-Object {
    -not (Test-SkipDir $_.FullName) -and
    ($exts -contains $_.Extension -or $names -contains $_.Name)
}

$utf8NoBom = New-Object System.Text.UTF8Encoding($false)
$changed = 0

foreach ($f in $files) {
    $rel = $f.FullName.Substring($root.Length + 1)
    if ($skipFiles -contains $rel) { continue }

    $text = Get-Content -LiteralPath $f.FullName -Raw -ErrorAction SilentlyContinue
    if ($null -eq $text) { continue }
    $orig = $text

    # Rule 1 + 2: GitHub owner in URLs (covers maven.pkg / npm.pkg / ghcr too).
    $text = $text.Replace('github.com/mbreissi/', 'github.com/edgecommons/')
    $text = $text.Replace('ghcr.io/mbreissi/', 'ghcr.io/edgecommons/')

    # Rule 3: npm scope.
    if ($KeepAddonScope) {
        $text = $text.Replace('@mbreissi/ggcommons', '@edgecommons/ggcommons')
        $text = $text.Replace('@mbreissi:registry', '@edgecommons:registry')
        $text = $text.Replace('scope: "@mbreissi"', 'scope: "@edgecommons"')
        $text = $text.Replace('"@mbreissi"', '"@edgecommons"')
    }
    else {
        $text = $text.Replace('@mbreissi', '@edgecommons')
    }

    if ($text -ne $orig) {
        $changed++
        Write-Host "  $rel" -ForegroundColor Yellow
        if ($Apply) { [System.IO.File]::WriteAllText($f.FullName, $text, $utf8NoBom) }
    }
}

Write-Host ""
if ($Apply) {
    Write-Host "APPLIED: rewrote $changed file(s)." -ForegroundColor Cyan
    Write-Host "Next: npm install (regenerate the lock), then rebuild + test all four libs + the CLI."
}
else {
    Write-Host "DRY RUN: $changed file(s) would change. No files written." -ForegroundColor Cyan
    Write-Host "Re-run with -Apply (add -KeepAddonScope to leave @mbreissi/ggstreamlog-node)."
}
