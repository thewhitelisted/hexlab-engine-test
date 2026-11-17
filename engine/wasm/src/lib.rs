//! The engine in the browser (docs/SCOPE.md, M3: "the Rust engine compiled to WASM for instant
//! in-browser calculations"), built for `wasm32-unknown-unknown` with no bindings generator. Each
//! export takes a JSON string the page wrote into the module's memory and returns one there;
//! `web/src/engine.ts` is the page's side.
//!
//! - `check_position`: whether a position typed into the endgame calculator's editor can be built,
//!   and if not, why: the same `engine_core::entered::build` the server runs.

use std::sync::Arc;

use engine_core::entered::{self, Entered};
use engine_core::{Board, BoardLayout};
use serde_json::{Value, json};

/// Whether `{"layout": ..., "position": ...}` is a position that can happen:
/// `{"ok": true}`, or `{"ok": false, "problem": "..."}` with the first thing wrong.
pub fn check(input: &str) -> String {
    let answer = |problem: Option<String>| match problem {
        None => json!({ "ok": true }),
        Some(p) => json!({ "ok": false, "problem": p }),
    };
    let problem = (|| {
        let v: Value = serde_json::from_str(input).map_err(|e| format!("not a position: {e}"))?;
        let layout: BoardLayout =
            serde_json::from_value(v["layout"].clone()).map_err(|e| format!("not a board: {e}"))?;
        let position: Entered = serde_json::from_value(v["position"].clone())
            .map_err(|e| format!("not a position: {e}"))?;
        let board = Board::from_layout(&layout).map_err(|e| e.to_string())?;
        entered::build(&position, Arc::new(board), 0).map(|_| ())
    })()
    .err();
    answer(problem).to_string()
}

/// Room for `len` bytes of input, for the page to write into.
#[unsafe(no_mangle)]
pub extern "C" fn alloc(len: usize) -> *mut u8 {
    let mut buf = Vec::<u8>::with_capacity(len);
    let ptr = buf.as_mut_ptr();
    std::mem::forget(buf);
    ptr
}

/// Gives back what `alloc` lent, or an answer once the page has read it.
///
/// # Safety
/// `ptr` and `len` must come from `alloc`, or from an answer, and be given back once.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dealloc(ptr: *mut u8, len: usize) {
    // SAFETY: the caller hands back a buffer of `len` bytes this module allocated.
    drop(unsafe { Vec::from_raw_parts(ptr, 0, len) });
}

/// `check` on the `len` bytes at `ptr`, which it takes back. The answer's pointer and length come
/// back packed into one number: pointer in the high 32 bits, length in the low.
///
/// # Safety
/// `ptr` and `len` must be a buffer from `alloc` that the page filled with UTF-8.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn check_position(ptr: *mut u8, len: usize) -> u64 {
    // SAFETY: the caller passes a filled buffer from `alloc`, which this takes over.
    let input = unsafe { Vec::from_raw_parts(ptr, len, len) };
    let answer = check(&String::from_utf8_lossy(&input))
        .into_bytes()
        .into_boxed_slice();
    let (out, out_len) = (answer.as_ptr() as u64, answer.len() as u64);
    std::mem::forget(answer);
    (out << 32) | out_len
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine_core::record::GameRecord;
    use engine_core::sim::{Limits, RandomPolicy, play_out};
    use engine_core::{GameConfig, GameState, Rng};

    #[test]
    fn a_position_is_checked_as_the_server_builds_it() {
        let config = GameConfig::official(4);
        let mut state = GameState::new(&config, 3);
        let layout = GameRecord::start(&config, 3, &state).board;
        let limits = Limits {
            max_turns: 40,
            ..Limits::default()
        };
        play_out(
            &mut state,
            RandomPolicy::Builder,
            &limits,
            &mut Rng::seed(3),
            &mut Vec::new(),
        );
        let position = Entered::seen(&state, 2);
        let good = json!({"layout": layout, "position": position}).to_string();
        assert_eq!(check(&good), r#"{"ok":true}"#);

        // One more card in the bank of a resource the others hold some of.
        let mut broken = position.clone();
        let r = (0..5)
            .find(|&r| u32::from(broken.bank[r]) + u32::from(broken.hand[r]) < 19)
            .expect("the others hold some resource");
        broken.bank[r] += 1;
        let answer: Value = serde_json::from_str(&check(
            &json!({"layout": layout, "position": broken}).to_string(),
        ))
        .unwrap();
        assert_eq!(answer["ok"], false);
        assert!(
            answer["problem"]
                .as_str()
                .unwrap()
                .contains("the bank and your hand leave")
        );
        let answer: Value = serde_json::from_str(&check("{}")).unwrap();
        assert!(
            answer["problem"]
                .as_str()
                .unwrap()
                .starts_with("not a board")
        );
    }

    /// Audit 2.4 to 2.6 and 7.5, from the browser's side: whatever the page sends, `check`
    /// answers with a problem; it never panics (a panic in the module leaves the page without
    /// its engine). The exported `check_position` packs a 32-bit pointer, so it runs only in the
    /// browser's build and isn't called here.
    #[test]
    fn hostile_input_gets_an_answer_not_a_panic() {
        let config = GameConfig::official(4);
        let mut state = GameState::new(&config, 3);
        let layout = GameRecord::start(&config, 3, &state).board;
        let limits = Limits {
            max_turns: 40,
            ..Limits::default()
        };
        play_out(
            &mut state,
            RandomPolicy::Builder,
            &limits,
            &mut Rng::seed(3),
            &mut Vec::new(),
        );
        let position = json!(Entered::seen(&state, 2));
        let layout = json!(layout);
        let hex = |q: i64, r: i64| json!({"q": q, "r": r, "resource": null, "number": null});
        let mut inputs: Vec<String> = vec![
            String::new(),
            "null".into(),
            "[]".into(),
            "{".into(),
            "\u{0}\u{ffff}".into(),
            json!({"layout": layout}).to_string(),
            json!({"position": position}).to_string(),
            json!({"layout": 5, "position": position}).to_string(),
            json!({"layout": layout, "position": "x"}).to_string(),
        ];
        // Boards: far-out hexes, none, the same one twice, far too many.
        let many: Vec<Value> = (0..400).map(|i| hex(i % 20 - 10, i / 20 - 10)).collect();
        for hexes in [
            vec![hex(-128, -128)],
            vec![hex(127, 127), hex(-128, 127)],
            vec![],
            vec![hex(0, 0), hex(0, 0)],
            many,
        ] {
            let board = json!({"hexes": hexes, "ports": []});
            inputs.push(json!({"layout": board, "position": position}).to_string());
        }
        // Positions: one field at a time set to something impossible.
        for (field, value) in [
            ("seat", json!(200)),
            ("current", json!(4)),
            ("robber", json!(255)),
            ("turn", json!(0)),
            ("longest_road", json!(4)),
            ("largest_army", json!(255)),
            ("hand", json!([255, 255, 255, 255, 255])),
            ("bank", json!([255, 0, 0, 0, 0])),
            ("dev", json!([255, 255, 255, 255, 255])),
            ("players", json!([])),
            (
                "buildings",
                json!([{"vertex": 65535, "owner": 0, "city": true}]),
            ),
            ("roads", json!([{"edge": 65535, "owner": 9}])),
        ] {
            let mut broken = position.clone();
            broken[field] = value;
            inputs.push(json!({"layout": layout, "position": broken}).to_string());
        }
        let mut broken = position.clone();
        broken["players"][1]["cards"] = json!(4294967295u32);
        broken["players"][2]["dev_cards"] = json!(4294967295u32);
        inputs.push(json!({"layout": layout, "position": broken}).to_string());

        for input in &inputs {
            let answer: Value = serde_json::from_str(&check(input)).expect("an answer in JSON");
            assert_eq!(answer["ok"], false, "{input}");
            assert!(
                answer["problem"].as_str().is_some_and(|p| !p.is_empty()),
                "{input}"
            );
        }
    }
}
