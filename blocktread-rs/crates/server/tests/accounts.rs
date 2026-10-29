//! Accounts on a server that keeps them: logging in, what a level opens, parts to buy, and what
//! matches pay.

use std::time::{Duration, Instant};
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};
use blocktread_server::hub::Hub;
use blocktread_server::store::Accounts;
use blocktread_server::{HubConfig, NetEvent, Outgoing};
use blocktread_shared::blocks::BlockType;
use blocktread_shared::economy::Unlocks;
use blocktread_shared::presets::preset;
use blocktread_shared::protocol::*;

fn hub(match_secs: f32) -> Hub {
    Hub::new(HubConfig { queue_wait: 0.0, match_secs, accounts: Accounts::Memory, ..HubConfig::default() }).unwrap()
}

fn connect(h: &mut Hub, pid: u16, name: &str) -> UnboundedReceiver<Outgoing> {
    let (out, rx) = unbounded_channel();
    h.handle(NetEvent::Connect { pid, name: name.into(), watch: false, out });
    rx
}

fn msg(h: &mut Hub, pid: u16, msg: ClientMsg) {
    h.handle(NetEvent::Msg { pid, msg });
}

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

/// Tick (the store answers from its own thread) until a message `want` picks out arrives.
fn until<T>(h: &mut Hub, rx: &mut UnboundedReceiver<Outgoing>, mut want: impl FnMut(&ServerMsg) -> Option<T>) -> T {
    let t0 = Instant::now();
    loop {
        h.tick();
        if let Some(t) = drain(rx).iter().find_map(&mut want) {
            return t;
        }
        assert!(t0.elapsed() < Duration::from_secs(10), "never came");
        std::thread::sleep(Duration::from_millis(2));
    }
}

/// The buggy with armour on its deck: over the light tier's budget, nothing else changed.
fn heavy_rover() -> blocktread_shared::blocks::Design {
    let mut d = preset("rover");
    for (x, z) in [(0, 1), (0, 2), (-1, 1), (1, 1), (-1, 2), (1, 2)] {
        d.blocks.push(blocktread_shared::blocks::Block::new(x, 1, z, BlockType::Armor, None));
    }
    let a = blocktread_shared::analysis::analyze(&d);
    assert!(a.first_error().is_none() && a.stats.cost > 80.0, "{:?} {}", a.first_error().map(|e| e.msg.clone()), a.stats.cost);
    d
}

fn notice(h: &mut Hub, rx: &mut UnboundedReceiver<Outgoing>) -> String {
    until(h, rx, |m| match m {
        ServerMsg::Notice { text } => Some(text.clone()),
        _ => None,
    })
}

fn sign_up(h: &mut Hub, rx: &mut UnboundedReceiver<Outgoing>, pid: u16, user: &str) -> (String, Profile) {
    msg(h, pid, ClientMsg::Auth(AuthReq::Register { user: user.into(), pass: "secret1".into() }));
    until(h, rx, |m| match m {
        ServerMsg::Account { user: u, token, profile } if u == user => Some((token.clone(), *profile)),
        ServerMsg::AuthFailed { reason } => panic!("{reason}"),
        _ => None,
    })
}

#[test]
fn signing_up_logs_in_and_the_token_logs_in_again() {
    let mut h = hub(60.0);
    let mut a = connect(&mut h, 1, "随便");
    let (token, p) = sign_up(&mut h, &mut a, 1, "老王");
    assert_eq!(p, Profile::default());
    let mut b = connect(&mut h, 2, "x");
    msg(&mut h, 2, ClientMsg::Auth(AuthReq::Resume { token }));
    let user = until(&mut h, &mut b, |m| match m {
        ServerMsg::Account { user, .. } => Some(user.clone()),
        _ => None,
    });
    assert_eq!(user, "老王");
    msg(&mut h, 2, ClientMsg::Auth(AuthReq::Login { user: "老王".into(), pass: "nope".into() }));
    let why = until(&mut h, &mut b, |m| match m {
        ServerMsg::AuthFailed { reason } => Some(reason.clone()),
        _ => None,
    });
    assert!(why.contains("不对"), "{why}");
}

#[test]
fn a_newcomer_waits_for_2v2_and_cant_drive_parts_not_bought() {
    let mut h = hub(60.0);
    let mut a = connect(&mut h, 1, "游客");
    // guest or new account alike: level 1
    msg(&mut h, 1, ClientMsg::Queue { mode: Mode::Teams, tier: None });
    assert!(notice(&mut h, &mut a).contains("2v2"));
    // the room takes them, but not in a vehicle with parts they haven't bought
    msg(&mut h, 1, ClientMsg::Queue { mode: Mode::Ffa, tier: None });
    for _ in 0..15 {
        h.tick();
    }
    msg(&mut h, 1, ClientMsg::Pick { design: preset("wasp") });
    let why = notice(&mut h, &mut a);
    assert!(why.contains("悬浮器"), "{why}");
    // nor a heavy one: the room's cap follows the level
    msg(&mut h, 1, ClientMsg::Pick { design: heavy_rover() });
    assert!(notice(&mut h, &mut a).contains("等级"));
}

#[test]
fn the_test_range_has_no_rules() {
    let mut h = hub(60.0);
    let mut a = connect(&mut h, 1, "游客");
    msg(&mut h, 1, ClientMsg::Queue { mode: Mode::Solo, tier: None });
    for _ in 0..15 {
        h.tick();
    }
    msg(&mut h, 1, ClientMsg::Pick { design: preset("rhino") });
    msg(&mut h, 1, ClientMsg::Ready);
    until(&mut h, &mut a, |m| matches!(m, ServerMsg::Welcome { mode: Mode::Solo, .. }).then_some(()));
}

#[test]
fn a_round_in_the_room_pays_accounts_and_coins_buy_parts() {
    let mut h = hub(2.0);
    let mut a = connect(&mut h, 1, "甲");
    let mut b = connect(&mut h, 2, "乙");
    sign_up(&mut h, &mut a, 1, "车手甲");
    for pid in [1, 2] {
        msg(&mut h, pid, ClientMsg::Queue { mode: Mode::Ffa, tier: None });
    }
    for _ in 0..15 {
        h.tick();
    }
    for pid in [1, 2] {
        msg(&mut h, pid, ClientMsg::Pick { design: preset("rover") });
        msg(&mut h, pid, ClientMsg::Ready);
    }
    // a two-second round: the account is paid for it, the guest isn't
    let (coins, xp) = until(&mut h, &mut a, |m| match m {
        ServerMsg::Rewarded { coins, xp, .. } => Some((*coins, *xp)),
        _ => None,
    });
    assert!(coins > 0 && xp > 0, "{coins} {xp}");
    assert!(!drain(&mut b).iter().any(|m| matches!(m, ServerMsg::Rewarded { .. })), "a guest earns nothing");
    // not enough for a part yet
    msg(&mut h, 1, ClientMsg::Unlock { block: BlockType::Railgun });
    assert!(notice(&mut h, &mut a).contains("金币不够"));
    // a starter part is had already: the profile comes back unchanged
    msg(&mut h, 1, ClientMsg::Unlock { block: BlockType::Cannon });
    let p = until(&mut h, &mut a, |m| match m {
        ServerMsg::Profile(p) => Some(*p),
        _ => None,
    });
    assert!(Unlocks(p.unlocks).has(BlockType::Cannon));
    // a guest can't buy
    msg(&mut h, 2, ClientMsg::Unlock { block: BlockType::Ram });
    assert!(notice(&mut h, &mut b).contains("登录"));
}
