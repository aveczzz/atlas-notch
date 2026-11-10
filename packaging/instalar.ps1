#Requires -RunAsAdministrator
<#
  Instala a Dynamic Island com identidade de pacote (sparse package), para as notificações
  usarem o evento do Windows em vez de consultar a lista a cada 2 s.

  Antes: `cargo build --release` (sem administrador).
  Depois, numa janela do PowerShell **como administrador**, na pasta do repositório:
      powershell -ExecutionPolicy Bypass -File packaging\instalar.ps1

  O que faz (tudo desfazível com packaging\desinstalar.ps1):
  1. Copia island.exe e island-hook.exe para %LOCALAPPDATA%\dynamic-island.
  2. Cria um certificado de teste "CN=DynamicIsland" (só para assinar este pacote) e põe-no
     como confiável em Pessoas Fidedignas do computador. É isto que precisa de administrador.
  3. Empacota packaging\sparse (manifest + logótipo) num .msix e assina-o.
  4. Regista o pacote para o teu utilizador, apontando para a pasta do passo 1.
#>
param([string]$Destino = (Join-Path $env:LOCALAPPDATA "dynamic-island"))
$ErrorActionPreference = "Stop"
$raiz = Split-Path $PSScriptRoot
$release = Join-Path $raiz "target\release"
foreach ($exe in "island.exe", "island-hook.exe") {
    if (-not (Test-Path (Join-Path $release $exe))) { throw "Falta ${exe}: corre primeiro 'cargo build --release' (sem administrador)." }
}
$sdk = Get-ChildItem "${env:ProgramFiles(x86)}\Windows Kits\10\bin\10.*\x64\makeappx.exe" | Sort-Object FullName | Select-Object -Last 1
if (-not $sdk) { throw "Windows SDK não encontrado (makeappx.exe)." }
$makeappx = $sdk.FullName; $signtool = Join-Path $sdk.DirectoryName "signtool.exe"

# 1. Ficheiros no sítio final. A island tem de estar fechada para o .exe poder ser substituído.
Get-Process island -ErrorAction SilentlyContinue | Stop-Process -Force
New-Item -ItemType Directory -Force $Destino | Out-Null
Copy-Item (Join-Path $release "island.exe"), (Join-Path $release "island-hook.exe") $Destino -Force
"1/4 ficheiros em $Destino"

# 2. Certificado de teste.
$subject = "CN=DynamicIsland"
$cert = Get-ChildItem Cert:\CurrentUser\My | Where-Object Subject -eq $subject | Select-Object -First 1
if (-not $cert) {
    $cert = New-SelfSignedCertificate -Type Custom -Subject $subject -KeyUsage DigitalSignature `
        -FriendlyName "Dynamic Island (teste)" -CertStoreLocation Cert:\CurrentUser\My `
        -TextExtension @("2.5.29.37={text}1.3.6.1.5.5.7.3.3", "2.5.29.19={text}")
}
if (-not (Get-ChildItem Cert:\LocalMachine\TrustedPeople | Where-Object Thumbprint -eq $cert.Thumbprint)) {
    $cer = Join-Path $env:TEMP "dynamic-island.cer"
    Export-Certificate -Cert $cert -FilePath $cer | Out-Null
    Import-Certificate -FilePath $cer -CertStoreLocation Cert:\LocalMachine\TrustedPeople | Out-Null
    Remove-Item $cer
}
"2/4 certificado $($cert.Thumbprint) confiável"

# 3. Pacote assinado.
$msix = Join-Path $release "island-sparse.msix"
& $makeappx pack /d (Join-Path $PSScriptRoot "sparse") /p $msix /nv /o | Out-Null
if ($LASTEXITCODE) { throw "makeappx falhou ($LASTEXITCODE)" }
& $signtool sign /fd SHA256 /sha1 $cert.Thumbprint /s My $msix | Out-Null
if ($LASTEXITCODE) { throw "signtool falhou ($LASTEXITCODE)" }
"3/4 pacote $msix"

# 4. Registar (substitui uma versão anterior).
Get-AppxPackage -Name DynamicIsland | Remove-AppxPackage
Add-AppxPackage -Path $msix -ExternalLocation $Destino
"4/4 registado: $((Get-AppxPackage -Name DynamicIsland).PackageFullName)"
""
"Feito. Abre a island com: $Destino\island.exe"
"Na primeira vez o Windows pode perguntar se a Dynamic Island pode ler as notificações."
