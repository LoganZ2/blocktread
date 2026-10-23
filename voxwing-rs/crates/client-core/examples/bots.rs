//! Headless netcode check: AI-flown clients play over real QUIC (with simulated ping) against an
//! embedded server, then report how well prediction matched the server.
//!
//!   cargo run -p voxwing_client_core --example bots -- --clients 4 --secs 40 --latency 50 --mode ffa

use std::time::{Duration, Instant};
use voxwing_client_core::{ClientCore, ConnState, CoreEvent};
use voxwing_server::{ServerConfig, start};
use voxwing_shared::ai::{AiPilot, Contact, Skill};
use voxwing_shared::flight::InputCmd;
use voxwing_shared::presets::{PRESETS, preset};
use voxwing_shared::protocol::Mode;

fn main() -> anyhow::Result<()> {
    let mut clients = 3usize;
    let mut secs = 30.0f32;
    let mut latency_ms = 40u64;
    let mut mode = Mode::Ffa;
    let mut server: Option<String> = None;
    let mut a = std::env::args().skip(1);
    while let Some(k) = a.next() {
        match k.as_str() {
            "--clients" => clients = a.next().unwrap().parse()?,
            "--secs" => secs = a.next().unwrap().parse()?,
            "--latency" => latency_ms = a.next().unwrap().parse()?,
            "--mode" => mode = if a.next().as_deref() == Some("coop") { Mode::Coop } else { Mode::Ffa },
            "--server" => server = a.next(),
            _ => anyhow::bail!("unknown arg {k}"),
        }
    }
    let _handle;
    let addr = match server {
        Some(s) => s,
        None => {
            let h = start(ServerConfig { bind: "127.0.0.1:0".parse()?, mode, bots: 3, verbose: true })?;
            let addr = h.addr.to_string();
            _handle = h;
            addr
        }
    };
    println!("server {addr} · {clients} clients · one-way latency {latency_ms} ms · {secs} s");
    let lat = Duration::from_millis(latency_ms);
    let mut cs: Vec<(ClientCore, AiPilot, u32)> = (0..clients)
        .map(|i| {
            let p = &PRESETS[i % PRESETS.len()];
            (ClientCore::connect(&addr, &format!("bot{i}"), preset(p.key), lat), AiPilot::new(Skill::ffa_bot(), 99 + i as u64), 0)
        })
        .collect();
    let t0 = Instant::now();
    let mut last = Instant::now();
    let mut kills_seen = 0u32;
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
            core.update(dt, &mut |c: &ClientCore, _tick| ai_input(c, ai));
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
        println!("ships visible to bot0: {} · kill events seen (all clients): {kills_seen}", core.ships.len());
    }
    for (core, _, _) in &cs {
        core.leave();
    }
    std::thread::sleep(Duration::from_millis(300));
    Ok(())
}

fn ai_input(c: &ClientCore, ai: &mut AiPilot) -> InputCmd {
    let (Some(pred), Some(me)) = (c.pred.as_ref(), c.my()) else { return InputCmd::default() };
    let others: Vec<Contact> = c
        .ships
        .values()
        .filter(|s| Some(s.id) != c.my_ship)
        .map(|s| Contact { id: s.id, team: s.team, pos: s.pos, vel: s.vel, radius: s.body.radius })
        .collect();
    ai.think(&pred.kin, &me.body, 0.0, me.team, &others, &c.rocks, voxwing_shared::DT)
}
