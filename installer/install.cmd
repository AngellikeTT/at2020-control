@echo off
rem Per-user install: no administrator rights needed.
setlocal
set "DEST=%LOCALAPPDATA%\Programs\AT2020 Control Panel"
if not exist "%DEST%" mkdir "%DEST%"
copy /y "%~dp0at2020-control.exe" "%DEST%\at2020-control.exe" >nul
copy /y "%~dp0uninstall.cmd" "%DEST%\uninstall.cmd" >nul
set "LNKDIR=%APPDATA%\Microsoft\Windows\Start Menu\Programs"
powershell -NoProfile -Command "$s=(New-Object -ComObject WScript.Shell).CreateShortcut('%LNKDIR%\AT2020 Control Panel.lnk'); $s.TargetPath='%DEST%\at2020-control.exe'; $s.WorkingDirectory='%DEST%'; $s.Save()"
set "KEY=HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\AT2020ControlPanel"
reg add "%KEY%" /v DisplayName /t REG_SZ /d "AT2020USB-X Control Panel" /f >nul
reg add "%KEY%" /v DisplayIcon /t REG_SZ /d "%DEST%\at2020-control.exe" /f >nul
reg add "%KEY%" /v UninstallString /t REG_SZ /d "\"%DEST%\uninstall.cmd\"" /f >nul
reg add "%KEY%" /v InstallLocation /t REG_SZ /d "%DEST%" /f >nul
reg add "%KEY%" /v Publisher /t REG_SZ /d "AT2020 Control Panel" /f >nul
reg add "%KEY%" /v NoModify /t REG_DWORD /d 1 /f >nul
exit /b 0
