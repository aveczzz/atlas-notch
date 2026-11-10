# Dynamic Island para Windows

App em Rust que mostra uma "ilha" no topo do ecrã: Claude Code, música, volume por app,
notificações e qualquer app externa através de um named pipe. Objetivo número um:
**ser invisível em recursos**. Responde sempre em português de Portugal.

## Arquitetura

```
crates/
  proto/     protocolo IPC (JSON por linha). Sem dependências de plataforma.
  core/      Engine, árbitro, trait Module, View, animações (molas). Lógica pura.
  modules/   claude, media, mixer, notifications, external. Lógica pura.
  platform/  único sítio com Win32/WinRT e `unsafe`. headless/ + win/ (janela e loop feitos, resto esqueleto).
  app/       binário `island`: liga tudo.
  hook/      binário `island-hook`: chamado pelos hooks do Claude Code.
```

Fluxo unidirecional:

```
fontes (threads/callbacks) --Event--> Engine --> módulos --> árbitro --> View --> renderer
                                         \--> Command --> platform::win::exec
```

- Os módulos **nunca** chamam o sistema. Recebem `Event`, devolvem `View` e `Command`.
- O tempo entra sempre como parâmetro `now: Instant`. Nunca `Instant::now()` em `core`/`modules`.
- O árbitro escolhe a view com maior `Priority`; em empate, a que mudou mais recentemente.
- Apps externas não podem usar `Priority::Attention` (o protocolo nem aceita).

## Regras inquebráveis

1. `core`, `modules`, `proto` e `hook` têm `#![forbid(unsafe_code)]` e não dependem de `windows`.
   Se precisares de uma API do sistema num módulo, cria um `Event` ou `Command` novo.
2. **Zero polling.** A thread UI dorme até: evento no canal, deadline de um módulo,
   ou frame de animação. Se te apetecer um `loop { sleep }`, pára e pergunta.
   Exceção documentada: notificações (ver `platform/src/win/notifications.rs`).
3. Só se redesenha quando `Engine::dispatch`/`on_deadline` devolvem `true` ou há uma mola a mexer.
4. Callbacks COM/WinRT só fazem `proxy.send(..)`. Nunca bloqueiam, nunca tocam na UI.
5. Toda a lógica nova em `core`/`modules` vem com testes com relógio falso.
6. Estado com tamanho controlado por fora (sessões, fontes externas, histórico) tem limite máximo.
7. O `island-hook` sai sempre com código 0, nunca escreve no stdout, nunca espera mais de ~50 ms.

## Orçamento de recursos (verificar a cada fase)

| Métrica | Alvo |
|---|---|
| CPU em repouso | 0,0% (Process Explorer, 1 min) |
| Wakeups em repouso | 0 (ETW / `wpr` se houver dúvidas) |
| RAM privada (o que o Gestor de Tarefas mostra; o working set total inclui DLLs partilhadas do Windows) | < 25 MB |
| CPU a animar | < 2% |
| Arranque | < 150 ms até a janela aparecer |

## Segurança

- Named pipe: `PIPE_REJECT_REMOTE_CLIENTS`, DACL só com o SID do utilizador atual,
  `FILE_FLAG_FIRST_PIPE_INSTANCE`. Detalhes em `platform/src/win/ipc.rs`.
- Tudo o que vem do pipe passa por `island_proto::decode_line` (limites de tamanho, versão, texto cortado).
- Nada de credenciais na app. Não há OAuth nem acesso ao Microsoft 365: o email chega
  pelas notificações do Windows.

## windows-rs: cuidado redobrado

O crate `windows` (versão 0.62 no `Cargo.toml`) muda assinaturas entre versões.
- Antes de usar uma API, confirma a assinatura em docs.rs **para a versão do Cargo.lock**,
  ou corre `cargo doc -p windows --open`. Não confiar em exemplos antigos.
- Se faltar uma feature, acrescenta-a em `crates/platform/Cargo.toml` (a lista já tem as previstas).
- Cada bloco `unsafe` leva um comentário `// SAFETY:` a explicar porquê é seguro.
- Interfaces COM implementadas por nós usam `#[implement]`.

## Desenho

O aspeto-alvo é o do NotchNook (macOS). As decisões, medidas e cores estão em **`DESIGN.md`** e as
capturas de referência em `docs/ui/`. Resumo:
- notch preto colado ao topo, com cantos de cima côncavos;
- sempre visível em repouso, cresce só o necessário nos eventos, painel completo no hover;
- separadores "Nook" (música + Claude Code) e "Tray" (ficheiros).

A island vive **sempre no ecrã interno do portátil** (deteção genérica em `win/window.rs`).

## Roadmap (fazer por ordem, uma fase de cada vez, testar antes de avançar)

O peso de cada fase é o esforço estimado. Os pesos somam 100 e servem para a barra de progresso
geral: a fundação já feita (proto, core, módulos, hook) vale **10**. Ao reportar progresso, mostrar
a barra da fase atual e a do projeto calculada com estes pesos.

1. **Janela + loop** (peso 5). ✅
   - `win::window::create`, `win::run` com `MsgWaitForMultipleObjectsEx` e `WM_APP_WAKE`.
   - Pílula preta estática: brush GDI + `SetWindowRgn`. `WS_EX_NOREDIRECTIONBITMAP` só entra na fase 2.
   - Fica no ecrã interno do portátil. Clique direito fecha.
   - Confirmado: 0 wakeups em repouso, ~8 MB, arranque < 100 ms.
2. **Renderer e forma** (peso 20). ✅
   - Cadeia D3D11/D2D/DComp e texto DirectWrite. A forma vem de `win/geometry.rs`: o path do D2D e a região da janela saem do mesmo contorno.
   - Estados repouso/evento/expandido com `IslandShape`; hover com 250 ms de atraso a encolher.
   - **O dispositivo é WARP (CPU) de propósito.** A GPU por omissão era a NVIDIA: 30 threads, uma delas a acordar sozinha, e GPU dedicada sempre ligada. A integrada gastava ~60 MB. Tabela em `render.rs`.
   - Pulso "precisa de ti" animado pelo compositor (`AddSinusoidal`): 0 ciclos enquanto pulsa.
   - Device lost: evento do `ID3D11Device4` no `MsgWait` (sem polling) e recriação.
   - Medido (release, ecrã do portátil a 125%):
     - arranque ~80-95 ms;
     - repouso e a pulsar: 0 ciclos;
     - animação: 0,7-1,3% do CPU total;
     - working set ~24,8 MB (no limite), memória privada ~11 MB.
   - Por verificar à mão: Win+Ctrl+Shift+B (device lost) e mudança de escala com a app aberta.
3. **Painel** (peso 10). ✅
   - `Module::widget` + `Widget` tipado no `core`. O engine junta os blocos de todos os módulos e pede redraw quando o painel muda, mesmo que o vencedor do compacto não mude.
   - `win/panel.rs`: separadores Nook/Tray, engrenagem e colunas música / Claude / apps externas. Os mesmos retângulos (`geometry::panel_layout`) servem o desenho e os cliques.
   - Clique esquerdo → `Hit` → `Event::Action` → `Command` → `exec::run`. Por agora o `exec` só regista, com a fase de cada comando.
   - Animações limitadas a ~80 frames/s (o ecrã do portátil é de 144 Hz; com WARP era o dobro do CPU sem diferença visível).
   - Medido (release): arranque 70-120 ms, repouso e a pulsar 0 ciclos, animação 1,5% do CPU, working set 24,9 MB.
   - Falta: a capa do álbum real e o ícone da app (fase 5); o separador Tray é só um aviso (fase 8); a engrenagem só regista (fase 9).
   - Ideia se a RAM ou o CPU apertarem: redesenhar só a zona da forma (`BeginDraw` com retângulo).
4. **IPC + hook** (peso 8). ✅
   - `win/ipc.rs`: o pipe é reservado antes da janela (`FILE_FLAG_FIRST_PIPE_INSTANCE`), por isso uma 2.ª island sai com código 1. A DACL só tem o utilizador atual. Uma thread por cliente, no máximo 8.
   - `read_ipc` lê cada linha com teto (64 KB): uma linha gigante é descartada sem crescer a memória.
   - Canal limitado (256) com `try_send`: as fontes nunca bloqueiam.
   - Hook: ignora o `idle_prompt` e tolera BOM. `AskUserQuestion`/`ExitPlanMode` contam como "precisa de ti"; ferramentas MCP aparecem como "a usar <servidor>". `ttl_ms` ≤ 24 h.
   - `exec::FocusEditor`: procura a janela do editor (VS Code, Cursor, …) pelo projeto como segmento do título; testado com um clique real.
   - Medido: hook ~35 ms (máx. ~50 ms) com a island aberta ou fechada.
   - **Nunca segurar `io::stderr().lock()` na thread UI**: bloqueava para sempre o `eprintln!` das outras threads (bug encontrado nesta fase).
   - A instalação dos hooks no `~/.claude/settings.json` fica para o utilizador (README): editar a config do Claude Code a partir de uma sessão do Claude é bloqueado.
5. **Media** (peso 10). ✅
   - `win/media.rs`: thread `media`, dona do manager GSMTC. Os handlers WinRT só mandam um sinal; os `join` e os comandos correm nesta thread. Vários sinais seguidos dão um só refresh. A capa só se pede quando título/artista mudam (máx. 4 MB).
   - Comandos: `exec::Sinks.media` → `Try*Async` (fire-and-forget). Testado com o Media Player: pausar pelo painel chega ao leitor e o botão muda.
   - Capa: WIC no CPU, reduzida logo ao tamanho de ecrã (~32 KB), desenhada com pincel de bitmap dentro do retângulo arredondado (corta capas não quadradas).
   - Testar sem som: abrir um WAV de silêncio no leitor predefinido. Um `MediaPlayer` num PowerShell sem janela não aparece no GSMTC.
   - Medido: arranque 84 ms, repouso e a pulsar 0 ciclos, animação 1,75%.
   - RAM: o utilizador aceitou (2026-11-08) medir a memória privada, não o working set total.
   - Badge com o ícone da app (`win/appicon.rs`): `shell:AppsFolder\<AUMID>` ou o executável a correr, codificado em PNG na thread `media`, uma vez por app. Carregar o Shell sobe a memória privada para ~17 MB (dentro do orçamento) e o working set total para ~50 MB (DLLs partilhadas).
6. **Áudio** (peso 12). ✅
   - `win/audio.rs`: thread `audio`, dona do `IMMDeviceEnumerator` e do dispositivo atual. Os callbacks COM (`#[implement]`) só fazem `proxy.send` ou mandam um sinal: sessão nova ou expirada, ou dispositivo trocado, reconstroem o estado na thread.
   - `AudioEvent::MasterVolume { quiet }`: o estado inicial, a troca de dispositivo e os ecos nossos (`event_context`) não abrem o HUD. A heurística `pending_master` do módulo foi removida.
   - Separador **Som** (decisão do utilizador, 2026-11-09): volume geral e uma linha por app (slider, %, mute). Arrastar com `SetCapture`; a ilha não encolhe enquanto se arrasta.
   - Testado com o Core Audio real: HUD com as teclas de volume; arrastar "Sons do sistema" 100% → 40% → 100%. A troca de dispositivo (auscultadores) está por testar à mão.
   - Testes por script: um número negativo passado a uma função PowerShell (`Msg -196`) chega como texto e não é escalado; usar `(-196)`. O contexto de DPI por thread perde-se no PowerShell desta ferramenta: passar a escala da island.
7. **Notificações** (peso 10). ✅
   - `win/notifications.rs`: thread `notifications`. As que existiam ao arrancar não aparecem; dispensar no painel chama `RemoveNotification`.
   - **Com o sparse package instalado** (`packaging/instalar.ps1`, admin uma vez): identidade `DynamicIsland_..._dp0bm1cdv5j1t` e o evento `NotificationChanged` funciona. Medido: a notificação aparece em < 1 s e o repouso fica em ~2,5 M ciclos em 30 s (praticamente 0).
   - Sem o pacote, o acesso também é concedido neste Windows, mas sem evento: a lista é consultada a cada 2 s (~275 M ciclos em 30 s, 0,26% de um núcleo). É a única exceção ao zero polling, e só sem pacote.
   - Pacote: `packaging/sparse` (manifest + logo), manifest `<msix>` embutido no `island.exe` (`crates/app/build.rs`, `embed-resource`), certificado de teste CN=DynamicIsland. A island instalada vive em `%LOCALAPPDATA%\dynamic-island` (atualizar = copiar o `island.exe` novo com a island fechada).
   - O atalho sem certificado (`-AllowUnsigned` + OID no Publisher) **não funciona**: o validador side-by-side recusa esse Publisher no `<msix>` e o `.exe` nem arranca.
   - **Não abrir a island como administrador**: fica elevada e isolada das outras janelas (aconteceu ao abri-la a partir do PowerShell do instalador).
   - Hover: só conta o rato dentro da forma visível (`geometry::contains`), não a região da janela, que a encolher ainda tem o tamanho do painel (pedido do utilizador: reabria sozinha).
8. **Tray** (peso 8). ✅
   - `modules/tray.rs`: lista de ficheiros (máx. 7, a mais recente primeiro, sem duplicados) com chaves estáveis; abrir é `Command::OpenFile` (`ShellExecuteW`); o ✕ só tira da lista.
   - `win/dragdrop.rs`: `IDropTarget` (OLE: o `run` passou de `CoInitializeEx` para `OleInitialize`). Arrastar ficheiros por cima do notch abre a ilha **já no separador Tray**, com a zona de largar acesa; os ícones vêm do Shell numa thread à parte.
   - Para fora: premir num ficheiro e arrastar mais de 6 px faz `DoDragDrop` com o `IDataObject` do Shell; se o ficheiro for movido, sai da lista. Clique sem arrastar abre-o.
   - Testado com arrastar OLE real (uma janela WinForms como fonte e outra como destino, rato simulado por coordenadas absolutas): para dentro e para fora funcionam com a janela `NOACTIVATE`. Abrir com clique está por testar à mão.
   - A lista não sobrevive a reiniciar a app (fica para a config da fase 9).
9. **Polimento** (peso 7).
   - ✅ Esconder em ecrã inteiro (`win/fullscreen.rs`): appbar `ABM_NEW` **sem** `ABM_SETPOS` (não reserva espaço) → o shell manda `ABN_FULLSCREENAPP`; a cada aviso vê-se se a janela em primeiro plano cobre o monitor da island (ignora o ambiente de trabalho). 0 polling. Testado com uma janela sem bordas a cobrir o ecrã: esconde e volta.
   - ✅ Forma do notch afinada pela captura do NotchNook: orelhas 12, cantos de baixo 38 no painel (640×176).
   - ✅ Config `%APPDATA%\dynamic-island\config.toml` (`platform/src/config.rs`, leitor próprio de `chave = true|false`, sem crate de TOML; gralhas dão aviso e valem os valores por omissão). Chaves: `arrancar_com_windows`, `esconder_em_ecra_inteiro`, `guardar_tray` (todas `true` por omissão). A engrenagem cria o ficheiro se faltar e abre-o (associação de `.toml`, senão Bloco de Notas). `FindFirstChangeNotificationW` na pasta entra no `MsgWait`: gravar aplica logo, 0 polling (medido: 0 ciclos em 60 s de repouso).
   - ✅ Arranque com o Windows (`win/settings.rs`): valor `DynamicIsland` em `HKCU\...\Run`, **só** para a cópia instalada em `%LOCALAPPDATA%\dynamic-island` (builds em `target\` nunca se registam). Testado: `false` no config tira o valor na hora, voltar a `true` repõe.
   - ✅ Menu no clique direito ("Esconder", "Fechar") e **Ctrl+Alt+I** global para esconder/mostrar (escondida não desenha; o estado continua a andar).
   - ✅ Tray entre arranques: `tray.txt` (um caminho por linha, escrita atómica com rename), gravado só quando a lista muda; ao arrancar repõe os que ainda existem. Não grava enquanto a lista reposta não chega (senão apagava-a). Testado: o ficheiro volta depois de reiniciar e o ✕ grava a lista vazia.
   - Por verificar à mão: Win+D não deve esconder a island; a engrenagem abre o `config.toml` (no teste não apareceu janela nova: provavelmente abriu como separador no VS Code).
   - ✅ **Logs sem pânico**: macro `log!` em `platform/src/lib.rs` (`let _ = writeln!(stderr, ..)`) em vez de `eprintln!`, que entrava em pânico com o stderr num pipe fechado e, com `panic = "abort"`, fechava a app. O hook também. Testado: lançada por um processo que sai, a island continua viva depois de escrever logs.
10. **Liquid glass e sistema** (em curso; plano em `~/.claude/plans/`).
   - ✅ **Vidro com desfoque real**. Testado num spike com a janela inativa (`NOACTIVATE`): `HostBackdropBrush`, `DWMWA_SYSTEMBACKDROP_TYPE` (acrílico), acrílico/blur por `SetWindowCompositionAttribute` e `DwmEnableBlurBehindWindow` **não desfocam** (cor plana ou só transparentes). Solução própria (`win/backdrop.rs`): ao sair do repouso (a pílula ainda no sítio) fotografa-se do ecrã a zona da janela (GDI `BitBlt`, uma vez), remenda-se a pílula preta repetindo a linha abaixo, desfoca-se uma vez (Gaussian Blur do D2D, 24 DIP, mais saturação a 170%, para um bitmap) e cada frame só pinta esse bitmap com uma tinta escura a 40% por cima, brilho branco a 10% na faixa de topo (24 DIP, recortada) e rebordo branco com gradiente (46% em cima, 6% em baixo). Em repouso a fotografia é deitada fora. Sem a fotografia (falha), cai para preto translúcido a 72%. Sem o azul. `vidro = false` volta a preto quase opaco (0,96). **Limitação**: a fotografia é estática enquanto a island está aberta (um vídeo por baixo não se mexe no vidro).
   - ✅ **Transição cruzada** entre conteúdos do notch (0,18 s, smoothstep): o conteúdo antigo desvanece, recortado à forma atual, enquanto o novo aparece; a forma já animava a largura por molas. Só conta se mudar o módulo, o texto ou a notificação (arrastar o volume, que só mexe na barra, não pisca).
   - Medido: repouso 0 ciclos, RAM privada ~10 MB; o vidro com desfoque custa ~60 ms de CPU a mais por abertura (captura e desfoque, uma vez); o resto da animação é o custo-base do WARP (~140 ms).
   - ✅ Painel redesenhado: separadores em pílula segmentada que **abrem ao passar o rato** (espera de 120 ms, `ui.tab_at` entra no timeout do loop: 0 polling; sair da janela cancela), botões da música em círculos translúcidos, sliders do Som de 8 DIP sem botão.
   - ✅ **Avisos do sistema** (`Event::System`, módulo `modules/src/system.rs`, `Priority::Attention`, 1,5 s para teclas e copiar, 2,5 s para bateria e dispositivo; um novo substitui o anterior; chave `avisos_do_sistema` no config):
     - Caps/Num/Scroll Lock: `win/keys.rs`, thread própria com janela `HWND_MESSAGE` e raw input `RIDEV_INPUTSINK` (nunca um hook). Estado lido uma vez no arranque e invertido a cada premir, sem repetições. Medido: 0,4 ms de CPU por tecla, na thread das teclas; a de UI não acorda.
     - Carregador e bateria fraca (20% e 10%, uma vez cada, só sem carregador): `win/sysevents.rs`, `RegisterPowerSettingNotification` → `WM_POWERBROADCAST`; a notificação inicial que o Windows manda ao registar não conta como mudança; desktops sem bateria não avisam.
     - Copiado: `AddClipboardFormatListener` → `WM_CLIPBOARDUPDATE`, só o tipo (texto, imagem, ficheiros; **nunca o conteúdo**), rajadas a menos de 300 ms juntam-se, e ignora conteúdo marcado como sensível (`ExcludeClipboardContentFromMonitorProcessing`, gestores de palavras-passe).
     - Dispositivo de áudio: `OnDefaultDeviceChanged` → `Msg::DeviceChanged` → nome amigável (`PKEY_Device_FriendlyName`).
     - Testado com teclas e clipboard reais (estado ativado/desativado certo, expira sozinho). **Por testar à mão**: ligar/desligar o carregador e trocar de auscultadores.
   - ✅ **Toast rico das notificações** (`Mode::Toast`, 380×72, raio 26): ícone da app (`appicon::for_app(aumid)`, cache de 32 apps), título, nome da app ao lado (se couber) e uma linha de texto com reticências. `Compact.toast: Option<Toast>` leva app, corpo, ícone e o controlo `open`. **Clicar abre a app** (`Command::OpenApp` → `shell:AppsFolder\<aumid>`) e fecha o toast; enquanto o toast está à mostra, o hover não abre o painel (senão o clique nunca chegava ao toast; largar ficheiros continua a abrir). Testado com um toast real do Windows: ícone, texto, hover, clique (abriu a app) e expirar. RAM privada ~16 MB, repouso 0 ciclos.
   - Fase 10 completa. Por testar à mão: ligar/desligar o carregador (o aviso apareceu sozinho num teste, quando o carregador foi desligado) e trocar de auscultadores.

## Como trabalhar neste repo

- `cargo test --workspace` tem de passar sempre. `cargo clippy --workspace --all-targets` sem avisos.
- Testar módulos sem Windows: `cargo run -p island -- --headless` e escrever JSON no stdin
  (ver `crates/platform/src/headless.rs`).
- Testar o backend Windows antes de haver pipe: `cargo run -p island -- --stdin` abre a janela e
  também lê o protocolo pelo stdin. A view vai para o stderr (só se vê em debug).
- Os cliques passam para as apps de baixo por causa da região da janela, não de `HTTRANSPARENT`
  (esse só funciona entre janelas da mesma thread).
- Módulo novo: ver o comentário no topo de `crates/modules/src/lib.rs`.
- Mudanças no protocolo: atualizar `PROTOCOL_VERSION` só se forem incompatíveis, e os testes de `proto`.
- Preferir mudanças pequenas e revistas a ficheiros gigantes. Se uma tarefa tocar em
  mais de uma fase do roadmap, divide-a.

## Ideias futuras (não implementar sem pedido)

- Ações de volta para apps externas (botões definidos no `show`, resposta pelo pipe).
- Chat rápido com o Claude via API, com atalho global.
- Temas e config por ficheiro com hot reload.
