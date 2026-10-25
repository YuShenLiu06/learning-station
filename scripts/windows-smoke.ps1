$ErrorActionPreference = 'Stop'
$installer = @(Get-ChildItem src-tauri/target/release/bundle/nsis/*-setup.exe)
if ($installer.Count -ne 1) { throw 'Expected exactly one installer' }
$root = Join-Path $env:RUNNER_TEMP 'learning-station-windows-preview'
$install = Join-Path $root 'app'
$env:LEARNING_STATION_LIBRARY = Join-Path $root 'library'
$env:LEARNING_STATION_STATE = Join-Path $root 'state'
Remove-Item Env:HOME -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force $install | Out-Null
$setup = Start-Process $installer[0].FullName -ArgumentList "/S /D=$install" -Wait -PassThru
if ($setup.ExitCode -ne 0) { throw "Installer failed: $($setup.ExitCode)" }
$node = Join-Path $install 'runtime/node.exe'
$spec = (Get-Content runtime/manifest.json -Raw | ConvertFrom-Json).windowsX64
if ((Get-FileHash $node -Algorithm SHA256).Hash.ToLowerInvariant() -ne $spec.sha256) { throw 'Bundled Node digest mismatch' }
if ((& $node --version) -ne $spec.version) { throw 'Bundled Node version mismatch' }
$entry = Join-Path $install 'pi-sidecar/sidecar.mjs'
& $node --check $entry
if ($LASTEXITCODE) { throw 'Sidecar syntax check failed' }
$app = Start-Process (Join-Path $install 'learning-station.exe') -PassThru
try {
  for ($attempt = 0; $attempt -lt 30; $attempt++) {
    Start-Sleep -Seconds 1
    $app.Refresh()
    if ($app.HasExited) { throw "Application exited during startup: $($app.ExitCode)" }
    if ($app.MainWindowHandle -ne 0 -and (Test-Path $env:LEARNING_STATION_STATE)) { break }
  }
  if ($app.MainWindowHandle -eq 0) { throw 'No application window appeared' }
  if (!(Test-Path $env:LEARNING_STATION_STATE)) { throw 'Application did not initialize isolated state' }
  New-Item -ItemType Directory -Force delivery | Out-Null
  Copy-Item $installer[0].FullName delivery/learning-station-v0.1.0-windows-x64-preview-setup.exe
  Copy-Item docs/releases/windows-preview.md delivery/README.md
  $hash = (Get-FileHash delivery/*-setup.exe -Algorithm SHA256).Hash.ToLowerInvariant()
  "$hash  learning-station-v0.1.0-windows-x64-preview-setup.exe" | Set-Content delivery/SHA256SUMS.txt -Encoding utf8
  @{
    sourceSha = $env:GITHUB_SHA
    runner = 'windows-2025'
    architecture = 'x64'
    installerSha256 = $hash
    nodeSha256 = $spec.sha256
    silentInstall = 'passed'
    startupWithoutHome = 'window and isolated state observed'
    realAccount = 'not tested'
    interactiveUserAcceptance = 'not tested'
  } | ConvertTo-Json | Set-Content delivery/build-evidence.json -Encoding utf8
} finally {
  if (!$app.HasExited) { Stop-Process -Id $app.Id -Force }
}
