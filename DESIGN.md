# Desenho da island

Referência visual: o **NotchNook** (macOS). Capturas em `docs/ui/`. São capturas de terceiros,
só para referência: não publicar com o repositório.

- `ref-painel-completo.png`: o painel expandido inteiro, no contexto do ecrã
- `ref-painel-perto.png`: o mesmo painel visto de perto (tipografia, divisórias, calendário)
- `ref-musica-detalhe.png`: o bloco de música em detalhe (capa, badge da app, controlos)

Decisões do utilizador (2026-11-07):

| Tema | Decisão |
|---|---|
| Onde aparece | **Sempre no ecrã interno do portátil**, em qualquer portátil (deteção pela ligação `INTERNAL`/eDP). Sem ecrã interno ativo: monitor primário. Já implementado. |
| Em repouso | Notch pequeno **sempre visível**, colado ao topo. Mostra o resumo do módulo vencedor. |
| Eventos | **Cresce só o necessário** (como as live activities do iPhone) e volta ao repouso. O painel completo só abre com o rato. |
| Blocos do painel | **Música + Claude Code** no separador "Nook". **Tray** (ficheiros) num separador próprio. Sem calendário e sem espelho (webcam). |

## Forma

- Preto puro `#000`, **colada à borda de cima** do ecrã (`TOP_MARGIN = 0`; hoje flutua 8 DIP).
- Cantos de cima **côncavos** ("orelhas", raio 12 DIP) que fundem a ilha com a borda do ecrã,
  como o notch do Mac. Cantos de baixo convexos e largos, como o painel do NotchNook (afinado com a captura de referência, 2026-11-10).
- **Vidro** (2026-11-10): em repouso preto puro; ao crescer, vidro com o ecrã de trás desfocado (fotografia tirada uma vez ao abrir, ver CLAUDE.md fase 10) saturação a 170% e tinta escura a 40%, brilho de topo e rebordo branco com gradiente. Transição cruzada de 0,18 s entre conteúdos.
- Três tamanhos-alvo (DIP, a afinar no ecrã):

  | Estado | Tamanho | Raio de baixo | Conteúdo |
  |---|---|---|---|
  | Repouso | 190 × 32 | 14 | ícone à esquerda, indicador/texto curto à direita |
  | Evento | até ~360 × 40, largura pelo texto | 20 | ex.: `activesync: precisa de ti`, HUD de volume |
  | Expandido (hover) | 640 × 176 | 38 | painel com separadores |

- **Borda azul suave** (`#3AA0FF`) para distinguir a ilha do resto do ecrã (pedido do utilizador, 2026-11-08):
  - linha de 1 DIP a 45% (2026-11-10: o utilizador pediu "menos forte, mais clean");
  - brilho por fora quase impercetível: uma camada de 4 DIP a 6%;
  - não passa pela borda de cima do ecrã;
  - a região da janela é alargada 4 DIP para o brilho não ser cortado.
- Transições com as molas que já existem (`IslandShape`, `SpringParams::BOUNCY`).
- A região da janela segue a forma, incluindo as orelhas: atualizar quando o **alvo** muda, nunca a cada frame.

## Painel expandido

Cabeçalho (~28 DIP):
- À esquerda, os separadores **Nook**, **Som** e **Tray**. O selecionado é uma cápsula `rgba(255,255,255,.12)` com ícone + texto.
- À direita, a engrenagem: abre a configuração (fase 9).

Separador **Nook**: blocos lado a lado, separados por divisórias verticais finas `rgba(255,255,255,.08)`.

1. **Música**
   - Capa 72×72 com raio 12. O ícone da app (badge ~22×22) fica no canto inferior direito, a sair da capa.
   - Título (branco, semibold 13), álbum (55% de opacidade) e artista (40%).
   - Controlos ⏮ ⏯ ⏭ brancos, ~16 DIP.
2. **Claude Code**: uma entrada de duas linhas por sessão ativa (projeto por cima, estado por baixo, a 55%).
   O botão de abrir é só um ícone (↗): com três colunas, a do Claude fica com ~126 DIP.
   - Ponto de estado à esquerda:

     | Cor | Estado |
     |---|---|
     | azul | a trabalhar |
     | laranja a pulsar | precisa de ti |
     | verde | terminou |

   - Texto `projeto: estado`.
   - Botão "Abrir no VS Code".
   - Sem sessões: o bloco mostra só "Sem sessões" em cinzento.

3. **Apps externas** (mensagens `show`): título, detalhe e barra de progresso numa terceira coluna.

Larguras das colunas, com mais de uma: música 260, apps externas 150, e o Claude fica com o resto.

Separador **Som** (misturador): volume geral e uma linha por app, com nome, slider, percentagem e mute. Arrasta-se o slider; as linhas que não cabem ficam de fora.

Separador **Tray**:
- Arrastar ficheiros por cima do notch abre a ilha já neste separador, com a zona de largar acesa a azul.
- Uma fila de até 7 ficheiros: ícone do Explorador (48 DIP), nome por baixo e um ✕ no canto.
- Clicar abre o ficheiro; arrastá-lo leva-o para outra app (se for movido, sai da lista); o ✕ só tira da lista.

## Tipografia e cor

- Fonte **Segoe UI Variable** (Windows 11), com Segoe UI como alternativa. DirectWrite com antialiasing em escala de cinzentos (o fundo é transparente).
- Texto branco a 100%, 55% e 35% de opacidade.
- Cores de estado:

  | Uso | Cor |
  |---|---|
  | Destaque | `#0A84FF` |
  | Atenção | `#FF9F0A` |
  | Sucesso | `#30D158` |

## Impacto na arquitetura

1. **O painel mostra vários módulos ao mesmo tempo.** Hoje o engine escolhe **um** vencedor e o
   renderer só tem `View.expanded: Vec<Row>` desse módulo. Proposta para a fase 3:
   - A `View` continua a servir o repouso e os eventos, escolhida pelo árbitro, como agora.
   - Novo `Module::widget(&self, now) -> Option<Widget>`. O engine junta os widgets por uma ordem fixa.
   - `Widget` tipado: `Media { .. }`, `Claude { .. }`, `Rows(Vec<Row>)` (para apps externas).
     `Row` genérico não chega ao acabamento das referências (capa com badge, controlos).
2. **Tamanho da janela**: `MAX_WIDTH`/`MAX_HEIGHT` passam para ~640×180 + orelhas. A região
   continua a deixar passar os cliques fora da forma.
3. **Ícone da app na capa**: AUMID do GSMTC → `AppInfo::GetFromAppUserModelId` → logo (fase 5).
4. **Tray**: `IDropTarget` com `#[implement]` na janela, e `DoDragDrop` para arrastar para fora.
   A janela é `NOACTIVATE`: confirmar que o drag & drop OLE funciona sem ativar a janela.
