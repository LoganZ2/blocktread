//! Headless netcode check: AI-flown clients queue, pick a ship, play over real QUIC (with
//! simulated ping) against an embedded server, then report how well prediction matched the server.
//! Free-for-all (the default) drives them into the room, whose rounds are `--match-secs` long.
//!
//!   cargo run -p blocktread_client_core --example bots -- --clients 4 --secs 40 --latency 50 --mode 2v2

use std::time::{Duration, Instant};
use blocktread_client_core::{ClientCore, ConnState, CoreEvent, Phase};
use blocktread_server::{HubConfig, ServerConfig, start};
use blocktread_shared::ai::{AiPilot, Contact, Skill};
use blocktread_shared::arena::{HILL_HALF, HILL_TOP};
use blocktread_shared::flight::InputCmd;
use blocktread_shared::presets::{PRESETS, preset};
use blocktread_shared::protocol::{Mode, Round};

fn main() -> anyhow::Result<()> {
    let mut clients = 3usize;
    let mut secs = 30.0f32;
    let mut latency_ms = 40u64;
    let mut mode = Mode::Ffa;
    let mut server: Option<String> = None;
    let mut match_secs = 480.0f32;
    let mut a = std::env::args().skip(1);
    while let Some(k) = a.next() {
        match k.as_str() {
            "--clients" => clients = a.next().unwrap().parse()?,
            "--secs" => secs = a.next().unwrap().parse()?,
            "--latency" => latency_ms = a.next().unwrap().parse()?,
            "--mode" => mode = if a.next().as_deref() == Some("2v2") { Mode::Teams } else { Mode::Ffa },
            "--server" => server = a.next(),
            "--match-secs" => match_secs = a.next().unwrap().parse()?,
            _ => anyhow::bail!("unknown arg {k}"),
        }
    }
    let _handle;
    let addr = match server {
        Some(s) => s,
        None => {
            let hub = HubConfig { queue_wait: 2.0, match_secs, ..HubConfig::default() };
            let h = start(ServerConfig { bind: "127.0.0.1:0".parse()?, hub, verbose: true, status: None })?;
            let addr = h.addr.to_string();
            _handle = h;
            addr
        }
    };
    println!("server {addr} · {clients} clients · {} · one-way latency {latency_ms} ms · {secs} s", mode.label());
    let lat = Duration::from_millis(latency_ms);
    let mut cs: Vec<(ClientCore, AiPilot, u32)> = (0..clients).map(|i| (ClientCore::connect(&addr, &format!("bot{i}"), lat), AiPilot::new(Skill::ffa_bot(), 99 + i as u64), 0)).collect();
    let mut matches_over = 0u32;
    let t0 = Instant::now();
    let mut last = Instant::now();
    let mut kills_seen = 0u32;
    let (mut round, mut rounds_over) = (Round::None, 0u32);
    while t0.elapsed().as_secs_f32() < secs {
        std::thread::sleep(Duration::from_millis(8));
        let dt = last.elapsed().as_secs_f32().min(0.1);
        last = Instant::now();
        let dbg = std::env::var("BOT_DEBUG").is_ok() && (t0.elapsed().as_millis() / 16) % 120 == 0;
        for (ci, (core, ai, spawns)) in cs.iter_mut().enumerate() {
            if dbg && ci == 0 && let (Some(p), Some(me)) = (core.pred.as_ref(), core.my()) {
                let near = core.ships.values().filter(|s| Some(s.id) != core.my_ship).map(|s| (s.pos.distance(p.kin.pos), s.name.clone())).min_by(|a, b| a.0.total_cmp(&b.0));
                let cmd = p.last_cmd().unwrap_or_default();
                println!("t={:.1} pos={:.0?} spd={:.0} near={:?} buttons={} range={:.0} weapons={}", t0.elapsed().as_secs_f32(), p.kin.pos, p.kin.vel.length(), near, cmd.buttons, me.body.gun_range, me.body.stats.weapons.len());
            }
            core.update(dt, &mut |c: &ClientCore, tick| ai_input(c, ai, tick));
            lobby(core, ci, mode, &mut matches_over);
            // the room's rounds, as the first client sees them
            if ci == 0 && core.round != round {
                round = core.round;
                println!("t={:.1} room: {round:?}{}", t0.elapsed().as_secs_f32(), if core.left > 0.0 { format!(" · {:.0} s", core.left) } else { String::new() });
                if round == Round::Over {
                    rounds_over += 1;
                    println!("  standings: {:?}", core.scores.iter().map(|r| (r.name.as_str(), r.kills, r.deaths)).collect::<Vec<_>>());
                }
            }
            for e in core.events.drain(..) {
                match e {
                    CoreEvent::ShipKilled { .. } => kills_seen += 1,
                    CoreEvent::YourShip(Some(_)) => *spawns += 1,
                    _ => {}
                }
            }
        }
    }
    println!("\nclient  state      rtt   snaps  KB/s  checks  corrections  mean err  max err   lead  speed  spawns");
    for (i, (core, _, spawns)) in cs.iter().enumerate() {
        let st = match &core.state {
            ConnState::InGame => "in game".to_string(),
            ConnState::Lobby => "lobby".to_string(),
            other => format!("{other:?}"),
        };
        let (checks, corr, mean, max, lead, speed) = core
            .pred
            .as_ref()
            .map(|p| (p.checks, p.corrections, p.sum_err / p.checks.max(1) as f32, p.max_err, p.lead(), p.speed))
            .unwrap_or_default();
        println!(
            "bot{i:<4} {st:<10} {:>4.0}ms {:>6} {:>5.1} {:>7} {:>12} {:>8.3}m {:>7.2}m {:>6.1} {:>6.3} {:>7}",
            core.rtt() * 1000.0,
            core.stats.snapshots,
            core.stats.bytes_in as f32 / 1024.0 / secs,
            checks,
            corr,
            mean,
            max,
            lead,
            speed,
            spawns
        );
    }
    if let Some((core, _, _)) = cs.first() {
        println!("\nscores: {:?}", core.scores.iter().map(|r| (r.name.as_str(), r.kills, r.deaths)).collect::<Vec<_>>());
        println!("ships visible to bot0: {} · kill events seen (all clients): {kills_seen} · match ends seen: {matches_over} · rounds ended: {rounds_over}", core.ships.len());
    }
    for (core, _, _) in &cs {
        core.leave();
    }
    std::thread::sleep(Duration::from_millis(300));
    Ok(())
}

/// The lobby side of a bot: queue, pick a ship (looking at the others' first), lock in, and after
/// the results queue again.
fn lobby(core: &mut ClientCore, i: usize, mode: Mode, over: &mut u32) {
    if core.state != ConnState::Lobby {
        return;
    }
    match core.phase.clone() {
        Phase::Idle => core.queue(mode, None),
        Phase::Picking(p) if !p.ready[p.you] => {
            if p.ships[p.you].is_none() {
                println!("bot{i} matched: seat {} of {} · {}", p.you, p.slots.len(), p.slots.iter().map(|s| format!("{}{}", s.name, if s.bot { "(AI)" } else { "" })).collect::<Vec<_>>().join(", "));
                core.pick(preset(PRESETS[i % PRESETS.len()].key));
            } else {
                core.ready();
            }
        }
        Phase::Over { winner, rows } => {
            *over += 1;
            println!("bot{i} match over: {winner} · {:?}", rows.iter().map(|r| (r.name.as_str(), r.team, r.kills)).collect::<Vec<_>>());
            core.queue(mode, None);
        }
        _ => {}
    }
}

fn ai_input(c: &ClientCore, ai: &mut AiPilot, tick: u32) -> InputCmd {
    let (Some(pred), Some(me)) = (c.pred.as_ref(), c.my()) else { return InputCmd::default() };
    let others: Vec<Contact> = c
        .ships
        .values()
        .filter(|s| Some(s.id) != c.my_ship)
        .map(|s| Contact { id: s.id, team: s.team, pos: s.pos, vel: s.vel, radius: s.body.radius })
        .collect();
    // in 2v2, make for the hill
    ai.goal = c.terrain.arena.as_ref().filter(|_| c.hill.is_some()).map(|a| (a.hill.centre().with_y(HILL_TOP), HILL_HALF - 3.0));
    ai.think(&pred.kin, &me.body, 0.0, me.team, &others, &c.rocks, &c.terrain, tick)
}
