//! Gadgets on a vehicle: jump jets, mine layers, smoke.

use std::sync::Arc;
use blocktread_shared::DT;
use blocktread_shared::analysis::{analyze, place_reason};
use blocktread_shared::blocks::{Block, BlockType, Gadget};
use blocktread_shared::drive::{spawn_height, step_drive};
use blocktread_shared::flight::{BTN_JUMP, BTN_MINE, BTN_SMOKE, InputCmd, Kin};
use blocktread_shared::glam::{IVec3, Quat, Vec3};
use blocktread_shared::presets::preset;
use blocktread_shared::ship::ShipBody;
use blocktread_shared::terrain::Terrain;
use blocktread_shared::weapons::{WeaponState, step_weapons};

fn settled(body: &ShipBody, ground: &Terrain) -> Kin {
    let y = spawn_height(body, ground, 0.0, 0.0, Quat::IDENTITY);
    let mut k = Kin::new(Vec3::new(0.0, y, 0.0), Quat::IDENTITY, &body.stats);
    for t in 0..(2.0 / DT) as u32 {
        step_drive(&mut k, body, &InputCmd { tick: t, ..Default::default() }, ground, DT);
    }
    k
}

#[test]
fn jump_jets_throw_the_vehicle_up_once_a_cooldown() {
    let (body, ground) = (ShipBody::new(Arc::new(preset("rover"))), Terrain::flat());
    assert_eq!(body.gadget(Gadget::Jump), 2);
    let mut k = settled(&body, &ground);
    let (rest, full) = (k.pos.y, k.energy);
    let jump = InputCmd { buttons: BTN_JUMP, ..Default::default() };
    let mut top = rest;
    let mut jumps = 0;
    let mut was = 0.0;
    // holding the key: it jumps, and again only once the cooldown is over
    for t in 0..(5.0 / DT) as u32 {
        step_drive(&mut k, &body, &InputCmd { tick: t, ..jump }, &ground, DT);
        if k.jump_cd > was {
            jumps += 1;
        }
        was = k.jump_cd;
        if t < (2.0 / DT) as u32 {
            top = top.max(k.pos.y);
        }
    }
    println!("rose {:.2} m; {jumps} jumps in 5 s", top - rest);
    assert!(top - rest > 2.0, "a rover with two jets should clear a couple of metres, rose {:.2}", top - rest);
    assert_eq!(jumps, 2, "once at once, once more after the cooldown");
    // each jump drew on the battery for each jet (it refills meanwhile, but not all the way)
    assert!(k.energy < full, "energy {} of {full}", k.energy);
}

#[test]
fn a_jump_jet_needs_the_cell_below_clear() {
    let d = preset("rover");
    // the jets sit in the floor at (0, 0, -2) and (0, 0, 2): nothing may go under them, and no
    // jet goes over a block
    assert!(place_reason(&d.blocks, IVec3::new(0, -1, 2), BlockType::Hull).unwrap().contains("喷口"));
    assert!(place_reason(&d.blocks, IVec3::new(0, 2, 0), BlockType::JumpJet).unwrap().contains("正下方"));
    // a jet on the roof, over the hull: it doesn't work, and the checks say so
    let mut roof = d.clone();
    roof.blocks.push(Block::new(0, 2, 0, BlockType::JumpJet, None));
    let a = analyze(&roof);
    assert_eq!(a.stats.gadgets_blocked.len(), 1);
    assert!(a.checks.iter().any(|c| c.title == "装置" && c.msg.contains("被挡住")));
    assert_eq!(ShipBody::new(Arc::new(roof)).gadget(Gadget::Jump), 2);
}

#[test]
fn mines_and_smoke_go_once_a_cooldown_on_a_battery_draw() {
    let mut d = preset("scorpion");
    d.blocks.push(Block::new(0, 2, 3, BlockType::MineLayer, None));
    let body = ShipBody::new(Arc::new(d));
    assert_eq!((body.gadget(Gadget::Mine), body.gadget(Gadget::Smoke)), (1, 1));
    let mut k = Kin::new(Vec3::ZERO, Quat::IDENTITY, &body.stats);
    let mut ws = WeaponState::new(&body);
    let (mut mines, mut smokes) = (0, 0);
    let both = InputCmd { buttons: BTN_MINE | BTN_SMOKE, ..Default::default() };
    for t in 0..(10.0 / DT) as u32 {
        let e = k.energy;
        step_weapons(&mut k, &body, &mut ws, &InputCmd { tick: t, ..both }, DT, t, false, &mut Vec::new());
        if ws.mined || ws.smoked {
            assert!(k.energy < e, "it takes energy");
        }
        mines += ws.mined as u32;
        smokes += ws.smoked as u32;
        k.energy = body.stats.energy_cap;
    }
    let (mine, smoke) = (BlockType::MineLayer.gadget().unwrap(), BlockType::Smoke.gadget().unwrap());
    assert_eq!(mines, (10.0 / mine.cooldown).ceil() as u32);
    assert_eq!(smokes, (10.0 / smoke.cooldown).ceil() as u32);
    // with no gadget, nothing
    let rover = ShipBody::new(Arc::new(preset("rover")));
    let mut ws = WeaponState::new(&rover);
    step_weapons(&mut Kin::new(Vec3::ZERO, Quat::IDENTITY, &rover.stats), &rover, &mut ws, &both, DT, 1, false, &mut Vec::new());
    assert!(!ws.mined && !ws.smoked);
}
