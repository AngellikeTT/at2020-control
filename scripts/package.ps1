<#
 Builds the release binary and produces, in .\dist:
   at2020-control.exe                      portable single-file executable (icon embedded)
   AT2020-Control-<ver>-portable.zip       portable zip with licences
   AT2020-Control-<ver>-Setup.exe          per-user installer (Inno Setup if available, else IExpress)
 Usage:  powershell -ExecutionPolicy Bypass -File scripts\package.ps1
#>
param([string]$Out = "dist")
$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root
if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) { $env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH" }

$ver = (Select-String -Path Cargo.toml -Pattern '^version = "([^"]+)"' | Select-Object -First 1).Matches[0].Groups[1].Value
$env:AT2020_BUILD_ID = (git rev-parse --short HEAD 2>$null)
cargo build --release --workspace
if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }

New-Item -ItemType Directory -Force -Path $Out | Out-Null
$exe = Join-Path $Out "at2020-control.exe"
Copy-Item target\release\at2020-control.exe $exe -Force

# Embed the icon using a throw-away copy of the app as the tool (a running exe cannot be edited).
$tool = Join-Path $env:TEMP "at2020-embed-tool.exe"
Copy-Item target\release\at2020-control.exe $tool -Force
# The release exe uses the Windows GUI subsystem, so wait for it explicitly.
$exeFull = (Resolve-Path $exe).Path
$icoFull = (Resolve-Path assets\app.ico).Path
$proc = Start-Process -FilePath $tool -ArgumentList @("--embed-icon", "`"$exeFull`"", "`"$icoFull`"") -Wait -PassThru -WindowStyle Hidden
if ($proc.ExitCode -ne 0) { throw "embedding the icon failed (exit $($proc.ExitCode))" }
Remove-Item $tool -Force -ErrorAction SilentlyContinue

# Portable zip
$stage = Join-Path $env:TEMP "at2020-portable"
Remove-Item $stage -Recurse -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force -Path $stage | Out-Null
Copy-Item $exe $stage
Copy-Item README.md $stage
Copy-Item crates\app\assets\fonts\*-OFL.txt $stage
$zip = Join-Path $Out "AT2020-Control-$ver-portable.zip"
Remove-Item $zip -Force -ErrorAction SilentlyContinue
Compress-Archive -Path "$stage\*" -DestinationPath $zip

# Installer
$setup = Join-Path $Out "AT2020-Control-$ver-Setup.exe"
Remove-Item $setup -Force -ErrorAction SilentlyContinue
$iscc = Get-Command iscc -ErrorAction SilentlyContinue
$isccPath = $null
if ($iscc) { $isccPath = $iscc.Source }
else {
    foreach ($p in @("${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe", "$env:ProgramFiles\Inno Setup 6\ISCC.exe", "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe")) {
        if (Test-Path $p) { $isccPath = $p; break }
    }
}
$outFull = (Resolve-Path $Out).Path
if ($isccPath) {
    & $isccPath "/DAppVersion=$ver" "/DSourceExe=$((Resolve-Path $exe).Path)" "/DOutDir=$outFull" installer\at2020-control.iss
    if ($LASTEXITCODE -ne 0) { throw "Inno Setup failed" }
}
else {
    Write-Host "Inno Setup not found; building an IExpress installer instead."
    $work = Join-Path $env:TEMP "at2020-iexpress"
    Remove-Item $work -Recurse -Force -ErrorAction SilentlyContinue
    New-Item -ItemType Directory -Force -Path $work | Out-Null
    Copy-Item $exe $work
    Copy-Item installer\install.cmd, installer\uninstall.cmd $work
    $lines = @(
        '[Version]', 'Class=IEXPRESS', 'SEDVersion=3',
        '[Options]', 'PackagePurpose=InstallApp', 'ShowInstallProgramWindow=0', 'HideExtractAnimation=1',
        'UseLongFileName=1', 'InsideCompressed=0', 'CAB_FixedSize=0', 'CAB_ResvCodeSigning=0', 'RebootMode=N',
        'InstallPrompt=%InstallPrompt%', 'DisplayLicense=%DisplayLicense%', 'FinishMessage=%FinishMessage%',
        'TargetName=%TargetName%', 'FriendlyName=%FriendlyName%', 'AppLaunched=%AppLaunched%',
        'PostInstallCmd=%PostInstallCmd%', 'AdminQuietInstCmd=%AdminQuietInstCmd%',
        'UserQuietInstCmd=%UserQuietInstCmd%', 'SourceFiles=SourceFiles',
        '[Strings]',
        'InstallPrompt=Install AT2020USB-X Control Panel for the current user?',
        'DisplayLicense=',
        'FinishMessage=AT2020USB-X Control Panel is installed. Find it in the Start menu.',
        "TargetName=$setup",
        'FriendlyName=AT2020USB-X Control Panel Setup',
        'AppLaunched=cmd /c install.cmd', 'PostInstallCmd=<None>',
        'AdminQuietInstCmd=cmd /c install.cmd', 'UserQuietInstCmd=cmd /c install.cmd',
        'FILE0="install.cmd"', 'FILE1="uninstall.cmd"', 'FILE2="at2020-control.exe"',
        '[SourceFiles]', "SourceFiles0=$work\",
        '[SourceFiles0]', '%FILE0%=', '%FILE1%=', '%FILE2%='
    )
    $sedPath = Join-Path $work "setup.sed"
    Set-Content -Path $sedPath -Value $lines -Encoding ASCII
    $setupFull = Join-Path $outFull "AT2020-Control-$ver-Setup.exe"
    (Get-Content $sedPath) -replace [regex]::Escape("TargetName=$setup"), "TargetName=$setupFull" | Set-Content $sedPath -Encoding ASCII
    & "$env:WINDIR\System32\iexpress.exe" /N /Q $sedPath | Out-Null
    $deadline = (Get-Date).AddSeconds(90)
    while (-not (Test-Path $setupFull) -and (Get-Date) -lt $deadline) { Start-Sleep -Milliseconds 500 }
    if (-not (Test-Path $setupFull)) { throw "IExpress did not produce the installer" }
}
Get-ChildItem $Out | Select-Object Name, Length
