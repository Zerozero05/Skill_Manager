param(
    [Parameter(Mandatory)][ValidatePattern('^[0-9]+$')][string]$CandidateRunId,
    [Parameter(Mandatory)][string]$CandidateArtifact,
    [Parameter(Mandatory)][ValidatePattern('^[0-9a-f]{64}$')][string]$ExpectedExecutableSha256
)

$ErrorActionPreference = 'Stop'
$projectRoot = Split-Path -Parent $PSScriptRoot
$utf8 = [Text.UTF8Encoding]::new($false)
if ($env:GITHUB_REPOSITORY -ne 'Zerozero05/Skill_Manager' -or $env:GITHUB_SHA -notmatch '^[0-9a-f]{40}$') {
    throw 'Release preparation requires the exact repository and GitHub Actions source commit'
}
Set-Location -LiteralPath $projectRoot
& ./scripts/package-candidate.ps1 -CheckConfiguration
$version = [string](Get-Content -LiteralPath package.json -Raw | ConvertFrom-Json).version
$staging = Join-Path $projectRoot 'outputs/release-staging'
$assets = Join-Path $projectRoot 'outputs/release-assets'
$verification = Join-Path $projectRoot 'outputs/release-verification'
foreach ($path in @($staging, $assets, $verification)) {
    if (Test-Path -LiteralPath $path) { throw "Preparation output already exists: $path" }
    New-Item -ItemType Directory -Path $path | Out-Null
}
function Get-Sha256([string]$Path) { (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant() }
function Write-Utf8([string]$Path, [string]$Content) { [IO.File]::WriteAllText($Path, $Content, $utf8) }
function Invoke-GitHubJson([string]$Endpoint) {
    $response = & gh api $Endpoint
    if ($LASTEXITCODE -ne 0) { throw "GitHub read failed: $Endpoint" }
    $response | ConvertFrom-Json
}

$run = Invoke-GitHubJson "repos/$env:GITHUB_REPOSITORY/actions/runs/$CandidateRunId"
if ($run.conclusion -ne 'success') { throw 'The candidate workflow must have completed successfully' }
$candidate = Join-Path $staging 'candidate'
& gh run download $CandidateRunId --repo $env:GITHUB_REPOSITORY --name $CandidateArtifact --dir $candidate
if ($LASTEXITCODE -ne 0) { throw 'Candidate artifact download failed' }
foreach ($line in Get-Content -LiteralPath (Join-Path $candidate 'SHA256SUMS.txt')) {
    if ($line -notmatch '^([0-9a-f]{64})  (.+)$') { throw 'Invalid candidate checksum line' }
    $expected = $Matches[1]; $relative = $Matches[2]
    $path = [IO.Path]::GetFullPath((Join-Path $candidate $relative))
    if (-not $path.StartsWith($candidate + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) { throw 'Candidate path escaped staging' }
    if ((Get-Sha256 $path) -ne $expected) { throw "Candidate checksum mismatch: $relative" }
}
$info = Get-Content -LiteralPath (Join-Path $candidate 'BUILD_INFO.json') -Raw | ConvertFrom-Json
$program = Join-Path $candidate 'skill-manager.exe'
if ($info.version -ne $version -or [string]$info.workflowRunId -ne $CandidateRunId -or $info.repository -ne $env:GITHUB_REPOSITORY -or $info.sourceCommit -notmatch '^[0-9a-f]{40}$') {
    throw 'Candidate build identity does not match the requested release'
}
if ((Get-Sha256 $program) -ne $ExpectedExecutableSha256 -or $info.executableSha256 -ne $ExpectedExecutableSha256) { throw 'Candidate executable differs from the independently verified program' }
if ((Get-Item -LiteralPath $program).VersionInfo.ProductVersion -ne $version) { throw 'Executable version does not match source' }

# Documentation and this packaging layer may evolve after the tested application.
# All actual application inputs must still match the verified build commit.
$comparison = Invoke-GitHubJson "repos/$env:GITHUB_REPOSITORY/compare/$($info.sourceCommit)...$env:GITHUB_SHA"
$allowed = @('README.md', 'CHANGELOG.md', 'docs/USER_GUIDE.md', "docs/RELEASE_v$version.md", '.github/workflows/package-release.yml', 'scripts/prepare-release.ps1')
$changed = @($comparison.files | ForEach-Object filename)
if ($changed.Count -ge 300) { throw 'Source comparison is too large to establish candidate equivalence' }
foreach ($path in $changed) { if ($path -notin $allowed) { throw "Application input changed after the verified candidate: $path" } }
if ((Get-Sha256 (Join-Path $projectRoot 'docs/THIRD_PARTY_NOTICES.txt')) -ne (Get-Sha256 (Join-Path $candidate 'THIRD_PARTY_NOTICES.txt'))) { throw 'Candidate license notices differ from source' }

$target = Join-Path $projectRoot 'src-tauri/target/release'
New-Item -ItemType Directory -Path $target -Force | Out-Null
Copy-Item -LiteralPath $program -Destination (Join-Path $target 'skill-manager.exe')
New-Item -ItemType Directory -Path (Join-Path $projectRoot 'dist') -Force | Out-Null
& pnpm tauri bundle --bundles nsis --ci --no-sign --no-binary-patching 2>&1 | Tee-Object -FilePath (Join-Path $verification 'nsis-bundle.log') -Encoding utf8NoBOM
if ($LASTEXITCODE -ne 0) { throw 'NSIS bundling failed' }
if ((Get-Sha256 (Join-Path $target 'skill-manager.exe')) -ne $ExpectedExecutableSha256) { throw 'Bundler modified the verified executable' }
$installers = @(Get-ChildItem -LiteralPath (Join-Path $target 'bundle/nsis') -File -Filter '*.exe')
if ($installers.Count -ne 1) { throw 'Expected one Windows NSIS installer' }
$installer = Join-Path $assets "Skill_Manager_v${version}_windows_x64_setup.exe"
Copy-Item -LiteralPath $installers[0].FullName -Destination $installer
$sevenZip = Join-Path $env:ProgramFiles '7-Zip/7z.exe'
if (-not (Test-Path -LiteralPath $sevenZip)) { throw 'The Windows runner is missing 7-Zip for installer verification' }
& $sevenZip t $installer 2>&1 | Tee-Object -FilePath (Join-Path $verification 'nsis-archive-test.log') -Encoding utf8NoBOM
if ($LASTEXITCODE -ne 0) { throw 'Installer archive integrity check failed' }
$expanded = Join-Path $staging 'installer-contents'
& $sevenZip x $installer "-o$expanded" -y 2>&1 | Tee-Object -FilePath (Join-Path $verification 'nsis-extraction.log') -Encoding utf8NoBOM
if ($LASTEXITCODE -ne 0) { throw 'Installer extraction failed' }
$embeddedPrograms = @(Get-ChildItem -LiteralPath $expanded -File -Recurse -Filter 'skill-manager.exe')
if ($embeddedPrograms.Count -ne 1 -or (Get-Sha256 $embeddedPrograms[0].FullName) -ne $ExpectedExecutableSha256) { throw 'Installer does not contain the exact verified executable' }

$portable = Join-Path $staging 'portable'
New-Item -ItemType Directory -Path $portable | Out-Null
Copy-Item -LiteralPath $program -Destination (Join-Path $portable 'skill-manager.exe')
Copy-Item -LiteralPath (Join-Path $projectRoot 'docs/USER_GUIDE.md') -Destination (Join-Path $portable 'USER_GUIDE.md')
Copy-Item -LiteralPath (Join-Path $candidate 'THIRD_PARTY_NOTICES.txt') -Destination (Join-Path $portable 'THIRD_PARTY_NOTICES.txt')
Write-Utf8 (Join-Path $portable 'data-directory.txt') ".`n"
$portableZip = Join-Path $assets "Skill_Manager_v${version}_windows_x64_portable.zip"
[IO.Compression.ZipFile]::CreateFromDirectory($portable, $portableZip, [IO.Compression.CompressionLevel]::Optimal, $false)
$portableArchive = [IO.Compression.ZipFile]::OpenRead($portableZip)
try {
    if ($portableArchive.Entries.Count -ne 4) { throw 'Portable ZIP contains unexpected files' }
    $stream = $portableArchive.GetEntry('skill-manager.exe').Open()
    try { $portableExecutableSha256 = [Convert]::ToHexString([Security.Cryptography.SHA256]::HashData($stream)).ToLowerInvariant() }
    finally { $stream.Dispose() }
    if ($portableExecutableSha256 -ne $ExpectedExecutableSha256) { throw 'Portable ZIP executable differs from the verified program' }
} finally { $portableArchive.Dispose() }
Copy-Item -LiteralPath (Join-Path $projectRoot 'docs/USER_GUIDE.md') -Destination (Join-Path $assets 'USER_GUIDE.md')
$sourceZip = Join-Path $assets "Skill_Manager_v${version}_source.zip"
& git archive --format=zip --prefix=Skill_Manager/ -o $sourceZip $env:GITHUB_SHA
if ($LASTEXITCODE -ne 0) { throw 'Source archive creation failed' }
$archive = [IO.Compression.ZipFile]::OpenRead($sourceZip)
try {
    $files = @($archive.Entries | Where-Object { $_.Name } | Sort-Object FullName | ForEach-Object {
        $stream = $_.Open()
        try { $hash = [Convert]::ToHexString([Security.Cryptography.SHA256]::HashData($stream)).ToLowerInvariant() }
        finally { $stream.Dispose() }
        [ordered]@{ path = $_.FullName.Substring('Skill_Manager/'.Length); bytes = $_.Length; sha256 = $hash }
    })
} finally { $archive.Dispose() }
$manifest = [ordered]@{
    formatVersion = 1; version = $version; repository = $env:GITHUB_REPOSITORY
    sourceCommit = $env:GITHUB_SHA; candidateRunId = $CandidateRunId; candidateBuildCommit = $info.sourceCommit
    candidateExecutableSha256 = $ExpectedExecutableSha256; sourceArchiveSha256 = Get-Sha256 $sourceZip
    installerExecutableSha256 = Get-Sha256 $embeddedPrograms[0].FullName
    installerWizardVerified = $false; installerArchiveVerified = $true
    applicationInputsMatchCandidate = $true; postCandidateFiles = $changed; files = $files
}
Write-Utf8 (Join-Path $assets 'SOURCE_MANIFEST.json') (($manifest | ConvertTo-Json -Depth 6) + "`n")
$releaseNotes = Join-Path $projectRoot "docs/RELEASE_v$version.md"
if (Test-Path -LiteralPath $releaseNotes) { Copy-Item -LiteralPath $releaseNotes -Destination (Join-Path $assets 'RELEASE_NOTES.md') }
else {
    $changelog = [IO.File]::ReadAllText((Join-Path $projectRoot 'CHANGELOG.md'))
    $match = [regex]::Match($changelog, '(?ms)^## v' + [regex]::Escape($version) + ' [^\n]*\n(.*?)(?=^## v|\z)')
    if (-not $match.Success) { throw 'Release changelog section is missing' }
    Write-Utf8 (Join-Path $assets 'RELEASE_NOTES.md') ("# v$version`n`n" + $match.Groups[1].Value.Trim() + "`n`n## Windows 附件`n`n便携 ZIP 与 NSIS 安装器包含同一份已验证 EXE；需要现有 Windows x64 与 WebView2 环境。本次不新增自动更新签名。源码、指南及摘要随附；安装器包内容已解包核验，安装向导运行尚待单独验证。`n`n源码提交：$env:GITHUB_SHA`n候选构建：$($info.sourceCommit)`n候选检查：https://github.com/$env:GITHUB_REPOSITORY/actions/runs/$CandidateRunId`n程序 SHA256：$ExpectedExecutableSha256`n")
}
$assetNames = @('RELEASE_NOTES.md', 'USER_GUIDE.md', 'SOURCE_MANIFEST.json', "Skill_Manager_v${version}_source.zip", "Skill_Manager_v${version}_windows_x64_portable.zip", "Skill_Manager_v${version}_windows_x64_setup.exe")
$sums = @($assetNames | Sort-Object | ForEach-Object { (Get-Sha256 (Join-Path $assets $_)) + '  ' + $_ })
Write-Utf8 (Join-Path $assets 'SHA256SUMS.txt') (($sums -join "`n") + "`n")
Write-Utf8 (Join-Path $verification 'BUILD_INFO.json') (([pscustomobject]$manifest | Select-Object -Property * -ExcludeProperty files | ConvertTo-Json -Depth 5) + "`n")
if ($env:GITHUB_OUTPUT) { [IO.File]::AppendAllText($env:GITHUB_OUTPUT, "version=$version`n", $utf8) }
if ($env:GITHUB_STEP_SUMMARY) { [IO.File]::AppendAllText($env:GITHUB_STEP_SUMMARY, "Prepared seven v$version assets from candidate $CandidateRunId. Installer and portable EXE SHA256: $ExpectedExecutableSha256. No tag or GitHub Release was created.`n", $utf8) }
Write-Output "Prepared seven release assets for review; installer archive matches the verified executable. No publication was performed."
