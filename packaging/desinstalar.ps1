<#
  Desfaz o packaging\instalar.ps1 para o teu utilizador. NAO precisa de administrador.

  - Fecha a island.
  - Tira os hooks e a linha de estado da island do %USERPROFILE%\.claude\settings.json (com backup;
    o que e de outras ferramentas fica como esta).
  - Tira o arranque com o Windows (HKCU\...\Run).
  - Com -ApagarDados apaga tambem %LOCALAPPDATA%\dynamic-island (binarios, limites guardados) e
    %APPDATA%\dynamic-island (configuracao, lista da Tray, log).

  As notificacoes em tempo real (pacote + certificado) desfazem-se com packaging\desinstalar-notificacoes.ps1.
#>
[CmdletBinding()]
param(
    [string]$Destino = (Join-Path $env:LOCALAPPDATA "dynamic-island"),
    [string]$Definicoes = (Join-Path $env:USERPROFILE ".claude\settings.json"),
    [switch]$ApagarDados,
    [switch]$NaoParar
)
$ErrorActionPreference = "Stop"
if (-not $NaoParar) { Get-Process island -ErrorAction SilentlyContinue | Stop-Process -Force }

# Claude Code: tira os nossos hooks e a linha de estado (precisa do island-hook.exe ainda instalado).
$hook = Join-Path $Destino "island-hook.exe"
if (Test-Path $Definicoes) {
    if (Test-Path $hook) {
        $linhas = & $hook settings uninstall --settings $Definicoes
        if ($LASTEXITCODE -ne 0) { Write-Warning "Nao consegui limpar $Definicoes." }
        else { "Claude Code: $($linhas -join '; ')" }
    } else {
        Write-Warning "Sem $hook nao consigo limpar o settings.json: tira a mao os hooks e o statusLine com island-hook."
    }
} else {
    "Claude Code: nao ha $Definicoes"
}

# Arranque com o Windows.
Remove-ItemProperty -Path "HKCU:\Software\Microsoft\Windows\CurrentVersion\Run" -Name "DynamicIsland" -ErrorAction SilentlyContinue
"Arranque com o Windows: retirado"

if ($ApagarDados) {
    Remove-Item $Destino -Recurse -Force -ErrorAction SilentlyContinue
    Remove-Item (Join-Path $env:APPDATA "dynamic-island") -Recurse -Force -ErrorAction SilentlyContinue
    "Dados apagados"
} else {
    "Os ficheiros em $Destino ficam (usa -ApagarDados para os apagar)."
}
