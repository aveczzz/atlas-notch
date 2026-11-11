# Exemplo: mostrar algo na island a partir de qualquer script PowerShell.
# Uso: .\send.ps1 -Source backup -Title "Backup do servidor" -Progress 0.4 -TtlMs 5000
param(
    [Parameter(Mandatory)][string]$Source,
    [Parameter(Mandatory)][string]$Title,
    [string]$Detail,
    [ValidateSet("ambient", "info", "active")][string]$Priority = "info",
    [Nullable[double]]$Progress,
    [Nullable[int]]$TtlMs
)

$msg = @{ type = "show"; source = $Source; title = $Title; priority = $Priority }
if ($Detail)   { $msg.detail = $Detail }
if ($null -ne $Progress) { $msg.progress = $Progress }
if ($null -ne $TtlMs)    { $msg.ttl_ms = $TtlMs }
$line = (@{ v = 1; msg = $msg } | ConvertTo-Json -Compress -Depth 4) + "`n"

$pipe = New-Object System.IO.Pipes.NamedPipeClientStream(".", "dynamic-island", [System.IO.Pipes.PipeDirection]::Out)
try {
    $pipe.Connect(200)   # ms; se a island não estiver a correr, falha e pronto
    $bytes = [System.Text.Encoding]::UTF8.GetBytes($line)
    $pipe.Write($bytes, 0, $bytes.Length)
} catch {
    # island fechada: ignorar
} finally {
    $pipe.Dispose()
}
