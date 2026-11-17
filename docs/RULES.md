# Rules implemented by the engine

The engine implements the base game for 2–4 players. Every setting that differs between the official rules, online platforms and house rules is a field of `Rules` (`engine/core/src/rules.rs`), never a code branch. This page lists what each preset does and marks the places where the engine had to choose something the rules leave open.

Sources for `official`: the base-game *Game Rules & Almanac* (catan.com, `catan_base_rules_2020_200707.pdf`, SHA-256 `3fed941b…f6f4ac024`), and the publisher's base-game FAQ (catan.com/faq/basegame), both read on 2026-11-02. For `colonist-1v1`, the platform's 2023–2024 blog posts on ranked 1v1 settings, summarised in `~/reports/Catan mastery and product ideas.md`. **The 1v1 preset has not been checked against the live game as of 2026-10.**

## Presets

| Setting | `official` | `colonist-1v1` | `catanatron-compat` (testing only) |
|---|---|---|---|
| Players | 2–4 | 2 | 2–4 |
| Points to win | 10 | 15 | 10 |
| Discard on a 7 when holding more than | 7 cards | 9 cards | 7 cards |
| Friendly robber | off | on, protects opponents with ≤ 2 **visible** points | off |
| Dice | two fair dice | balanced deck (see below) | two fair dice |
| Development cards before rolling | any card | any card | any card |
| Player-to-player trading | on, open trading (counter-offers), ≤ 3 offers per turn | off | off |
| Bank shortage | official rule | official rule | nobody gets it |
| Board | official spiral number placement, ports shuffled | random numbers, no adjacent 6/8, ports shuffled | official spiral |

## Base rules

**Board.** There are 19 land hexes: 4 forest, 3 hills, 4 pasture, 4 fields, 3 mountains and 1 desert. The 18 number tokens are 2, 3–6 ×2, 8–11 ×2 and 12. The 9 harbours sit on every other hex of the surrounding sea ring and face the same land hexes as on the physical frame. Four are 3:1 and five are 2:1, one per resource. The robber starts on the desert. Official variable setup lays tokens A–R (5, 2, 6, 3, 8, 10, 9, 12, 11, 4, 8, 10, 9, 4, 5, 6, 3, 11) in a spiral from a random corner, skipping the desert.

**Supply.** Each player has 15 roads, 5 settlements and 4 cities. Upgrading to a city returns the settlement to the player's supply. The bank holds 19 cards of each resource. The development deck has 14 knights, 5 victory points, and 2 each of Road Building, Year of Plenty and Monopoly.

**Initial placement.** Players place in snake order (1, 2, … n, n, … 2, 1), a settlement and then a road touching it each time. The second settlement collects one card from each adjacent resource hex.

**Building.**

- **Distance rule:** a settlement can't be next to any other building.
- **Settlements:** after initial placement, a settlement must touch one of the player's roads.
- **Roads:** a road must connect to the player's own building, or to the player's own road through a corner that no opponent occupies. You can't build through an opponent's settlement.

**Production.** Each hex whose number was rolled pays 1 card to each adjacent settlement and 2 to each adjacent city, unless the robber is on it. **Bank shortage (official):** if the bank can't pay everyone owed a resource, nobody receives it. The exception is when only one player is owed it: that player receives what's left. `catanatron-compat` uses "nobody receives it" instead.

**Sevens.**

1. Every player holding more than the discard limit discards half their cards, rounded down.
2. The roller must then move the robber to a different hex.
3. The roller must steal one random card from an opponent with a building on that hex, if any such opponent has cards. With several candidates, the roller chooses one.

*Engine choice:* discards are taken one card at a time, in seat order starting from the roller. The rules have everyone discard at once, and the order changes nothing.

**Development cards.**

- **Timing:** one card per turn, at any time during your turn, including before rolling. A card can't be played on the turn it was bought. Victory point cards are never played; they count as soon as you hold them.
- **Knight:** move the robber, as on a 7, but without discards. A knight played before rolling returns you to the roll.
- **Road Building:** place 2 free roads, or 1 if you have only one road piece left. *Engine choice:* the card can only be played when at least one road can be placed. The rules don't forbid playing it uselessly, but doing so is never better than holding it. Once played, the roads are placed before anything else, and the turn can't end with them unplaced: the almanac says you "immediately place 2 free roads". The second is skipped if there's nowhere left to put it.
- **Year of Plenty:** take any 2 cards from the bank. *Engine choice:* both must be available. Neither the almanac nor the FAQ says what happens when the bank is short; Catanatron allows taking one.
- **Monopoly:** every other player gives you all their cards of one resource.

**Longest Road.**

- **Length:** the longest trail through your roads that uses no road twice and doesn't pass through a corner holding an opponent's building. A road leading into such a corner still counts.
- **Taking it:** you need at least 5 roads, and strictly more than the current holder.
- **When an opponent cuts a road:**
  - the holder keeps the card while still tied for the longest road, with 5 or more;
  - otherwise it goes to the unique longest road of 5 or more;
  - if two or more others tie, or nobody has 5, the card is set aside until someone is uniquely longest.
- *Reading, checked against the official sources (2026-11-02):* the 5-road minimum holds for the holder too, so a holder cut below 5 loses the card even while tied.
  - The almanac's "Special Case" says a holder whose road is broken "and [who is] tied for longest road" keeps the card, and that the card is set aside "if no one has a 5+ segment road". On its own that leaves open a holder tied at 4.
  - The publisher's FAQ settles it. Question 28 says the holder keeps the card only if they "still [meet] the requirements for the Longest Road (either alone or together with another player)". Question 29 defines the Longest Road as "at least five individual road pieces".
  - This page used to say "keeps the card while still tied" without the minimum, which contradicted "nobody has 5"; the reference model found the gap (`tools/refmodel/`). JSettlers2 reads it the same way.

**Largest Army.** You need at least 3 knights played, and strictly more than the current holder.

**Bank and harbour trades.** The rate is 4:1, 3:1 with a generic harbour, or 2:1 for that resource with a matching harbour. You always get the best rate you have. The bank must hold the card you ask for.

**Player trades.**

- Only the player whose turn it is can trade with other players, and only after rolling. The others may not trade among themselves.
- A trade must give and ask for at least one card each, with no resource on both sides, and at most 19 of a resource. That rules out gifts and like-for-like swaps (rulebook, 5th edition, Almanac "Trade": "You may not give away cards", "You may not trade like resources").
- **Open trading** (`open_trading`, on in `official` since 2026-11-12). The rulebook (5th edition, "Domestic Trade"): "You can announce which resources you need and what you are willing to trade for them. The other players can also make their own proposals and counteroffers."
  - The player on turn offers any trade to the others. Every other player holding a card answers, in seat order from the player on turn: accept (if they can pay), decline, or propose other terms of their own (a counter-offer, `ProposeTrade`). Those with no cards can do none of these and are skipped.
  - The player on turn then takes one acceptance, or one counter-offer it can pay, or none. With nothing to take, the offer lapses at once and play goes on.
  - **Counter-offers only answer an offer** (`trade_proposals` off; the owner's ruling, 2026-11-13). The others propose only when the player on turn has offered a trade, as the online game plays it: no one proposes unasked. From 2026-11-12 to 2026-11-13 the engine also had a round after each roll (after the robber, on a 7) in which every other player holding a card could propose a trade to the player on turn, or pass. That flow is the preset `official-proposal-round`, for replaying what was measured or generated with it (positions-v5), and records from those days carry it.
  - *Engine discretization:* at the table anyone may speak up at any time in the turn. The engine gives each other player one answer to each offer.
- **Without open trading** (`official-no-counters`, and every result before 2026-11-12): other players answer in seat order starting after the proposer, accept or decline only, and players who couldn't pay are skipped. The proposer then confirms one accepting player or cancels. If nobody accepts, or nobody can pay, the offer lapses at once.
- *Engine cap:* at most `max_offers_per_turn` offers by the player on turn per turn (3 in `official`), so bot games end. The rulebook has no limit, and the Play page lifts the cap for the person; the bots there keep to three a turn (`trade::OFFERS_PER_TURN`). Counter-offers aren't capped: one per player per offer. An offer that lapses still counts toward the cap.
- `legal_actions` lists only offers and proposals of one or two cards **of one resource** for one card of another (`OfferSpace`: "1-for-1 and 2-for-1"). Rule checking accepts any valid trade, and so does the Play page for the person's own offers and proposals (`server/src/play.rs`, `Table::act`).

**Winning.** You win as soon as you reach the target on your own turn, counting victory point cards. Points gained on someone else's turn, such as Longest Road changing hands after a cut, win at the start of your next turn, before the roll (FAQ question 75: you don't have to roll).

## Variant details

**Friendly robber.** The robber can't go on a hex where an opponent has a building and **visible** points (buildings and awards) of the configured maximum or fewer. If every hex is protected, it may go anywhere. Catanatron's version counts hidden victory point cards too. The live 1v1 rule still needs checking.

**Balanced dice.** A shuffled deck of the 36 two-dice outcomes, refilled once 12 remain. The online 1v1 platform also lowers the chance of repeated numbers and balances 7s between players. Those adjustments aren't published and aren't modelled.

## Known differences from Catanatron

The differential test (`tools/difftest/`) replays games in Catanatron and treats these as expected:

- **Longest Road after a road touches an opponent's building.** Catanatron doesn't count the segment into the building. After a cut it gives the award to whoever is longest, without the tie and minimum-length rules.
- **Wins on someone else's turn.** Catanatron ends the game when anyone reaches the target, even off-turn.
- **Single-card Year of Plenty.** Catanatron allows it when the bank is short.
- **Roads through an opponent's settlement.** Catanatron occasionally offers one, because its cached road network can keep a corner an opponent has since built on. This breaks the rules; the test drops those actions and counts them.
- **Discard order.** Catanatron asks players to discard from seat 0; we start from the roller.
- **Recorded road lengths.** Catanatron reports 0 until a player builds a road in normal play.

## Known differences from JSettlers2

The second differential test (`tools/jsettlers/`) replays games in JSettlers2 and treats these as expected:

- **No bank limit.** JSettlers2 doesn't count the bank, so a roll pays in full when the bank is short, and bank trades and Year of Plenty don't need the cards to be there.
- **Winning with a knight.** If the knight that gives Largest Army wins the game, JSettlers2 still has the winner move the robber before the game ends.
- **Initial roads.** JSettlers2's server accepts an initial road anywhere on the player's network; its client requires it to touch the settlement just placed, as here.
- **Road Building.** JSettlers2 lets the player play it with nowhere to build, and end the turn before placing the free roads once the dice are rolled. When the second road has nowhere to go, it waits for the player to cancel it.
- **Discards.** JSettlers2 takes each player's whole discard at once; we take it one card at a time.
