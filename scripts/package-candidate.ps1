param([string]$OutputDirectory, [switch]$CheckConfiguration)

$ErrorActionPreference = 'Stop'
$projectRoot = Split-Path -Parent $PSScriptRoot
if (-not $OutputDirectory) { $OutputDirectory = Join-Path $projectRoot 'outputs/ci-candidate' }
$OutputDirectory = [IO.Path]::GetFullPath($OutputDirectory)
$utf8 = [Text.UTF8Encoding]::new($false)

$packagePath = Join-Path $projectRoot 'package.json'
$configPath = Join-Path $projectRoot 'src-tauri/tauri.conf.json'
function Resolve-CandidateVersion([string]$Value) {
    $versionFile = [IO.Path]::GetFullPath((Join-Path (Split-Path -Parent $configPath) $Value))
    if ([IO.File]::Exists($versionFile)) {
        if (-not $versionFile.Equals($packagePath, [StringComparison]::OrdinalIgnoreCase)) {
            throw 'The Tauri version file must reference the source package.json'
        }
        $Value = [string](Get-Content -LiteralPath $versionFile -Raw | ConvertFrom-Json).version
    }
    $parsed = $null
    if (-not [System.Management.Automation.SemanticVersion]::TryParse($Value, [ref]$parsed)) {
        throw 'The Tauri version must be a semantic version or the source package.json path'
    }
    $parsed.ToString()
}
$package = Get-Content -LiteralPath $packagePath -Raw | ConvertFrom-Json
$config = Get-Content -LiteralPath $configPath -Raw | ConvertFrom-Json
$cargo = [regex]::Match([IO.File]::ReadAllText((Join-Path $projectRoot 'src-tauri/Cargo.toml')), '(?m)^version\s*=\s*"([^"]+)"\s*$')
$version = [string]$package.version
$configVersion = Resolve-CandidateVersion ([string]$config.version)
if (-not $cargo.Success -or $cargo.Groups[1].Value -ne $version -or $configVersion -ne $version) {
    throw 'Package, Cargo and Tauri versions must match'
}
if ($CheckConfiguration) {
    [pscustomobject]@{ version = $version; cargoVersion = $cargo.Groups[1].Value; tauriVersion = $configVersion; tauriVersionSource = $config.version } | ConvertTo-Json -Compress
    return
}
if ($env:GITHUB_SHA -notmatch '^[0-9a-f]{40}$') { throw 'Candidate packaging requires the exact GitHub Actions source commit' }

$program = Join-Path $projectRoot 'src-tauri/target/release/skill-manager.exe'
$guide = Join-Path $projectRoot 'docs/USER_GUIDE.md'
$notices = Join-Path $projectRoot 'docs/THIRD_PARTY_NOTICES.txt'
$logRoot = Join-Path $projectRoot 'check-logs'
$logNames = @('package-manager.log', 'dependencies.log', 'frontend-build.log', 'frontend-tests.log', 'rust-toolchain.log', 'rust-format.log', 'rust-tests.log', 'rust-clippy.log', 'windows-release-build.log')
foreach ($inputPath in @($program, $guide, $notices) + @($logNames | ForEach-Object { Join-Path $logRoot $_ })) {
    if (-not [IO.File]::Exists($inputPath)) { throw "Required candidate input does not exist: $inputPath" }
}
if ((Get-Item -LiteralPath $program).VersionInfo.ProductVersion -ne $version) {
    throw 'Compiled program version does not match the source version'
}
$programStream = [IO.File]::OpenRead($program)
$binary = [IO.BinaryReader]::new($programStream)
try {
    if ($binary.ReadUInt16() -ne 0x5a4d) { throw 'Candidate is not a Windows executable' }
    $programStream.Position = 0x3c
    $peOffset = $binary.ReadUInt32()
    $programStream.Position = $peOffset
    if ($binary.ReadUInt32() -ne 0x00004550 -or $binary.ReadUInt16() -ne 0x8664) {
        throw 'Candidate must be a Windows x64 PE executable'
    }
} finally { $binary.Dispose(); $programStream.Dispose() }

if ([IO.Directory]::Exists($OutputDirectory)) {
    $entries = [IO.Directory]::EnumerateFileSystemEntries($OutputDirectory).GetEnumerator()
    try { if ($entries.MoveNext()) { throw 'Candidate output directory must be empty' } }
    finally { $entries.Dispose() }
}
[IO.Directory]::CreateDirectory($OutputDirectory) | Out-Null
$checks = Join-Path $OutputDirectory 'checks'
[IO.Directory]::CreateDirectory($checks) | Out-Null
Copy-Item -LiteralPath $program -Destination (Join-Path $OutputDirectory 'skill-manager.exe')
Copy-Item -LiteralPath $guide -Destination (Join-Path $OutputDirectory 'USER_GUIDE.md')
Copy-Item -LiteralPath $notices -Destination (Join-Path $OutputDirectory 'THIRD_PARTY_NOTICES.txt')
[IO.File]::WriteAllText((Join-Path $OutputDirectory 'data-directory.txt'), ".`n", $utf8)
foreach ($name in $logNames) { Copy-Item -LiteralPath (Join-Path $logRoot $name) -Destination (Join-Path $checks $name) }

$info = [ordered]@{
    formatVersion = 1
    candidate = $true
    version = $version
    sourceCommit = $env:GITHUB_SHA
    repository = $env:GITHUB_REPOSITORY
    workflowRunId = $env:GITHUB_RUN_ID
    workflowRunAttempt = $env:GITHUB_RUN_ATTEMPT
    builtAtUtc = [DateTime]::UtcNow.ToString('o')
    executableSha256 = (Get-FileHash -LiteralPath (Join-Path $OutputDirectory 'skill-manager.exe') -Algorithm SHA256).Hash.ToLowerInvariant()
    nativeGuiVerified = $false
    checks = $logNames
}
[IO.File]::WriteAllText((Join-Path $OutputDirectory 'BUILD_INFO.json'), ($info | ConvertTo-Json -Depth 4) + "`n", $utf8)
$sums = @(Get-ChildItem -LiteralPath $OutputDirectory -File -Recurse | Sort-Object FullName | ForEach-Object {
    $relative = [IO.Path]::GetRelativePath($OutputDirectory, $_.FullName).Replace('\', '/')
    (Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash.ToLowerInvariant() + '  ' + $relative
})
[IO.File]::WriteAllText((Join-Path $OutputDirectory 'SHA256SUMS.txt'), ($sums -join "`n") + "`n", $utf8)
if ($env:GITHUB_OUTPUT) { [IO.File]::AppendAllText($env:GITHUB_OUTPUT, "version=$version`n", $utf8) }
Write-Output "Prepared Windows x64 candidate $version; native GUI validation remains a separate gate."
