param(
  [string]$RunId = ("hierarchical-" + (Get-Date -Format "yyyyMMddHHmmss")),
  [string]$Version = ("1.0." + ([int](Get-Date -Format "Hmmss"))),
  [string]$OutputRoot = ""
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..\..\..")).Path
$orgRoot = (Resolve-Path (Join-Path $repoRoot "..")).Path
$configComponentRoot = Join-Path $orgRoot "config-component"

if ($OutputRoot -eq "") {
  $OutputRoot = Join-Path $repoRoot "build\gg-hierarchical-config-interop\$RunId"
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

function Write-Utf8NoBom {
  param([string]$Path, [string]$Content)
  $encoding = New-Object System.Text.UTF8Encoding($false)
  [System.IO.File]::WriteAllText($Path, $Content, $encoding)
}

function Copy-DirectoryContents {
  param([string]$Source, [string]$Destination)
  New-Item -ItemType Directory -Force -Path $Destination | Out-Null
  Copy-Item -Path (Join-Path $Source "*") -Destination $Destination -Recurse -Force
}

function Copy-Artifact {
  param([string]$ComponentName, [string]$Source, [string]$Name)
  $destDir = Join-Path $artifactsDir (Join-Path $ComponentName $Version)
  New-Item -ItemType Directory -Force -Path $destDir | Out-Null
  Copy-Item -LiteralPath $Source -Destination (Join-Path $destDir $Name) -Force
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
  $archive = [System.IO.Compression.ZipFile]::Open($zipPath, [System.IO.Compression.ZipArchiveMode]::Create)
  try {
    foreach ($file in Get-ChildItem -LiteralPath $sourceFull -Recurse -File) {
      $sourcePrefix = $sourceFull.TrimEnd('\', '/') + [System.IO.Path]::DirectorySeparatorChar
      if (-not $file.FullName.StartsWith($sourcePrefix, [System.StringComparison]::OrdinalIgnoreCase)) {
        throw "Refusing to archive unexpected path outside source parent: $($file.FullName)"
      }
      $relative = $file.FullName.Substring($sourcePrefix.Length).Replace('\', '/')
      [System.IO.Compression.ZipFileExtensions]::CreateEntryFromFile(
        $archive,
        $file.FullName,
        $relative,
        [System.IO.Compression.CompressionLevel]::Optimal) | Out-Null
    }
  } finally {
    $archive.Dispose()
  }
}

function New-SkeletonRecipe {
  param(
    [string]$ComponentName,
    [string]$ArtifactName,
    [bool]$Unarchive,
    [string]$Lifecycle
  )
  $unarchiveLine = ""
  if ($Unarchive) {
    $unarchiveLine = "        Unarchive: ZIP`n"
  }
  $content = @"
---
RecipeFormatVersion: "2020-01-25"
ComponentName: "$ComponentName"
ComponentVersion: "$Version"
ComponentDescription: "$ComponentName hierarchical-config validation build"
ComponentPublisher: "edgecommons"
ComponentConfiguration:
  DefaultConfiguration:
    accessControl:
      aws.greengrass.ipc.pubsub:
        "${ComponentName}:pubsub:1":
          policyDescription: "Allow local IPC pub/sub for hierarchical-config validation."
          operations: ["aws.greengrass#PublishToTopic", "aws.greengrass#SubscribeToTopic"]
          resources: ["*"]
      aws.greengrass.ipc.mqttproxy:
        "${ComponentName}:northbound:1":
          policyDescription: "Allow IoT Core pub/sub for skeleton demo paths."
          operations: ["aws.greengrass#PublishToIoTCore", "aws.greengrass#SubscribeToIoTCore"]
          resources: ["*"]
Manifests:
  - Platform: { os: linux }
    Artifacts:
      - URI: "s3://BUCKET/$ComponentName/$Version/$ArtifactName"
$unarchiveLine    Lifecycle:
$Lifecycle
"@
  Write-Utf8NoBom -Path (Join-Path $recipesDir "$ComponentName-$Version.yaml") -Content $content
}

$components = [ordered]@{
  ConfigComponent = "com.mbreissi.edgecommons.ConfigComponent"
  Java = "com.mbreissi.edgecommons.JavaComponentSkeleton"
  Python = "com.mbreissi.edgecommons.PythonComponentSkeleton"
  Rust = "com.mbreissi.edgecommons.RustComponentSkeleton"
  Ts = "com.mbreissi.edgecommons.TsComponentSkeleton"
  Verifier = "com.mbreissi.edgecommons.HierarchicalConfigVerifier"
}

$javaJar = Join-Path $repoRoot "examples\java\target\java-component-skeleton-1.0.0.jar"
$rustSkeleton = Join-Path $repoRoot "build\gg-rust-skeleton-target\release\rust-component-skeleton"
$rustVerifier = Join-Path $repoRoot "build\gg-rust-target\release\interop-rust-node"
$configComponent = Join-Path $repoRoot "build\gg-configcomponent-target\release\config-component"

foreach ($required in @($javaJar, $rustSkeleton, $rustVerifier, $configComponent)) {
  if (-not (Test-Path -LiteralPath $required)) {
    throw "Required build output is missing: $required"
  }
}
if (-not (Test-Path -LiteralPath $configComponentRoot)) {
  throw "Missing sibling ConfigComponent repo: $configComponentRoot"
}

Copy-Artifact -ComponentName $components.ConfigComponent -Source $configComponent -Name "config-component"
Copy-Artifact -ComponentName $components.Java -Source $javaJar -Name "java-component-skeleton-1.0.0.jar"
Copy-Artifact -ComponentName $components.Rust -Source $rustSkeleton -Name "rust-component-skeleton"
Copy-Artifact -ComponentName $components.Verifier -Source $rustVerifier -Name "interop-rust-node"

$pythonStage = Join-Path $stageDir "python-component-skeleton"
New-Item -ItemType Directory -Force -Path $pythonStage | Out-Null
Copy-Item -LiteralPath (Join-Path $repoRoot "examples\python\main.py") -Destination $pythonStage -Force
Copy-Item -LiteralPath (Join-Path $repoRoot "examples\python\app") -Destination $pythonStage -Recurse -Force
Copy-Item -LiteralPath (Join-Path $repoRoot "examples\python\requirements.txt") -Destination $pythonStage -Force
Write-Utf8NoBom -Path (Join-Path $pythonStage "requirements-hierarchical.txt") -Content "psutil>=5.9.6`nawsiotsdk>=1.19.0`nawsiot>=0.1.3`npaho-mqtt>=2.0.0`nwatchdog>=3.0.0`n"
$pythonLibStage = Join-Path $pythonStage "libs\python"
New-Item -ItemType Directory -Force -Path $pythonLibStage | Out-Null
Copy-Item -LiteralPath (Join-Path $repoRoot "libs\python\edgecommons") -Destination $pythonLibStage -Recurse -Force
Copy-Item -LiteralPath (Join-Path $repoRoot "libs\python\setup.py") -Destination $pythonLibStage -Force
Copy-Item -LiteralPath (Join-Path $repoRoot "libs\python\pyproject.toml") -Destination $pythonLibStage -Force
Copy-Item -LiteralPath (Join-Path $repoRoot "libs\python\README.md") -Destination $pythonLibStage -Force
New-ZipArtifact -ComponentName $components.Python -ZipName "python-component-skeleton.zip" -SourcePath $pythonStage

$tsStage = Join-Path $stageDir "ts-component-skeleton"
New-Item -ItemType Directory -Force -Path $tsStage | Out-Null
Copy-Item -LiteralPath (Join-Path $repoRoot "examples\ts\dist") -Destination $tsStage -Recurse -Force
$tsLibStage = Join-Path $tsStage "libs\ts"
New-Item -ItemType Directory -Force -Path $tsLibStage | Out-Null
Copy-Item -LiteralPath (Join-Path $repoRoot "libs\ts\dist") -Destination $tsLibStage -Recurse -Force
$tsSkeletonPackage = @'
{
  "name": "ts-component-skeleton-greengrass-bundle",
  "version": "1.0.0",
  "private": true,
  "main": "dist/main.js",
  "dependencies": {
    "@edgecommons/edgecommons": "file:./libs/ts"
  }
}
'@
$tsLibPackage = @'
{
  "name": "@edgecommons/edgecommons",
  "version": "0.2.0",
  "main": "dist/index.js",
  "dependencies": {
    "ajv": "^8.20.0",
    "aws-iot-device-sdk-v2": "^1.27.0",
    "mqtt": "^5"
  }
}
'@
Write-Utf8NoBom -Path (Join-Path $tsStage "package.json") -Content $tsSkeletonPackage
Write-Utf8NoBom -Path (Join-Path $tsLibStage "package.json") -Content $tsLibPackage
New-ZipArtifact -ComponentName $components.Ts -ZipName "ts-component-skeleton.zip" -SourcePath $tsStage

$configRecipe = @"
---
RecipeFormatVersion: "2020-01-25"
ComponentName: "$($components.ConfigComponent)"
ComponentVersion: "$Version"
ComponentDescription: "Dedicated EdgeCommons hierarchical-configuration catalog server validation build"
ComponentPublisher: "edgecommons"
ComponentDependencies:
  aws.greengrass.TokenExchangeService:
    VersionRequirement: ">=2.0.0"
    DependencyType: "HARD"
ComponentConfiguration:
  DefaultConfiguration:
    ComponentConfig:
      logging:
        level: "INFO"
        rust_format: "{timestamp} [{level}] [{component}] {target} - {message}"
      heartbeat:
        enabled: true
        intervalSecs: 5
        destination: "local"
        measures: { cpu: true, memory: true, disk: false }
      metricEmission:
        target: "log"
        targetConfig:
          logFileName: "/greengrass/v2/logs/{ComponentFullName}.metric.log"
      component:
        token: "edgecommons-config-component"
        global:
          configComponent:
            catalogSource:
              type: "file"
              path: "/greengrass/v2/work/com.mbreissi.edgecommons.ConfigComponent/catalog.json"
              watch: true
            pushOnCatalogReload: true
            allowVolatileCatalogUpdates: true
        instances: []
    accessControl:
      aws.greengrass.ipc.pubsub:
        "$($components.ConfigComponent):config-server-subscribe:1":
          policyDescription: "Allow config server subscriptions."
          operations:
            - "aws.greengrass#SubscribeToTopic"
          resources:
            - "*"
        "$($components.ConfigComponent):config-server-publish:1":
          policyDescription: "Allow config server replies and set-config pushes."
          operations:
            - "aws.greengrass#PublishToTopic"
          resources:
            - "*"
Manifests:
  - Platform: { os: linux }
    Artifacts:
      - URI: "s3://BUCKET/$($components.ConfigComponent)/$Version/config-component"
    Lifecycle:
      Install:
        Script: "chmod +x {artifacts:path}/config-component"
      Run:
        Script: "exec {artifacts:path}/config-component --platform GREENGRASS -c GG_CONFIG"
"@
Write-Utf8NoBom -Path (Join-Path $recipesDir "$($components.ConfigComponent)-$Version.yaml") -Content $configRecipe

$javaLifecycle = @'
      Run:
        Script: "exec java -jar {artifacts:path}/java-component-skeleton-1.0.0.jar --platform GREENGRASS -c CONFIG_COMPONENT"
'@
New-SkeletonRecipe -ComponentName $components.Java -ArtifactName "java-component-skeleton-1.0.0.jar" -Unarchive $false -Lifecycle $javaLifecycle

$pythonLifecycle = @'
      Install:
        Script: |
          python3 -m venv /greengrass/v2/work/com.mbreissi.edgecommons.PythonComponentSkeleton/venv
          /greengrass/v2/work/com.mbreissi.edgecommons.PythonComponentSkeleton/venv/bin/python -m pip install {artifacts:decompressedPath}/python-component-skeleton/libs/python
          /greengrass/v2/work/com.mbreissi.edgecommons.PythonComponentSkeleton/venv/bin/python -m pip install -r {artifacts:decompressedPath}/python-component-skeleton/requirements-hierarchical.txt
      Run:
        Script: "exec /greengrass/v2/work/com.mbreissi.edgecommons.PythonComponentSkeleton/venv/bin/python -u {artifacts:decompressedPath}/python-component-skeleton/main.py --platform GREENGRASS -c CONFIG_COMPONENT"
'@
New-SkeletonRecipe -ComponentName $components.Python -ArtifactName "python-component-skeleton.zip" -Unarchive $true -Lifecycle $pythonLifecycle

$rustLifecycle = @'
      Install:
        Script: "chmod +x {artifacts:path}/rust-component-skeleton"
      Run:
        Script: "exec {artifacts:path}/rust-component-skeleton --platform GREENGRASS -c CONFIG_COMPONENT"
'@
New-SkeletonRecipe -ComponentName $components.Rust -ArtifactName "rust-component-skeleton" -Unarchive $false -Lifecycle $rustLifecycle

$tsLifecycle = @'
      Install:
        Script: |
          cd {artifacts:decompressedPath}/ts-component-skeleton
          npm install --omit=dev --no-audit --no-fund
      Run:
        Script: "exec node {artifacts:decompressedPath}/ts-component-skeleton/dist/main.js --platform GREENGRASS -c CONFIG_COMPONENT"
'@
New-SkeletonRecipe -ComponentName $components.Ts -ArtifactName "ts-component-skeleton.zip" -Unarchive $true -Lifecycle $tsLifecycle

$verifierRecipe = @"
---
RecipeFormatVersion: "2020-01-25"
ComponentName: "$($components.Verifier)"
ComponentVersion: "$Version"
ComponentDescription: "One-shot verifier for hierarchical update-catalog push behavior"
ComponentPublisher: "edgecommons"
ComponentConfiguration:
  DefaultConfiguration:
    accessControl:
      aws.greengrass.ipc.pubsub:
        "$($components.Verifier):pubsub:1":
          policyDescription: "Allow local IPC pub/sub for hierarchical-config verification."
          operations: ["aws.greengrass#PublishToTopic", "aws.greengrass#SubscribeToTopic"]
          resources: ["*"]
Manifests:
  - Platform: { os: linux }
    Artifacts:
      - URI: "s3://BUCKET/$($components.Verifier)/$Version/interop-rust-node"
    Lifecycle:
      Install:
        Script: "chmod +x {artifacts:path}/interop-rust-node"
      Run:
        Script: |
          set -e
          set -u
          OUT="/tmp/edgecommons_full_interop"
          mkdir -p "`$OUT"
          exec {artifacts:path}/interop-rust-node gg-config-update-file \
            ecv1/lab-5950x/config/cmd/update-catalog \
            /tmp/edgecommons-full-interop/catalog-update-second-pass.json \
            JavaComponentSkeleton,PythonComponentSkeleton,RustComponentSkeleton,TsComponentSkeleton \
            "`$OUT/update-result.json"
"@
Write-Utf8NoBom -Path (Join-Path $recipesDir "$($components.Verifier)-$Version.yaml") -Content $verifierRecipe

$catalogInitial = @'
{
  "schemaVersion": 1,
  "version": "initial-hierarchical-full-interop",
  "provenance": { "source": "file", "uri": "greengrass-full-interop" },
  "hierarchy": {
    "levels": ["enterprise", "site", "zone", "line", "device"]
  },
  "nodes": {
    "enterprise/acme": {
      "scope": { "enterprise": "acme" },
      "config": {
        "hierarchy": {
          "levels": ["enterprise", "site", "zone", "line", "device"]
        },
        "identity": { "enterprise": "acme" },
        "logging": { "level": "INFO" },
        "heartbeat": { "enabled": true, "intervalSecs": 5, "destination": "local" },
        "tags": { "lineageMarker": "gg-hierarchical-initial", "enterpriseOwner": "central-ops" }
      }
    },
    "site/integration-lab": {
      "parent": "enterprise/acme",
      "scope": { "enterprise": "acme", "site": "integration-lab" },
      "config": {
        "identity": { "site": "integration-lab" },
        "metricEmission": {
          "target": "log",
          "namespace": "edgecommons_hierarchical"
        }
      }
    },
    "zone/greengrass-zone": {
      "parent": "site/integration-lab",
      "scope": {
        "enterprise": "acme",
        "site": "integration-lab",
        "zone": "greengrass-zone"
      },
      "config": {
        "identity": { "zone": "greengrass-zone" },
        "tags": { "zoneClass": "validation" }
      }
    },
    "line/line-7": {
      "parent": "zone/greengrass-zone",
      "scope": {
        "enterprise": "acme",
        "site": "integration-lab",
        "zone": "greengrass-zone",
        "line": "line-7"
      },
      "config": {
        "identity": { "line": "line-7" },
        "logging": { "level": "WARN" }
      }
    }
  },
  "components": {
    "JavaComponentSkeleton": {
      "parent": "line/line-7",
      "config": {
        "component": {
          "token": "java-component-skeleton",
          "global": { "publish_interval": 3, "unique_token": "java-initial" },
          "instances": []
        },
        "tags": { "componentMarker": "java" }
      }
    },
    "PythonComponentSkeleton": {
      "parent": "line/line-7",
      "config": {
        "component": {
          "token": "python-component-skeleton",
          "global": { "publish_interval": 3, "unique_token": "python-initial" },
          "instances": []
        },
        "tags": { "componentMarker": "python" }
      }
    },
    "RustComponentSkeleton": {
      "parent": "line/line-7",
      "config": {
        "component": {
          "token": "rust-component-skeleton",
          "global": { "publish_interval": 3, "unique_token": "rust-initial" },
          "instances": []
        },
        "tags": { "componentMarker": "rust" }
      }
    },
    "TsComponentSkeleton": {
      "parent": "line/line-7",
      "config": {
        "component": {
          "token": "ts-component-skeleton",
          "global": { "publish_interval": 3, "unique_token": "ts-initial" },
          "instances": []
        },
        "tags": { "componentMarker": "ts" }
      }
    }
  }
}
'@
$catalogUpdate = @'
{
  "schemaVersion": 1,
  "version": "second-pass-hierarchical-full-interop",
  "provenance": { "source": "message", "uri": "greengrass-full-interop-second-pass" },
  "hierarchy": {
    "levels": ["enterprise", "site", "zone", "line", "device"]
  },
  "nodes": {
    "enterprise/acme": {
      "scope": { "enterprise": "acme" },
      "config": {
        "hierarchy": {
          "levels": ["enterprise", "site", "zone", "line", "device"]
        },
        "identity": { "enterprise": "acme" },
        "logging": { "level": "INFO" },
        "heartbeat": { "enabled": true, "intervalSecs": 5, "destination": "local" },
        "tags": { "lineageMarker": "gg-hierarchical-second-pass", "enterpriseOwner": "central-ops" }
      }
    },
    "site/integration-lab": {
      "parent": "enterprise/acme",
      "scope": { "enterprise": "acme", "site": "integration-lab" },
      "config": {
        "identity": { "site": "integration-lab" },
        "metricEmission": {
          "target": "log",
          "namespace": "edgecommons_hierarchical"
        }
      }
    },
    "zone/greengrass-zone": {
      "parent": "site/integration-lab",
      "scope": {
        "enterprise": "acme",
        "site": "integration-lab",
        "zone": "greengrass-zone"
      },
      "config": {
        "identity": { "zone": "greengrass-zone" },
        "tags": { "zoneClass": "validation" }
      }
    },
    "line/line-7": {
      "parent": "zone/greengrass-zone",
      "scope": {
        "enterprise": "acme",
        "site": "integration-lab",
        "zone": "greengrass-zone",
        "line": "line-7"
      },
      "config": {
        "identity": { "line": "line-7" },
        "logging": { "level": "DEBUG" }
      }
    }
  },
  "components": {
    "JavaComponentSkeleton": {
      "parent": "line/line-7",
      "config": {
        "component": {
          "token": "java-component-skeleton",
          "global": { "publish_interval": 21, "unique_token": "java-updated" },
          "instances": []
        },
        "tags": { "componentMarker": "java" }
      }
    },
    "PythonComponentSkeleton": {
      "parent": "line/line-7",
      "config": {
        "component": {
          "token": "python-component-skeleton",
          "global": { "publish_interval": 23, "unique_token": "python-updated" },
          "instances": []
        },
        "tags": { "componentMarker": "python" }
      }
    },
    "RustComponentSkeleton": {
      "parent": "line/line-7",
      "config": {
        "component": {
          "token": "rust-component-skeleton",
          "global": { "publish_interval": 29, "unique_token": "rust-updated" },
          "instances": []
        },
        "tags": { "componentMarker": "rust" }
      }
    },
    "TsComponentSkeleton": {
      "parent": "line/line-7",
      "config": {
        "component": {
          "token": "ts-component-skeleton",
          "global": { "publish_interval": 31, "unique_token": "ts-updated" },
          "instances": []
        },
        "tags": { "componentMarker": "ts" }
      }
    }
  }
}
'@
$configUpdate = @'
{
  "com.mbreissi.edgecommons.ConfigComponent": {
    "MERGE": {
      "ComponentConfig": {
        "component": {
          "token": "edgecommons-config-component",
          "global": {
            "configComponent": {
              "catalogSource": {
                "type": "file",
                "path": "/greengrass/v2/work/com.mbreissi.edgecommons.ConfigComponent/catalog.json",
                "watch": true
              },
              "pushOnCatalogReload": true,
              "allowVolatileCatalogUpdates": true
            }
          },
          "instances": []
        }
      }
    }
  }
}
'@

$catalogInitialPath = Join-Path $outputRootPath "catalog-initial.json"
$catalogUpdatePath = Join-Path $outputRootPath "catalog-update-second-pass.json"
$configUpdatePath = Join-Path $outputRootPath "configcomponent-update.json"
Write-Utf8NoBom -Path $catalogInitialPath -Content $catalogInitial
Write-Utf8NoBom -Path $catalogUpdatePath -Content $catalogUpdate
Write-Utf8NoBom -Path $configUpdatePath -Content $configUpdate

[pscustomobject]@{
  RunId = $RunId
  Version = $Version
  OutputRoot = $outputRootPath
  RecipeDir = $recipesDir
  ArtifactDir = $artifactsDir
  CatalogInitial = $catalogInitialPath
  CatalogUpdate = $catalogUpdatePath
  ConfigComponentUpdate = $configUpdatePath
  Components = ($components.Values -join ",")
}
