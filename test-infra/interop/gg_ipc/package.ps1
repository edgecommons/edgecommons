param(
  [string]$RunId = ("bin-" + (Get-Date -Format "yyyyMMddHHmmss")),
  [string]$Version = ("1.0." + ([int](Get-Date -Format "Hmmss"))),
  [string]$OutputRoot = "",
  [string]$BinaryHex = "000102030a0d1f207f80feff",
  [string]$Langs = "python,java,rust,ts",
  [ValidateSet("gg-binary-matrix", "gg-log-matrix")]
  [string]$Role = "gg-binary-matrix"
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..\..\..")).Path
if ($OutputRoot -eq "") {
  $OutputRoot = Join-Path $repoRoot "build\gg-ipc-binary-interop\$RunId"
}
$outputRootPath = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($OutputRoot)
$repoPrefix = $repoRoot.TrimEnd([System.IO.Path]::DirectorySeparatorChar) + [System.IO.Path]::DirectorySeparatorChar
if (-not $outputRootPath.StartsWith($repoPrefix, [System.StringComparison]::OrdinalIgnoreCase)) {
  throw "Refusing to stage outside repo: $outputRootPath"
}

if (Test-Path -LiteralPath $outputRootPath) {
  $resolved = (Resolve-Path -LiteralPath $outputRootPath).Path
  if (-not $resolved.StartsWith($repoPrefix, [System.StringComparison]::OrdinalIgnoreCase)) {
    throw "Refusing to delete outside repo: $resolved"
  }
  Remove-Item -LiteralPath $resolved -Recurse -Force
}

$recipesDir = Join-Path $outputRootPath "recipes"
$artifactsDir = Join-Path $outputRootPath "artifacts"
$stageDir = Join-Path $outputRootPath "stage"
New-Item -ItemType Directory -Force -Path $recipesDir, $artifactsDir, $stageDir | Out-Null

$components = @{
  python = "com.mbreissi.edgecommons.InteropBinaryPython"
  java = "com.mbreissi.edgecommons.InteropBinaryJava"
  rust = "com.mbreissi.edgecommons.InteropBinaryRust"
  rustpeer = "com.mbreissi.edgecommons.InteropBinaryRustPeer"
  ts = "com.mbreissi.edgecommons.InteropBinaryTs"
}

function Copy-DirectoryContents {
  param([string]$Source, [string]$Destination)
  New-Item -ItemType Directory -Force -Path $Destination | Out-Null
  Copy-Item -Path (Join-Path $Source "*") -Destination $Destination -Recurse -Force
}

function Write-Utf8NoBom {
  param([string]$Path, [string]$Content)
  $encoding = New-Object System.Text.UTF8Encoding($false)
  [System.IO.File]::WriteAllText($Path, $Content, $encoding)
}

function New-ZipArtifact {
  param([string]$ComponentName, [string]$ZipName, [string]$SourcePath)
  $artifactComponentDir = Join-Path $artifactsDir (Join-Path $ComponentName $Version)
  New-Item -ItemType Directory -Force -Path $artifactComponentDir | Out-Null
  $zipPath = Join-Path $artifactComponentDir $ZipName
  if (Test-Path -LiteralPath $zipPath) {
    Remove-Item -LiteralPath $zipPath -Force
  }
  Add-Type -AssemblyName System.IO.Compression
  Add-Type -AssemblyName System.IO.Compression.FileSystem
  $sourceFull = (Resolve-Path -LiteralPath $SourcePath).Path
  $sourceParent = Split-Path -Parent $sourceFull
  $sourceParentPrefix = $sourceParent.TrimEnd('\', '/') + [System.IO.Path]::DirectorySeparatorChar
  $archive = [System.IO.Compression.ZipFile]::Open(
    $zipPath,
    [System.IO.Compression.ZipArchiveMode]::Create)
  try {
    foreach ($file in Get-ChildItem -LiteralPath $sourceFull -Recurse -File) {
      if (-not $file.FullName.StartsWith($sourceParentPrefix, [System.StringComparison]::OrdinalIgnoreCase)) {
        throw "Refusing to archive unexpected path outside source parent: $($file.FullName)"
      }
      $relative = $file.FullName.Substring($sourceParentPrefix.Length)
      $entryName = $relative.Replace('\', '/')
      [System.IO.Compression.ZipFileExtensions]::CreateEntryFromFile(
        $archive,
        $file.FullName,
        $entryName,
        [System.IO.Compression.CompressionLevel]::Optimal) | Out-Null
    }
  } finally {
    $archive.Dispose()
  }
  return $zipPath
}

function New-Recipe {
  param(
    [string]$ComponentName,
    [string]$ZipName,
    [string]$Description,
    [string]$Lifecycle
  )

  $template = @'
---
RecipeFormatVersion: "2020-01-25"
ComponentName: "__COMPONENT__"
ComponentVersion: "__VERSION__"
ComponentDescription: "__DESCRIPTION__"
ComponentPublisher: "edgecommons"
ComponentConfiguration:
  DefaultConfiguration:
    accessControl:
      aws.greengrass.ipc.pubsub:
        "__COMPONENT__:pubsub:1":
          policyDescription: "Allow local IPC pub/sub for binary interop verification."
          operations:
            - "aws.greengrass#PublishToTopic"
            - "aws.greengrass#SubscribeToTopic"
          resources:
            - "*"
Manifests:
  - Platform:
      os: linux
    Artifacts:
      - URI: "s3://BUCKET/__COMPONENT__/__VERSION__/__ZIP__"
        Unarchive: ZIP
    Lifecycle:
__LIFECYCLE__
'@
  $content = $template.
    Replace("__COMPONENT__", $ComponentName).
    Replace("__VERSION__", $Version).
    Replace("__DESCRIPTION__", $Description).
    Replace("__ZIP__", $ZipName).
    Replace("__LIFECYCLE__", $Lifecycle)
  Write-Utf8NoBom -Path (Join-Path $recipesDir "$ComponentName-$Version.yaml") -Content $content
}

$javaJar = Get-ChildItem (Join-Path $repoRoot "libs\java\target") -Filter "edgecommons-*.jar" |
  Where-Object { -not $_.Name.StartsWith("original-") -and -not $_.Name.EndsWith("-sources.jar") -and -not $_.Name.EndsWith("-javadoc.jar") } |
  Sort-Object LastWriteTime -Descending |
  Select-Object -First 1
if ($null -eq $javaJar) {
  throw "No built Java edgecommons jar found under libs/java/target."
}

$rustBinary = Join-Path $repoRoot "build\gg-rust-target\release\interop-rust-node"
if (-not (Test-Path -LiteralPath $rustBinary)) {
  throw "No Linux Rust binary found at $rustBinary. Build it with WSL CARGO_TARGET_DIR=.../core/build/gg-rust-target."
}

$javaStage = Join-Path $stageDir "ggjava"
New-Item -ItemType Directory -Force -Path $javaStage | Out-Null
Copy-Item -LiteralPath $javaJar.FullName -Destination (Join-Path $javaStage "edgecommons.jar") -Force
Copy-Item -Path (Join-Path $repoRoot "test-infra\interop\java_node\out\*.class") -Destination $javaStage -Force
New-ZipArtifact -ComponentName $components.java -ZipName "java.zip" -SourcePath $javaStage | Out-Null

$pythonStage = Join-Path $stageDir "ggpython"
New-Item -ItemType Directory -Force -Path $pythonStage | Out-Null
Copy-Item -LiteralPath (Join-Path $repoRoot "test-infra\interop\python_node.py") -Destination $pythonStage -Force
$pythonLibStage = Join-Path $pythonStage "libs\python"
New-Item -ItemType Directory -Force -Path $pythonLibStage | Out-Null
Copy-Item -LiteralPath (Join-Path $repoRoot "libs\python\edgecommons") -Destination $pythonLibStage -Recurse -Force
Copy-Item -LiteralPath (Join-Path $repoRoot "libs\python\setup.py") -Destination $pythonLibStage -Force
Copy-Item -LiteralPath (Join-Path $repoRoot "libs\python\pyproject.toml") -Destination $pythonLibStage -Force
Copy-Item -LiteralPath (Join-Path $repoRoot "libs\python\README.md") -Destination $pythonLibStage -Force
New-ZipArtifact -ComponentName $components.python -ZipName "python.zip" -SourcePath $pythonStage | Out-Null

$rustStage = Join-Path $stageDir "ggrust"
New-Item -ItemType Directory -Force -Path $rustStage | Out-Null
Copy-Item -LiteralPath $rustBinary -Destination (Join-Path $rustStage "interop-rust-node") -Force
New-ZipArtifact -ComponentName $components.rust -ZipName "rust.zip" -SourcePath $rustStage | Out-Null
New-ZipArtifact -ComponentName $components.rustpeer -ZipName "rust.zip" -SourcePath $rustStage | Out-Null

$tsStage = Join-Path $stageDir "ggts"
$tsLibDist = Join-Path $tsStage "libs\ts\dist"
$tsNodeDist = Join-Path $tsStage "test-infra\interop\ts_node\dist"
New-Item -ItemType Directory -Force -Path $tsLibDist, $tsNodeDist | Out-Null
Copy-DirectoryContents -Source (Join-Path $repoRoot "libs\ts\dist") -Destination $tsLibDist
Copy-Item -LiteralPath (Join-Path $repoRoot "test-infra\interop\ts_node\dist\interop_node.js") -Destination $tsNodeDist -Force
$tsPackage = @'
{
  "private": true,
  "type": "commonjs",
  "dependencies": {
    "ajv": "^8.20.0",
    "aws-iot-device-sdk-v2": "^1.27.0",
    "mqtt": "^5"
  }
}
'@
Write-Utf8NoBom -Path (Join-Path $tsStage "package.json") -Content $tsPackage
New-ZipArtifact -ComponentName $components.ts -ZipName "ts.zip" -SourcePath $tsStage | Out-Null

$envPrefix = "export EDGECOMMONS_GG_READY_LANGS=python,java,rust,rustpeer,ts`n          export EDGECOMMONS_GG_READY_WAIT_SECS=240`n          export EDGECOMMONS_GG_SUBSCRIBE_DELAY_SECS=2`n          export EDGECOMMONS_GG_WAIT_SECS=90"

$pythonLifecycle = @"
      Install:
        Script: |
          PY_WORK="/greengrass/v2/work/$($components.python)"
          PY_SRC="`$PY_WORK/libs-python"
          PY_VENV="`$PY_WORK/venv"
          rm -rf "`$PY_SRC" "`$PY_VENV"
          mkdir -p "`$PY_SRC"
          cp -R "{artifacts:decompressedPath}/python/ggpython/libs/python/." "`$PY_SRC/"
          python3 -m venv "`$PY_VENV"
          "`$PY_VENV/bin/python" -m pip install --upgrade pip
          "`$PY_VENV/bin/python" -m pip install "`$PY_SRC"
      Run:
        Script: |
          $envPrefix
          PY_WORK="/greengrass/v2/work/$($components.python)"
          exec env PYTHONPATH="`$PY_WORK/libs-python" "`$PY_WORK/venv/bin/python" -u "{artifacts:decompressedPath}/python/ggpython/python_node.py" "$Role" "$RunId" "$Langs" "$BinaryHex"
"@

$javaLifecycle = @"
      Run:
        Script: |
          $envPrefix
          exec java -cp "{artifacts:decompressedPath}/java/ggjava/edgecommons.jar:{artifacts:decompressedPath}/java/ggjava" InteropNode "$Role" "$RunId" "$Langs" "$BinaryHex"
"@

$rustLifecycle = @"
      Install:
        Script: "chmod +x {artifacts:decompressedPath}/rust/ggrust/interop-rust-node"
      Run:
        Script: |
          $envPrefix
          exec "{artifacts:decompressedPath}/rust/ggrust/interop-rust-node" "$Role" "$RunId" "$Langs" "$BinaryHex"
"@

$rustPeerLifecycle = @"
      Install:
        Script: "chmod +x {artifacts:decompressedPath}/rust/ggrust/interop-rust-node"
      Run:
        Script: |
          export EDGECOMMONS_GG_READY_LANG=rustpeer
          $envPrefix
          exec "{artifacts:decompressedPath}/rust/ggrust/interop-rust-node" "$Role" "$RunId" "$Langs" "$BinaryHex"
"@

$tsLifecycle = @"
      Install:
        Script: |
          cd "{artifacts:decompressedPath}/ts/ggts"
          npm install --omit=dev --no-audit --no-fund
      Run:
        Script: |
          $envPrefix
          cd "{artifacts:decompressedPath}/ts/ggts"
          exec node test-infra/interop/ts_node/dist/interop_node.js "$Role" "$RunId" "$Langs" "$BinaryHex"
"@

New-Recipe -ComponentName $components.python -ZipName "python.zip" -Description "Python Greengrass IPC binary message interop verifier." -Lifecycle $pythonLifecycle
New-Recipe -ComponentName $components.java -ZipName "java.zip" -Description "Java Greengrass IPC binary message interop verifier." -Lifecycle $javaLifecycle
New-Recipe -ComponentName $components.rust -ZipName "rust.zip" -Description "Rust Greengrass IPC binary message interop verifier." -Lifecycle $rustLifecycle
New-Recipe -ComponentName $components.rustpeer -ZipName "rust.zip" -Description "Rust peer Greengrass IPC binary message interop verifier." -Lifecycle $rustPeerLifecycle
New-Recipe -ComponentName $components.ts -ZipName "ts.zip" -Description "TypeScript Greengrass IPC binary message interop verifier." -Lifecycle $tsLifecycle

[pscustomobject]@{
  RunId = $RunId
  Version = $Version
  OutputRoot = $outputRootPath
  RecipeDir = $recipesDir
  ArtifactDir = $artifactsDir
  BinaryHex = $BinaryHex
  Langs = $Langs
  Role = $Role
}
