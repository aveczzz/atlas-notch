# Dynamic Island para Windows

Uma ilha no topo do ecrã, feita em Rust, com consumo de recursos perto de zero.

- **Claude Code**: vê se está a trabalhar, se precisa de ti ou se terminou, por projeto.
- **Música**: o que está a tocar, com controlos.
- **Volume**: HUD ao mudar e misturador por app.
- **Notificações**: Outlook, Teams, etc. (via notificações do Windows).
- **Qualquer app**: manda uma linha de JSON para o named pipe e aparece.

## Estado

| Parte | Estado |
|---|---|
| Protocolo, engine, árbitro, animações | Feito, testado |
| Módulos (Claude, media, misturador, notificações, externo) | Feito, testado |
| `island-hook` | Feito, testado |
| Backend headless (dev) | Feito |
| Backend Windows: janela + event loop (fase 1) | Feito: sempre no ecrã do portátil, 0 wakeups em repouso, arranque < 100 ms |
| Renderer e forma do notch (fase 2) | Feito: D2D/DirectComposition em WARP, orelhas, hover, pulso pelo compositor, ~25 MB |
| Painel com blocos (fase 3) | Feito: Nook (música, Claude Code, apps externas), cliques nos botões, separadores Som e Tray |
| IPC + hook do Claude Code (fase 4) | Feito: pipe seguro, instância única, "Abrir no VS Code" |
| Música (fase 5) | Feito: GSMTC (Spotify, browser, Media Player), capa, ícone da app, controlos |
| Áudio (fase 6) | Feito: HUD de volume, separador Som com volume por app |
| Notificações (fase 7) | Feito: com o pacote (`packaging/instalar.ps1`, admin uma vez) usa o evento do Windows; sem ele, consulta a cada 2 s |
| Tray (fase 8) | Feito: arrastar ficheiros para o notch, abrir com clique, arrastar para fora |
| Polimento (fase 9) | Feito: esconde em ecrã inteiro, arranca com o Windows, `config.toml` pela engrenagem (aplica ao gravar), Tray lembrada, Ctrl+Alt+I e menu do clique direito |
| Liquid glass e sistema (fase 10) | Feito: vidro com desfoque (fotografia do ecrã tirada uma vez ao abrir) e transições cruzadas, separadores por hover, avisos de Caps/Num/Scroll Lock, carregador, bateria, dispositivo de áudio e Copiado, toast rico das notificações com clique para abrir a app |

## Experimentar já (qualquer SO)

```sh
cargo test --workspace
echo '{"v":1,"msg":{"type":"show","source":"teste","title":"Olá","ttl_ms":2000}}' \
  | cargo run -p island -- --headless
```

No Windows, `cargo run -p island` mostra a pílula no topo do ecrã. Clique direito fecha.

## Ligar ao Claude Code

1. `cargo build --release` e copiar `target\release\island-hook.exe` para
   `%LOCALAPPDATA%\dynamic-island\` (um sítio fixo: o `cargo build` não o tranca enquanto um hook corre).
2. Juntar a chave `hooks` de `examples/claude-code-hooks.json` ao `~/.claude/settings.json`, trocando
   `island-hook` pelo caminho completo **com barras normais** (o Claude Code corre os hooks no Git Bash):
   `C:/Users/<tu>/AppData/Local/dynamic-island/island-hook.exe`.
3. Reiniciar as sessões do Claude Code (os hooks são lidos no arranque) e abrir a island.

O hook nunca atrasa o Claude: ~35 ms, sai sempre com 0, e com a island fechada não faz nada.
Diagnóstico: `ISLAND_HOOK_DEBUG=1`.

## Mandar coisas de outras apps

```powershell
.\examples\send.ps1 -Source backup -Title "Backup GASCOMB" -Progress 0.4
```

Protocolo: uma mensagem JSON por linha no pipe `\\.\pipe\dynamic-island`.
Tipos: `show`, `clear`, `claude_hook`. Ver `crates/proto/src/lib.rs`.
