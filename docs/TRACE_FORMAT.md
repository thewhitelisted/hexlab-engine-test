# Game trace format

`hexlab diff-export` writes random games as JSON lines, one game per line. Other engines replay them and compare: Catanatron in the differential test (`tools/difftest/`), and the independent reference model. This page describes the format only. What the rules are is in [RULES.md](RULES.md).

```bash
hexlab diff-export --preset official --games 100 --seed 1 --policy uniform
```

- `--preset`: `official`, `official-proposal-round` (`official` with a round of proposals after each roll, as on 2026-11-12), `official-no-counters` (`official` without open trading), `colonist-1v1` or `catanatron-compat` (RULES.md, "Presets"). `--players N` sets the player count, 2 to 4 (default 4); `colonist-1v1` always has two.
- `--policy`: `uniform` picks uniformly among legal actions, so games are long and full of trade offers. `builder` builds whenever it can, so games reach a winner in a realistic number of turns.
- Games stop at a winner, or after turn 1000 with no winner.

## A game

| Field | Meaning |
|---|---|
| `seed` | The game's seed. |
| `rules` | The rules in force: `num_players`, `vp_to_win`, `discard_limit` (a 7 makes players holding more than this discard), `friendly_robber_max_vp` (null when off), `dice` (`random` or `balanced`), `dev_timing` (`anytime` or `knight_before_roll`), `player_trading`, `open_trading` (counter-offers, RULES.md "Open trading"), `trade_proposals` (with it, a round of proposals after each roll too; absent in records from 2026-11-12, which had the round), `max_offers_per_turn`, `shortage` (`official` or `nobody_gets`). |
| `board` | The land hexes, each with axial `q`, `r`, `resource` (null for the desert) and `number` (null for the desert); the harbours, each with `kind` (`generic` or `{"special": resource}`), the `sea` hex holding it and the `land` hex whose shared edge it serves; and the robber's starting hex. |
| `hexes` | Hex id `i` is at axial position `hexes[i]` = `[q, r]`. |
| `vertex_pos` | Vertex id `v` is at lattice point `vertex_pos[v]` = `[x, y]`. |
| `edge_vertices` | Edge id `e` joins vertices `edge_vertices[e]` = `[v, w]`. |
| `winner` | Player id, or null if the turn limit stopped the game. |
| `steps` | Every decision in order, below. |

**Geometry.** The hex at axial `(q, r)` has its centre at lattice point `(2q + r, 3r)`. Its six corners are the centre plus `(0, -2)`, `(1, -1)`, `(1, 1)`, `(0, 2)`, `(-1, 1)`, `(-1, -1)`: top first, then clockwise with y growing downward. A vertex touches the hexes it is a corner of. Two vertices are adjacent when an edge joins them. A harbour serves the two vertices shared by its `sea` and `land` hexes.

**Players** are numbered from 0. Player 0 places first and takes the first turn.

**Orders.** Resources: 0 wood, 1 brick, 2 sheep, 3 wheat, 4 ore. Development cards: 0 knight, 1 victory point, 2 road building, 3 year of plenty, 4 monopoly. Every per-resource or per-card array uses these orders.

## A step

| Field | Meaning |
|---|---|
| `p` | The player deciding. |
| `legal` | Every action legal for `p` before this step, as codes. |
| `a` | The action taken. |
| `o` | Its chance outcome, or null: `[d1, d2]` for a roll, a card index for a purchase, a resource index for a steal. |
| `s` | The position after the step (below). |

## Action codes

| Code | Action |
|---|---|
| `S{v}` | Build a settlement on vertex `v` (in placement too) |
| `C{v}` | Build a city on vertex `v` |
| `R{e}` | Build a road on edge `e` (in placement and Road Building too) |
| `ROLL` | Roll the dice |
| `BUY` | Buy a development card |
| `KN` | Play a knight |
| `RB` | Play Road Building |
| `Y{a}{b}` | Play Year of Plenty for resources `a` and `b` |
| `MO{r}` | Play Monopoly on resource `r` |
| `MV{h}:{p}` | Move the robber to hex `h` and steal from player `p`; `MV{h}:-` steals from nobody |
| `DI{r}` | Discard one card of resource `r` |
| `MT{a}{b}` | Trade with the bank: give resource `a` at your best rate, get one `b` |
| `OF{give}/{get}` | Offer a trade to the other players: five comma-separated counts each, e.g. `OF2,0,0,0,0/0,0,0,0,1` offers two wood for one ore |
| `ACC` | Accept the offer on the table |
| `REJ` | Decline it, or pass in a round of proposals after a roll (`trade_proposals`) |
| `PR{give}/{get}` | Propose a trade to the player on turn (open trading): a counter-offer when answering an offer, or a proposal in a round after a roll (`trade_proposals`). Counts as in `OF`, `give` being what the proposer gives |
| `CF{p}` | The player on turn completes the trade with player `p`: on the offer's terms if `p` accepted, on `p`'s proposal otherwise |
| `CAN` | The player on turn withdraws the offer, or turns the proposals down |
| `END` | End the turn |

## The position after a step

| Field | Meaning |
|---|---|
| `h` | Each player's resource cards, five counts. |
| `b` | The bank's resource cards. |
| `dv` | Each player's unplayed development cards, five counts, including any bought this turn. |
| `pl` | Each player's played development cards. |
| `deck` | Cards left in the development deck. |
| `rob` | The robber's hex id. |
| `vp` | Each player's points, including hidden victory point cards. |
| `lr`, `la` | Who holds Longest Road and Largest Army, or -1. |
| `len` | Each player's longest road. |
| `cur` | Whose turn it is. |
| `act` | Who decides next (another player during discards and trade answers), or -1 when the game is over. |
| `ph` | The phase: `setup_settlement`, `setup_road`, `pre_roll`, `discard`, `move_robber`, `main`, `road_building`, `trade_response`, `trade_confirm` or `game_over`. |
