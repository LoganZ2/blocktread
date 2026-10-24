//! The hub's match cycle without a network: queue, AI fill, the ship pick, the match, results.

use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};
use voxwing_server::hub::Hub;
use voxwing_server::{HubConfig, NetEvent, Outgoing};
use voxwing_shared::presets::preset;
use voxwing_shared::protocol::*;

fn hub(queue_wait: f32, match_secs: f32) -> Hub {
    Hub::new(HubConfig { queue_wait, match_secs, kill_limit: 10 })
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
    assert_eq!(slots.iter().filter(|s| s.bot).count(), 2, "AI takes the two empty seats");
    // the two players are on opposite teams, two seats a side
    assert_ne!(slots[ya as usize].team, slots[yb as usize].team);
    for t in [1, 2] {
        assert_eq!(slots.iter().filter(|s| s.team == t).count(), 2);
    }

    // a's pick shows up for b, and the match waits for both to lock in
    msg(&mut h, 1, ClientMsg::Pick { design: preset("falcon") });
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
    msg(&mut h, 1, ClientMsg::Queue { mode: Mode::Ffa, tier: Some("light".into()) });
    ticks(&mut h, 15);
    assert_eq!(found(&drain(&mut a)).unwrap().1.len(), 6);
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
    msg(&mut h, 1, ClientMsg::Queue { mode: Mode::Ffa, tier: None });
    msg(&mut h, 2, ClientMsg::Queue { mode: Mode::Ffa, tier: None });
    ticks(&mut h, 15);
    let (_, slots) = found(&drain(&mut a)).unwrap();
    let k = slots.iter().position(|s| s.name == "乙").unwrap();
    h.handle(NetEvent::Leave { pid: 2 });
    let m = drain(&mut a);
    assert!(m.iter().any(|m| matches!(m, ServerMsg::SlotPick { slot, design: Some(_), ready: true } if *slot as usize == k)), "{m:?}");
    // with everyone else set, a's lock-in starts the match
    msg(&mut h, 1, ClientMsg::Pick { design: preset("falcon") });
    msg(&mut h, 1, ClientMsg::Ready);
    ticks(&mut h, 2);
    assert!(drain(&mut a).iter().any(|m| matches!(m, ServerMsg::Welcome { .. })));
}

#[test]
fn the_match_ends_on_time_with_results_and_everyone_can_queue_again() {
    let mut h = hub(0.0, 2.0);
    let mut a = connect(&mut h, 1, "甲");
    msg(&mut h, 1, ClientMsg::Queue { mode: Mode::Ffa, tier: None });
    ticks(&mut h, 15);
    msg(&mut h, 1, ClientMsg::Pick { design: preset("falcon") });
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
    assert_eq!(rows, 6, "every seat on the final scoreboard");
    // back in the lobby: the next queue makes a new match
    msg(&mut h, 1, ClientMsg::Queue { mode: Mode::Teams, tier: None });
    ticks(&mut h, 15);
    assert!(found(&drain(&mut a)).is_some_and(|(_, s)| s.len() == 4));
}
