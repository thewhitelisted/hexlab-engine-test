# Rules engine (`engine/core`)

A deterministic, allocation-light rules engine. It is the base for the bots, the training tools and self-play. Rules are described in [RULES.md](RULES.md).

## Now (2026-11-18)

- **Presets:** five. `official` (open trading with counter-offers), `official-no-counters` and `official-proposal-round` (the two earlier trading flows, kept so that older results replay), `colonist-1v1` and `catanatron-compat`.
- **Tests in `engine/core`:** unit tests in the source files, rule tests in `tests/rules.rs`, random play in `tests/fuzz.rs`, and `tests/hostile_records.rs` (records from outside, and records written before the player count had its own type).
- **Input from outside, as far as issue #23 covers it**, is read into checked types and refused with an error, and nothing read that way panics later: a stored record's player count, board and steps (the rules check each step before a review, the dashboard or the endgame calculator sees it), boards, typed-in positions, `--players`, and the contents of `policy-eval` arrays, `openings` study files and `perft` queries. Not yet: a record's other rule numbers, files that aren't regular or are too large to read, and other command-line numbers, which can still panic or exhaust memory (for example `study --first-game`, `positions --games` and `--per-game`). Those are issue #69.
- **Cross-checks against other implementations:** Catanatron, JSettlers2, a reference model in Python and a phase model in Quint (docs/TRACEABILITY.md). Each script builds the engine first and stops if it doesn't build.
- **The speed figures below are from 2026-10-26** and have not been measured again since open trading was added.

## Using it

```rust
use engine_core::{GameConfig, GameState, PlayerCount};

let config = GameConfig::official(PlayerCount::FOUR);
let mut state = GameState::new(&config, seed);    // board, deck and dice all from the seed
let mut legal = Vec::new();
while !state.is_over() {
    state.legal_actions(&mut legal);              // for state.acting_player()
    let action = pick(&state, &legal);
    let outcome = state.apply(action)?;           // dice, card drawn, card stolen...
}
```

**Building a state:**

- `GameState::new(config, seed)` creates a game. Everything random, including the board, deck shuffle, dice and steals, comes from one seeded RNG, so a seed reproduces a game.
- `GameState::with_board(rules, board, rng)` starts a game on a given board, and `GameState::empty_position(rules, board, seed)` plus the `put_*` / `set_*` editors and `finish_edit()` build an arbitrary position. The analysis board and the rule tests use these (`edit.rs`). `with_board` returns an error for a board that can't hold a game of the rules' player count from the initial placement on (`Board::hosts`): one land hex leaves the robber nowhere to go, and too few corners can leave a player no spot in the initial placement. `empty_position` starts after the placement, so it checks only the first (`Board::robber_can_move`). `set_turn` takes turns 1 to `MAX_TURN` (100,000).
- The player count is a `PlayerCount`, 2 to 4, defined once in `rules.rs`. The command line's `--players`, a stored record's `num_players` and a request's preset all become one through it, and anything else is refused. It is written as the bare number, so records read and write the same bytes as before it existed (`tests/hostile_records.rs`).

**Applying moves:**

- `apply(action)` checks the action and returns the chance `Outcome`.
- `apply_forced(action, Some(Forced::Dice(3, 4)))` imposes the outcome instead. Use it for replays, search over chance events and testing.
- `apply_legal(action)` skips the rule check in release builds. Use it in tight loops that only play listed moves.

**Information and records:**

- `observe(player)` is what one player may see.
- `BeliefTracker` keeps an exact probability distribution over everyone's hidden hands from one player's view.
- `GameRecord` stores a game with all chance outcomes, round-trips through JSON and replays bit-exactly (`position_hash` checks the result). `replay_steps` shows its callback a step only after the rules have applied it with its recorded outcome, so the review, the dashboard and the endgame calculator never work on a step that can't happen.

## Design

| Concern | Choice |
|---|---|
| Geometry | Board topology is built from any set of axial hex coordinates, so 5–6 player and sea boards are data, not code. Vertex ids come from an integer lattice; the standard board's topology is built once and shared. |
| Variants | `Rules` and `BoardGen` are plain serializable config. Presets: `official`, `official-no-counters`, `official-proposal-round`, `colonist-1v1`, `catanatron-compat` (`GameConfig::preset`; RULES.md lists what each sets). |
| Randomness | Own xoshiro256** (`rng.rs`), so replays never change when dependencies are upgraded. |
| Hidden information | The state is the referee's full view. `Outcome::visible_to` and `observe` give each player's view; the belief tracker works only from that view. |
| Speed | Per-player bitsets mirror the board (roads, buildings, road ends, blocked corners), so legal roads and settlements are a few bit operations. Longest road is computed per road network: a tree's diameter when nobody has cut it, otherwise a pruned search. |
| Correctness | Bitset move generation is checked against simple reference predicates (`can_road`, `can_settle`), and fast road length against a brute-force search, at every fuzzed step. |

Module map: `topology` (geometry), `board` (tiles, ports, generators), `rules` (config), `state` (game state), `legal` (move generation), `apply` (rule checks and transitions), `longest_road`, `observation`, `belief`, `record`, `edit`, `sim` (random playouts), `invariants`.

## Testing

| Layer | What it checks | How to run |
|---|---|---|
| Unit tests | Topology counts, board contents, RNG, dice, bitsets | `cargo test` |
| Rule scenarios (`tests/rules.rs`) | 56 tests on positions built with the editor: snake order, distance rule, road blocking, every Longest Road transfer, cut and tie case, Largest Army, development-card timing, wins on and off turn, robber and friendly robber, discards, bank shortages, harbours, the trade flow, Monopoly and Year of Plenty, Road Building, belief pruning, record replay | `cargo test` |
| Fuzzer | Random games checked at **every step**: card and piece conservation, the distance rule, road anchoring, award consistency, bitsets against reference predicates, fast against brute-force road length, `check` against `legal_actions` for every candidate action, exact belief tracking that keeps the true hands, plus JSON round trip and replay of every game | `hexlab fuzz --games N --threads T` |
| Differential test | Same games replayed in Catanatron: legal sets, hands, bank, cards, robber, awards and wins compared at every step | `tools/difftest/run.sh` |
| Perft | Every legal decision sequence a few plies deep from edge-case positions, chance outcomes enumerated, counted by both engines; a difference is followed down to the position where they disagree | `tools/difftest/perft.sh` |
| JSettlers2 | The same kind of replay in a second independent engine, which also implements player trading | `tools/jsettlers/run.sh` |
| Reference model | Exported games replayed step by step in a separate Python implementation written from RULES.md | `tools/refmodel/run.sh` |
| Phase model | Engine games checked as runs of a Quint model of turns and phases | `tools/phasemodel/check.sh` |

`scripts/check.sh full` runs every check, a fuzz smoke test among them; the pre-push hook runs it before every push, and CI runs it when started by hand (docs/OPERATIONS.md, "Checks before a push"). The scripts in `tools/` are run by hand; each builds the engine first. `scripts/fuzz-long.sh` is the 1M-game run.

## Performance

Measured on the VPS (6 vCPU, x86_64) on 2026-10-26, release build, one thread unless noted. Random games are much longer than real ones (about 150 turns against roughly 60–80), so actions per second is the fairer measure.

| Workload | Games/s/thread | Actions/s |
|---|---|---|
| 4 players, builder policy, offers excluded from the action space | ~8,600 | ~4.2M |
| 4 players, builder policy, 1-for-1 and 2-for-1 offers listed | ~5,000 | ~2.5M |
| 1v1 preset, builder policy (about 250 turns per game) | ~6,800 | — |
| 4 players, 4 threads | ~32,000 total | — |

Per-action cost is roughly 130 ns to generate moves, 100 ns for the random policy and 130 ns to apply a move. Placing a road costs about 600 ns (longest-road update); everything else is under 200 ns. `hexlab bench --breakdown` prints the full table.

The target of 10,000 random games per second per thread is not met: we reach ~8,600 without offers. The workload is chosen to be comparable with other engines' published figures. Rollout speed for bots will be measured directly at a later stage.
