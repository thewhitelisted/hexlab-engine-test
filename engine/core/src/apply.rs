//! Rule checking and state transitions.

use crate::actions::{Action, Forced, Outcome, RuleError};
use crate::rules::{DevTiming, ShortageRule};
use crate::state::{
    GameState, LARGEST_ARMY_MIN, PendingTrade, Phase, Resume, city_code, settlement_code,
};
use crate::types::*;

/// Purposes for the game's derived random streams (`Rng::derive`). The game's generator is only
/// advanced by the starting deck shuffle; every later chance event derives its own stream from it,
/// keyed by when it happens, so games that diverge still share their dice turn by turn.
const DICE: u64 = 1;
const STEAL: u64 = 2;

impl GameState {
    /// Applies `action` for the acting player, drawing any chance outcome from the game RNG.
    pub fn apply(&mut self, action: Action) -> Result<Outcome, RuleError> {
        self.apply_forced(action, None)
    }

    /// Applies `action`, using `forced` as the chance outcome when given.
    pub fn apply_forced(
        &mut self,
        action: Action,
        forced: Option<Forced>,
    ) -> Result<Outcome, RuleError> {
        self.check(action)?;
        if let Some(f) = forced {
            self.check_forced(action, f)?;
        }
        Ok(self.apply_unchecked(action, forced))
    }

    /// Applies an action known to be legal (e.g. taken from [`GameState::legal_actions`]). The
    /// rule check only runs in debug builds.
    pub fn apply_legal(&mut self, action: Action) -> Outcome {
        debug_assert_eq!(self.check(action), Ok(()), "{action:?} is not legal");
        self.apply_unchecked(action, None)
    }

    /// Whether `action` is legal for the acting player right now.
    pub fn check(&self, action: Action) -> Result<(), RuleError> {
        use RuleError::*;
        let topo = &self.board.topology;
        let nv = topo.num_vertices();
        let ne = topo.num_edges();
        let vertex_ok = |v: VertexId| {
            if (v as usize) < nv {
                Ok(())
            } else {
                Err(InvalidId)
            }
        };
        let edge_ok = |e: EdgeId| {
            if (e as usize) < ne {
                Ok(())
            } else {
                Err(InvalidId)
            }
        };
        let p = self.current;
        let ps = &self.players[p as usize];
        let afford = |c: &Hand| {
            if ps.hand.contains(c) {
                Ok(())
            } else {
                Err(CannotAfford)
            }
        };

        match (self.phase, action) {
            (Phase::GameOver, _) => Err(GameOver),
            (Phase::SetupSettlement, Action::BuildSettlement { vertex }) => {
                vertex_ok(vertex)?;
                if self.vertex_open(vertex) {
                    Ok(())
                } else {
                    Err(IllegalLocation)
                }
            }
            (Phase::SetupRoad { vertex }, Action::BuildRoad { edge }) => {
                edge_ok(edge)?;
                let touches = topo.edge_vertices[edge as usize].contains(&vertex);
                if touches && self.edge[edge as usize] == 0 {
                    Ok(())
                } else {
                    Err(IllegalLocation)
                }
            }
            (Phase::PreRoll, Action::RollDice) => Ok(()),
            (
                Phase::PreRoll | Phase::Main,
                Action::PlayKnight
                | Action::PlayRoadBuilding
                | Action::PlayYearOfPlenty { .. }
                | Action::PlayMonopoly { .. },
            ) => self.check_dev_play(action),
            (Phase::Discard, Action::Discard { resource }) => {
                let q = self.acting_player();
                if self.players[q as usize].hand.get(resource) > 0 {
                    Ok(())
                } else {
                    Err(NoCard)
                }
            }
            (Phase::MoveRobber { .. }, Action::MoveRobber { hex, victim }) => {
                if hex as usize >= topo.num_hexes() {
                    return Err(InvalidId);
                }
                if self.robber_hexes() & (1 << hex) == 0 {
                    return Err(InvalidRobberHex);
                }
                let victims = self.robber_victims(hex);
                match victim {
                    Some(v) if v < self.rules.num_players.get() && victims & (1 << v) != 0 => {
                        Ok(())
                    }
                    None if victims == 0 => Ok(()),
                    _ => Err(InvalidVictim),
                }
            }
            (Phase::Main, Action::BuildRoad { edge }) => {
                edge_ok(edge)?;
                if ps.roads_left == 0 {
                    return Err(NoPiecesLeft);
                }
                afford(&cost::ROAD)?;
                if self.can_road(p, edge) {
                    Ok(())
                } else {
                    Err(IllegalLocation)
                }
            }
            (Phase::Main, Action::BuildSettlement { vertex }) => {
                vertex_ok(vertex)?;
                if ps.settlements_left == 0 {
                    return Err(NoPiecesLeft);
                }
                afford(&cost::SETTLEMENT)?;
                if self.can_settle(p, vertex) {
                    Ok(())
                } else {
                    Err(IllegalLocation)
                }
            }
            (Phase::Main, Action::BuildCity { vertex }) => {
                vertex_ok(vertex)?;
                if ps.cities_left == 0 {
                    return Err(NoPiecesLeft);
                }
                afford(&cost::CITY)?;
                if self.vertex[vertex as usize] == settlement_code(p) {
                    Ok(())
                } else {
                    Err(IllegalLocation)
                }
            }
            (Phase::Main, Action::BuyDevCard) => {
                if self.dev_deck.is_empty() {
                    return Err(DeckEmpty);
                }
                afford(&cost::DEV_CARD)
            }
            (Phase::Main, Action::MaritimeTrade { give, get }) => {
                if give == get {
                    return Err(InvalidTrade);
                }
                if ps.hand.get(give) < ps.trade_ratio(give) {
                    return Err(CannotAfford);
                }
                if self.bank.get(get) == 0 {
                    Err(BankEmpty)
                } else {
                    Ok(())
                }
            }
            (Phase::Main, Action::OfferTrade { give, get }) => {
                if !self.rules.player_trading {
                    return Err(TradingDisabled);
                }
                if self.offers_this_turn >= self.rules.max_offers_per_turn {
                    return Err(OfferLimitReached);
                }
                if !valid_trade(&give, &get) {
                    return Err(InvalidTrade);
                }
                afford(&give)
            }
            (Phase::Main, Action::EndTurn) => Ok(()),
            (Phase::RoadBuilding { .. }, Action::BuildRoad { edge }) => {
                edge_ok(edge)?;
                if ps.roads_left == 0 {
                    return Err(NoPiecesLeft);
                }
                if self.can_road(p, edge) {
                    Ok(())
                } else {
                    Err(IllegalLocation)
                }
            }
            (Phase::TradeResponse, Action::AcceptTrade) => {
                let t = self.trade.expect("trade response without a trade");
                let q = self.acting_player();
                if t.is_round() {
                    Err(InvalidTrade)
                } else if self.players[q as usize].hand.contains(&t.get) {
                    Ok(())
                } else {
                    Err(CannotAfford)
                }
            }
            (Phase::TradeResponse, Action::RejectTrade) => Ok(()),
            (Phase::TradeResponse, Action::ProposeTrade { give, get }) => {
                if !self.rules.open_trading {
                    return Err(TradingDisabled);
                }
                if !valid_trade(&give, &get) {
                    return Err(InvalidTrade);
                }
                if self.players[self.acting_player() as usize]
                    .hand
                    .contains(&give)
                {
                    Ok(())
                } else {
                    Err(CannotAfford)
                }
            }
            (Phase::TradeConfirm, Action::ConfirmTrade { with }) => {
                let t = self.trade.expect("trade confirm without a trade");
                match (with < self.rules.num_players.get() && with != p)
                    .then(|| t.terms(with))
                    .flatten()
                {
                    Some((give, _)) if ps.hand.contains(&give) => Ok(()),
                    Some(_) => Err(CannotAfford),
                    None => Err(InvalidTrade),
                }
            }
            (Phase::TradeConfirm, Action::CancelTrade) => Ok(()),
            _ => Err(WrongPhase),
        }
    }

    fn check_dev_play(&self, action: Action) -> Result<(), RuleError> {
        use RuleError::*;
        if self.dev_played_this_turn {
            return Err(DevCardAlreadyPlayed);
        }
        let card = match action {
            Action::PlayKnight => DevCard::Knight,
            Action::PlayRoadBuilding => DevCard::RoadBuilding,
            Action::PlayYearOfPlenty { .. } => DevCard::YearOfPlenty,
            Action::PlayMonopoly { .. } => DevCard::Monopoly,
            _ => unreachable!("not a development card play"),
        };
        if self.phase == Phase::PreRoll
            && card != DevCard::Knight
            && self.rules.dev_timing == DevTiming::KnightBeforeRoll
        {
            return Err(DevCardNotAllowedNow);
        }
        if self.players[self.current as usize].dev[card.index()] == 0 {
            return Err(NoCard);
        }
        if let Action::PlayYearOfPlenty { a, b } = action
            && !self.bank_can_pay_pair(a, b)
        {
            return Err(BankEmpty);
        }
        if card == DevCard::RoadBuilding {
            let p = self.current;
            if self.players[p as usize].roads_left == 0 || !self.any_road_spot(p) {
                return Err(IllegalLocation);
            }
        }
        Ok(())
    }

    fn check_forced(&self, action: Action, forced: Forced) -> Result<(), RuleError> {
        let ok = match (action, forced) {
            // With balanced dice the roll has to be one the deck can still deal.
            (Action::RollDice, Forced::Dice(a, b)) => self.dice.can_roll(self.rules.dice, a, b),
            (Action::BuyDevCard, Forced::DevCard(c)) => self.dev_deck.contains(&c),
            (
                Action::MoveRobber {
                    victim: Some(v), ..
                },
                Forced::Steal(r),
            ) => self.players[v as usize].hand.get(r) > 0,
            _ => false,
        };
        if ok {
            Ok(())
        } else {
            Err(RuleError::InvalidForcedOutcome)
        }
    }

    fn apply_unchecked(&mut self, action: Action, forced: Option<Forced>) -> Outcome {
        let p = self.current;
        let pi = p as usize;
        match action {
            Action::BuildSettlement { vertex } => {
                let setup = self.phase == Phase::SetupSettlement;
                if !setup {
                    self.pay(p, &cost::SETTLEMENT);
                }
                self.place_settlement(p, vertex);
                if let Some(port) = self.board.vertex_port[vertex as usize] {
                    self.players[pi].ports |= port.bit();
                }
                if setup {
                    if self.setup_step >= self.rules.num_players.get() {
                        let gains = self.setup_gains(vertex);
                        self.bank.sub(&gains);
                        self.players[pi].hand.add(&gains);
                    }
                    self.phase = Phase::SetupRoad { vertex };
                } else {
                    self.recompute_roads_through(vertex);
                    self.update_longest_road();
                    self.check_win();
                }
                Outcome::None
            }
            Action::BuildRoad { edge } => {
                match self.phase {
                    Phase::SetupRoad { .. } => {
                        self.place_road(p, edge);
                        self.setup_step += 1;
                        if self.setup_step == 2 * self.rules.num_players.get() {
                            self.current = 0;
                            self.turn = 1;
                            self.phase = Phase::PreRoll;
                        } else {
                            self.current = self.setup_player(self.setup_step);
                            self.phase = Phase::SetupSettlement;
                        }
                    }
                    Phase::Main => {
                        self.pay(p, &cost::ROAD);
                        self.place_road(p, edge);
                        self.update_longest_road();
                        self.check_win();
                    }
                    Phase::RoadBuilding { remaining, resume } => {
                        self.place_road(p, edge);
                        self.update_longest_road();
                        self.check_win();
                        if !self.is_over() {
                            let remaining = remaining - 1;
                            self.phase = if remaining == 0
                                || self.players[pi].roads_left == 0
                                || !self.any_road_spot(p)
                            {
                                resume.phase()
                            } else {
                                Phase::RoadBuilding { remaining, resume }
                            };
                        }
                    }
                    _ => unreachable!("checked"),
                }
                Outcome::None
            }
            Action::BuildCity { vertex } => {
                self.pay(p, &cost::CITY);
                self.vertex[vertex as usize] = city_code(p);
                self.players[pi].settlements_left += 1;
                self.players[pi].cities_left -= 1;
                self.check_win();
                Outcome::None
            }
            Action::BuyDevCard => {
                self.pay(p, &cost::DEV_CARD);
                let card = match forced {
                    Some(Forced::DevCard(c)) => {
                        let i = self
                            .dev_deck
                            .iter()
                            .rposition(|&d| d == c)
                            .expect("checked");
                        self.dev_deck.remove(i)
                    }
                    _ => self.dev_deck.pop().expect("checked"),
                };
                self.players[pi].dev_new[card.index()] += 1;
                self.check_win();
                Outcome::DevCard { card: Some(card) }
            }
            Action::RollDice => {
                let (a, b) = match forced {
                    Some(Forced::Dice(a, b)) => {
                        // Checked: the roll comes out of the balanced deck as if dealt.
                        self.dice.take(self.rules.dice, a, b);
                        (a, b)
                    }
                    _ => self.dice.roll(
                        self.rules.dice,
                        &mut self.rng.derive(DICE, self.turn as u64),
                    ),
                };
                self.last_roll = Some((a, b));
                let sum = a + b;
                if sum == 7 {
                    let limit = self.rules.discard_limit as u32;
                    let mut any = false;
                    for q in 0..self.num_players() {
                        let total = self.players[q].hand.total();
                        self.discard_pending[q] = if total > limit { (total / 2) as u8 } else { 0 };
                        any |= self.discard_pending[q] > 0;
                    }
                    self.phase = if any {
                        Phase::Discard
                    } else {
                        Phase::MoveRobber {
                            resume: self.after_roll(),
                        }
                    };
                } else {
                    let gains = self.production_gains(sum);
                    for (q, g) in gains.iter().enumerate().take(self.num_players()) {
                        self.bank.sub(g);
                        self.players[q].hand.add(g);
                    }
                    self.resume(self.after_roll());
                }
                Outcome::Dice { a, b }
            }
            Action::PlayKnight => {
                let resume = self.resume_point();
                self.play_dev(DevCard::Knight);
                self.update_largest_army(p);
                self.phase = Phase::MoveRobber { resume };
                self.check_win();
                Outcome::None
            }
            Action::PlayRoadBuilding => {
                let resume = self.resume_point();
                self.play_dev(DevCard::RoadBuilding);
                self.phase = Phase::RoadBuilding {
                    remaining: self.players[pi].roads_left.min(2),
                    resume,
                };
                Outcome::None
            }
            Action::PlayYearOfPlenty { a, b } => {
                self.play_dev(DevCard::YearOfPlenty);
                for r in [a, b] {
                    self.bank.sub_n(r, 1);
                    self.players[pi].hand.add_n(r, 1);
                }
                Outcome::None
            }
            Action::PlayMonopoly { resource } => {
                self.play_dev(DevCard::Monopoly);
                let mut taken = [0u8; MAX_PLAYERS];
                let n = self.num_players();
                for (q, t) in taken.iter_mut().enumerate().take(n) {
                    if q != pi {
                        *t = self.players[q].hand.get(resource);
                        self.players[q].hand.sub_n(resource, *t);
                        self.players[pi].hand.add_n(resource, *t);
                    }
                }
                Outcome::Monopoly { taken }
            }
            Action::MoveRobber { hex, victim } => {
                let Phase::MoveRobber { resume } = self.phase else {
                    unreachable!("checked")
                };
                self.robber = hex;
                let outcome = match victim {
                    Some(v) => {
                        let r = match forced {
                            Some(Forced::Steal(r)) => r,
                            _ => self.random_card(v),
                        };
                        self.players[v as usize].hand.sub_n(r, 1);
                        self.steals += 1;
                        self.players[pi].hand.add_n(r, 1);
                        Outcome::Stole { resource: Some(r) }
                    }
                    None => Outcome::None,
                };
                // After the steal, so the round of proposals sees the hands it leaves.
                self.resume(resume);
                outcome
            }
            Action::Discard { resource } => {
                let q = self.acting_player() as usize;
                self.players[q].hand.sub_n(resource, 1);
                self.bank.add_n(resource, 1);
                self.discard_pending[q] -= 1;
                if self.discard_pending.iter().all(|&d| d == 0) {
                    self.phase = Phase::MoveRobber {
                        resume: self.after_roll(),
                    };
                }
                Outcome::None
            }
            Action::MaritimeTrade { give, get } => {
                let ratio = self.players[pi].trade_ratio(give);
                self.players[pi].hand.sub_n(give, ratio);
                self.bank.add_n(give, ratio);
                self.bank.sub_n(get, 1);
                self.players[pi].hand.add_n(get, 1);
                Outcome::None
            }
            Action::OfferTrade { give, get } => {
                self.offers_this_turn += 1;
                self.trade = Some(PendingTrade::offer(give, get));
                self.advance_trade();
                Outcome::None
            }
            Action::AcceptTrade => {
                let q = self.acting_player();
                if let Some(t) = self.trade.as_mut() {
                    t.accepted |= 1 << q;
                }
                self.advance_trade();
                Outcome::None
            }
            Action::RejectTrade => {
                self.advance_trade();
                Outcome::None
            }
            Action::ProposeTrade { give, get } => {
                let q = self.acting_player();
                if let Some(t) = self.trade.as_mut() {
                    t.proposals[q as usize] = (give, get);
                }
                self.advance_trade();
                Outcome::None
            }
            Action::ConfirmTrade { with } => {
                let t = self.trade.take().expect("checked");
                let (give, get) = t.terms(with).expect("checked");
                let w = with as usize;
                self.players[pi].hand.sub(&give);
                self.players[w].hand.add(&give);
                self.players[w].hand.sub(&get);
                self.players[pi].hand.add(&get);
                self.phase = Phase::Main;
                Outcome::None
            }
            Action::CancelTrade => {
                self.trade = None;
                self.phase = Phase::Main;
                Outcome::None
            }
            Action::EndTurn => {
                let ps = &mut self.players[pi];
                for i in 0..NUM_DEV_TYPES {
                    ps.dev[i] += ps.dev_new[i];
                    ps.dev_new[i] = 0;
                }
                self.dev_played_this_turn = false;
                self.offers_this_turn = 0;
                self.current = (p + 1) % self.rules.num_players.get();
                self.turn += 1;
                self.phase = Phase::PreRoll;
                // Points gained off-turn (e.g. Longest Road changing hands) win now.
                self.check_win();
                Outcome::None
            }
        }
    }

    fn resume_point(&self) -> Resume {
        if self.phase == Phase::PreRoll {
            Resume::PreRoll
        } else {
            Resume::Main
        }
    }

    fn play_dev(&mut self, card: DevCard) {
        let ps = &mut self.players[self.current as usize];
        ps.dev[card.index()] -= 1;
        ps.dev_played[card.index()] += 1;
        self.dev_played_this_turn = true;
    }

    fn pay(&mut self, p: PlayerId, c: &Hand) {
        self.players[p as usize].hand.sub(c);
        self.bank.add(c);
    }

    fn random_card(&mut self, victim: PlayerId) -> Resource {
        let hand = self.players[victim as usize].hand;
        let key = ((self.turn as u64) << 32) | self.steals as u64;
        let mut k = self.rng.derive(STEAL, key).below(hand.total());
        for r in Resource::ALL {
            let c = hand.get(r) as u32;
            if k < c {
                return r;
            }
            k -= c;
        }
        unreachable!("victim has cards")
    }

    /// Resources a second initial settlement on `v` collects (one per adjacent resource hex).
    pub fn setup_gains(&self, v: VertexId) -> Hand {
        let mut gains = Hand::EMPTY;
        for &h in self.board.topology.vertex_hexes[v as usize].as_slice() {
            if let Some(r) = self.board.tiles[h as usize].resource
                && self.bank.get(r) > gains.get(r)
            {
                gains.add_n(r, 1);
            }
        }
        gains
    }

    /// What each player collects on a roll of `sum` (not 7). When the bank can't pay everyone a
    /// resource, nobody gets it, unless only one player is owed it, who then takes what's left.
    pub fn production_gains(&self, sum: u8) -> [Hand; MAX_PLAYERS] {
        let mut gains = [Hand::EMPTY; MAX_PLAYERS];
        let mut owed = [0u32; NUM_RESOURCES];
        let mut owed_to = [0u8; NUM_RESOURCES];
        let topo = &self.board.topology;
        for &h in &self.board.hexes_by_number[sum as usize] {
            if h == self.robber {
                continue;
            }
            let Some(r) = self.board.tiles[h as usize].resource else {
                continue;
            };
            for &v in &topo.hex_vertices[h as usize] {
                if let Some(b) = self.building(v) {
                    let n = if b.city { 2 } else { 1 };
                    gains[b.owner as usize].add_n(r, n);
                    owed[r.index()] += n as u32;
                    owed_to[r.index()] |= 1 << b.owner;
                }
            }
        }
        for r in Resource::ALL {
            let i = r.index();
            let bank = self.bank.get(r) as u32;
            if owed[i] <= bank {
                continue;
            }
            if owed_to[i].count_ones() == 1 && self.rules.shortage == ShortageRule::Official {
                let q = owed_to[i].trailing_zeros() as usize;
                gains[q].0[i] = bank as u8;
            } else {
                for g in gains.iter_mut() {
                    g.0[i] = 0;
                }
            }
        }
        gains
    }

    fn update_largest_army(&mut self, p: PlayerId) {
        let k = self.players[p as usize].knights_played();
        if k < LARGEST_ARMY_MIN {
            return;
        }
        match self.largest_army {
            Some(h) if h == p => {}
            Some(h) if k <= self.players[h as usize].knights_played() => {}
            _ => self.largest_army = Some(p),
        }
    }

    fn advance_trade(&mut self) {
        let n = self.rules.num_players.get();
        let Some(mut t) = self.trade else { return };
        loop {
            t.responder_offset += 1;
            if t.responder_offset >= n {
                // Something to take: an acceptance, or a proposal the player on turn can pay.
                let hand = self.players[self.current as usize].hand;
                let takeable = (0..n)
                    .filter(|&q| q != self.current)
                    .any(|q| t.terms(q).is_some_and(|(give, _)| hand.contains(&give)));
                if takeable {
                    self.trade = Some(t);
                    self.phase = Phase::TradeConfirm;
                } else {
                    self.trade = None;
                    self.phase = Phase::Main;
                }
                return;
            }
            let q = (self.current + t.responder_offset) % n;
            let hand = &self.players[q as usize].hand;
            // With open trading anyone holding a card can answer (a proposal needs one);
            // without, only those who can pay.
            let answers = if self.rules.open_trading {
                !hand.is_empty()
            } else {
                hand.contains(&t.get)
            };
            if answers {
                self.trade = Some(t);
                self.phase = Phase::TradeResponse;
                return;
            }
        }
    }

    /// Where play goes once a roll is settled: the round of proposals with open trading.
    fn after_roll(&self) -> Resume {
        if self.rules.player_trading && self.rules.open_trading && self.rules.trade_proposals {
            Resume::AfterRoll
        } else {
            Resume::Main
        }
    }

    /// Resumes play at `r`. `AfterRoll` opens the round in which each other player, in seat
    /// order, may propose a trade to the player on turn; it's skipped when the player on turn
    /// holds no cards, since every proposal asks for one.
    fn resume(&mut self, r: Resume) {
        if r == Resume::AfterRoll && !self.players[self.current as usize].hand.is_empty() {
            self.trade = Some(PendingTrade::offer(Hand::EMPTY, Hand::EMPTY));
            self.advance_trade();
        } else {
            self.phase = r.phase();
        }
    }

    /// Ends the game if the current player has reached the target. Players only win on their
    /// own turn.
    pub(crate) fn check_win(&mut self) {
        if self.phase == Phase::GameOver || self.turn == 0 {
            return;
        }
        if self.total_vp(self.current) >= self.rules.vp_to_win {
            self.winner = Some(self.current);
            self.phase = Phase::GameOver;
            self.trade = None;
        }
    }
}

/// A trade's shape: at least one card each way, no resource on both sides, and no more of a
/// resource than the bank holds in all (rulebook: no gifts, no like-for-like trades).
fn valid_trade(give: &Hand, get: &Hand) -> bool {
    !give.is_empty()
        && !get.is_empty()
        && give.disjoint_types(get)
        && give.0.iter().chain(&get.0).all(|&c| c <= BANK_PER_RESOURCE)
}
