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

## Instalar (máquina nova)

Precisas do Windows 10/11, do [Rust](https://rustup.rs) (com as ferramentas de compilação do Visual
Studio) e do Git. **Nada disto precisa de administrador.**

```powershell
git clone https://github.com/aveczzz/atlas-notch.git
cd atlas-notch
cargo build --release
powershell -ExecutionPolicy Bypass -File packaging\instalar.ps1
```

Se não puderes instalar o Rust nessa máquina, compila noutra e leva `island.exe` e `island-hook.exe`
(`target\release\`) para uma pasta, com `packaging\instalar.ps1` ao lado: o instalador usa os binários
que estiverem na mesma pasta (ou `-Binarios <pasta>`).

O `instalar.ps1` (como o teu utilizador):

1. copia `island.exe` e `island-hook.exe` para `%LOCALAPPDATA%\dynamic-island`;
2. **liga ao Claude Code** (ver abaixo);
3. abre a island, que se regista para arrancar com o Windows (`config.toml`, chave `arrancar_com_windows`).

Opções: `-Binarios`, `-Destino`, `-Definicoes <settings.json>`, `-SubstituirStatusLine`,
`-SemClaudeCode`, `-NaoIniciar`, `-NaoParar`. Desfazer: `packaging\desinstalar.ps1` (`-ApagarDados`
apaga também a configuração e o estado).

**Notificações em tempo real (opcional, precisa de administrador):** `packaging\instalar-notificacoes.ps1`
numa janela do PowerShell como administrador (precisa do Windows SDK). Sem isto a island consulta as
notificações a cada 2 s e funciona na mesma, sem erros.

## Ligar ao Claude Code

O `instalar.ps1` trata disto, mas não é magia: junta ao `~/.claude/settings.json` (o do teu utilizador)

- os **hooks** (`SessionStart`, `UserPromptSubmit`, `PreToolUse`, `PostToolUse`, `Notification`, `Stop`,
  `SubagentStop`, `SessionEnd`), todos com `island-hook.exe`, para a island saber o que o Claude está a fazer;
- a **linha de estado** (`island-hook.exe statusline`), que recebe os limites da conta (5 horas e semana),
  mostra `5h 29% | 7d 55%` e guarda o último valor, para o painel os mostrar.

Garantias: faz **backup** antes (`settings.json.island-backup-<unix>`), cria o ficheiro se não existir,
**não duplica** se o correres duas vezes, só toca no que é da island (os teus hooks e as chaves das outras
ferramentas ficam como estão, pela mesma ordem) e, se já tiveres uma linha de estado tua, **pergunta
antes de a substituir**. Se o ficheiro não for JSON válido, não lhe toca. O caminho do `island-hook.exe`
é absoluto e com barras normais (o Claude Code corre os hooks no Git Bash), e vem da máquina onde instalas:
não há caminhos fixos.

Para ver o que mudaria sem gravar nada: `island-hook.exe settings install --dry-run`.

Depois **reinicia as sessões do Claude Code** (os hooks são lidos no arranque).

Duas coisas a saber sobre a linha de estado:

- O Claude Code só a corre no terminal (`claude`), **não na extensão do VS Code**; por isso os limites só
  se atualizam com sessões de terminal (a barra mantém o último valor até a janela reiniciar).
- Os limites só existem em planos Pro/Max e só depois da primeira resposta de cada sessão.

O hook nunca atrasa o Claude: ~35 ms, sai sempre com 0, e com a island fechada não faz nada.
Diagnóstico: `ISLAND_HOOK_DEBUG=1`. Os hooks só enviam à island o tipo de evento, o projeto, a
ferramenta, o modelo e o tamanho do contexto; nunca o conteúdo da conversa.

**Outros agentes (Codex):** `island-hook.exe --agent codex` marca os eventos como do Codex (muda o ícone
do notch). A ligação ao Codex faz-se à mão e depende do que ele envia.

## Mandar coisas de outras apps

```powershell
.\examples\send.ps1 -Source backup -Title "Backup GASCOMB" -Progress 0.4
```

Protocolo: uma mensagem JSON por linha no pipe `\\.\pipe\dynamic-island`.
Tipos: `show`, `clear`, `claude_hook`, `claude_usage`. Ver `crates/proto/src/lib.rs`.
