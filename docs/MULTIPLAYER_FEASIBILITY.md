# Azeroth Survivors — viabilidade de multiplayer cooperativo (e PvP)

Diagnóstico baseado na leitura do código em `eb9bbe3`. Nada foi compilado, executado nem medido.
Convenção: **[F]** = fato confirmado no código; **[H]** = hipótese a testar.

## 1. Veredito

**Viável com ressalvas — e mais perto de "complexo" do que o README sugere.**

- Coop de 2 a 4 jogadores como MVP de portfólio: viável, mas é uma **reescrita do loop de jogo
  para servidor autoritativo**, não uma "camada de rede" sobre o que existe.
- O reaproveitamento é grande em **dados e apresentação** (classes, habilidades, mapas, modelos,
  HUD, áudio) e pequeno em **lógica de simulação** (acoplada a um herói único e a recursos globais).
- PvP (fase 2) é incremental *depois* do coop autoritativo; Aliança×Horda (fase 3) é só dados.
- Nada em nenhum crate deste repositório já implementa multiplayer do Survivors.

## 2. Arquitetura atual (o que existe de verdade)

### 2.1 O jogo é 100% local e de herói único [F]

- `survivors/mod.rs` abre dizendo: *"There is no server: the hero, the enemies and the spells are
  local entities"*. Entrada: `run_survivors` (`lib.rs:230`) liga um flag e chama o mesmo
  `launch()` do cliente WoW completo, com `CaptureMode` desligando controlador do jogador e FrameXML.
- **Estado da partida = um `Resource` global `Run`** (`mod.rs`): HP, XP, nível, habilidades,
  passivas, escudo, espinhos, cooldown de mobilidade, kills, bosses, `damage_done`. Há 1 `Run`
  para o app inteiro. ~200 referências a `run.` / `Res<Run>` em `survivors/*` (hud.rs 67,
  enemies.rs 34, hero.rs 32, abilities.rs 31).
- **Herói único assumido por todo sistema:** `hero.single()` / `single_mut()` em `drive_hero`,
  `hero_vitals`, `place_camera`, `aim_cursor`, `move_enemies`, `spawn_enemies`, `cast_abilities`,
  `collect_gems`. Se houver 2 `Hero`, `single()` falha e esses sistemas retornam em silêncio.
- **Inimigos:** componente `Enemy` com HP, status (slow/root/stun/fear/DoTs). `move_enemies`
  persegue "o herói"; `spawn_enemies` gera em anel em volta de "o herói" (34–42 yd), bosses a
  30 yd, "anel de cerco" a 26 yd. Alvo de população: `target_alive` = até **220 vivos**
  (`data.rs:1331`), spawn a cada 0,12 s. Não há noção de "qual jogador".
- **Dano:** mensagem `Hit { target, damage, dot, effect, mark, splash, crit, ... }` →
  `PendingHits` (agendados por `run.elapsed`) → `apply_hits`. `Hit` **não tem origem** (quem
  atacou), e só o alvo-inimigo é consultado. Dreno de vida (`Effect::Drain`) escreve em `run.hp`.
  Dano em jogador acontece em `move_enemies` via `hurt_hero(&mut Run, ...)`.
- **Progressão:** XP/gemas (`Loot` → `Gem` → `collect_gems` credita em `run.xp`), level-up
  entra em `Phase::LevelUp` e **pausa `Time<Virtual>` do mundo inteiro** (`OnEnter(LevelUp)` →
  `pause`). Cartas sorteadas com `Rng` global semeado pelo relógio (`SystemTime`).
- **Fases (`Phase`)** — Menu/Playing/LevelUp/Paused/GameOver — são um `State` global do Bevy.
- **Dados estáticos:** `data.rs` (2020 linhas) com `CLASSES`, `ABILITIES`, `MAPS` (16 zonas),
  rosters de inimigos/bosses, curvas de dificuldade — são `static` puros, **100% reaproveitáveis**
  (cliente e servidor leem a mesma tabela).
- **Unidades** (`units.rs`): spawn de `Guid + NetEntity + ObjectStore` com campos
  estilo descritor do WoW (`FIELD_HEALTH`, `FIELD_FACTION`...). O Survivors reusa o pipeline de
  render/animação/spell-visual do cliente WoW alimentando esses campos localmente.
  Herói = `FACTION_FRIENDLY`, inimigos = `FACTION_MONSTER`.
- Testes: não há `#[test]` em `survivors/` [F] (grep retornou 0).

### 2.2 O que `wenilla-host` realmente é [F]

Servidor `axum` local, ~300 linhas úteis (`ws.rs` 143, `data.rs` 412, `static_site.rs` 61):
1. serve o bundle wasm estático;
2. serve `/data/*` lendo dos MPQs do WoW (sem login; só loopback);
3. **`/ws/{porta}`: relay cego de bytes WebSocket↔TCP** para uma allowlist fixa `{3724, 8085}`
   (`ALLOWED_PORTS`, realmd/mangosd). Não decodifica nada, não tem sessão, sala, identidade
   nem estado de jogo.

É infraestrutura de *transporte* para o cliente WoW falar com um servidor MaNGOS externo. Não é
base de servidor de jogo.

### 2.3 `wenilla-realm` **não existe neste repositório** [F]

- `AGENTS.md` e o `--help` do host referenciam `crates/wenilla-realm` ("login, play page, admin
  panel, relay") e `Arnesen/wenilla-realm`. Em `crates/` há **apenas** `wenilla` e
  `wenilla-host` (`ls crates | grep realm` vazio); o workflow `check.yml` citado no AGENTS.md
  também só testa host/UI/wasm.
- O `AGENTS.md` descreve o upstream **wenilla** (fork do benilla), não este fork do Survivors:
  boa parte dele (pin bot, `realmctl`, `play.html`) **não se aplica aqui**.
- Mesmo no upstream, pelas descrições, o realm é login + hospedagem + relay sobre o mesmo relay
  de bytes. **Nada indica lógica de salas ou de jogo.** Conclusão: *a existência do realm não é
  prova de multiplayer*; [H] seria no máximo um ponto de partida para autenticação/hospedagem,
  e nem está disponível para leitura aqui.

### 2.4 Rede reaproveitável? [F]

Há bastante rede no cliente — mas é **a pilha do protocolo WoW 1.12.1** (`benilla-protocol`:
`auth` SRP, `messages`, `world`, `transport/web.rs` com `Conn` sobre `WebSocket`). Ela é falada
com um MaNGOS, não com um servidor do Survivors. Reaproveitável, com adaptação:
- `transport/web.rs` + `transport/native.rs`: abstração de conexão binária (wasm via WebSocket,
  nativo via TCP), com remontagem de stream. **Útil como inspiração/ou base** do transporte.
- `ui_party/*` (5,8 k linhas), `target/relations.rs`, `ui_reputation.rs`, `ui_duel.rs`,
  `ui_battlefield*.rs`: UI/lógica de grupo, hostilidade, duelo e BG do **cliente WoW dirigidos por
  pacotes do servidor** (`GroupList`, `GroupInvite`...). Existe um ecossistema de *apresentação*
  (retratos de party, nameplates, marcas de raid), mas **não há lógica de grupo/PvP funcionando
  no Survivors**: `survivors/` nunca referencia `ui_party`, `target` nem grupo.

### 2.5 Grupos, facção, alvos, hostilidade: o que há de funcional [F]

| Sistema | Existe no cliente WoW | Funciona no Survivors? |
|---|---|---|
| Grupo/party/raid (`ui_party`) | Sim, orientado a pacotes de servidor | **Não** — não é usado |
| Facção / reação (`target/relations.rs`, `ring_reaction`) | Sim, `CanAttack/CanAssist` do cliente 1.12 contra tabelas de facção/reputação | Só como campo `FIELD_FACTION` em entidades (herói amigável, inimigo monstro). **Nenhum código decide dano por facção**; dano é "habilidade → `Enemy`" |
| Seleção de alvos | Sim (clique, TAB, hover) | **Não** — habilidades são automáticas, `nearest/toughest/densest` sobre `Enemy` |
| PvP/duelo | UI de duelo/BG para servidor | **Não** |
| Seleção de personagem | `char_select` do WoW | Não; o Survivors tem um menu próprio (`menu.rs`), local |

Portanto: dá para **reusar vocabulário e UI** (retratos, nomes, cores de facção, `FIELD_FACTION`
para renderizar nameplates), mas **a regra de quem pode ferir quem terá de ser escrita**.

### 2.6 Premissas de ambiente relevantes [F]

- O jogo carrega dados do cliente WoW (~5,5 GB baixados por `make build`; lidos via `/data`
  do host ou arquivos locais). Todo jogador online precisa da mesma fonte de dados, e **os dados
  da Blizzard não podem ser hospedados por nós** (README: o repo não distribui conteúdo Blizzard).
  Isso limita o "demonstrável no portfólio" a: o jogador traz/aponta uma fonte de dados.
- Wasm: sem threads, sem `std::time`; a simulação roda no `Update` single-threaded.
- Sem CI de testes de jogo; a suíte inteira (~320 testes) exige patch chain do WoW.

## 3. Arquitetura mínima recomendada

**Servidor autoritativo headless em Rust**, com cliente "burro" que prevê só o próprio movimento.

```
 Navegador (wasm, Bevy)  <--WebSocket binário (postcard/bincode)-->  game-server (Rust, tokio)
  - input (WASD, F, cartas)                                           - lobby/salas (HashMap<RoomId, Room>)
  - renderiza snapshots                                               - simulação 20–30 Hz por sala
  - predição local do próprio herói                                   - dono de: heróis, inimigos, hits, XP, RNG
```

Componentes:

1. **`survivors-sim` (crate nova, sem Bevy de render):** extrair a lógica de `abilities.rs`,
   `enemies.rs`, `hero.rs` (movimento/vitals), `data.rs` para uma simulação pura (structs +
   funções). Cliente solo e servidor usam a mesma. É o item de maior custo e maior valor.
   Alternativa [H]: rodar `bevy_app` headless no servidor com os mesmos systems — evita a
   extração, mas puxa o grafo pesado do `benilla-app` (Lua, áudio, render) e **não foi validado**
   que compila sem GPU/dados do WoW.
2. **`survivors-net` (crate nova):** mensagens `ClientMsg` (Join, Input{seq,dir,aim,mobility},
   PickCard) e `ServerMsg` (Welcome, Snapshot{tick, players, enemies, events}, LevelUpOffer,
   GameOver). Serialização compacta; framing por mensagem WebSocket (um frame = uma mensagem).
3. **`survivors-server` (crate nova, binário):** WebSocket (`axum`+`tokio`, o `ws.rs` serve de
   referência de estilo), salas públicas/privadas (lista, código de convite, `max_players`),
   ciclo lobby→partida→resultados, tick fixo, RNG com seed da sala.
4. **Modo cliente `online` em `benilla-app/survivors`:** em vez de simular, aplica snapshots em
   entidades com `Guid`+`ObjectStore` (o pipeline de render/animação do WoW já é alimentado por
   campos), interpola ~100 ms, prediz movimento local, toca VFX por *eventos* (`Cast`, `Hit`,
   `Death`), e abre menus de sala (lista/criar/entrar por código).
5. **Hospedagem:** um processo + TLS (WSS) atrás de proxy; sem banco no MVP (salas em memória).

Decisões de sincronização (todas **[H]** até prototipar):
- Servidor envia posição/vida/estado dos jogadores e dos inimigos **na área de interesse**;
  inimigos como entradas pequenas (id, x, z, hp quantizado, flags). 220 inimigos × ~12 B ×
  20 Hz ≈ 53 KB/s por cliente sem delta — aceitável; com delta/AOI cai bastante.
- Habilidades: o servidor resolve o dano; o cliente recebe `CastEvent{caster, spell, alvo/destino}`
  e usa o pipeline atual de efeitos visuais. Projéteis e áreas visuais são locais e cosméticos.
- Level-up **não pode pausar o mundo**: o `Time<Virtual>::pause` atual não existe em coop.
  Opção: oferta de cartas individual com timeout curto, mundo segue rodando (ou "slow-mo"
  quando todos estão em oferta).
- Latência: com tick de 20–30 Hz e RTT 50–150 ms, jogo de horda aceita bem; crítico é o
  movimento do jogador (predição + reconciliação), o resto tolera atraso.

## 4. Reaproveitamento

| Parte | Reuso | Observação |
|---|---|---|
| `data.rs` (classes, habilidades, mapas, rosters, curvas) | ~95% | Mover para crate compartilhada |
| Render/animação/VFX de unidades (`units.rs`, spell visuals, `Marks`, `Fx`) | ~80% | Cliente continua alimentando `ObjectStore` |
| Menu, HUD, cartas, minimapa (`menu/hud/cards/minimap.rs`, ~4 k linhas) | ~50% | HUD lê `Run`; precisa ler "estado do meu jogador" vindo do servidor + painel de equipe |
| `hero.rs` (movimento, câmera, pulo) | ~40% | Câmera/input ficam; movimento autoritativo vai ao servidor |
| `abilities.rs` (814), `enemies.rs` (516), `mobility.rs` (278) | ~30–50% da lógica | Reescrever contra N heróis e origem de dano; a matemática de dano/cooldown/CC se reaproveita |
| `Run` / `Phase` / `Rng` | ~0% como estão | Viram `PlayerState` por jogador + estado da sala |
| `wenilla-host` | Transporte WS (conceito) | Relay de bytes não serve ao servidor de jogo |
| Pilha de protocolo WoW (`benilla-protocol`) | Só `transport/` como referência | Não usar MaNGOS (ver riscos) |

Estimativa grosseira: ~25–35% do código de *gameplay* sobrevive quase intacto; ~60–70% do código
de *apresentação* sobrevive. O trabalho novo é a simulação multi-jogador e a camada de rede.

## 5. Plano do MVP cooperativo (menor versão divertida)

Salas: `create_room{public|private, max_players 2..4, map, class}`, lista pública, código de
6 caracteres, host = quem criou, partida inicia pelo host (ou auto com N prontos), reentrada
desativada no MVP.

Partida: 1 mapa, 1 equipe, inimigos escalonados por nº de jogadores (alvo vivo ×(1+0,6·(n−1)),
HP ×), XP compartilhado ou por proximidade, cada jogador com suas cartas; chefe a cada N minutos;
vitória/derrota coletiva; jogador morto assiste (ou revive com timer), partida acaba quando todos
morrem. Fora do MVP: ressurreição, loot compartilhado complexo, chat (opcional: ping/emotes).

## 6. Fases futuras

**Fase 2 — PvP por equipes.** Depois do coop autoritativo, é principalmente regra:
- `team: u8` por jogador na simulação; função `can_hurt(a, b)` única (mesmo time: não; inimigo
  ou time rival: sim). Hoje não existe; o `ring_reaction` do cliente WoW **não** deve ser usado
  para isso.
- `Hit` ganha `source`; habilidades passam a escolher alvos em dois conjuntos (inimigos + jogadores
  rivais); `hurt_hero` vira `hurt_player(source)`; tabela de balanceamento PvE→PvP (dano em jogador
  separado, imunidade pós-respawn, tempo de respawn).
- Salas ganham seleção de time e placar. Cores de nameplate/UI via `FIELD_FACTION`/times.
- O custo real está em **balanceamento e netcode justo** (lag compensation não necessário para
  habilidades auto-direcionadas, mas hit-registration entre jogadores é percebida).

**Fase 3 — Facções (Aliança×Horda).** Dados: raças e `faction template` já aparecem no menu
(oito raças) e no pipeline de unidades. Mapear raça→facção e restringir classes/raças por lado.
Pouco código se a fase 2 estiver pronta; o resto é design (mapas por facção, NPCs neutros).

## 7. Arquivos e módulos

**Alterados (existentes):**
- `survivors/mod.rs` — `Run`/`Phase`/`Rng` globais; dividir em estado de jogador vs. de sala.
- `survivors/hero.rs` — `single()` em `drive_hero`, `hero_vitals`, `aim_cursor`, `place_camera`.
- `survivors/enemies.rs` — `spawn_enemies`, `move_enemies` (alvo mais próximo de N jogadores),
  `collect_gems`, `drop_loot`.
- `survivors/abilities.rs` — `cast_abilities`, `tick_areas`, `apply_hits`; `Hit.source`; dreno,
  espinhos, escudo por jogador.
- `survivors/mobility.rs` — cooldown/efeito por jogador, validado no servidor.
- `survivors/cards.rs`, `hud.rs` (2232 linhas, 67 acessos a `run`) — oferta por jogador, HUD do
  jogador local + painel dos aliados.
- `survivors/menu.rs` — telas de lobby/sala.
- `survivors/data.rs` — extrair para crate compartilhada.
- `benilla-app/src/lib.rs` — novo ponto de entrada de modo online; `wenilla/src/lib.rs` idem.
- `web/` + `scripts/web-build.sh` — query/UI para URL do servidor e código de sala.

**Novos (previstos):** `crates/survivors-core` (dados + simulação), `crates/survivors-net`
(protocolo), `crates/survivors-server` (salas + tick), módulo `survivors/online/` no cliente.

## 8. Estimativas

Premissas: uma pessoa, com assistência de IA para código repetitivo, mas **incluindo** integração,
depuração e playtest (IA não elimina esse custo, e o grafo do `benilla-app` é grande e de build
lento); 1 dia útil = 6 h produtivas; desenvolvedor já familiarizado com Rust/Bevy;
dados do WoW disponíveis localmente. Intervalos refletem incerteza, não margem de segurança.

| Entrega | Escopo | Horas | Dias úteis |
|---|---|---|---|
| **PoC 2 jogadores** | Servidor mínimo WS (1 sala fixa), 2 heróis movendo e vendo um ao outro, inimigos spawnados e perseguindo o mais próximo, 1–2 habilidades autoritativas, sem cartas/HUD novo | **40–70 h** | **7–12 d** |
| **MVP coop** | Salas públicas/privadas (2–4), lobby, simulação extraída, todas as habilidades/classes do menu, cartas por jogador, bosses, HUD de equipe, morte/fim de partida, reconexão básica, deploy WSS, testes de simulação | **200–340 h** | **35–55 d** |
| **PvP por equipes** | `team`, `can_hurt`, origem de dano, alvos rivais, placar, respawn, balanceamento inicial | **70–130 h** | **12–22 d** |
| **Facções A×H** | Mapear raça→facção, restrições, mapas/spawns por lado, polimento | **30–70 h** | **5–12 d** |

Incertezas principais: (a) se a lógica puder ser extraída sem puxar `benilla-app`, o PoC fica no
piso do intervalo; (b) a qualidade do movimento/predição no wasm é o maior fator de retrabalho;
(c) o custo de CPU de 220 inimigos + 2–4 jogadores no servidor e no wasm não foi medido;
(d) tempo de build/iteração do workspace é grande e afeta todas as estimativas.

## 9. Cinco maiores riscos técnicos

1. **Acoplamento ao estado singleton (`Run`, `Hero`, `Phase`, `Time<Virtual>`).** Cada sistema
   assume 1 herói e a pausa global do level-up. O custo está espalhado por ~10 arquivos e
   `hud.rs` (2,2 k linhas) lê `Run` por toda parte. Subestimar isto é o erro mais provável.
2. **Servidor autoritativo sem reaproveitar o grafo Bevy do cliente.** Extrair a simulação limpa
   é caro; rodá-la headless dentro do `benilla-app` é incerto (dependências de render/Lua/áudio/
   dados MPQ). Também mantém os dois caminhos (solo e online) sincronizados.
3. **Netcode no wasm sem threads: predição, interpolação e largura de banda** com até ~220
   inimigos e efeitos. Ruim em RTT alto; o cliente já tem hitches documentados em
   `docs/PERFORMANCE.md` (ex.: colisores, áudio) que podem mascarar ou agravar jitter.
4. **Dados do WoW e hospedagem.** Cada jogador precisa do cliente 1.12.1 (≈5,5 GB) e o repo não
   pode redistribuir conteúdo Blizzard — o "demo de portfólio" público fica dependente do
   visitante fornecer dados ou de um acordo/uso privado. Risco de produto e jurídico, não só técnico.
5. **Ausência de testes e de realm utilizável como base.** O Survivors não tem testes
   automatizados; `wenilla-realm` não está no repo e a pilha de rede existente é a do protocolo
   WoW. Refatorar ~5 k linhas de gameplay sem rede de segurança, mais balanceamento (escala por nº
   de jogadores, XP, cartas), tende a gerar regressões e semanas de ajuste.

Fora da lista, mas relevante: anti-cheat básico (autoridade resolve), licença/fonte do original
(README credita Aethus; confirmar que um fork online é aceitável antes de publicar).

## 10. Recomendação

**Vale a pena como projeto de portfólio, se o escopo for protegido.** Não vale como "adicionar
multiplayer rápido": o ganho demonstrável é a arquitetura (servidor autoritativo, salas,
sincronização no navegador via wasm).

**Menor MVP divertido:** 2–4 jogadores, uma sala por código de convite, **um mapa**, três classes
no máximo, todas as cartas existentes, bosses compartilhados, XP compartilhado, fim de partida
coletivo, sem lista pública no primeiro corte (lista pública entra depois, é barata).

**Primeiro experimento técnico (1–2 semanas, descartável):** servidor Rust headless com uma sala
fixa simulando **N heróis + inimigos perseguindo o mais próximo + uma habilidade** (movimento e
dano autoritativos, snapshots a 20 Hz) e um cliente wasm que renderiza esses snapshots com o
pipeline `units.rs`. Critérios de sucesso, medidos: (1) dois navegadores se veem e movem
rubber-banding perceptível com RTT 100 ms simulado; (2) 200 inimigos sem estourar banda
(~<100 KB/s por cliente) nem CPU do servidor; (3) o frame time do cliente wasm não piora de forma
mensurável vs. solo; (4) confirmação de que a simulação pode ser extraída (ou rodar headless)
sem puxar render/dados MPQ. Se (1) ou (4) falharem, o veredito passa a "complexo" e a
recomendação muda para um MVP menor (co-op local-first ou 2 jogadores com host-cliente).
