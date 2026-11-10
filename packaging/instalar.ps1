<#
  Instala a Dynamic Island para o teu utilizador. NAO precisa de administrador.

  Depois de `cargo build --release` (ou com os binarios ja compilados ao lado deste script):
      powershell -ExecutionPolicy Bypass -File packaging\instalar.ps1

  O que faz (tudo desfazivel com packaging\desinstalar.ps1):
  1. Copia island.exe e island-hook.exe para %LOCALAPPDATA%\dynamic-island.
  2. Liga ao Claude Code: junta os hooks e a linha de estado (limites de 5 h e da semana no
     painel) ao %USERPROFILE%\.claude\settings.json. Faz backup antes, cria o ficheiro se nao
     existir, nao duplica se correres duas vezes e so toca no que e da island. Se ja tiveres uma
     linha de estado tua, pergunta antes de a substituir.
  3. Abre a island. Ela proprio se regista para arrancar com o Windows (config.toml).

  As notificacoes em tempo real precisam de administrador e sao opcionais: packaging\instalar-notificacoes.ps1.
  Sem isso a island consulta as notificacoes a cada 2 s e funciona na mesma.

  Opcoes:
    -Binarios <pasta>         onde estao island.exe e island-hook.exe (por omissao: target\release
                              ou a pasta deste script, para um .zip de binarios)
    -Destino <pasta>          onde instalar (por omissao %LOCALAPPDATA%\dynamic-island)
    -Definicoes <ficheiro>    outro settings.json do Claude Code
    -SubstituirStatusLine     substitui a tua linha de estado sem perguntar
    -SemClaudeCode            so copia os binarios, nao mexe no settings.json
    -NaoIniciar               nao abre a island no fim
    -NaoParar                 nao fecha uma island que esteja aberta
#>
[CmdletBinding()]
param(
    [string]$Destino = (Join-Path $env:LOCALAPPDATA "dynamic-island"),
    [string]$Binarios,
    [string]$Definicoes = (Join-Path $env:USERPROFILE ".claude\settings.json"),
    [switch]$SubstituirStatusLine,
    [switch]$SemClaudeCode,
    [switch]$NaoIniciar,
    [switch]$NaoParar
)
$ErrorActionPreference = "Stop"
$problemas = 0

# 1. Encontrar os binarios.
$candidatas = @()
if ($Binarios) { $candidatas += $Binarios }
$candidatas += (Join-Path (Split-Path $PSScriptRoot) "target\release"), $PSScriptRoot
$origem = $candidatas | Where-Object { (Test-Path (Join-Path $_ "island.exe")) -and (Test-Path (Join-Path $_ "island-hook.exe")) } | Select-Object -First 1
if (-not $origem) {
    throw "Nao encontro island.exe e island-hook.exe. Corre 'cargo build --release' ou usa -Binarios <pasta>."
}

# 2. Copiar. A island tem de estar fechada para o .exe poder ser substituido.
if (-not $NaoParar) { Get-Process island -ErrorAction SilentlyContinue | Stop-Process -Force }
New-Item -ItemType Directory -Force $Destino | Out-Null
Copy-Item (Join-Path $origem "island.exe"), (Join-Path $origem "island-hook.exe") $Destino -Force
# O script de teste antigo da linha de estado ja nao existe: o island-hook faz esse trabalho.
Remove-Item (Join-Path $Destino "island-statusline.ps1") -ErrorAction SilentlyContinue
$hook = Join-Path $Destino "island-hook.exe"
"1/3 ficheiros em $Destino"

# 3. Ligar ao Claude Code.
function Ler-Resultado($linhas) {
    $r = @{}
    foreach ($l in $linhas) { if ($l -match '^([a-z_]+)=(.*)$') { $r[$Matches[1]] = $Matches[2] } }
    return $r
}
if ($SemClaudeCode) {
    "2/3 Claude Code: ignorado (-SemClaudeCode)"
} else {
    # Simulacao primeiro: ve o que ia mudar sem gravar nada, para poder perguntar.
    $simulacao = Ler-Resultado (& $hook settings install --dry-run --settings $Definicoes --exe $hook)
    if ($LASTEXITCODE -ne 0) {
        Write-Warning "Nao alterei $Definicoes (ver o erro acima). Os binarios ficaram instalados."
        $problemas++
    } else {
        $substituir = $SubstituirStatusLine.IsPresent
        if ($simulacao["statusline"] -eq "outra" -and -not $substituir) {
            ""
            "Ja tens uma linha de estado no Claude Code:"
            "    $($simulacao['statusline_comando'])"
            $interativo = [Environment]::UserInteractive -and -not [Console]::IsInputRedirected
            if ($interativo) {
                $resposta = Read-Host "Substitui-la pela da Dynamic Island (mostra os limites de 5 h e da semana no painel)? [s/N]"
                $substituir = ($resposta -match '^(s|sim|y|yes)$')
            } else {
                Write-Warning "Sem consola para perguntar: mantive a tua. Usa -SubstituirStatusLine para a trocar."
            }
        }
        $argumentos = @("settings", "install", "--settings", $Definicoes, "--exe", $hook)
        if ($substituir) { $argumentos += "--replace-statusline" }
        $feito = Ler-Resultado (& $hook @argumentos)
        if ($LASTEXITCODE -ne 0) {
            Write-Warning "Nao consegui atualizar $Definicoes."
            $problemas++
        } else {
            "2/3 Claude Code ($Definicoes): hooks +$($feito['hooks_adicionados']) ~$($feito['hooks_atualizados']) =$($feito['hooks_iguais']); linha de estado: $($feito['statusline']); $($feito['resultado'])"
            if ($feito.ContainsKey("backup")) { "     backup: $($feito['backup'])" }
            if ($feito["statusline"] -eq "outra") { "     (a tua linha de estado ficou; os limites so aparecem no painel com a da island)" }
        }
    }
}

# 4. Abrir.
if (-not $NaoIniciar) {
    Start-Process (Join-Path $Destino "island.exe")
    "3/3 island aberta"
} else {
    "3/3 island nao aberta (-NaoIniciar)"
}
""
if ($problemas) { "Terminado, mas com avisos (ver acima)." } else { "Feito." }
"Reinicia as sessoes do Claude Code (os hooks sao lidos no arranque)."
"Notificacoes em tempo real (opcional, precisa de administrador): packaging\instalar-notificacoes.ps1"
if ($problemas) { exit 1 }
