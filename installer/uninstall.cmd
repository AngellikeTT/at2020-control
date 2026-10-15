@echo off
setlocal
set "DEST=%LOCALAPPDATA%\Programs\AT2020 Control Panel"
taskkill /im at2020-control.exe /f >nul 2>&1
del /f /q "%APPDATA%\Microsoft\Windows\Start Menu\Programs\AT2020 Control Panel.lnk" >nul 2>&1
reg delete "HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\AT2020ControlPanel" /f >nul 2>&1
reg delete "HKCU\Software\Microsoft\Windows\CurrentVersion\Run" /v "AT2020 Control Panel" /f >nul 2>&1
rem Settings in %APPDATA%\AT2020 Control Panel are kept; delete that folder to remove them.
start "" /min cmd /c "ping 127.0.0.1 -n 2 >nul & rmdir /s /q ""%DEST%"""
exit /b 0
