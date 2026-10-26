//! Print the derived numbers of every preset, for balancing: `cargo run -p blocktread_shared --example stats`.

use std::sync::Arc;
use blocktread_shared::analysis::{Stats, analyze};
use blocktread_shared::blocks::BlockType;
use blocktread_shared::presets::{PRESETS, preset};
use blocktread_shared::ship::ShipBody;

fn row(name: &str, s: &Stats) {
    let d = s.thrust_drift(1.0);
    println!(
        "{:<18} {:>5.1} {:>6.1} {:>6.1} {:>6.1} {:>6.1}   {:>5.0} {:>5.0} {:>5.0}   {:>4.0} {:>4.0} {:>4.0}   {:>5.1} {:>5.1} {:>5.1}   {:>6.1} {:>6.1} {:>5}",
        name,
        s.mass,
        s.max_speed,
        s.pitch_rate.to_degrees(),
        s.yaw_rate.to_degrees(),
        s.roll_rate.to_degrees(),
        s.inertia.x,
        s.inertia.y,
        s.inertia.z,
        s.control.x,
        s.control.y,
        s.control.z,
        d.x.to_degrees(),
        d.y.to_degrees(),
        s.roll_drift.to_degrees(),
        s.dps,
        s.dps_sustained,
        s.fire_endurance.map_or("-".into(), |t| format!("{t:.0}s")),
    );
}

fn main() {
    println!(
        "{:<18} {:>5} {:>6} {:>6} {:>6} {:>6}   {:>5} {:>5} {:>5}   {:>4} {:>4} {:>4}   {:>5} {:>5} {:>5}   {:>6} {:>6} {:>5}",
        "ship (deg/s)", "mass", "speed", "pitch", "yaw", "roll", "Ip", "Iy", "Ir", "Cp", "Cy", "Cr", "drP", "drY", "drR", "dps", "sust", "lasts"
    );
    for p in PRESETS {
        row(p.key, &analyze(&preset(p.key)).stats);
    }
    // what losing an engine does: knock out each preset's rightmost engine
    for p in PRESETS {
        let mut body = ShipBody::new(Arc::new(preset(p.key)));
        let right = body.stats.engines.iter().copied().max_by_key(|&i| body.design.blocks[i].x);
        if let Some(i) = right.filter(|_| body.stats.engines.len() > 1) {
            body.destroy(i);
            body.settle();
            row(&format!("{} -R engine", p.key), &body.stats);
        }
    }
    // and losing a wing: knock out the left wing's root column, the rest falls off with it
    for p in PRESETS {
        let mut body = ShipBody::new(Arc::new(preset(p.key)));
        let blocks = &body.design.blocks;
        let wings: Vec<usize> = (0..blocks.len()).filter(|&i| blocks[i].ty == BlockType::Wing && blocks[i].x < 0).collect();
        let root_x = wings.iter().map(|&i| blocks[i].x).max();
        let roots: Vec<usize> = wings.into_iter().filter(|&i| Some(blocks[i].x) == root_x).collect();
        for i in roots {
            body.destroy(i);
        }
        body.settle();
        row(&format!("{} -L wing", p.key), &body.stats);
    }
}
