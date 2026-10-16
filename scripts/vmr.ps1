<#
 Read-mostly helper around Voicemeeter's official Remote API (VoicemeeterRemote64.dll),
 used to diagnose how Voicemeeter holds the microphone.

   . scripts\vmr.ps1; Get-VmState               # strips, devices, live input levels
   . scripts\vmr.ps1; Restart-VmEngine          # Command.Restart = 1 (what Voicemeeter's own
                                                # menu "Restart Audio Engine" does)
#>
$dll = "C:\Program Files (x86)\VB\Voicemeeter\VoicemeeterRemote64.dll"
if (-not ("VMR" -as [type])) {
    Add-Type -ErrorAction Stop @"
using System; using System.Runtime.InteropServices; using System.Text;
public static class VMR {
  const string D = @"$dll";
  [DllImport(D)] public static extern int VBVMR_Login();
  [DllImport(D)] public static extern int VBVMR_Logout();
  [DllImport(D)] public static extern int VBVMR_IsParametersDirty();
  [DllImport(D)] public static extern int VBVMR_GetVoicemeeterType(out int t);
  [DllImport(D, CharSet=CharSet.Ansi)] public static extern int VBVMR_GetParameterFloat(string n, out float v);
  [DllImport(D, CharSet=CharSet.Ansi)] public static extern int VBVMR_GetParameterStringA(string n, StringBuilder s);
  [DllImport(D, CharSet=CharSet.Ansi)] public static extern int VBVMR_SetParameterFloat(string n, float v);
  [DllImport(D)] public static extern int VBVMR_Input_GetDeviceNumber();
  [DllImport(D, CharSet=CharSet.Ansi)] public static extern int VBVMR_Input_GetDeviceDescA(int i, out int type, StringBuilder name, StringBuilder hwid);
  [DllImport(D)] public static extern int VBVMR_GetLevel(int type, int ch, out float v);
}
"@
}

function Get-VmState {
    $null = [VMR]::VBVMR_Login(); Start-Sleep -Milliseconds 300; $null = [VMR]::VBVMR_IsParametersDirty()
    $t = 0; $null = [VMR]::VBVMR_GetVoicemeeterType([ref]$t); "voicemeeter type=$t (1=standard, 2=banana, 3=potato)"
    foreach ($s in 0, 1) {
        $sb = New-Object Text.StringBuilder 512; $null = [VMR]::VBVMR_GetParameterStringA("Strip[$s].device.name", $sb)
        $sr = 0.0; $null = [VMR]::VBVMR_GetParameterFloat("Strip[$s].device.sr", [ref]$sr)
        "Strip[$s] device='$sb' sr=$sr"
    }
    $peak = @(0.0, 0.0, 0.0, 0.0)
    for ($k = 0; $k -lt 20; $k++) {
        foreach ($ch in 0..3) { $v = 0.0; $null = [VMR]::VBVMR_GetLevel(0, $ch, [ref]$v); if ($v -gt $peak[$ch]) { $peak[$ch] = $v } }
        Start-Sleep -Milliseconds 100
    }
    "input peak levels over 2 s (strip0 L/R, strip1 L/R): " + (($peak | ForEach-Object { [math]::Round($_, 6) }) -join ', ')
    $n = [VMR]::VBVMR_Input_GetDeviceNumber()
    for ($i = 0; $i -lt $n; $i++) {
        $nm = New-Object Text.StringBuilder 512; $hw = New-Object Text.StringBuilder 512; $ty = 0
        $null = [VMR]::VBVMR_Input_GetDeviceDescA($i, [ref]$ty, $nm, $hw)
        if ($nm.ToString() -match 'AT2020' -or $ty -eq 4) { "  input device: type=$ty (1=MME 3=WDM 4=KS 5=ASIO) '$nm'" }
    }
    $null = [VMR]::VBVMR_Logout()
}

function Restart-VmEngine {
    $null = [VMR]::VBVMR_Login(); Start-Sleep -Milliseconds 300
    "Command.Restart -> " + [VMR]::VBVMR_SetParameterFloat("Command.Restart", 1.0)
    Start-Sleep -Seconds 3
    $null = [VMR]::VBVMR_Logout()
}
