//! Balance testing: AI-flown 1v1 duels between every pair of ships, many times each, as a
//! win-rate table. Both pilots fly with the same skill, so the table compares the ships.
//!
//!   cargo run --release -p voxwing-server --example duel -- [--games 60] [--secs 150] [--weapons]
//!
//! `--weapons` adds 游隼 variants with its four guns swapped for four of each weapon type.
//! `--equal-cost` then tops every variant up with armour and hull under its belly to the same
//! cost, which is the fair question: on the same budget, is some weapon simply better?
//! Set VOXWING_BALANCE to a copy of balance.ron to try numbers without rebuilding.

use std::sync::atomic::{AtomicUsize, Ordering};
use voxwing_server::game::{Duel, duel};
use voxwing_shared::analysis::{analyze, place_reason};
use voxwing_shared::blocks::{Block, BlockType, Design};
use voxwing_shared::glam::IVec3;
use voxwing_shared::presets::{PRESETS, preset};

fn main() {
    let mut games = 60usize;
    let mut secs = 150.0f32;
    let mut weapons = false;
    let mut equal = false;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--games" => games = args.next().and_then(|v| v.parse().ok()).unwrap_or(games),
            "--secs" => secs = args.next().and_then(|v| v.parse().ok()).unwrap_or(secs),
            "--weapons" => weapons = true,
            "--equal-cost" => equal = true,
            _ => panic!("unknown argument {a}"),
        }
    }
    let mut ships: Vec<(String, Design)> = PRESETS.iter().map(|p| (p.name.to_string(), preset(p.key))).collect();
    if weapons {
        for t in BlockType::ALL.into_iter().filter(|t| t.weapon().is_some()) {
            let mut d = preset("bison");
            for b in d.blocks.iter_mut().filter(|b| b.ty.weapon().is_some()) {
                b.ty = t;
            }
            ships.push((format!("野牛·{}", t.info().name), d));
        }
        if equal {
            let target = ships[PRESETS.len()..].iter().map(|(_, d)| analyze(d).stats.cost).fold(0.0, f32::max);
            for (_, d) in ships[PRESETS.len()..].iter_mut() {
                pad(d, target);
            }
        }
    }
    let n = ships.len();
    // every unordered pair, `games` times, alternating which side each ship starts on
    let jobs: Vec<(usize, usize, usize)> = (0..n).flat_map(|i| (i + 1..n).flat_map(move |j| (0..games).map(move |k| (i, j, k)))).collect();
    let next = AtomicUsize::new(0);
    let threads = std::thread::available_parallelism().map_or(4, |p| p.get());
    let results: Vec<Vec<(usize, usize, Duel)>> = std::thread::scope(|sc| {
        let handles: Vec<_> = (0..threads)
            .map(|_| {
                sc.spawn(|| {
                    let mut out = Vec::new();
                    loop {
                        let at = next.fetch_add(1, Ordering::Relaxed);
                        let Some(&(i, j, k)) = jobs.get(at) else { break };
                        let (x, y) = if k % 2 == 0 { (i, j) } else { (j, i) };
                        let mut d = duel(&ships[x].1, &ships[y].1, 0x5eed + at as u64 * 7919, secs);
                        // report from i's side
                        if x != i {
                            d.winner = d.winner.map(|w| 1 - w);
                            d.hull.swap(0, 1);
                        }
                        out.push((i, j, d));
                    }
                    out
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });
    // wins[i][j]: how often i beat j; time[i][j]: average fight length
    let mut wins = vec![vec![0.0f32; n]; n];
    let mut time = vec![vec![0.0f32; n]; n];
    let mut timeouts = 0;
    for (i, j, d) in results.into_iter().flatten() {
        let (wi, wj) = match d.winner {
            Some(0) => (1.0, 0.0),
            Some(_) => (0.0, 1.0),
            None => (0.5, 0.5),
        };
        wins[i][j] += wi / games as f32;
        wins[j][i] += wj / games as f32;
        time[i][j] += d.secs / games as f32;
        time[j][i] += d.secs / games as f32;
        if d.secs >= secs {
            timeouts += 1;
        }
    }
    let cost = |d: &Design| analyze(d).stats.cost;
    println!("{} ships × {games} duels per pair, {secs} s limit, {timeouts} timed out", n);
    print!("{:<14}{:>6}", "win% vs →", "cost");
    for (name, _) in &ships {
        print!("{:>10}", name.chars().take(6).collect::<String>());
    }
    println!("{:>8}", "overall");
    for i in 0..n {
        print!("{:<14}{:>6.0}", ships[i].0, cost(&ships[i].1));
        for j in 0..n {
            if i == j {
                print!("{:>10}", "—");
            } else {
                let flag = if wins[i][j] > 0.7 { "!" } else { " " };
                print!("{:>9.0}{flag}", wins[i][j] * 100.0);
            }
        }
        println!("{:>7.0}%", (0..n).filter(|&j| j != i).map(|j| wins[i][j]).sum::<f32>() / (n - 1) as f32 * 100.0);
    }
    println!("\naverage fight length (s)");
    for i in 0..n {
        print!("{:<14}", ships[i].0);
        for j in 0..n {
            if i == j { print!("{:>10}", "—") } else { print!("{:>10.0}", time[i][j]) }
        }
        println!();
    }
}

/// Add armour (then hull, for the last few points) under the ship until it costs `target`.
fn pad(d: &mut Design, target: f32) {
    let spots: Vec<IVec3> = (-2..=-1).rev().flat_map(|y| [0, -1, 1].into_iter().flat_map(move |x| (-4..=3).map(move |z| IVec3::new(x, y, z)))).collect();
    for ty in [BlockType::Armor, BlockType::Hull] {
        for &p in &spots {
            let cost = analyze(d).stats.cost;
            if cost + ty.info().cost > target + 0.5 {
                continue;
            }
            if place_reason(&d.blocks, p, ty).is_none() {
                d.blocks.push(Block::new(p.x, p.y, p.z, ty, None));
                if !analyze(d).ok() {
                    d.blocks.pop();
                }
            }
        }
    }
}
