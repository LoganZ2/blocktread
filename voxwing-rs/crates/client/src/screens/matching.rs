//! The match screen: connected to a server, between flights. It shows the connection, the queue,
//! the ship pick (everyone's choices as they make them) and the results; the flight screen takes
//! over while a match is played.

use super::lobby::cost_label;
use super::{Overlays, Showcase};
use crate::flight::Session;
use crate::flight::hud::{spawn_score_table, team_color, team_label};
use crate::hangar::Hangar;
use crate::ui::icons::Icons;
use crate::ui::kit::*;
use crate::ui::*;
use crate::{DevOpts, Screen};
use bevy::prelude::*;
use voxwing_client_core::{ConnState, Phase, Pick};
use voxwing_shared::balance::balance;
use voxwing_shared::protocol::Mode;

pub struct MatchPlugin;

impl Plugin for MatchPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PickView>()
            .add_systems(OnExit(Screen::Match), |mut sc: ResMut<Showcase>| sc.0 = None)
            .add_systems(
                Update,
                (match_flow, rebuild_stage, refresh_queue, refresh_pick, match_actions, match_keys, send_pick, sync_showcase, show_notice)
                    .chain()
                    .run_if(in_state(Screen::Match))
                    .run_if(resource_exists::<Session>),
            );
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Stage {
    Connecting,
    /// Connected, about to queue; or spectating, waiting for a match to watch.
    Waiting,
    Queued,
    Picking,
    Over,
}

fn stage(s: &Session) -> Stage {
    match (&s.core.state, &s.core.phase) {
        (ConnState::Connecting, _) => Stage::Connecting,
        (_, Phase::Queued { .. }) => Stage::Queued,
        (_, Phase::Picking(_)) => Stage::Picking,
        (_, Phase::Over { .. }) => Stage::Over,
        _ => Stage::Waiting,
    }
}

/// The ship pick's local state: whose ship the showcase shows (None: your own), and whether
/// your ship has been sent since the pick began.
#[derive(Resource, Default)]
struct PickView {
    view: Option<usize>,
    sent: bool,
}

#[derive(Component)]
struct StageRoot;
#[derive(Component)]
struct QueueTimer;
#[derive(Component)]
struct QueueNote;
#[derive(Component)]
struct PickTimer;
#[derive(Component)]
struct PickMine;
#[derive(Component)]
struct Roster;
#[derive(Component)]
struct ConfirmBtn;
#[derive(Component)]
struct ViewLabel;

fn mmss(secs: f32) -> String {
    let s = secs.max(0.0) as u32;
    format!("{}:{:02}", s / 60, s % 60)
}

/// The pick's budget, if it has one.
fn budget(p: &Pick) -> Option<f32> {
    p.tier.as_deref().and_then(|k| balance().tier(k)).map(|t| t.budget)
}

fn tier_text(tier: Option<&str>) -> String {
    tier.and_then(|k| balance().tier(k)).map_or("造价不限".into(), |t| format!("{} ≤ {:.0}", t.name, t.budget))
}

/// Why the current ship can't fly this match, if it can't.
fn pick_problem(hangar: &Hangar, p: &Pick) -> Option<String> {
    let a = &hangar.analysis;
    if let Some(e) = a.first_error() {
        return Some(format!("没有通过起飞检查：{}", e.msg));
    }
    budget(p).filter(|b| a.stats.cost > b + 1e-3).map(|b| format!("造价 {:.0}，超出本局上限 {b:.0}。换一架。", a.stats.cost))
}

/// Queue as soon as the connection is up; move to flight when the match starts.
fn match_flow(mut session: ResMut<Session>, mut next: ResMut<NextState<Screen>>, dev: Res<DevOpts>, time: Res<Time>, mut held: Local<f32>) {
    let s = &mut *session;
    if s.core.state == ConnState::InGame {
        next.set(Screen::Flight);
        return;
    }
    if s.core.state != ConnState::Lobby {
        return;
    }
    if matches!(s.core.phase, Phase::Idle) && !s.core.watching {
        match s.want.take() {
            Some((mode, tier)) => s.core.queue(mode, tier),
            None => next.set(Screen::Lobby),
        }
    }
    // autopilot runs (tests, screenshots) lock in by themselves
    if dev.autopilot
        && let Phase::Picking(p) = &s.core.phase
        && p.ships[p.you].is_some()
        && !p.ready[p.you]
    {
        *held += time.delta_secs();
        if *held > 1.5 {
            *held = 0.0;
            s.core.ready();
        }
    }
}

/// A centred panel over a lightly dimmed screen. Returns (layer, panel).
fn center_panel(commands: &mut Commands, width: f32) -> (Entity, Entity) {
    let o = commands
        .spawn((
            Node { position_type: PositionType::Absolute, width: percent(100), height: percent(100), justify_content: JustifyContent::Center, align_items: AlignItems::Center, ..default() },
            BackgroundColor(Color::srgba(0.0, 0.005, 0.01, 0.35)),
            StageRoot,
            DespawnOnExit(Screen::Match),
        ))
        .id();
    let p = spawn_panel(commands, Some(o), Node { width: px(width), max_height: percent(86), padding: px(24).all(), flex_direction: FlexDirection::Column, row_gap: px(GAP), ..default() });
    commands.entity(p).insert((solid_plate(), SlideIn::new(0.0, 0.0, 18.0)));
    (o, p)
}

#[allow(clippy::too_many_arguments)]
fn rebuild_stage(mut commands: Commands, session: Res<Session>, roots: Query<Entity, With<StageRoot>>, font: Res<UiFont>, mut built: Local<Option<Stage>>, mut view: ResMut<PickView>) {
    let st = stage(&session);
    if !roots.is_empty() && *built == Some(st) {
        return;
    }
    *built = Some(st);
    *view = PickView::default();
    for e in &roots {
        commands.entity(e).despawn();
    }
    let f = &*font;
    let core = &session.core;
    match (st, &core.phase) {
        (Stage::Picking, Phase::Picking(p)) => build_pick(&mut commands, f, p),
        (Stage::Over, Phase::Over { winner, rows }) => {
            let (_, p) = center_panel(&mut commands, 620.0);
            let me = rows.iter().find(|r| Some(r.pid) == core.pid);
            let won = me.map(|r| if core.mode == Some(Mode::Teams) { winner.starts_with(team_label(r.team)) } else { winner.starts_with(&r.name) });
            let head = commands.spawn((row(GAP), ChildOf(p))).id();
            commands.spawn((heading(f, winner.clone(), 28.0, INK), ChildOf(head)));
            match won {
                Some(true) => commands.spawn((chip(f, "胜利", OK), ChildOf(head))),
                Some(false) if winner != "平局" => commands.spawn((chip(f, "落败", DANGER), ChildOf(head))),
                _ => commands.spawn((chip(f, if core.watching { "观战" } else { "平局" }, MUTED), ChildOf(head))),
            };
            if let Some(r) = me {
                commands.spawn((text(f, format!("你的战绩：击毁 {} · 阵亡 {} · {} 分", r.kills, r.deaths, r.score), 14.0, MUTED), ChildOf(p)));
            }
            spawn_score_table(&mut commands, f, p, rows, core.mode, core.pid);
            let foot = commands.spawn((Node { justify_content: JustifyContent::FlexEnd, column_gap: px(GAP_S), ..default() }, ChildOf(p))).id();
            spawn_btn(&mut commands, f, foot, Kind::Normal, "返回", UiAction::LeaveFlight, Some("Esc"));
            let again = spawn_btn(&mut commands, f, foot, Kind::Accent, if core.watching { "继续观战" } else { "再来一局" }, UiAction::PlayAgain, Some("Enter"));
            commands.entity(again).insert(bevy::input_focus::AutoFocus);
        }
        (Stage::Queued, Phase::Queued { mode, tier, .. }) => {
            let (_, p) = center_panel(&mut commands, 440.0);
            let r = commands.spawn((row(GAP), ChildOf(p))).id();
            commands.spawn((spinner(), ChildOf(r)));
            commands.spawn((heading(f, "正在匹配", 22.0, INK), ChildOf(r)));
            commands.spawn((tech(f, "0:00", 22.0, ACCENT), QueueTimer, Node { margin: UiRect::left(Val::Auto), ..default() }, ChildOf(r)));
            let chips = commands.spawn((row(GAP_S), ChildOf(p))).id();
            commands.spawn((chip(f, mode.label(), ACCENT), ChildOf(chips)));
            commands.spawn((chip(f, tier_text(tier.as_deref()), MUTED), ChildOf(chips)));
            commands.spawn((text(f, "", 13.0, MUTED), QueueNote, ChildOf(p)));
            let foot = commands.spawn((Node { justify_content: JustifyContent::FlexEnd, ..default() }, ChildOf(p))).id();
            spawn_btn(&mut commands, f, foot, Kind::Normal, "取消匹配", UiAction::CancelQueue, Some("Esc"));
        }
        _ => {
            let (_, p) = center_panel(&mut commands, 420.0);
            let (title, sub) = match st {
                Stage::Connecting => ("正在连接", format!("{} · 5 秒无响应会自动放弃", session.addr)),
                _ if core.watching => ("等待对局", "服务器上的下一局开始后就能观看".to_string()),
                _ => ("正在进入", session.addr.clone()),
            };
            let r = commands.spawn((row(GAP), ChildOf(p))).id();
            commands.spawn((spinner(), ChildOf(r)));
            commands.spawn((col(2.0), ChildOf(r), children![heading(f, title, 20.0, INK), text(f, sub, 12.0, MUTED)]));
            let foot = commands.spawn((Node { justify_content: JustifyContent::FlexEnd, ..default() }, ChildOf(p))).id();
            spawn_btn(&mut commands, f, foot, Kind::Normal, "取消", UiAction::LeaveFlight, Some("Esc"));
        }
    }
}

fn build_pick(commands: &mut Commands, f: &UiFont, p: &Pick) {
    let root = commands.spawn((Node { position_type: PositionType::Absolute, width: percent(100), height: percent(100), ..default() }, Pickable::IGNORE, IgnoreHover, StageRoot, DespawnOnExit(Screen::Match))).id();
    let top = commands
        .spawn((
            Node { position_type: PositionType::Absolute, left: px(EDGE), right: px(EDGE), top: px(EDGE), height: px(H_MD), column_gap: px(GAP), align_items: AlignItems::Center, ..default() },
            IgnoreHover,
            ChildOf(root),
        ))
        .id();
    spawn_title(commands, f, top, "选择战机", "SHIP SELECT");
    commands.spawn((chip(f, p.mode.label(), ACCENT), ChildOf(top)));
    commands.spawn((chip(f, tier_text(p.tier.as_deref()), MUTED), ChildOf(top)));
    commands.spawn((Node { flex_grow: 1.0, ..default() }, ChildOf(top)));
    commands.spawn((text(f, "开战倒计时", 13.0, MUTED), ChildOf(top)));
    commands.spawn((tech(f, "", 26.0, INK), PickTimer, Node { width: px(48), justify_content: JustifyContent::FlexEnd, ..default() }, TextLayout::justify(Justify::Right), ChildOf(top)));

    let panel_top = EDGE + H_MD + GAP;
    // left: your ship, flipped through with ‹ ›
    let left = spawn_panel(commands, Some(root), Node { position_type: PositionType::Absolute, left: px(EDGE), top: px(panel_top), bottom: px(EDGE), width: px(344), padding: px(PAD).all(), flex_direction: FlexDirection::Column, row_gap: px(GAP), ..default() });
    commands.entity(left).insert(SlideIn::new(0.05, -40.0, 0.0));
    commands.spawn((col(GAP), PickMine, ChildOf(left)));
    commands.spawn((Node { flex_grow: 1.0, ..default() }, ChildOf(left)));
    let cb = spawn_btn(commands, f, left, Kind::Primary, "确认出战", UiAction::ConfirmPick, Some("Enter"));
    commands.entity(cb).insert(ConfirmBtn);

    // right: everyone in the match and what they fly
    let right = spawn_panel(commands, Some(root), Node { position_type: PositionType::Absolute, right: px(EDGE), top: px(panel_top), bottom: px(EDGE), width: px(400), padding: px(PAD).all(), flex_direction: FlexDirection::Column, row_gap: px(GAP), ..default() });
    commands.entity(right).insert(SlideIn::new(0.1, 40.0, 0.0));
    let body = spawn_scroll_body(commands, right);
    commands.entity(body).insert(Roster);

    // what the showcase shows, when it isn't your own ship
    commands.spawn((
        text(f, "", 13.0, INK),
        ViewLabel,
        Node { position_type: PositionType::Absolute, left: px(0), right: px(0), bottom: px(EDGE + 8.0), justify_content: JustifyContent::Center, ..default() },
        TextLayout::justify(Justify::Center),
        Pickable::IGNORE,
        ChildOf(root),
    ));
}

/// Your ship's card in the pick: name with ‹ ›, its numbers, and whether it can fly this match.
fn build_mine(commands: &mut Commands, f: &UiFont, icons: &Icons, parent: Entity, hangar: &Hangar, p: &Pick) {
    let locked = p.ready[p.you];
    let a = &hangar.analysis;
    let s = &a.stats;
    let sec = spawn_section(commands, f, parent, "你的战机|YOUR SHIP");
    let r = commands.spawn((Node { justify_content: JustifyContent::SpaceBetween, align_items: AlignItems::Center, ..default() }, ChildOf(sec))).id();
    let pick = commands.spawn((row(GAP_S), ChildOf(r))).id();
    if !locked {
        spawn_icon_btn(commands, f, pick, icons.left.clone(), UiAction::ShipPrev);
    }
    commands.spawn((heading(f, hangar.design.name.clone(), 22.0, INK), ChildOf(pick)));
    if !locked {
        spawn_icon_btn(commands, f, pick, icons.right.clone(), UiAction::ShipNext);
        commands.spawn((tech(f, format!("{} / {}", hangar.fleet.current + 1, hangar.fleet.ships.len()), 12.0, MUTED), ChildOf(r)));
    } else {
        commands.spawn((chip(f, "已确认", OK), ChildOf(r)));
    }
    for (label, frac, val, c) in [
        ("造价", s.cost / budget(p).unwrap_or(350.0), cost_label(s.cost), INK.with_alpha(0.7)),
        ("速度", s.max_speed / 125.0, format!("{:.0} m/s", s.max_speed), ACCENT),
        ("转向", s.pitch_rate / 2.0, format!("{:.0} 度/秒", s.pitch_rate.to_degrees()), ACCENT),
        ("火力", s.dps / 200.0, format!("{:.0} DPS", s.dps), SIGNAL),
        ("持续", s.dps_sustained / 200.0, format!("{:.0} DPS", s.dps_sustained), SIGNAL),
        ("耐久", s.hp / 3500.0, format!("{:.0}", s.hp), OK),
        ("护盾", s.shield_cap / 200.0, if s.shield_cap > 0.0 { format!("{:.0}", s.shield_cap) } else { "无".into() }, Color::srgb(0.4, 0.62, 1.0)),
    ] {
        let (gauge, v) = spawn_stat(commands, f, sec, label, c);
        commands.entity(gauge).insert(Plate::gauge(c, 24.0, frac.clamp(0.0, 1.0)));
        commands.entity(v).insert(Text::new(val));
    }
    let note = match (locked, pick_problem(hangar, p)) {
        (true, _) => ("已锁定。等其他人确认，或倒计时结束后开战。".to_string(), MUTED),
        (false, Some(why)) => (why, WARN),
        (false, None) => ("其他人能看到你正在选的飞机。← → 换一架，Enter 确认。".to_string(), MUTED),
    };
    commands.spawn((para(f, note.0, 13.0, note.1), ChildOf(parent)));
}

/// One seat in the roster: team stripe, name, ship and cost, picking or locked in.
#[allow(clippy::too_many_arguments)]
fn spawn_seat(commands: &mut Commands, f: &UiFont, parent: Entity, p: &Pick, k: usize, viewing: bool) {
    let slot = &p.slots[k];
    let you = k == p.you;
    let name = if you { format!("{}（你）", slot.name) } else { slot.name.clone() };
    let card = spawn_btn(commands, f, parent, Kind::Card, name, UiAction::ViewSlot(if you { None } else { Some(k) }), None);
    commands.entity(card).insert((Node { padding: UiRect::new(px(16), px(12), px(10), px(10)), row_gap: px(2), ..btn_node(Kind::Card) }, Selected(viewing)));
    let stripe = if p.mode == Mode::Teams { team_color(slot.team) } else if you { ACCENT } else { LINE_HI };
    commands.spawn((Node { position_type: PositionType::Absolute, left: px(0), top: px(8), bottom: px(8), width: px(3), ..default() }, BackgroundColor(stripe), Pickable::IGNORE, ChildOf(card)));
    let (label, c) = match (slot.bot, p.ready[k]) {
        (true, _) => ("AI", MUTED),
        (false, true) => ("已确认", OK),
        (false, false) => ("选择中", WARN),
    };
    let tag = commands.spawn((chip(f, label, c), Pickable::IGNORE, ChildOf(card))).id();
    commands.entity(tag).insert(Node { position_type: PositionType::Absolute, right: px(12), top: px(10), height: px(22), padding: UiRect::horizontal(px(8)), align_items: AlignItems::Center, ..default() });
    let line = match &p.ships[k] {
        Some(d) => {
            let a = voxwing_shared::analysis::analyze(d);
            // an AI seat is already named after its ship
            let ship = if slot.bot { String::new() } else { format!("{} · ", d.name) };
            format!("{ship}造价 {} · {:.0} m/s · {:.0} DPS", cost_label(a.stats.cost), a.stats.max_speed, a.stats.dps)
        }
        None => "还没选".into(),
    };
    commands.spawn((text(f, line, 13.0, MUTED), Pickable::IGNORE, ChildOf(card)));
}

type PickSig = (Vec<(Option<String>, usize, bool)>, Option<usize>);

#[allow(clippy::too_many_arguments)]
fn refresh_pick(
    mut commands: Commands,
    session: Res<Session>,
    hangar: Res<Hangar>,
    view: Res<PickView>,
    font: Res<UiFont>,
    icons: Res<Icons>,
    mine: Query<Entity, With<PickMine>>,
    roster: Query<Entity, With<Roster>>,
    confirm: Query<(Entity, &Children), With<ConfirmBtn>>,
    mut texts: ParamSet<(Query<&mut Text, With<PickTimer>>, Query<&mut Text, With<ViewLabel>>, Query<&mut Text, With<BtnText>>)>,
    mut last: Local<Option<(bool, PickSig)>>,
) {
    let Phase::Picking(p) = &session.core.phase else {
        *last = None;
        return;
    };
    let (Ok(mine), Ok(roster)) = (mine.single(), roster.single()) else { return };
    let f = &*font;
    let left = format!("{}", p.left.ceil() as u32);
    for mut t in &mut texts.p0() {
        if t.0 != left {
            t.0 = left.clone();
        }
    }
    let sig: PickSig = (p.ships.iter().zip(&p.ready).map(|(d, &r)| (d.as_ref().map(|d| d.name.clone()), d.as_ref().map_or(0, |d| d.blocks.len()), r)).collect(), view.view);
    let fresh = last.is_none();
    let (was_locked, was_sig) = last.clone().unwrap_or((false, (Vec::new(), None)));
    let locked = p.ready[p.you];
    if fresh || hangar.is_changed() || was_locked != locked {
        commands.entity(mine).despawn_children();
        build_mine(&mut commands, f, &icons, mine, &hangar, p);
    }
    if fresh || was_sig != sig {
        commands.entity(roster).despawn_children();
        let groups: Vec<(Option<u16>, Vec<usize>)> = if p.mode == Mode::Teams {
            let mut teams: Vec<u16> = p.slots.iter().map(|s| s.team).collect();
            teams.dedup();
            teams.into_iter().map(|t| (Some(t), (0..p.slots.len()).filter(|&k| p.slots[k].team == t).collect())).collect()
        } else {
            vec![(None, (0..p.slots.len()).collect())]
        };
        for (team, seats) in groups {
            let label = match team {
                Some(t) => format!("{}|{}", team_label(t), if t == 1 { "BLUE" } else { "RED" }),
                None => format!("对局成员|PLAYERS · {}", p.slots.len()),
            };
            let sec = spawn_section(&mut commands, f, roster, &label);
            for k in seats {
                spawn_seat(&mut commands, f, sec, p, k, view.view == Some(k));
            }
        }
        commands.spawn((para(f, "点一名对手，中间会换成他选的飞机。", 12.0, MUTED), ChildOf(roster)));
    }
    *last = Some((locked, sig));
    // the confirm button: off once locked in, or while the ship can't fly this match
    if let Ok((cb, kids)) = confirm.single() {
        set_enabled(&mut commands, cb, !locked && pick_problem(&hangar, p).is_none());
        let want = if locked { "等待其他人…" } else { "确认出战" };
        for &k in kids {
            if let Ok(mut t) = texts.p2().get_mut(k)
                && t.0 != want
            {
                t.0 = want.into();
            }
        }
    }
    let label = match view.view.and_then(|k| Some((&p.slots[k], p.ships[k].as_ref()?))) {
        Some((s, d)) => format!("正在查看 {} 的「{}」 · 点自己的名字回到你的飞机", s.name, d.name),
        None => String::new(),
    };
    for mut t in &mut texts.p1() {
        if t.0 != label {
            t.0 = label.clone();
        }
    }
}

fn refresh_queue(session: Res<Session>, mut timer: Query<&mut Text, (With<QueueTimer>, Without<QueueNote>)>, mut note: Query<&mut Text, With<QueueNote>>) {
    let Phase::Queued { waited, players, fill_in, .. } = &session.core.phase else { return };
    let t = mmss(*waited);
    for mut x in &mut timer {
        if x.0 != t {
            x.0 = t.clone();
        }
    }
    let n = if *fill_in > 0.5 { format!("队列中 {players} 人 · {} 秒后由 AI 补齐空位", fill_in.ceil() as u32) } else { format!("队列中 {players} 人 · 正在组建对局…") };
    for mut x in &mut note {
        if x.0 != n {
            x.0 = n.clone();
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn match_actions(mut msgs: MessageReader<UiAction>, mut session: ResMut<Session>, mut next: ResMut<NextState<Screen>>, mut view: ResMut<PickView>, hangar: Res<Hangar>, mut toasts: ResMut<Toasts>, mut modal: ResMut<ModalReq>) {
    for m in msgs.read() {
        match m.clone() {
            UiAction::CancelQueue => {
                session.core.unqueue();
                next.set(Screen::Lobby);
            }
            UiAction::LeaveFlight => {
                modal.0 = None;
                next.set(Screen::Lobby);
            }
            UiAction::ConfirmPick => {
                let Phase::Picking(p) = &session.core.phase else { continue };
                if p.ready[p.you] {
                    continue;
                }
                if let Some(why) = pick_problem(&hangar, p) {
                    toasts.push(why, ToastKind::Warn);
                    continue;
                }
                // the server gets the ship on screen, then the lock-in, in that order
                let mut d = hangar.design.clone();
                d.name = d.sanitized_name();
                session.core.pick(d);
                session.core.ready();
            }
            UiAction::ViewSlot(k) => view.view = k,
            UiAction::PlayAgain => {
                let core = &mut session.core;
                if core.watching {
                    core.watch_again();
                } else if let Some((mode, tier)) = core.queued_for.clone() {
                    core.queue(mode, tier);
                }
            }
            _ => {}
        }
    }
}

fn match_keys(keys: Res<ButtonInput<KeyCode>>, overlays: Res<Overlays>, mut modal: ResMut<ModalReq>, session: Res<Session>, mut w: MessageWriter<UiAction>) {
    if overlays.any(&modal) {
        return;
    }
    let esc = keys.just_pressed(KeyCode::Escape);
    let enter = keys.just_pressed(KeyCode::Enter);
    match stage(&session) {
        Stage::Queued if esc => {
            w.write(UiAction::CancelQueue);
        }
        Stage::Connecting | Stage::Waiting if esc => {
            w.write(UiAction::LeaveFlight);
        }
        Stage::Picking => {
            if esc {
                modal.show("退出对局？", "你的席位会交给 AI 驾驶。", vec![("取消", UiAction::CloseModal, Kind::Normal), ("退出", UiAction::LeaveFlight, Kind::Danger)], None);
            }
            if enter {
                w.write(UiAction::ConfirmPick);
            }
            if keys.any_just_pressed([KeyCode::ArrowLeft, KeyCode::Comma]) {
                w.write(UiAction::ShipPrev);
            }
            if keys.any_just_pressed([KeyCode::ArrowRight, KeyCode::Period]) {
                w.write(UiAction::ShipNext);
            }
        }
        Stage::Over => {
            if esc {
                w.write(UiAction::LeaveFlight);
            }
            if enter {
                w.write(UiAction::PlayAgain);
            }
        }
        _ => {}
    }
}

/// Everyone sees the ship you are on: send it when the pick starts and whenever you flip to
/// another one that can fly this match.
fn send_pick(mut session: ResMut<Session>, hangar: Res<Hangar>, mut view: ResMut<PickView>) {
    let Phase::Picking(p) = &session.core.phase else { return };
    if p.ready[p.you] || (view.sent && !hangar.is_changed()) {
        return;
    }
    view.sent = true;
    if pick_problem(&hangar, p).is_none() {
        let mut d = hangar.design.clone();
        d.name = d.sanitized_name();
        session.core.pick(d);
    }
}

/// The showcase shows the seat you clicked; your own ship otherwise.
fn sync_showcase(session: Res<Session>, view: Res<PickView>, mut sc: ResMut<Showcase>) {
    let want = match (&session.core.phase, view.view) {
        (Phase::Picking(p), Some(k)) => p.ships.get(k).cloned().flatten(),
        _ => None,
    };
    if sc.0 != want {
        sc.0 = want;
    }
}

/// Messages from the server (a pick it refused) as toasts.
fn show_notice(mut session: ResMut<Session>, mut toasts: ResMut<Toasts>) {
    if let Some(t) = session.core.notice.take() {
        toasts.push(t, ToastKind::Warn);
    }
}
