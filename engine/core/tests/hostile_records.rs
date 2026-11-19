//! Game records come from outside: a stored game, a file, a request. Each is read into checked
//! types or refused with an error, and nothing that is read panics when it is replayed.
//!
//! The input space, and where each class is tested:
//! - player count: 0, 1, 2-4, 5, 6, 7, 255, 256, -1, a string, null, missing
//!   (`a_record_with_a_player_count_the_game_doesnt_have_is_refused`);
//! - board: one hex, two hexes, a ring with too few spots for the initial placement, the standard
//!   board (`a_record_on_a_board_with_no_game_is_refused`; the boards themselves are tested in
//!   `board.rs`);
//! - any number anywhere in a record set to a boundary value (0, 1, 7, 255, 256, 2^32 - 1,
//!   2^64 - 1, -1 and others), steps dropped, repeated or swapped
//!   (`every_record_that_parses_replays_without_panicking`);
//! - records written by the code before this change (`fixtures/records-956495d.jsonl`): read,
//!   replayed and written back byte for byte
//!   (`records_written_before_read_replay_and_write_back_byte_for_byte`).

use std::hash::{Hash, Hasher};
use std::panic::{AssertUnwindSafe, catch_unwind};

use engine_core::board::HexSpec;
use engine_core::record::{GameRecord, ReplayError};
use engine_core::*;
use serde_json::{Value, json};

const FIXTURES: &str = include_str!("fixtures/records-956495d.jsonl");
const FIXTURE_HASHES: &str = include_str!("fixtures/records-956495d-config-hash.txt");

/// A record of a game just started: official rules, `players` players, the standard board.
fn fresh(players: u64) -> Value {
    let mut v: Value = serde_json::from_str(FIXTURES.lines().next().unwrap()).unwrap();
    v["steps"] = json!([]);
    v["winner"] = Value::Null;
    v["config"]["rules"]["num_players"] = players.into();
    v
}

/// Reads `v` as a record and replays it, catching a panic: `Ok(Ok(_))` it replayed, `Ok(Err)` it
/// was refused (when read or when replayed), `Err` it panicked. The callback checks that each
/// step it is shown is one the rules apply, with the outcome recorded (review of PR #64).
fn read_and_replay(v: &Value) -> Result<Result<(), String>, String> {
    let Ok(record) = serde_json::from_value::<GameRecord>(v.clone()) else {
        return Ok(Err("not read".into()));
    };
    catch_unwind(AssertUnwindSafe(|| {
        record
            .replay_steps(|before, step| {
                let mut s = before.clone();
                let applied = s.apply_forced(step.action, step.outcome.as_forced());
                assert_eq!(applied, Ok(step.outcome), "shown a step that doesn't apply");
            })
            .map(|_| ())
            .map_err(|e: ReplayError| e.to_string())
    }))
    .map_err(|p| {
        p.downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| p.downcast_ref::<String>().cloned())
            .unwrap_or_default()
    })
}

#[test]
fn a_record_with_a_player_count_the_game_doesnt_have_is_refused() {
    for n in [0u64, 1, 5, 6, 7, 255, 256, u64::MAX] {
        let v = fresh(n);
        assert!(
            serde_json::from_value::<GameRecord>(v).is_err(),
            "a record of {n} players was read"
        );
    }
    for bad in [json!(-1), json!("4"), json!(null), json!(4.0), json!([4])] {
        let mut v = fresh(4);
        v["config"]["rules"]["num_players"] = bad.clone();
        assert!(
            serde_json::from_value::<GameRecord>(v).is_err(),
            "a record of {bad} players was read"
        );
    }
    let mut v = fresh(4);
    v["config"]["rules"]
        .as_object_mut()
        .unwrap()
        .remove("num_players");
    assert!(serde_json::from_value::<GameRecord>(v).is_err());
    for n in 2..=4 {
        assert_eq!(read_and_replay(&fresh(n)), Ok(Ok(())), "{n} players");
    }
}

fn hex(q: i8, r: i8, number: Option<u8>) -> HexSpec {
    HexSpec {
        q,
        r,
        resource: number.map(|_| Resource::Wheat),
        number,
    }
}

#[test]
fn a_record_on_a_board_with_no_game_is_refused() {
    let one = vec![hex(0, 0, None)];
    let two = vec![hex(0, 0, None), hex(1, 0, Some(6))];
    let cases: Vec<(&str, Vec<HexSpec>, u64)> = vec![
        ("one desert, 2 players", one.clone(), 2),
        ("one desert, 4 players", one, 4),
        (
            "one hex with a number, 2 players",
            vec![hex(0, 0, Some(8))],
            2,
        ),
        ("two hexes, 3 players", two.clone(), 3),
        ("two hexes, 4 players", two, 4),
    ];
    for (what, hexes, players) in cases {
        let mut v = fresh(players);
        v["board"] = json!({ "hexes": hexes, "ports": [] });
        match read_and_replay(&v) {
            Ok(Err(_)) => {}
            other => panic!("{what}: {other:?}"),
        }
    }
}

/// Every number in `v`, as a path of keys and indexes.
fn number_paths(v: &Value, at: &mut Vec<Value>, out: &mut Vec<Vec<Value>>) {
    match v {
        Value::Number(_) => out.push(at.clone()),
        Value::Array(xs) => {
            for (i, x) in xs.iter().enumerate() {
                at.push(i.into());
                number_paths(x, at, out);
                at.pop();
            }
        }
        Value::Object(m) => {
            for (k, x) in m {
                at.push(k.clone().into());
                number_paths(x, at, out);
                at.pop();
            }
        }
        _ => {}
    }
}

fn at_path<'a>(v: &'a mut Value, path: &[Value]) -> &'a mut Value {
    path.iter().fold(v, |v, key| match key {
        Value::String(k) => &mut v[k.as_str()],
        Value::Number(i) => &mut v[i.as_u64().unwrap() as usize],
        _ => unreachable!(),
    })
}

/// Records from the fixtures with one thing changed at random: a number set to a boundary
/// value, a step dropped, repeated or swapped with another. Whatever is read replays to a state
/// or an error, never a panic.
#[test]
fn every_record_that_parses_replays_without_panicking() {
    let boundary: Vec<Value> = vec![
        json!(0),
        json!(1),
        json!(2),
        json!(3),
        json!(4),
        json!(5),
        json!(6),
        json!(7),
        json!(8),
        json!(12),
        json!(13),
        json!(18),
        json!(19),
        json!(20),
        json!(53),
        json!(54),
        json!(71),
        json!(72),
        json!(127),
        json!(128),
        json!(255),
        json!(256),
        json!(65535),
        json!(65536),
        json!(u32::MAX),
        json!(u64::from(u32::MAX) + 1),
        json!(u64::MAX),
        json!(-1),
        json!(i64::MIN),
    ];
    let records: Vec<Value> = FIXTURES
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let mut rng = Rng::seed(23);
    let (mut read, mut replayed, mut panics) = (0, 0, Vec::new());
    for round in 0..4000 {
        let mut v = records[round % records.len()].clone();
        let steps = v["steps"].as_array().map_or(0, Vec::len) as u32;
        match rng.below(4) {
            0 | 1 => {
                let mut paths = Vec::new();
                number_paths(&v, &mut Vec::new(), &mut paths);
                let path = &paths[rng.below(paths.len() as u32) as usize];
                *at_path(&mut v, path) =
                    boundary[rng.below(boundary.len() as u32) as usize].clone();
            }
            2 if steps > 1 => {
                let i = rng.below(steps) as usize;
                let s = v["steps"].as_array_mut().unwrap();
                if rng.below(2) == 0 {
                    s.remove(i);
                } else {
                    let x = s[i].clone();
                    s.insert(i, x);
                }
            }
            _ if steps > 1 => {
                let (i, j) = (rng.below(steps) as usize, rng.below(steps) as usize);
                v["steps"].as_array_mut().unwrap().swap(i, j);
            }
            _ => {}
        }
        match read_and_replay(&v) {
            Ok(Ok(())) => {
                read += 1;
                replayed += 1;
            }
            Ok(Err(e)) => read += usize::from(e != "not read"),
            Err(p) => panics.push(p),
        }
    }
    assert!(
        panics.is_empty(),
        "{} panics, first: {}",
        panics.len(),
        panics[0]
    );
    // The changes reach the replay, not only the reader.
    assert!(
        read > 3000 && replayed > 250,
        "read {read}, replayed {replayed}"
    );
}

/// The bytes a value feeds a hasher, which is what keys a cache or a table.
#[derive(Default)]
struct Fed(Vec<u8>);

impl Hasher for Fed {
    fn finish(&self) -> u64 {
        0
    }
    fn write(&mut self, bytes: &[u8]) {
        self.0.extend_from_slice(bytes);
    }
}

/// Records written by the code before the player count had its own type (commit 956495d), for
/// every preset and player count the presets take: each is read, replays to its recorded final
/// position, is written back to the same bytes, and its configuration feeds a hasher the same
/// bytes as it did then.
#[test]
fn records_written_before_read_replay_and_write_back_byte_for_byte() {
    let hashes: Vec<&str> = FIXTURE_HASHES.lines().collect();
    assert_eq!(FIXTURES.lines().count(), hashes.len());
    for (line, want) in FIXTURES.lines().zip(hashes) {
        let record: GameRecord = serde_json::from_str(line).unwrap();
        record.replay().unwrap();
        assert_eq!(serde_json::to_string(&record).unwrap(), line);
        let mut fed = Fed::default();
        record.config.hash(&mut fed);
        let hex: String = fed.0.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(hex, want);
    }
}
