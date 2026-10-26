//! The headless duel used for balance testing: deterministic for a seed, and it settles fights.

use blocktread_server::game::duel;
use blocktread_shared::presets::preset;

#[test]
fn a_heavy_vehicle_beats_a_light_one_and_the_same_seed_replays() {
    let (heavy, light) = (preset("rhino"), preset("rover"));
    let a = duel(&heavy, &light, 42, 120.0);
    // on the hull points left if not by a kill: with guns fixed to the hull, bots on rolling
    // ground often can't finish a fight in time (guns that turn to the aim come next)
    assert_eq!(a.winner, Some(0), "rhino should win: {a:?}");
    let b = duel(&heavy, &light, 42, 120.0);
    assert_eq!((a.winner, a.secs), (b.winner, b.secs), "same seed, same fight");
}
