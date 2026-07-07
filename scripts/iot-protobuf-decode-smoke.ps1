<#
.SYNOPSIS
Runs an AWS IoT Core Rules protobuf decode smoke test for EdgeCommons messages.

.DESCRIPTION
Uploads the canonical EdgeCommons protobuf descriptor to S3, creates a temporary
IoT rule that decodes binary protobuf payloads with decode(*, 'proto', ...),
publishes two canonical protobuf test vectors, and verifies the decoded S3
records contain the expected header, identity, tags, topic, timestamps, and
base64 byte value.

Prerequisites:
- AWS CLI v2 authenticated for the target account and region.
- DescriptorBucket is in the same region as AWS IoT Core.
- DescriptorBucket policy permits the AWS IoT service principal
  (iot.amazonaws.com) to s3:Get* for the descriptor key.
- RoleArn trusts iot.amazonaws.com and permits s3:PutObject to the output and
  error prefixes in OutputBucket. Include s3:GetBucketLocation on the bucket
  and s3:PutObjectAcl if the target bucket requires object ACL handling.
#>

[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$DescriptorBucket,

    [string]$OutputBucket,

    [Parameter(Mandatory = $true)]
    [string]$RoleArn,

    [string]$Region,

    [string]$Prefix,

    [string]$RuleName,

    [int]$TimeoutSeconds = 120,

    [switch]$KeepArtifacts
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$DescriptorPath = Join-Path $RepoRoot "protobuf-test-vectors\edgecommons-v1.desc"
$VectorHexPath = Join-Path $RepoRoot "protobuf-test-vectors\messages.pb.hex"
$RunId = [DateTimeOffset]::UtcNow.ToString("yyyyMMddHHmmss")

if ([string]::IsNullOrWhiteSpace($OutputBucket)) {
    $OutputBucket = $DescriptorBucket
}
if ([string]::IsNullOrWhiteSpace($Prefix)) {
    $Prefix = "edgecommons/iot-protobuf-smoke/$RunId"
}
$Prefix = $Prefix.Trim("/")
if ($Prefix.Contains("'")) {
    throw "Prefix must not contain single quotes because it is embedded in IoT SQL."
}
if ([string]::IsNullOrWhiteSpace($RuleName)) {
    $RuleName = "EdgeCommonsProtobufSmoke$RunId"
}
if ($RuleName -notmatch "^[A-Za-z0-9_]+$") {
    throw "RuleName must contain only letters, numbers, and underscores."
}
if ($TimeoutSeconds -lt 30) {
    throw "TimeoutSeconds must be at least 30."
}
if (-not (Test-Path -LiteralPath $DescriptorPath)) {
    throw "Descriptor file not found: $DescriptorPath"
}
if (-not (Test-Path -LiteralPath $VectorHexPath)) {
    throw "Vector hex file not found: $VectorHexPath"
}

function Invoke-Aws {
    param(
        [Parameter(Mandatory = $true)]
        [string[]]$Arguments
    )

    $output = & aws @Arguments 2>&1
    $exit = $LASTEXITCODE
    $text = ($output | ForEach-Object { $_.ToString() }) -join [Environment]::NewLine
    if ($exit -ne 0) {
        throw "aws $($Arguments -join ' ') failed with exit code $exit`n$text"
    }
    return $text
}

function Invoke-AwsJson {
    param(
        [Parameter(Mandatory = $true)]
        [string[]]$Arguments
    )

    $text = Invoke-Aws -Arguments ($Arguments + @("--output", "json"))
    if ([string]::IsNullOrWhiteSpace($text)) {
        return $null
    }
    return $text | ConvertFrom-Json
}

function Get-AwsRegion {
    if (-not [string]::IsNullOrWhiteSpace($Region)) {
        return $Region
    }
    if (-not [string]::IsNullOrWhiteSpace($env:AWS_REGION)) {
        return $env:AWS_REGION
    }
    if (-not [string]::IsNullOrWhiteSpace($env:AWS_DEFAULT_REGION)) {
        return $env:AWS_DEFAULT_REGION
    }
    $configured = Invoke-Aws -Arguments @("configure", "get", "region")
    if (-not [string]::IsNullOrWhiteSpace($configured)) {
        return $configured.Trim()
    }
    throw "No AWS region was provided. Pass -Region or configure AWS_REGION/AWS_DEFAULT_REGION."
}

function Get-VectorHex {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Id
    )

    foreach ($line in Get-Content -LiteralPath $VectorHexPath) {
        if ($line.StartsWith("$Id ")) {
            return $line.Substring($Id.Length + 1).Trim()
        }
    }
    throw "Vector '$Id' not found in $VectorHexPath"
}

function ConvertFrom-HexString {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Hex
    )

    if (($Hex.Length % 2) -ne 0) {
        throw "Hex string has odd length."
    }
    $bytes = New-Object byte[] ($Hex.Length / 2)
    for ($i = 0; $i -lt $bytes.Length; $i++) {
        $bytes[$i] = [Convert]::ToByte($Hex.Substring($i * 2, 2), 16)
    }
    return $bytes
}

function Write-VectorFile {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Id,

        [Parameter(Mandatory = $true)]
        [string]$Path
    )

    [IO.File]::WriteAllBytes($Path, (ConvertFrom-HexString -Hex (Get-VectorHex -Id $Id)))
}

function Get-JsonProp {
    param(
        [AllowNull()]
        [object]$Object,

        [Parameter(Mandatory = $true)]
        [string[]]$Names
    )

    if ($null -eq $Object) {
        return $null
    }
    foreach ($name in $Names) {
        $prop = $Object.PSObject.Properties[$name]
        if ($null -ne $prop) {
            return $prop.Value
        }
    }
    return $null
}

function Assert-Equal {
    param(
        [AllowNull()]
        [object]$Actual,

        [AllowNull()]
        [object]$Expected,

        [Parameter(Mandatory = $true)]
        [string]$Label
    )

    if ([string]$Actual -ne [string]$Expected) {
        throw "$Label mismatch. Expected '$Expected', got '$Actual'."
    }
}

function Get-EcString {
    param([AllowNull()][object]$Value)
    if ($Value -is [string]) {
        return $Value
    }
    return Get-JsonProp -Object $Value -Names @("stringValue", "string_value")
}

function Get-EcBytes {
    param([AllowNull()][object]$Value)
    return Get-JsonProp -Object $Value -Names @("bytesValue", "bytes_value")
}

function Get-FirstSample {
    param([Parameter(Mandatory = $true)][object]$Message)

    $body = Get-JsonProp -Object $Message -Names @("southboundSignalUpdate", "southbound_signal_update")
    if ($null -eq $body) {
        throw "Decoded message does not contain southboundSignalUpdate."
    }
    $samples = Get-JsonProp -Object $body -Names @("samples")
    if ($null -eq $samples) {
        throw "Decoded southboundSignalUpdate does not contain samples."
    }
    if ($samples -is [array]) {
        return $samples[0]
    }
    return $samples
}

function Find-RecordByCorrelation {
    param(
        [Parameter(Mandatory = $true)]
        [object[]]$Records,

        [Parameter(Mandatory = $true)]
        [string]$CorrelationId
    )

    foreach ($record in $Records) {
        $message = Get-JsonProp -Object $record -Names @("message")
        $header = Get-JsonProp -Object $message -Names @("header")
        $actual = Get-JsonProp -Object $header -Names @("correlationId", "correlation_id")
        if ($actual -eq $CorrelationId) {
            return $record
        }
    }
    throw "No decoded S3 record found for correlation id '$CorrelationId'."
}

function Get-S3Keys {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Bucket,

        [Parameter(Mandatory = $true)]
        [string]$KeyPrefix
    )

    $result = Invoke-AwsJson -Arguments @(
        "s3api", "list-objects-v2",
        "--bucket", $Bucket,
        "--prefix", $KeyPrefix
    )
    if ($null -eq $result) {
        return @()
    }
    if ($null -eq $result.PSObject.Properties["Contents"]) {
        return @()
    }
    $contents = @($result.Contents)
    return @($contents | Where-Object { $null -ne $_ } | ForEach-Object { $_.Key })
}

function Remove-S3PrefixQuiet {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Bucket,

        [Parameter(Mandatory = $true)]
        [string]$KeyPrefix
    )

    try {
        foreach ($key in Get-S3Keys -Bucket $Bucket -KeyPrefix $KeyPrefix) {
            [void](Invoke-Aws -Arguments @("s3api", "delete-object", "--bucket", $Bucket, "--key", $key))
        }
    } catch {
        Write-Warning "Failed to clean S3 prefix s3://${Bucket}/${KeyPrefix}: $($_.Exception.Message)"
    }
}

function Remove-S3ObjectQuiet {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Bucket,

        [Parameter(Mandatory = $true)]
        [string]$Key
    )

    try {
        [void](Invoke-Aws -Arguments @("s3api", "delete-object", "--bucket", $Bucket, "--key", $Key))
    } catch {
        Write-Warning "Failed to clean S3 object s3://${Bucket}/${Key}: $($_.Exception.Message)"
    }
}

$WorkDir = Join-Path ([IO.Path]::GetTempPath()) "edgecommons-iot-protobuf-smoke-$RunId"
$DescriptorKey = "$Prefix/edgecommons-v1.desc"
$DecodedPrefix = "$Prefix/decoded"
$ErrorPrefix = "$Prefix/errors"
$DecodedKeyTemplate = '{0}/decoded/${{timestamp()}}.json' -f $Prefix
$ErrorKeyTemplate = '{0}/errors/${{timestamp()}}.json' -f $Prefix
$SourceTopic = "$Prefix/input"
$CreatedRule = $false
$UploadedDescriptor = $false

try {
    New-Item -ItemType Directory -Force -Path $WorkDir | Out-Null
    $Region = Get-AwsRegion
    [void](Invoke-AwsJson -Arguments @("sts", "get-caller-identity", "--region", $Region))

    $TelemetryPath = Join-Path $WorkDir "telemetry_numeric.pb"
    $ByteTelemetryPath = Join-Path $WorkDir "telemetry_byte_timestamps.pb"
    $RulePayloadPath = Join-Path $WorkDir "rule-payload.json"
    Write-VectorFile -Id "telemetry_numeric" -Path $TelemetryPath
    Write-VectorFile -Id "telemetry_byte_timestamps" -Path $ByteTelemetryPath

    Write-Host "Uploading descriptor to s3://$DescriptorBucket/$DescriptorKey"
    [void](Invoke-Aws -Arguments @(
        "s3", "cp", $DescriptorPath, "s3://$DescriptorBucket/$DescriptorKey",
        "--region", $Region
    ))
    $UploadedDescriptor = $true

    $ruleSql = "SELECT topic() AS topic, decode(*, 'proto', '$DescriptorBucket', '$DescriptorKey', 'edgecommons/v1/message.proto', 'EdgeCommonsMessage') AS message FROM '$SourceTopic'"
    $rulePayload = [ordered]@{
        sql = $ruleSql
        awsIotSqlVersion = "2016-03-23"
        ruleDisabled = $false
        actions = @(
            [ordered]@{
                s3 = [ordered]@{
                    roleArn = $RoleArn
                    bucketName = $OutputBucket
                    key = $DecodedKeyTemplate
                }
            }
        )
        errorAction = [ordered]@{
            s3 = [ordered]@{
                roleArn = $RoleArn
                bucketName = $OutputBucket
                key = $ErrorKeyTemplate
            }
        }
    }
    $rulePayload | ConvertTo-Json -Depth 20 | Set-Content -LiteralPath $RulePayloadPath -Encoding UTF8

    Write-Host "Creating IoT rule $RuleName for topic $SourceTopic"
    [void](Invoke-Aws -Arguments @(
        "iot", "create-topic-rule",
        "--rule-name", $RuleName,
        "--topic-rule-payload", "file://$RulePayloadPath",
        "--region", $Region
    ))
    $CreatedRule = $true

    $endpoint = (Invoke-Aws -Arguments @(
        "iot", "describe-endpoint",
        "--endpoint-type", "iot:Data-ATS",
        "--query", "endpointAddress",
        "--output", "text",
        "--region", $Region
    )).Trim()
    $endpointUrl = "https://$endpoint"

    Write-Host "Publishing canonical protobuf vectors to $SourceTopic"
    [void](Invoke-Aws -Arguments @(
        "iot-data", "publish",
        "--endpoint-url", $endpointUrl,
        "--topic", $SourceTopic,
        "--payload", "fileb://$TelemetryPath",
        "--region", $Region
    ))
    [void](Invoke-Aws -Arguments @(
        "iot-data", "publish",
        "--endpoint-url", $endpointUrl,
        "--topic", $SourceTopic,
        "--payload", "fileb://$ByteTelemetryPath",
        "--region", $Region
    ))

    Write-Host "Waiting for decoded S3 records under s3://$OutputBucket/$DecodedPrefix"
    $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
    $decodedKeys = @()
    $errorKeys = @()
    while ((Get-Date) -lt $deadline) {
        Start-Sleep -Seconds 5
        $decodedKeys = @(Get-S3Keys -Bucket $OutputBucket -KeyPrefix $DecodedPrefix)
        $errorKeys = @(Get-S3Keys -Bucket $OutputBucket -KeyPrefix $ErrorPrefix)
        if ($decodedKeys.Count -ge 2) {
            break
        }
    }

    if ($decodedKeys.Count -lt 2) {
        $errorSummary = if ($errorKeys.Count -gt 0) { $errorKeys -join ", " } else { "none" }
        throw "Timed out waiting for decoded records. Error-action objects: $errorSummary"
    }

    $records = @()
    $index = 0
    foreach ($key in $decodedKeys) {
        $index += 1
        $localJson = Join-Path $WorkDir "decoded-$index.json"
        [void](Invoke-Aws -Arguments @(
            "s3api", "get-object",
            "--bucket", $OutputBucket,
            "--key", $key,
            $localJson,
            "--region", $Region
        ))
        $records += (Get-Content -LiteralPath $localJson -Raw | ConvertFrom-Json)
    }

    $numericRecord = Find-RecordByCorrelation -Records $records -CorrelationId "corr-vector-telemetry_numeric"
    $numericMessage = Get-JsonProp -Object $numericRecord -Names @("message")
    $numericHeader = Get-JsonProp -Object $numericMessage -Names @("header")
    $numericIdentity = Get-JsonProp -Object $numericMessage -Names @("identity")
    $numericTags = Get-JsonProp -Object $numericMessage -Names @("tags")
    $numericSample = Get-FirstSample -Message $numericMessage
    $siteRole = Get-JsonProp -Object $numericTags -Names @("siteRole", "site_role")

    Assert-Equal -Actual (Get-JsonProp -Object $numericRecord -Names @("topic")) -Expected $SourceTopic -Label "numeric topic"
    Assert-Equal -Actual (Get-JsonProp -Object $numericHeader -Names @("name")) -Expected "Telemetry" -Label "numeric header.name"
    Assert-Equal -Actual (Get-JsonProp -Object $numericIdentity -Names @("path")) -Expected "plant-a/line-2/gw-01" -Label "numeric identity.path"
    Assert-Equal -Actual (Get-EcString -Value $siteRole) -Expected "line-edge" -Label "numeric tags.siteRole"
    Assert-Equal -Actual (Get-JsonProp -Object $numericSample -Names @("sourceTsMs", "source_ts_ms")) -Expected "1783360799900" -Label "numeric source_ts_ms"
    Assert-Equal -Actual (Get-JsonProp -Object $numericSample -Names @("serverTsMs", "server_ts_ms")) -Expected "1783360800000" -Label "numeric server_ts_ms"

    $byteRecord = Find-RecordByCorrelation -Records $records -CorrelationId "corr-vector-telemetry_byte_timestamps"
    $byteMessage = Get-JsonProp -Object $byteRecord -Names @("message")
    $byteHeader = Get-JsonProp -Object $byteMessage -Names @("header")
    $byteSample = Get-FirstSample -Message $byteMessage
    $byteValue = Get-JsonProp -Object $byteSample -Names @("value")

    Assert-Equal -Actual (Get-JsonProp -Object $byteRecord -Names @("topic")) -Expected $SourceTopic -Label "byte topic"
    Assert-Equal -Actual (Get-JsonProp -Object $byteHeader -Names @("name")) -Expected "SouthboundSignalUpdate" -Label "byte header.name"
    Assert-Equal -Actual (Get-JsonProp -Object $byteSample -Names @("sourceTsMs", "source_ts_ms")) -Expected "1783360799900" -Label "byte source_ts_ms"
    Assert-Equal -Actual (Get-JsonProp -Object $byteSample -Names @("serverTsMs", "server_ts_ms")) -Expected "1783360800000" -Label "byte server_ts_ms"
    Assert-Equal -Actual (Get-EcBytes -Value $byteValue) -Expected "AAEC/v8=" -Label "byte sample bytesValue"

    Write-Host "PASS: AWS IoT Core protobuf decode smoke succeeded."
    Write-Host "Rule: $RuleName"
    Write-Host "Topic: $SourceTopic"
    Write-Host "Decoded records: $($decodedKeys -join ', ')"
} finally {
    if ($KeepArtifacts) {
        Write-Host "Keeping artifacts under prefix $Prefix and local work directory $WorkDir"
    } else {
        if ($CreatedRule) {
            try {
                [void](Invoke-Aws -Arguments @(
                    "iot", "delete-topic-rule",
                    "--rule-name", $RuleName,
                    "--region", $Region
                ))
            } catch {
                Write-Warning "Failed to delete IoT rule ${RuleName}: $($_.Exception.Message)"
            }
        }
        if ($UploadedDescriptor) {
            Remove-S3ObjectQuiet -Bucket $DescriptorBucket -Key $DescriptorKey
        }
        if ($CreatedRule) {
            Remove-S3PrefixQuiet -Bucket $OutputBucket -KeyPrefix $DecodedPrefix
            Remove-S3PrefixQuiet -Bucket $OutputBucket -KeyPrefix $ErrorPrefix
        }
        if (Test-Path -LiteralPath $WorkDir) {
            $tempRoot = [IO.Path]::GetFullPath([IO.Path]::GetTempPath())
            $resolvedWorkDir = [IO.Path]::GetFullPath($WorkDir)
            if ($resolvedWorkDir.StartsWith($tempRoot, [StringComparison]::OrdinalIgnoreCase)) {
                Remove-Item -LiteralPath $resolvedWorkDir -Recurse -Force
            } else {
                Write-Warning "Refusing to remove non-temp work directory: $resolvedWorkDir"
            }
        }
    }
}
