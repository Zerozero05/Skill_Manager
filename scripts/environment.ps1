param([string]$DependencyRoot)
$ErrorActionPreference='Stop'
$projectRoot=Split-Path -Parent $PSScriptRoot
if (!$DependencyRoot) {
    $developerConfig=Join-Path $projectRoot 'developer-paths.json'
    if (Test-Path -LiteralPath $developerConfig) {
        $developerPaths=Get-Content -LiteralPath $developerConfig -Raw | ConvertFrom-Json
        $DependencyRoot=$developerPaths.dependencyRoot
        if (!$DependencyRoot) { throw 'developer-paths.json must specify dependencyRoot' }
    } else { $DependencyRoot=$projectRoot }
}
if (![System.IO.Path]::IsPathFullyQualified($DependencyRoot)) { throw 'DependencyRoot must be an absolute directory' }
$DependencyRoot=[System.IO.Path]::GetFullPath($DependencyRoot)
if ($DependencyRoot) {
    # Builds reuse the chosen environment; dependency checks must not install or purge it implicitly.
    $env:pnpm_config_verify_deps_before_run='warn'
    $env:CARGO_HOME=Join-Path $DependencyRoot 'rust\cargo'
    $env:RUSTUP_HOME=Join-Path $DependencyRoot 'rust\rustup'
    $env:CARGO_TARGET_DIR=Join-Path $DependencyRoot 'target'
    $env:PNPM_HOME=Join-Path $DependencyRoot 'pnpm'
    $env:pnpm_config_store_dir=Join-Path $DependencyRoot 'pnpm-store'
    $env:npm_config_cache=Join-Path $DependencyRoot 'npm-cache'
    $env:COREPACK_HOME=Join-Path $DependencyRoot 'corepack'
    $env:XDG_CACHE_HOME=Join-Path $DependencyRoot 'cache'
    $env:TEMP=Join-Path $DependencyRoot 'temp'
    $env:TMP=$env:TEMP
    $env:PATH=(Join-Path $env:CARGO_HOME 'bin')+';'+$env:PATH
    $portable=Join-Path $DependencyRoot 'msvc-portable\msvc'
    if(Test-Path -LiteralPath $portable) {
        $toolset=Get-ChildItem -LiteralPath (Join-Path $portable 'VC\Tools\MSVC') -Directory | Sort-Object Name -Descending | Select-Object -First 1
        $sdk=Get-ChildItem -LiteralPath (Join-Path $portable 'Windows Kits\10\Include') -Directory | Sort-Object Name -Descending | Select-Object -First 1
        if(!$toolset -or !$sdk){throw 'Portable compiler or SDK is incomplete'}
        $env:PATH=(Join-Path $toolset.FullName 'bin\Hostx64\x64')+';'+(Join-Path $portable "Windows Kits\10\bin\$($sdk.Name)\x64")+';'+$env:PATH
        $env:INCLUDE=(Join-Path $toolset.FullName 'include')+';'+(Join-Path $sdk.FullName 'ucrt')+';'+(Join-Path $sdk.FullName 'shared')+';'+(Join-Path $sdk.FullName 'um')+';'+(Join-Path $sdk.FullName 'winrt')
        $env:LIB=(Join-Path $toolset.FullName 'lib\x64')+';'+(Join-Path $portable "Windows Kits\10\Lib\$($sdk.Name)\ucrt\x64")+';'+(Join-Path $portable "Windows Kits\10\Lib\$($sdk.Name)\um\x64")
        $env:VCToolsInstallDir=$toolset.FullName+'\'
        $env:WindowsSdkDir=(Join-Path $portable 'Windows Kits\10')+'\'
        $env:VSCMD_ARG_TGT_ARCH='x64'
        $env:VSCMD_ARG_HOST_ARCH='x64'
    }
}
