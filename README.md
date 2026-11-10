# Dynamic Island para Windows

Uma "ilha" no topo do ecrã do portátil, ao estilo do notch do Mac, feita em **Rust** e desenhada para
ser invisível em recursos: em repouso não faz **nada** (0% de CPU, 0 acordares, ~10 MB de RAM).

Em repouso é um notch preto, colado ao topo. Cresce só o necessário quando há algo para mostrar
e abre o painel completo (em vidro desfocado) quando passas o rato por cima.

## O que mostra

| | |
|---|---|
| **Claude Code** | o que cada sessão está a fazer (a editar, a correr comandos, à espera de ti, terminou), há quanto tempo, o modelo, o contexto usado e os **limites da conta** (5 horas e semana). O ícone muda para o do Claude ou do Codex. |
| **Música** | o que está a tocar (Spotify, browser, Leitor Multimedia...), capa, ícone da app e controlos. |
| **Volume** | HUD ao mudar o volume e um misturador com o volume e o mute de cada app. |
| **Notificações** | as notificações das apps (Outlook, Teams...) num toast com o ícone da app; clicar abre a app. |
| **Sistema** | avisos curtos de Caps/Num/Scroll Lock, carregador e bateria fraca, mudança de dispositivo de áudio e "Copiado" (só o tipo, nunca o conteúdo). |
| **Ficheiros** | arrasta ficheiros para o notch para os guardar (e arrasta-os de volta para qualquer app). |
| **Qualquer app** | manda uma linha de JSON para um named pipe e aparece (ver [Protocolo](#protocolo)). |

Fica sempre no **ecrã interno do portátil**, mesmo com monitores externos. Esconde-se sozinha quando uma
app ocupa o ecrã inteiro. `Ctrl+Alt+I` esconde e mostra; há também um ícone na bandeja do Windows
(a zona da seta ao lado do wifi) com o menu.

## Requisitos

- Windows 10 (versão 2004 ou mais recente) ou Windows 11.
- Para compilar: [Rust](https://rustup.rs) 1.88 ou mais recente e as *Build Tools* do Visual Studio com
  "Desktop development with C++". Não precisas de administrador para nada disto.
- Opcional: [Claude Code](https://claude.com/claude-code) e Git Bash (vem com o Git para Windows).

## Instalar

```powershell
git clone https://github.com/aveczzz/atlas-notch.git
cd atlas-notch
cargo build --release
powershell -ExecutionPolicy Bypass -File packaging\instalar.ps1
```

O `instalar.ps1` corre como o teu utilizador (**sem administrador**):

1. copia `island.exe` e `island-hook.exe` para `%LOCALAPPDATA%\dynamic-island`;
2. liga ao Claude Code (ver [abaixo](#ligar-ao-claude-code));
3. abre a island, que se regista para arrancar com o Windows.

Opções: `-Binarios <pasta>`, `-Destino <pasta>`, `-Definicoes <settings.json>`, `-SubstituirStatusLine`,
`-SemClaudeCode`, `-NaoIniciar`, `-NaoParar`. Para desinstalar: `packaging\desinstalar.ps1`
(`-ApagarDados` apaga também a configuração e o estado).

**Sem Rust na máquina?** Compila noutra, copia `island.exe` e `island-hook.exe` (de `target\release\`) para
uma pasta com o `packaging\instalar.ps1` ao lado (ou usa `-Binarios`) e corre o instalador. Os binários não
estão assinados: o SmartScreen ou o antivírus de uma máquina gerida podem reclamar.

**Verificar a máquina antes:** `packaging\diagnostico.ps1` é um relatório só de leitura (Windows, ecrãs e
escala, ferramentas, antivírus, bateria, áudio...). O nome do utilizador e o da máquina saem ocultados:
`powershell -NoProfile -ExecutionPolicy Bypass -File packaging\diagnostico.ps1 -Saida relatorio.txt`.

### Notificações em tempo real (opcional, precisa de administrador)

Sem isto a island consulta as notificações a cada 2 s e funciona na mesma. Com isto usa o evento do
Windows (sem consultas). Precisa do Windows SDK; numa janela do PowerShell **como administrador**:

```powershell
powershell -ExecutionPolicy Bypass -File packaging\instalar-notificacoes.ps1
```

Cria um certificado de teste (`CN=DynamicIsland`, só para assinar o pacote) e regista a island com
identidade de pacote. Desfaz-se com `packaging\desinstalar-notificacoes.ps1`. Depois fecha e volta a abrir a
island como o teu utilizador (não a partir da janela de administrador).

## Usar

- **Hover** no notch: abre o painel. Os separadores abrem ao passar o rato por cima: **Início**
  (música e Claude Code), **Som** (misturador) e **Ficheiros**.
- **Clique direito** no notch ou no ícone da bandeja: *Mostrar / esconder*, *Abrir configuração*, *Fechar*.
  Clique esquerdo no ícone da bandeja mostra/esconde. No Windows 11 o ícone começa no menu "^": arrasta-o
  para a barra para o teres sempre à vista.
- `Ctrl+Alt+I`: esconde/mostra.

### Configuração

`%APPDATA%\dynamic-island\config.toml` (o menu *Abrir configuração* cria-o e abre-o). Grava e aplica na hora:

```toml
arrancar_com_windows = true        # arrancar com o Windows (só a cópia instalada)
esconder_em_ecra_inteiro = true    # esconder quando uma app ocupa o ecrã inteiro
guardar_tray = true                # lembrar os ficheiros da aba Ficheiros
vidro = true                       # vidro com desfoque (false = preto quase opaco)
avisos_do_sistema = true           # Caps Lock, carregador, áudio, "Copiado"
```

O `island.log` (na mesma pasta) regista o arranque, a saída e qualquer erro. Se a island falhar ao
arrancar, tenta de novo sozinha (até 5 vezes).

## Ligar ao Claude Code

O `instalar.ps1` junta ao `~/.claude/settings.json` (o do teu utilizador):

- os **hooks** (`SessionStart`, `UserPromptSubmit`, `PreToolUse`, `PostToolUse`, `Notification`, `Stop`,
  `SubagentStop`, `SessionEnd`), todos com `island-hook.exe`, para a island saber o que o Claude faz;
- a **linha de estado** (`island-hook.exe statusline`), que recebe os limites da conta, mostra
  `5h 29% | 7d 55%` e guarda o último valor para o painel.

Garantias: faz **backup** antes (`settings.json.island-backup-<unix>`), cria o ficheiro se não existir,
**não duplica** se o correres duas vezes, só toca no que é da island (os teus hooks e as chaves de
outras ferramentas ficam como estão, pela mesma ordem), **pergunta antes de substituir** uma linha de
estado tua, e não toca num JSON inválido. O caminho é absoluto e com barras normais (o Claude Code corre
os hooks no Git Bash) e vem da máquina onde instalas, sem caminhos fixos. Para ver o que mudaria sem gravar:
`island-hook.exe settings install --dry-run`. Depois **reinicia as sessões do Claude Code**.

Duas limitações da linha de estado:

- O Claude Code só a corre no **terminal** (`claude`), não na extensão do VS Code. Por isso os limites só se
  atualizam com sessões de terminal (a barra mantém o último valor até a janela reiniciar). O resto
  (estado, duração, modelo, contexto) funciona nas duas.
- Os limites só existem em planos Pro/Max e só depois da primeira resposta de cada sessão.

**Privacidade:** o hook só envia à island o tipo de evento, o projeto, a ferramenta, o modelo e o tamanho do
contexto. Nunca o texto da conversa. A island não usa a rede e não lê credenciais.

**Codex:** `island-hook.exe --agent codex` marca os eventos como do Codex (muda o ícone). A ligação ao Codex
faz-se à mão e depende do que ele envia (por testar).

## Protocolo

Uma mensagem JSON por linha no named pipe `\\.\pipe\dynamic-island` (só aceita o teu utilizador, só local):

```powershell
.\examples\send.ps1 -Source backup -Title "Backup concluído" -Progress 0.4 -TtlMs 5000
```

```json
{"v":1,"msg":{"type":"show","source":"backup","title":"Backup concluído","priority":"info","ttl_ms":5000}}
```

Tipos: `show`, `clear`, `claude_hook`, `claude_usage`. Apps externas não podem pedir a prioridade de
atenção (reservada ao Claude e ao volume). Ver [`crates/proto/src/lib.rs`](crates/proto/src/lib.rs).

## Como funciona

```
fontes (threads/callbacks) --Event--> Engine --> módulos --> árbitro --> View --> renderer
                                         \--> Command --> plataforma (Windows)
```

| Crate | Papel |
|---|---|
| `proto` | protocolo IPC e adaptador do que o Claude Code envia. Sem dependências de plataforma. |
| `core` | motor, árbitro de prioridades, animações por molas. Lógica pura. |
| `modules` | Claude, música, misturador, notificações, sistema, Ficheiros, apps externas. Lógica pura, com testes. |
| `platform` | o único sítio com Win32/WinRT e `unsafe`: janela, Direct2D/DirectComposition, GSMTC, Core Audio... |
| `app` | o binário `island`. |
| `hook` | o binário `island-hook`: hooks e linha de estado do Claude Code, e o instalador do `settings.json`. |

Decisões de desenho: **zero polling** (a thread dorme até haver um evento, um prazo ou um frame de
animação), desenho em WARP (CPU) de propósito para não acordar a GPU dedicada, e o vidro é uma fotografia
do ecrã tirada **uma vez** ao abrir e desfocada uma vez (o Windows não deixa desfocar atrás de uma janela
que nunca é ativada). Detalhes, medidas e razões em [`CLAUDE.md`](CLAUDE.md) e [`DESIGN.md`](DESIGN.md).

| Orçamento | Alvo | Medido |
|---|---|---|
| CPU em repouso | 0% | 0 ciclos em 60 s |
| RAM privada | < 25 MB | ~10-16 MB |
| CPU a animar | < 2% | ~1,5% |
| Arranque | < 150 ms | ~80-120 ms |

## Desenvolvimento

```powershell
cargo test --workspace
cargo clippy --workspace --all-targets
```

Os módulos testam-se sem Windows: `cargo run -p island -- --headless` e escrever JSON no stdin.
`cargo run -p island -- --stdin` abre a janela e também lê o protocolo pelo stdin.

## Limitações conhecidas

- A fotografia do vidro é estática enquanto o painel está aberto (um vídeo por baixo não se mexe no vidro).
- A percentagem de contexto assume uma janela de 200 mil tokens (1 milhão se a conversa já passou disso).
- Os limites da conta não se atualizam com sessões da extensão do VS Code (ver acima).
- O ícone do Codex e a ligação ao Codex estão por testar com o Codex real.
- Sem binários assinados nem instalador `.msi`: o instalador é um conjunto de scripts PowerShell.

## Licença

MIT (declarada em `Cargo.toml`).
