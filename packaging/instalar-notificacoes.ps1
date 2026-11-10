#Requires -RunAsAdministrator
<#
  OPCIONAL, precisa de administrador. Da a island identidade de pacote (sparse package) para as
  notificacoes usarem o evento do Windows em vez de consultar a lista a cada 2 s.
  Sem isto tudo funciona na mesma: so as notificacoes demoram ate 2 s e gastam um pouco mais.

  Antes: packaging\instalar.ps1 (como o teu utilizador, sem administrador).
  Depois, numa janela do PowerShell COMO ADMINISTRADOR, na pasta do repositorio:
      powershell -ExecutionPolicy Bypass -File packaging\instalar-notificacoes.ps1

  O que faz (desfazivel com packaging\desinstalar-notificacoes.ps1):
  1. Cria um certificado de teste "CN=DynamicIsland" (so para assinar este pacote) e poe-no como
     confiavel em Pessoas Fidedignas do computador. E isto que precisa de administrador.
  2. Empacota packaging\sparse (manifest + logotipo) num .msix e assina-o. Precisa do Windows SDK
     (makeappx.exe e signtool.exe).
  3. Regista o pacote, apontando para a pasta onde a island esta instalada.

  Nao abras a island a partir desta janela: ficaria elevada e isolada das outras janelas.
#>
param([string]$Destino = (Join-Path $env:LOCALAPPDATA "dynamic-island"))
$ErrorActionPreference = "Stop"
$raiz = Split-Path $PSScriptRoot
if (-not (Test-Path (Join-Path $Destino "island.exe"))) {
    throw "A island nao esta instalada em $Destino. Corre primeiro packaging\instalar.ps1 (sem administrador)."
}
$sdk = Get-ChildItem "${env:ProgramFiles(x86)}\Windows Kits\10\bin\10.*\x64\makeappx.exe" -ErrorAction SilentlyContinue | Sort-Object FullName | Select-Object -Last 1
if (-not $sdk) { throw "Windows SDK nao encontrado (makeappx.exe). Instala o 'Windows 10/11 SDK' ou fica sem as notificacoes em tempo real." }
$makeappx = $sdk.FullName; $signtool = Join-Path $sdk.DirectoryName "signtool.exe"
$trabalho = Join-Path $env:TEMP "dynamic-island-pacote"
New-Item -ItemType Directory -Force $trabalho | Out-Null

# 1. Certificado de teste.
$subject = "CN=DynamicIsland"
$cert = Get-ChildItem Cert:\CurrentUser\My | Where-Object Subject -eq $subject | Select-Object -First 1
if (-not $cert) {
    $cert = New-SelfSignedCertificate -Type Custom -Subject $subject -KeyUsage DigitalSignature `
        -FriendlyName "Dynamic Island (teste)" -CertStoreLocation Cert:\CurrentUser\My `
        -TextExtension @("2.5.29.37={text}1.3.6.1.5.5.7.3.3", "2.5.29.19={text}")
}
if (-not (Get-ChildItem Cert:\LocalMachine\TrustedPeople | Where-Object Thumbprint -eq $cert.Thumbprint)) {
    $cer = Join-Path $trabalho "dynamic-island.cer"
    Export-Certificate -Cert $cert -FilePath $cer | Out-Null
    Import-Certificate -FilePath $cer -CertStoreLocation Cert:\LocalMachine\TrustedPeople | Out-Null
    Remove-Item $cer
}
"1/3 certificado $($cert.Thumbprint) confiavel"

# 2. Pacote assinado (fora do repositorio: o .msix nao e para o git).
$msix = Join-Path $trabalho "island-sparse.msix"
& $makeappx pack /d (Join-Path $PSScriptRoot "sparse") /p $msix /nv /o | Out-Null
if ($LASTEXITCODE) { throw "makeappx falhou ($LASTEXITCODE)" }
& $signtool sign /fd SHA256 /sha1 $cert.Thumbprint /s My $msix | Out-Null
if ($LASTEXITCODE) { throw "signtool falhou ($LASTEXITCODE)" }
"2/3 pacote $msix"

# 3. Registar (substitui uma versao anterior).
Get-AppxPackage -Name DynamicIsland | Remove-AppxPackage
Add-AppxPackage -Path $msix -ExternalLocation $Destino
"3/3 registado: $((Get-AppxPackage -Name DynamicIsland).PackageFullName)"
""
"Feito. Fecha a island e volta a abri-la (como o teu utilizador, nao a partir desta janela)."
"Na primeira vez o Windows pode perguntar se a Dynamic Island pode ler as notificacoes."
