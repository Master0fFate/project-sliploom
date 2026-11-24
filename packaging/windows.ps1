param(
    [Parameter(Mandatory)] [string] $Version,
    [Parameter(Mandatory)] [string] $Package
)

$ErrorActionPreference = 'Stop'
$binary = (Resolve-Path 'apps/desktop/target/release/seam-desktop.exe').Path
$notice = (Resolve-Path 'apps/desktop/assets/Roboto-OFL.txt').Path
$nsis = Join-Path ${env:ProgramFiles(x86)} 'NSIS/makensis.exe'
$Package = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($Package)
New-Item -ItemType Directory -Path (Split-Path -Parent $Package) -Force | Out-Null

$reader = [IO.BinaryReader]::new([IO.File]::OpenRead($binary))
try {
    $reader.BaseStream.Position = 0x3c
    $reader.BaseStream.Position = $reader.ReadInt32()
    if ($reader.ReadUInt32() -ne 0x4550 -or $reader.ReadUInt16() -ne 0x8664) {
        throw 'Expected a Windows x64 executable'
    }
} finally {
    $reader.Dispose()
}

& $nsis /V2 "/DPRODUCT_VERSION=$Version" "/DAPP_PATH=$binary" "/DFONT_NOTICE=$notice" "/DPACKAGE_PATH=$Package" packaging/Sliploom.nsi
if ($LASTEXITCODE -ne 0 -or !(Test-Path -LiteralPath $Package -PathType Leaf)) {
    throw 'NSIS did not produce an installer'
}

$installDir = Join-Path $env:LOCALAPPDATA 'Programs/Sliploom'
$uninstallKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\Sliploom'
$shortcut = Join-Path ([Environment]::GetFolderPath('Programs')) 'Sliploom.lnk'
if ((Test-Path $installDir) -or (Test-Path $uninstallKey) -or (Test-Path $shortcut)) {
    throw 'Installer verification needs a user profile without Sliploom installed'
}

$install = Start-Process -FilePath $Package -ArgumentList '/S' -Wait -PassThru
if ($install.ExitCode -ne 0) { throw "Sliploom install failed: $($install.ExitCode)" }
foreach ($file in @($binary, $notice)) {
    $installed = Join-Path $installDir (Split-Path -Leaf $file)
    if ((Get-FileHash -LiteralPath $file).Hash -ne (Get-FileHash -LiteralPath $installed).Hash) {
        throw "Unexpected installer payload: $installed"
    }
}
if (!(Test-Path $shortcut) -or (Get-ItemPropertyValue -Path $uninstallKey -Name DisplayVersion) -ne $Version) {
    throw 'Missing Start menu shortcut or incorrect uninstall metadata'
}

$uninstaller = Join-Path ([IO.Path]::GetTempPath()) "sliploom-uninstall-$([guid]::NewGuid()).exe"
Copy-Item -LiteralPath (Join-Path $installDir 'Uninstall.exe') -Destination $uninstaller
try {
    $uninstall = Start-Process -FilePath $uninstaller -ArgumentList "/S _?=$installDir" -Wait -PassThru
    if ($uninstall.ExitCode -ne 0 -or (Test-Path $installDir) -or (Test-Path $uninstallKey) -or (Test-Path $shortcut)) {
        throw 'Sliploom uninstall failed or left installed files'
    }
} finally {
    Remove-Item -LiteralPath $uninstaller
}

$hash = (Get-FileHash -Algorithm SHA256 -LiteralPath $Package).Hash.ToLowerInvariant()
Set-Content -LiteralPath "$Package.sha256" -Value "$hash  $(Split-Path -Leaf $Package)`n" -NoNewline -Encoding ascii
