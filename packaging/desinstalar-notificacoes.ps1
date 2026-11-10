#Requires -RunAsAdministrator
<#
  Desfaz o packaging\instalar-notificacoes.ps1: tira o registo do pacote e o certificado de teste.
  A island continua instalada e a funcionar (as notificacoes voltam a ser consultadas a cada 2 s).
#>
$ErrorActionPreference = "Stop"
Get-Process island -ErrorAction SilentlyContinue | Stop-Process -Force
Get-AppxPackage -Name DynamicIsland | Remove-AppxPackage
"pacote removido"
foreach ($store in "Cert:\LocalMachine\TrustedPeople", "Cert:\CurrentUser\My") {
    Get-ChildItem $store | Where-Object Subject -eq "CN=DynamicIsland" | Remove-Item
}
"certificado de teste removido"
