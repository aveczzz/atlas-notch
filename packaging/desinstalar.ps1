#Requires -RunAsAdministrator
<#
  Desfaz o packaging\instalar.ps1: tira o registo do pacote e o certificado de teste.
  Os ficheiros em %LOCALAPPDATA%\dynamic-island ficam (o island-hook.exe dos hooks do Claude
  Code também vive lá); apaga a pasta à mão se quiseres.
#>
$ErrorActionPreference = "Stop"
Get-Process island -ErrorAction SilentlyContinue | Stop-Process -Force
Get-AppxPackage -Name DynamicIsland | Remove-AppxPackage
"pacote removido"
foreach ($store in "Cert:\LocalMachine\TrustedPeople", "Cert:\CurrentUser\My") {
    Get-ChildItem $store | Where-Object Subject -eq "CN=DynamicIsland" | Remove-Item
}
"certificado de teste removido"
