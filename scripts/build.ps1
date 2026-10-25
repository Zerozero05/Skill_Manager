param(
    [string]$DependencyRoot,
    [ValidateSet('Check','Test','Build','Package','Dev')][string]$Task='Build'
)
. (Join-Path $PSScriptRoot 'environment.ps1') -DependencyRoot $DependencyRoot
Push-Location $projectRoot
try {
    if($Task -eq 'Dev') { & pnpm tauri dev; if($LASTEXITCODE){throw 'Development launch failed'}; return }
    if($Task -eq 'Package') { & pnpm tauri build --bundles nsis; if($LASTEXITCODE){throw 'Windows packaging failed'}; return }
    & pnpm build
    if($LASTEXITCODE){throw 'Frontend build failed'}
    Push-Location (Join-Path $projectRoot 'src-tauri')
    try {
        if($Task -eq 'Check') { & cargo check --locked }
        elseif($Task -eq 'Test') { & cargo test --locked }
        else { & cargo build --release --locked --features custom-protocol }
        if($LASTEXITCODE){throw 'Rust build/test failed'}
    } finally { Pop-Location }
} finally { Pop-Location }
