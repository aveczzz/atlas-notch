<#
  Diagnostico da maquina para a Dynamic Island. SO LEITURA: nao instala, nao altera registo,
  nao escreve fora de um ficheiro de relatorio opcional. Nao precisa de administrador
  (se tiveres, algumas verificacoes ficam mais completas).

      powershell -NoProfile -ExecutionPolicy Bypass -File packaging\diagnostico.ps1
      powershell -NoProfile -ExecutionPolicy Bypass -File packaging\diagnostico.ps1 -Saida relatorio.txt

  Se a politica de execucao bloquear o .ps1, cola o conteudo numa janela do PowerShell.
  O nome do utilizador e o da maquina sao substituidos por <utilizador> e <maquina> no relatorio.
  Nota: o Add-Type compila pequenos auxiliares em memoria (podes ver ficheiros temporarios em %TEMP%).
#>
[CmdletBinding()]
param([string]$Saida)
$ErrorActionPreference = "Continue"
$script:linhas = New-Object System.Collections.Generic.List[string]

function Limpar([string]$t) {
    if ($env:USERNAME)     { $t = $t -replace [regex]::Escape($env:USERNAME), '<utilizador>' }
    if ($env:COMPUTERNAME) { $t = $t -replace [regex]::Escape($env:COMPUTERNAME), '<maquina>' }
    return $t
}
function Say([string]$t = "") { $t = Limpar $t; $script:linhas.Add($t); Write-Host $t }
function Titulo([string]$t) { Say ""; Say "== $t =="; }
function Seguro([string]$nome, [scriptblock]$bloco) {
    try { & $bloco } catch { Say "  ${nome}: n/d ($($_.Exception.Message.Split("`n")[0]))" }
}

Say "Dynamic Island: diagnostico ($(Get-Date -Format 'yyyy-MM-dd HH:mm'))"

# ---------------------------------------------------------------- Sistema
Titulo "Sistema"
Seguro "sistema" {
    $os = Get-CimInstance Win32_OperatingSystem
    $cs = Get-CimInstance Win32_ComputerSystem
    $bios = Get-CimInstance Win32_BIOS
    Say "  Windows: $($os.Caption) versao $($os.Version) (build $($os.BuildNumber)), $($os.OSArchitecture)"
    Say "  Fabricante/modelo: $($cs.Manufacturer) / $($cs.Model)"
    Say "  BIOS: $($bios.Manufacturer) $($bios.SMBIOSBIOSVersion)"
    $texto = "$($cs.Manufacturer) $($cs.Model) $($bios.Manufacturer) $($bios.SMBIOSBIOSVersion)"
    $virtual = $texto -match 'VMware|Virtual|VirtualBox|Parallels|QEMU|KVM|Xen|HVM|Hyper-V'
    $apple = $texto -match 'Apple'
    $apleDrivers = @(Get-CimInstance Win32_PnPSignedDriver -ErrorAction SilentlyContinue | Where-Object { $_.Manufacturer -match 'Apple' })
    $bootCamp = (Test-Path "$env:ProgramFiles\Boot Camp") -or (@(Get-Service -ErrorAction SilentlyContinue | Where-Object { $_.DisplayName -match 'Boot Camp|Apple' }).Count -gt 0)
    if ($virtual) { Say "  Tipo: MAQUINA VIRTUAL ($texto)" }
    elseif ($apple) { Say "  Tipo: hardware Apple a correr Windows diretamente = Boot Camp (drivers Apple: $($apleDrivers.Count); painel Boot Camp/servicos: $bootCamp)" }
    else { Say "  Tipo: PC normal (nao e Mac nem VM)" }
    Say "  Hypervisor presente: $($cs.HypervisorPresent)  (VBS/Hyper-V do Windows tambem o ativam; nao quer dizer VM)"
    Say "  Dominio: $(if ($cs.PartOfDomain) { 'sim (' + $cs.Domain + ')' } else { 'nao' })"
}
Seguro "gestao" {
    $dsreg = (& dsregcmd /status 2>$null) -join "`n"
    foreach ($k in 'AzureAdJoined', 'DomainJoined', 'WorkplaceJoined', 'MdmUrl') {
        if ($dsreg -match "$k\s*:\s*(.+)") { Say "  ${k}: $($Matches[1].Trim())" }
    }
    $mdm = @(Get-ChildItem 'HKLM:\SOFTWARE\Microsoft\Enrollments' -ErrorAction SilentlyContinue | Where-Object { (Get-ItemProperty $_.PSPath -ErrorAction SilentlyContinue).ProviderID })
    Say "  Inscricoes MDM (Intune e afins): $($mdm.Count)"
}

# ---------------------------------------------------------------- Permissoes
Titulo "Permissoes e politicas"
Seguro "admin" {
    $id = [Security.Principal.WindowsIdentity]::GetCurrent()
    $p = New-Object Security.Principal.WindowsPrincipal($id)
    $elevado = $p.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
    $membro = $id.Groups -contains (New-Object Security.Principal.SecurityIdentifier 'S-1-5-32-544')
    Say "  Esta janela esta elevada (administrador): $elevado"
    Say "  Conta no grupo Administradores (mesmo sem elevar): $membro"
}
Seguro "politica" {
    Say "  ExecutionPolicy (Get-ExecutionPolicy -List):"
    Get-ExecutionPolicy -List | ForEach-Object { Say ("    {0,-14} {1}" -f $_.Scope, $_.ExecutionPolicy) }
    Say "  PowerShell: $($PSVersionTable.PSVersion) ($($PSVersionTable.PSEdition)); modo de linguagem: $($ExecutionContext.SessionState.LanguageMode)"
    if ($ExecutionContext.SessionState.LanguageMode -ne 'FullLanguage') { Say "  ATENCAO: modo restrito (AppLocker/WDAC): os .ps1 do instalador podem nao correr." }
}
Seguro "applocker" {
    $srp = Get-ChildItem 'HKLM:\SOFTWARE\Policies\Microsoft\Windows\SrpV2' -ErrorAction SilentlyContinue
    Say "  AppLocker (regras no registo): $(if ($srp) { 'SIM: ' + (($srp | ForEach-Object { $_.PSChildName }) -join ', ') } else { 'nao encontrado' })"
    $dg = Get-CimInstance -Namespace 'root\Microsoft\Windows\DeviceGuard' -ClassName Win32_DeviceGuard -ErrorAction SilentlyContinue
    if ($dg) {
        $nomes = @{ 0 = 'desligado'; 1 = 'auditoria'; 2 = 'imposto' }
        Say "  WDAC/Device Guard - integridade de codigo em modo utilizador: $($nomes[[int]$dg.UsermodeCodeIntegrityPolicyEnforcementStatus]); modo kernel: $($nomes[[int]$dg.CodeIntegrityPolicyEnforcementStatus])"
    }
}

# ---------------------------------------------------------------- Ecras
Titulo "Ecras (DPI por monitor)"
Seguro "monitores" {
    Add-Type -TypeDefinition @"
using System; using System.Collections.Generic; using System.Runtime.InteropServices;
public static class IslandMon {
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L, T, R, B; }
  [StructLayout(LayoutKind.Sequential, CharSet=CharSet.Unicode)] public struct MONITORINFOEX { public int cbSize; public RECT rcMonitor; public RECT rcWork; public int flags; [MarshalAs(UnmanagedType.ByValTStr, SizeConst=32)] public string szDevice; }
  delegate bool MonitorEnumProc(IntPtr m, IntPtr dc, ref RECT r, IntPtr data);
  [DllImport("user32.dll")] static extern bool EnumDisplayMonitors(IntPtr dc, IntPtr clip, MonitorEnumProc cb, IntPtr data);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] static extern bool GetMonitorInfoW(IntPtr m, ref MONITORINFOEX i);
  [DllImport("shcore.dll")] static extern int GetDpiForMonitor(IntPtr m, int type, out uint x, out uint y);
  [DllImport("user32.dll")] static extern bool SetProcessDpiAwarenessContext(IntPtr v);
  public static string[] List() {
    SetProcessDpiAwarenessContext(new IntPtr(-4)); // per-monitor v2: pixeis fisicos
    var r = new List<string>();
    EnumDisplayMonitors(IntPtr.Zero, IntPtr.Zero, (IntPtr m, IntPtr dc, ref RECT rc, IntPtr d) => {
      var i = new MONITORINFOEX(); i.cbSize = Marshal.SizeOf(typeof(MONITORINFOEX)); GetMonitorInfoW(m, ref i);
      uint dx = 96, dy = 96; GetDpiForMonitor(m, 0, out dx, out dy);
      r.Add(string.Format("{0}: {1}x{2} px em ({3},{4}), escala {5}% (DPI {6}){7}", i.szDevice,
        i.rcMonitor.R - i.rcMonitor.L, i.rcMonitor.B - i.rcMonitor.T, i.rcMonitor.L, i.rcMonitor.T, (int)Math.Round(dx * 100.0 / 96), dx, (i.flags & 1) != 0 ? ", PRINCIPAL" : ""));
      return true; }, IntPtr.Zero);
    return r.ToArray();
  }
}
"@
    $lista = [IslandMon]::List()
    Say "  Monitores ativos: $($lista.Count)"
    $lista | ForEach-Object { Say "    $_" }
}
Seguro "ligacoes" {
    # Quem e o ecra interno (o que a island procura): a ligacao fisica de cada ecra.
    $nomes = @{ '0' = 'VGA'; '4' = 'DVI'; '5' = 'HDMI'; '6' = 'LVDS (interno)'; '10' = 'DisplayPort externo'; '11' = 'DisplayPort EMBEBIDO (interno)'; '12' = 'UDI externo'; '13' = 'UDI EMBEBIDO (interno)'; '15' = 'Miracast'; '16' = 'indireto'; '2147483648' = 'INTERNO' }
    $ligs = Get-CimInstance -Namespace root\wmi -ClassName WmiMonitorConnectionParams -ErrorAction SilentlyContinue
    $ids = @{}
    Get-CimInstance -Namespace root\wmi -ClassName WmiMonitorID -ErrorAction SilentlyContinue | ForEach-Object {
        $ids[$_.InstanceName] = (($_.UserFriendlyName | Where-Object { $_ -ne 0 } | ForEach-Object { [char]$_ }) -join '')
    }
    foreach ($l in $ligs) {
        $t = [uint32]$l.VideoOutputTechnology
        $nome = $nomes["$t"]; if (-not $nome) { $nome = "tecnologia $t" }
        $modelo = $ids[$l.InstanceName]; if (-not $modelo) { $modelo = '(sem nome)' }
        Say "    $modelo -> $nome$(if (-not $l.Active) { ' [inativo]' })"
    }
    if (-not $ligs) { Say "    (WMI nao devolveu ligacoes)" }
}
Seguro "gpu" {
    Get-CimInstance Win32_VideoController | ForEach-Object { Say "  GPU: $($_.Name), driver $($_.DriverVersion), $($_.CurrentHorizontalResolution)x$($_.CurrentVerticalResolution)" }
}

# ---------------------------------------------------------------- Ferramentas
Titulo "Ferramentas"
function Versao($nome, $comando, $args2) {
    $c = Get-Command $comando -ErrorAction SilentlyContinue
    if (-not $c) { Say "  ${nome}: NAO instalado (ou fora do PATH)"; return }
    $v = try { (& $c.Source @args2 2>$null | Select-Object -First 1) } catch { '?' }
    Say "  ${nome}: $v  [$($c.Source)]"
}
Seguro "ferramentas" {
    Versao "rustc" "rustc" @("--version")
    Versao "cargo" "cargo" @("--version")
    Versao "rustup" "rustup" @("--version")
    Versao "git" "git" @("--version")
    Versao "claude (Claude Code)" "claude" @("--version")
    $bash = Get-Command bash -ErrorAction SilentlyContinue
    $gitBash = @("$env:ProgramFiles\Git\bin\bash.exe", "${env:ProgramFiles(x86)}\Git\bin\bash.exe", "$env:LOCALAPPDATA\Programs\Git\bin\bash.exe") | Where-Object { Test-Path $_ } | Select-Object -First 1
    Say "  Git Bash: $(if ($gitBash) { $gitBash } elseif ($bash) { $bash.Source + ' (outro bash, p. ex. WSL)' } else { 'NAO encontrado' })"
    $vs = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
    if (Test-Path $vs) { $p = & $vs -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property displayName 2>$null; Say "  Visual Studio Build Tools (C++): $(if ($p) { $p } else { 'sem o componente C++ (o Rust precisa dele para ligar)' })" }
    else { Say "  Visual Studio Build Tools: NAO instalado (o Rust precisa do 'Desktop development with C++')" }
    $sdk = Get-ChildItem "${env:ProgramFiles(x86)}\Windows Kits\10\bin\10.*\x64\makeappx.exe" -ErrorAction SilentlyContinue | Select-Object -Last 1
    Say "  Windows SDK (makeappx, so para as notificacoes em tempo real): $(if ($sdk) { 'sim' } else { 'nao' })"
    Say "  ~/.claude existe: $(Test-Path (Join-Path $env:USERPROFILE '.claude')); settings.json: $(Test-Path (Join-Path $env:USERPROFILE '.claude\settings.json'))"
    $inst = Join-Path $env:LOCALAPPDATA 'dynamic-island'
    Say "  Island ja instalada em %LOCALAPPDATA%\dynamic-island: $(Test-Path (Join-Path $inst 'island.exe')); a correr: $([bool](Get-Process island -ErrorAction SilentlyContinue))"
}
Seguro "escrita" {
    foreach ($d in @($env:LOCALAPPDATA, $env:APPDATA, $env:TEMP)) {
        $t = Join-Path $d ("island-teste-" + [guid]::NewGuid().ToString('N').Substring(0, 6))
        $ok = try { New-Item -ItemType Directory $t -ErrorAction Stop | Out-Null; Remove-Item $t -Force; $true } catch { $false }
        Say "  Posso criar pastas em ${d}: $ok"
    }
}

# ---------------------------------------------------------------- Seguranca
Titulo "Antivirus / EDR"
Seguro "av" {
    $av = @(Get-CimInstance -Namespace root\SecurityCenter2 -ClassName AntiVirusProduct -ErrorAction SilentlyContinue)
    if ($av.Count) {
        foreach ($a in $av) {
            $estado = [int]$a.productState
            $ligado = (($estado -shr 12) -band 0xF) -eq 1
            $atual = (($estado -shr 4) -band 0xF) -eq 0
            Say "  Antivirus registado: $($a.displayName)  (ativo: $ligado; assinaturas em dia: $atual)"
        }
    } else { Say "  Centro de seguranca: nenhum antivirus registado (normal em servidores/Windows gerido)" }
}
Seguro "defender" {
    $mp = Get-MpComputerStatus -ErrorAction Stop
    Say "  Microsoft Defender: antivirus=$($mp.AntivirusEnabled), tempo real=$($mp.RealTimeProtectionEnabled), anti-adulteracao=$($mp.IsTamperProtected), modo=$($mp.AMRunningMode)"
}
Seguro "edr" {
    $conhecidos = 'CSFalcon|CrowdStrike|SentinelAgent|SentinelOne|CarbonBlack|cb\.exe|Cylance|MsSense|SenseCnc|Sophos|Cortex|Traps|cyserver|TaniumClient|Tanium|Symantec|SepMaster|McAfee|mfemms|ESET|ekrn|Trend|ntrtscan|Kaspersky|avp\.exe|Bitdefender|bdservicehost|FortiEDR|Elastic|elastic-endpoint|Zscaler|ZSAService|Netskope|Jamf|ForeScout'
    $proc = @(Get-Process -ErrorAction SilentlyContinue | Where-Object { $_.ProcessName -match $conhecidos } | Select-Object -ExpandProperty ProcessName -Unique)
    $svc = @(Get-Service -ErrorAction SilentlyContinue | Where-Object { $_.Status -eq 'Running' -and ($_.Name -match $conhecidos -or $_.DisplayName -match $conhecidos) } | Select-Object -ExpandProperty DisplayName -Unique)
    Say "  Processos de seguranca/gestao conhecidos: $(if ($proc.Count) { $proc -join ', ' } else { 'nenhum reconhecido' })"
    Say "  Servicos a correr: $(if ($svc.Count) { ($svc | Select-Object -First 8) -join '; ' } else { 'nenhum reconhecido' })"
    Say "  (Um EDR pode bloquear executaveis novos sem assinatura, como o island.exe compilado localmente.)"
}

# ---------------------------------------------------------------- Hardware
Titulo "Bateria, carregador, audio, teclado, media"
Seguro "bateria" {
    Add-Type -TypeDefinition @"
using System; using System.Runtime.InteropServices;
public static class IslandPower {
  [StructLayout(LayoutKind.Sequential)] public struct SPS { public byte AC; public byte Flag; public byte Percent; public byte Saver; public int Life; public int Full; }
  [DllImport("kernel32.dll")] static extern bool GetSystemPowerStatus(out SPS s);
  public static string Read() { SPS s; if (!GetSystemPowerStatus(out s)) return "falhou";
    return string.Format("carregador={0} (0=bateria, 1=corrente, 255=?), bateria={1}% (flag {2}; 128 = sem bateria)", s.AC, s.Percent == 255 ? -1 : s.Percent, s.Flag); }
}
"@
    Say "  Estado da energia (GetSystemPowerStatus): $([IslandPower]::Read())"
    $b = @(Get-CimInstance Win32_Battery -ErrorAction SilentlyContinue)
    if ($b.Count) { $b | ForEach-Object { Say "  Bateria detetada: $($_.Name), carga $($_.EstimatedChargeRemaining)%, estado $($_.BatteryStatus) (1=a descarregar, 2=corrente)" } }
    else { Say "  Bateria: NENHUMA detetada pelo WMI (a island nao daria avisos de bateria)" }
}
Seguro "audio" {
    $dev = @(Get-CimInstance Win32_SoundDevice -ErrorAction SilentlyContinue)
    Say "  Dispositivos de som: $($dev.Count)"
    $dev | ForEach-Object { Say "    $($_.Name) [$($_.Status)] (fabricante $($_.Manufacturer))" }
    $pontos = @(Get-CimInstance Win32_PnPEntity -Filter "PNPClass='AudioEndpoint'" -ErrorAction SilentlyContinue | Where-Object { $_.Status -eq 'OK' })
    Say "  Pontos de audio ativos (altifalantes, auscultadores...): $($pontos.Count)"
    $pontos | Select-Object -First 6 | ForEach-Object { Say "    $($_.Name)" }
}
Seguro "teclado" {
    $k = @(Get-CimInstance Win32_Keyboard -ErrorAction SilentlyContinue)
    Say "  Teclados: $(($k | ForEach-Object { $_.Name }) -join '; ')  (tipo/teclas de funcao: $(($k | ForEach-Object { $_.NumberOfFunctionKeys }) -join ','))"
    Say "  Estado atual: Caps Lock=$([Console]::CapsLock), Num Lock=$([Console]::NumberLock)  (os teclados de Mac nao tem Num/Scroll Lock)"
}
Seguro "gsmtc" {
    Add-Type -AssemblyName System.Runtime.WindowsRuntime
    $asTask = ([System.WindowsRuntimeSystemExtensions].GetMethods() | Where-Object { $_.Name -eq 'AsTask' -and $_.GetParameters().Count -eq 1 -and $_.GetParameters()[0].ParameterType.Name -eq 'IAsyncOperation`1' })[0]
    $tipo = [Windows.Media.Control.GlobalSystemMediaTransportControlsSessionManager, Windows.Media.Control, ContentType = WindowsRuntime]
    $tarefa = $asTask.MakeGenericMethod($tipo).Invoke($null, @($tipo::RequestAsync()))
    $tarefa.Wait(5000) | Out-Null
    $gestor = $tarefa.Result
    $sessoes = @($gestor.GetSessions())
    Say "  GSMTC (controlo de media do Windows): disponivel; sessoes agora: $($sessoes.Count)"
    $sessoes | ForEach-Object { Say "    $($_.SourceAppUserModelId)" }
    if (-not $sessoes.Count) { Say "    (abre o Spotify, o browser ou o Leitor Multimedia a tocar e volta a correr)" }
}

if ($Saida) {
    [IO.File]::WriteAllLines($Saida, $script:linhas, (New-Object Text.UTF8Encoding $true))
    Say ""
    Say "Relatorio guardado em $Saida"
}
