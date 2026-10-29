//! The hub without a network: the 2v2 match cycle (queue, AI fill, the ship pick, the match,
//! results), the free-for-all room and the test range.

use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};
use blocktread_server::hub::Hub;
use blocktread_server::{HubConfig, NetEvent, Outgoing};
use blocktread_shared::presets::preset;
use blocktread_shared::protocol::*;

fn hub(queue_wait: f32, match_secs: f32) -> Hub {
    Hub::new(HubConfig { queue_wait, match_secs, win_points: 100 })
}

fn connect(h: &mut Hub, pid: u16, name: &str) -> UnboundedReceiver<Outgoing> {
    let (out, rx) = unbounded_channel();
    h.handle(NetEvent::Connect { pid, name: name.into(), watch: false, out });
    rx
}

fn msg(h: &mut Hub, pid: u16, msg: ClientMsg) {
    h.handle(NetEvent::Msg { pid, msg });
}

/// Every reliable message waiting for a connection.
fn drain(rx: &mut UnboundedReceiver<Outgoing>) -> Vec<ServerMsg> {
    let mut v = Vec::new();
    while let Ok(o) = rx.try_recv() {
        if let Outgoing::Reliable(b) = o
            && let Some(m) = decode::<ServerMsg>(&b)
        {
            v.push(m);
        }
    }
    v
}

fn ticks(h: &mut Hub, n: u32) {
    for _ in 0..n {
        h.tick();
    }
}

fn found(msgs: &[ServerMsg]) -> Option<(u8, Vec<SlotInfo>)> {
    msgs.iter().find_map(|m| match m {
        ServerMsg::MatchFound { you, slots, .. } => Some((*you, slots.clone())),
        _ => None,
    })
}

/// The newest scoreboard in `msgs`: (round, seconds left, rows).
fn board(msgs: &[ServerMsg]) -> Option<(Round, f32, Vec<ScoreRow>)> {
    msgs.iter().rev().find_map(|m| match m {
        ServerMsg::Scores { round, left, rows, .. } => Some((*round, *left, rows.clone())),
        _ => None,
    })
}

fn announced<'a>(msgs: &'a [ServerMsg], title: &str) -> Option<&'a str> {
    msgs.iter().find_map(|m| match m {
        ServerMsg::Announce { title: t, sub } if t == title => Some(sub.as_str()),
        _ => None,
    })
}

/// Connect and drive into the free-for-all room the way the client does: queue, send the
/// vehicle at once, lock in.
fn enter_room(h: &mut Hub, pid: u16, name: &str) -> UnboundedReceiver<Outgoing> {
    let mut rx = connect(h, pid, name);
    msg(h, pid, ClientMsg::Queue { mode: Mode::Ffa, tier: None });
    ticks(h, 15);
    let (_, slots) = found(&drain(&mut rx)).expect("straight to the vehicle");
    assert_eq!(slots.len(), 1, "no seats to fill, no AI: {slots:?}");
    msg(h, pid, ClientMsg::Pick { design: preset("bison") });
    msg(h, pid, ClientMsg::Ready);
    ticks(h, 2);
    rx
}

#[test]
fn two_players_get_a_2v2_filled_with_ai_and_play_once_both_lock_in() {
    let mut h = hub(0.0, 60.0);
    let mut a = connect(&mut h, 1, "甲");
    let mut b = connect(&mut h, 2, "乙");
    msg(&mut h, 1, ClientMsg::Queue { mode: Mode::Teams, tier: None });
    msg(&mut h, 2, ClientMsg::Queue { mode: Mode::Teams, tier: None });
    ticks(&mut h, 15);
    let (ya, slots) = found(&drain(&mut a)).expect("a match for the first player");
    let (yb, _) = found(&drain(&mut b)).expect("the same match for the second");
    assert_eq!(slots.len(), 4);
    let ai: Vec<&SlotInfo> = slots.iter().filter(|s| s.name != "甲" && s.name != "乙").collect();
    assert_eq!(ai.len(), 2, "AI takes the two empty seats");
    assert!(ai.iter().all(|s| !s.name.contains("AI")), "and they go by names like a player's: {ai:?}");
    // the two players are on opposite teams, two seats a side
    assert_ne!(slots[ya as usize].team, slots[yb as usize].team);
    for t in [1, 2] {
        assert_eq!(slots.iter().filter(|s| s.team == t).count(), 2);
    }

    // a's pick shows up for b, and the match waits for both to lock in
    msg(&mut h, 1, ClientMsg::Pick { design: preset("bison") });
    msg(&mut h, 1, ClientMsg::Ready);
    ticks(&mut h, 1);
    let seen = drain(&mut b);
    assert!(seen.iter().any(|m| matches!(m, ServerMsg::SlotPick { slot, design: Some(d), ready: true } if *slot == ya && !d.blocks.is_empty())), "b sees a's ship: {seen:?}");
    assert!(!drain(&mut a).iter().any(|m| matches!(m, ServerMsg::Welcome { .. })), "not started while b picks");
    msg(&mut h, 2, ClientMsg::Pick { design: preset("rhino") });
    msg(&mut h, 2, ClientMsg::Ready);
    ticks(&mut h, 2);
    for rx in [&mut a, &mut b] {
        let m = drain(rx);
        assert!(m.iter().any(|m| matches!(m, ServerMsg::Welcome { mode: Mode::Teams, .. })), "the match starts: {m:?}");
        assert_eq!(m.iter().filter(|m| matches!(m, ServerMsg::ShipSpawn { .. })).count(), 4, "four ships take off");
    }
}

#[test]
fn a_pick_over_the_budget_is_refused() {
    let mut h = hub(0.0, 60.0);
    let mut a = connect(&mut h, 1, "甲");
    msg(&mut h, 1, ClientMsg::Queue { mode: Mode::Teams, tier: Some("light".into()) });
    ticks(&mut h, 15);
    assert_eq!(found(&drain(&mut a)).unwrap().1.len(), 4);
    msg(&mut h, 1, ClientMsg::Pick { design: preset("rhino") });
    let m = drain(&mut a);
    assert!(m.iter().any(|m| matches!(m, ServerMsg::Notice { .. })), "told why: {m:?}");
    assert!(!m.iter().any(|m| matches!(m, ServerMsg::SlotPick { slot: 0, .. })));
}

#[test]
fn a_player_who_leaves_during_the_pick_is_replaced_by_ai() {
    let mut h = hub(0.0, 60.0);
    let mut a = connect(&mut h, 1, "甲");
    let _b = connect(&mut h, 2, "乙");
    msg(&mut h, 1, ClientMsg::Queue { mode: Mode::Teams, tier: None });
    msg(&mut h, 2, ClientMsg::Queue { mode: Mode::Teams, tier: None });
    ticks(&mut h, 15);
    let (_, slots) = found(&drain(&mut a)).unwrap();
    let k = slots.iter().position(|s| s.name == "乙").unwrap();
    h.handle(NetEvent::Leave { pid: 2 });
    ticks(&mut h, 1);
    let m = drain(&mut a);
    assert!(m.iter().any(|m| matches!(m, ServerMsg::SlotPick { slot, design: Some(_), ready: true } if *slot as usize == k)), "{m:?}");
    // with everyone else set, a's lock-in starts the match
    msg(&mut h, 1, ClientMsg::Pick { design: preset("bison") });
    msg(&mut h, 1, ClientMsg::Ready);
    ticks(&mut h, 2);
    assert!(drain(&mut a).iter().any(|m| matches!(m, ServerMsg::Welcome { .. })));
}

#[test]
fn the_match_ends_on_time_with_results_and_everyone_can_queue_again() {
    let mut h = hub(0.0, 2.0);
    let mut a = connect(&mut h, 1, "甲");
    msg(&mut h, 1, ClientMsg::Queue { mode: Mode::Teams, tier: None });
    ticks(&mut h, 15);
    msg(&mut h, 1, ClientMsg::Pick { design: preset("bison") });
    msg(&mut h, 1, ClientMsg::Ready);
    ticks(&mut h, 2);
    assert!(drain(&mut a).iter().any(|m| matches!(m, ServerMsg::Welcome { .. })));
    ticks(&mut h, 2 * 60 + 5);
    let m = drain(&mut a);
    let over = m.iter().find_map(|m| match m {
        ServerMsg::MatchOver { winner, rows } => Some((winner.clone(), rows.len())),
        _ => None,
    });
    let (winner, rows) = over.expect("results when the clock runs out");
    assert!(!winner.is_empty());
    assert_eq!(rows, 4, "every seat on the final scoreboard");
    // back in the lobby: the next queue makes a new match
    msg(&mut h, 1, ClientMsg::Queue { mode: Mode::Teams, tier: None });
    ticks(&mut h, 15);
    assert!(found(&drain(&mut a)).is_some_and(|(_, s)| s.len() == 4));
}

#[test]
fn a_vehicle_that_cant_drive_is_refused() {
    let mut h = hub(0.0, 60.0);
    let mut a = connect(&mut h, 1, "甲");
    msg(&mut h, 1, ClientMsg::Queue { mode: Mode::Teams, tier: None });
    ticks(&mut h, 15);
    found(&drain(&mut a)).unwrap();
    // just a cab: no wheels, no engine
    msg(&mut h, 1, ClientMsg::Pick { design: preset("blank") });
    let m = drain(&mut a);
    assert!(m.iter().any(|m| matches!(m, ServerMsg::Notice { text } if text.contains("出车检查"))), "told why: {m:?}");
    assert!(!m.iter().any(|m| matches!(m, ServerMsg::SlotPick { slot: 0, .. })));
}

#[test]
fn the_test_range_is_yours_alone_until_you_leave() {
    let mut h = hub(15.0, 2.0);
    let mut a = connect(&mut h, 1, "甲");
    let mut b = connect(&mut h, 2, "乙");
    // no wait for others, no AI, and no budget even if one is asked for
    msg(&mut h, 1, ClientMsg::Queue { mode: Mode::Solo, tier: Some("light".into()) });
    msg(&mut h, 2, ClientMsg::Queue { mode: Mode::Solo, tier: None });
    ticks(&mut h, 15);
    for rx in [&mut a, &mut b] {
        let m = drain(rx);
        let (_, slots) = found(&m).expect("a range at once");
        assert_eq!(slots.len(), 1, "one seat each, no AI: {slots:?}");
        assert!(m.iter().any(|m| matches!(m, ServerMsg::MatchFound { tier: None, .. })));
    }
    msg(&mut h, 1, ClientMsg::Pick { design: preset("rhino") });
    msg(&mut h, 1, ClientMsg::Ready);
    ticks(&mut h, 2);
    let m = drain(&mut a);
    assert!(m.iter().any(|m| matches!(m, ServerMsg::Welcome { mode: Mode::Solo, seed, .. } if *seed == blocktread_server::game::SOLO_SEED)), "the same map every time: {m:?}");
    assert_eq!(m.iter().filter(|m| matches!(m, ServerMsg::ShipSpawn { .. })).count(), 1, "only your own vehicle");
    // well past the match length: nobody else turns up, and it doesn't end
    ticks(&mut h, 20 * 60);
    let m = drain(&mut a);
    assert!(!m.iter().any(|m| matches!(m, ServerMsg::ShipSpawn { .. } | ServerMsg::MatchOver { .. })), "still alone, still going");
    assert_eq!(h.census().1, 2, "each player's range is their own");
    // leaving takes the range with you (no AI takes over the seat)
    h.handle(NetEvent::Leave { pid: 1 });
    ticks(&mut h, 2);
    assert_eq!(h.census().1, 1);
    h.handle(NetEvent::Leave { pid: 2 });
    assert_eq!(h.census().1, 0);
}

#[test]
fn free_for_all_is_one_room_everyone_drives_into_without_ai() {
    let mut h = hub(15.0, 480.0);
    let mut a = connect(&mut h, 1, "甲");
    // no budget, even if one is asked for
    msg(&mut h, 1, ClientMsg::Queue { mode: Mode::Ffa, tier: Some("light".into()) });
    ticks(&mut h, 15);
    let m = drain(&mut a);
    assert!(m.iter().any(|m| matches!(m, ServerMsg::MatchFound { mode: Mode::Ffa, tier: None, slots, .. } if slots.len() == 1)), "{m:?}");
    msg(&mut h, 1, ClientMsg::Pick { design: preset("rhino") });
    msg(&mut h, 1, ClientMsg::Ready);
    ticks(&mut h, 2);
    let m = drain(&mut a);
    assert!(m.iter().any(|m| matches!(m, ServerMsg::Welcome { mode: Mode::Ffa, .. })), "in at once: {m:?}");
    assert_eq!(m.iter().filter(|m| matches!(m, ServerMsg::ShipSpawn { .. })).count(), 1, "nobody else, no AI");

    // more than a match ever held, all in the same room
    let mut others: Vec<_> = (2..=9).map(|pid| enter_room(&mut h, pid, &format!("车{pid}"))).collect();
    assert_eq!(h.census().1, 1, "one room");
    ticks(&mut h, 15);
    let m = drain(&mut a);
    assert_eq!(m.iter().filter(|m| matches!(m, ServerMsg::ShipSpawn { .. })).count(), 8, "everyone who came in");
    let (_, _, rows) = board(&m).unwrap();
    assert_eq!(rows.len(), 9);
    assert!(rows.iter().all(|r| (1..=9).contains(&r.pid)), "no AI: {rows:?}");
    assert!(board(&drain(&mut others[7])).is_some_and(|(_, _, rows)| rows.len() == 9), "the last one in sees the same room");

    // leaving leaves a gap (no AI takes over); the room closes with its last player
    h.handle(NetEvent::Leave { pid: 1 });
    ticks(&mut h, 15);
    let (_, _, rows) = board(&drain(&mut others[0])).unwrap();
    assert_eq!(rows.len(), 8);
    assert!(rows.iter().all(|r| (2..=9).contains(&r.pid)));
    for pid in 2..=9 {
        h.handle(NetEvent::Leave { pid });
    }
    ticks(&mut h, 2);
    assert_eq!(h.census().1, 0);
}

#[test]
fn the_rooms_clock_runs_only_with_two_or_more_in() {
    let mut h = hub(15.0, 480.0);
    let mut a = enter_room(&mut h, 1, "甲");
    ticks(&mut h, 15);
    let m = drain(&mut a);
    assert!(announced(&m, "自由混战").is_some_and(|s| s.contains("等待")), "{m:?}");
    assert_eq!(board(&m).map(|b| (b.0, b.1)), Some((Round::Waiting, 0.0)), "alone: no clock");
    ticks(&mut h, 60 * 60);
    assert!(board(&drain(&mut a)).is_none_or(|b| b.0 == Round::Waiting), "an hour alone: still waiting");

    // a second player starts the round
    let mut b = enter_room(&mut h, 2, "乙");
    ticks(&mut h, 15);
    let m = drain(&mut a);
    assert!(announced(&m, "开战").is_some(), "{m:?}");
    let (round, left, _) = board(&m).unwrap();
    assert_eq!(round, Round::Running);
    assert!(left > 475.0 && left <= 480.0, "{left}");
    assert!(board(&drain(&mut b)).is_some_and(|b| b.0 == Round::Running));

    // a third comes into the round under way, on the same clock
    ticks(&mut h, 60 * 60);
    let mut c = enter_room(&mut h, 3, "丙");
    ticks(&mut h, 15);
    let m = drain(&mut c);
    assert!(announced(&m, "自由混战").is_some_and(|s| s.contains("本局还剩")), "{m:?}");
    let (round, left, _) = board(&m).unwrap();
    assert_eq!(round, Round::Running);
    assert!(left < 421.0, "{left}");

    // down to one: the round is called off and the room waits again
    h.handle(NetEvent::Leave { pid: 2 });
    h.handle(NetEvent::Leave { pid: 3 });
    ticks(&mut h, 15);
    let m = drain(&mut a);
    assert!(announced(&m, "等待中").is_some(), "{m:?}");
    assert_eq!(board(&m).map(|b| (b.0, b.1)), Some((Round::Waiting, 0.0)));
}

#[test]
fn a_round_ends_on_time_with_a_break_and_the_next_starts_from_nothing() {
    let mut h = hub(15.0, 2.0);
    let mut a = enter_room(&mut h, 1, "甲");
    let _b = enter_room(&mut h, 2, "乙");
    ticks(&mut h, 15);
    assert!(board(&drain(&mut a)).is_some_and(|b| b.0 == Round::Running));
    // time's up: the results stand for the break
    ticks(&mut h, 2 * 60);
    let m = drain(&mut a);
    assert!(announced(&m, "本局结束").is_some_and(|s| s.contains("没有人击毁")), "no kills, no winner: {m:?}");
    let (round, left, rows) = board(&m).unwrap();
    assert_eq!(round, Round::Over);
    assert!(left > 9.0 && left <= 10.0, "{left}");
    assert_eq!(rows.len(), 2);
    assert!(!m.iter().any(|m| matches!(m, ServerMsg::MatchOver { .. })), "the room never ends: nobody is sent back to the lobby");
    // then the next round
    ticks(&mut h, 10 * 60);
    let m = drain(&mut a);
    assert!(announced(&m, "开战").is_some(), "{m:?}");
    assert!(board(&m).is_some_and(|b| b.0 == Round::Running));
}

#[test]
fn a_vehicle_that_cant_drive_doesnt_get_into_the_room() {
    let mut h = hub(15.0, 480.0);
    let mut a = connect(&mut h, 1, "甲");
    msg(&mut h, 1, ClientMsg::Queue { mode: Mode::Ffa, tier: None });
    ticks(&mut h, 15);
    found(&drain(&mut a)).unwrap();
    msg(&mut h, 1, ClientMsg::Pick { design: preset("blank") });
    msg(&mut h, 1, ClientMsg::Ready);
    ticks(&mut h, 2);
    let m = drain(&mut a);
    assert!(m.iter().any(|m| matches!(m, ServerMsg::Notice { text } if text.contains("出车检查"))), "told why: {m:?}");
    assert!(!m.iter().any(|m| matches!(m, ServerMsg::Welcome { .. })));
    // leaving before getting in leaves nothing behind
    h.handle(NetEvent::Leave { pid: 1 });
    assert_eq!(h.census().1, 0);
}

#[test]
fn spectators_watch_the_room() {
    let mut h = hub(15.0, 480.0);
    let (out, mut w) = unbounded_channel();
    h.handle(NetEvent::Connect { pid: 9, name: "观众".into(), watch: true, out });
    let m = drain(&mut w);
    assert!(m.iter().any(|m| matches!(m, ServerMsg::Welcome { mode: Mode::Ffa, .. })), "into the (empty) room: {m:?}");
    // a player who comes in shows up, and is alone as far as the clock goes
    let _a = enter_room(&mut h, 1, "甲");
    ticks(&mut h, 15);
    let m = drain(&mut w);
    assert_eq!(m.iter().filter(|m| matches!(m, ServerMsg::ShipSpawn { .. })).count(), 1);
    assert_eq!(board(&m).map(|b| b.0), Some(Round::Waiting), "spectators don't start the clock");
    assert_eq!(h.census().1, 1);
}

#[test]
fn ai_joins_the_queue_one_by_one_like_players_do() {
    let mut h = hub(2.0, 60.0);
    let mut a = connect(&mut h, 1, "甲");
    msg(&mut h, 1, ClientMsg::Queue { mode: Mode::Teams, tier: None });
    // the count goes up a driver at a time, a second or more apart, until the match is made
    let mut counts = Vec::new();
    let mut made = None;
    for _ in 0..30 * 60 {
        ticks(&mut h, 1);
        let m = drain(&mut a);
        counts.extend(m.iter().filter_map(|m| match m {
            ServerMsg::Queued { players } => Some(*players),
            _ => None,
        }));
        if let Some(f) = found(&m) {
            made = Some(f);
            break;
        }
    }
    let (_, slots) = made.expect("a match once AI has filled the queue");
    counts.dedup();
    assert_eq!(counts, [1, 2, 3], "one more at a time: {counts:?}");
    assert_eq!(slots.len(), 4);
}

#[test]
fn ai_picks_at_a_players_pace() {
    let mut h = hub(2.0, 60.0);
    let mut a = connect(&mut h, 1, "甲");
    msg(&mut h, 1, ClientMsg::Queue { mode: Mode::Teams, tier: None });
    let mut t = 0;
    while found(&drain(&mut a)).is_none() {
        ticks(&mut h, 1);
        t += 1;
        assert!(t < 30 * 60);
    }
    ticks(&mut h, 1);
    let m = drain(&mut a);
    assert!(!m.iter().any(|m| matches!(m, ServerMsg::SlotPick { ready: true, .. })), "nobody locked in the moment the match was made: {m:?}");
    // a few seconds on, every AI seat has shown its vehicle and locked it in
    ticks(&mut h, 15 * 60);
    let m = drain(&mut a);
    assert_eq!(m.iter().filter(|m| matches!(m, ServerMsg::SlotPick { ready: true, .. })).count(), 3, "{m:?}");
}

#[test]
fn a_player_who_leaves_mid_match_drives_on_under_their_name() {
    let mut h = hub(0.0, 60.0);
    let mut a = connect(&mut h, 1, "甲");
    let _b = connect(&mut h, 2, "乙");
    msg(&mut h, 1, ClientMsg::Queue { mode: Mode::Teams, tier: None });
    msg(&mut h, 2, ClientMsg::Queue { mode: Mode::Teams, tier: None });
    ticks(&mut h, 15);
    for pid in [1, 2] {
        msg(&mut h, pid, ClientMsg::Pick { design: preset("bison") });
        msg(&mut h, pid, ClientMsg::Ready);
    }
    ticks(&mut h, 30);
    drain(&mut a);
    h.handle(NetEvent::Leave { pid: 2 });
    ticks(&mut h, 30);
    let m = drain(&mut a);
    assert!(!m.iter().any(|m| matches!(m, ServerMsg::ShipRemoved { .. } | ServerMsg::ShipSpawn { .. })), "the same vehicle drives on: {m:?}");
    assert!(!m.iter().any(|m| matches!(m, ServerMsg::Feed { text, .. } if text.contains("乙"))), "nobody is told: {m:?}");
    let (_, _, rows) = board(&m).expect("scores");
    assert!(rows.iter().any(|r| r.pid == 2 && r.name == "乙" && r.alive), "{rows:?}");
}
