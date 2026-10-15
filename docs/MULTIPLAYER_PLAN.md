# Plano — Fase 1: coop online (PoC → MVP)

Complementa `MULTIPLAYER_FEASIBILITY.md`. **[F]** = feito e testado; **[H]** = hipótese a validar.

## Marcos

| # | Marco | Entrega verificável | Estado |
|---|---|---|---|
| M0 | Protocolo + simulação autoritativa headless | `survivors-net`, `survivors-server::sim`; testes de determinismo, clamp de input, escala por nº de jogadores | **[F] feito** |
| M1 | Servidor WS de uma sala (2 jogadores) | `/ws`, Join/versão, Welcome, snapshots 20 Hz, sala cheia recusada, saída de jogador; teste E2E com 2 clientes reais | **[F] feito** |
| M2 | Cliente headless de medição | Bot que mede banda/latência com 220 inimigos e RTT simulado (critérios 1–2 do experimento) | próximo |
| M3 | Cliente wasm em modo `online` (`survivors/online.rs`, `?survivors_server=ws://…`): snapshots → unidades remotas interpoladas, predição + reconciliação do herói local, eventos → VFX. **[F] compila em wasm32; [H] não executado** (faltam os dados do WoW neste ambiente) | feito, falta validar no navegador |
| M4 | PoC fechado | 2 navegadores se veem, lutam contra a mesma horda, 1 habilidade com VFX por evento | — |
| M5 | Extrair lógica real | `data.rs` → crate compartilhada; habilidades/inimigos reais no servidor; `Hit.source` | MVP |
| M6 | Salas | Código de convite, lista pública, `max_players` 2–4, lobby/início/fim de partida | MVP |
| M7 | Progressão por jogador | Cartas de level-up sem pausar o mundo (oferta com timeout), HUD de equipe | MVP |
| M8 | Bosses, reconexão, deploy WSS, testes | — | MVP |

## Decisões já tomadas no PoC

- Um frame WebSocket binário = uma mensagem `postcard`; `PROTOCOL_VERSION` recusa clientes incompatíveis.
- Servidor autoritativo a 20 Hz; cliente envia só `{seq, dir, aim}`; servidor normaliza `dir`
  (velocidade nunca excede `BASE_SPEED`), descarta `seq` antigo e valores não finitos.
- Snapshot completo por tick (sem delta/AOI). Medido: 220 inimigos ≈ <3,2 KB por snapshot
  (≈64 KB/s a 20 Hz) — dentro do orçamento, sem delta, **[H]** ainda sem eventos/cartas.
- Sala vazia não simula. Escala de população: `1 + 0,6·(n−1)` sobre a curva solo.
- A simulação do PoC é uma **fatia fina** (perseguição, contato, 1 bolt), não a lógica real de
  `abilities.rs`/`enemies.rs`. Isso evita puxar o grafo Bevy do `benilla-app` agora; a extração
  real fica para M5, depois que M3 provar o cliente.

## O que o PoC ainda NÃO prova

1. Que o cliente wasm renderiza snapshots com `units.rs` sem custo de frame relevante (M3).
2. Predição/reconciliação do herói local sob RTT de 100 ms (M3).
3. Que a lógica real de habilidades cabe numa crate sem Bevy de render (M5).
4. CPU do servidor com 220 inimigos e várias salas (medir em M2).

## Critério de decisão

Se M3 falhar em (1) frame time ou (2) rubber-banding perceptível, o veredito sobe de "viável
com ressalvas" para "complexo" e o MVP encolhe (2 jogadores, sem lista pública).

## Como rodar

```bash
cargo test -p survivors-net -p survivors-server
cargo run -p survivors-server -- --bind 127.0.0.1:8100 --max-players 2   # ws://127.0.0.1:8100/ws
```
