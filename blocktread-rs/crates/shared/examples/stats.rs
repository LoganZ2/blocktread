//! Print the derived numbers of every preset, for balancing: `cargo run -p blocktread_shared --example stats`.

use blocktread_shared::analysis::{Stats, analyze};
use blocktread_shared::presets::{PRESETS, preset};

fn row(name: &str, s: &Stats) {
    println!(
        "{:<18} {:>5.0} {:>5.1} {:>6.1} {:>6.1} {:>6.1} {:>5.0}   {:>5.0} {:>5.0} {:>5.0}   {:>6.1} {:>6.1} {:>5}",
        name,
        s.cost,
        s.mass,
        s.max_speed,
        s.accel,
        s.power,
        s.tip,
        s.inertia.x,
        s.inertia.y,
        s.inertia.z,
        s.dps,
        s.dps_sustained,
        s.fire_endurance.map_or("-".into(), |t| format!("{t:.0}s")),
    );
}

fn main() {
    println!(
        "{:<18} {:>5} {:>5} {:>6} {:>6} {:>6} {:>5}   {:>5} {:>5} {:>5}   {:>6} {:>6} {:>5}",
        "vehicle", "cost", "mass", "speed", "accel", "power", "tip°", "Ip", "Iy", "Ir", "dps", "sust", "lasts"
    );
    for p in PRESETS {
        row(p.key, &analyze(&preset(p.key)).stats);
    }
}
