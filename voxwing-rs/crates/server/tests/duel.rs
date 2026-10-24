//! The headless duel used for balance testing: deterministic for a seed, and it settles fights.

use voxwing_server::game::duel;
use voxwing_shared::presets::preset;

#[test]
fn a_heavy_ship_beats_a_light_one_and_the_same_seed_replays() {
    let (heavy, light) = (preset("rhino"), preset("hummingbird"));
    let a = duel(&heavy, &light, 42, 120.0);
    assert_eq!(a.winner, Some(0), "rhino should win: {a:?}");
    assert!(a.secs < 120.0, "the fight should end in a kill");
    let b = duel(&heavy, &light, 42, 120.0);
    assert_eq!((a.winner, a.secs), (b.winner, b.secs), "same seed, same fight");
}
