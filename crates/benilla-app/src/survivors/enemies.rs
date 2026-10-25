//! The horde: spawning on a ring outside the view, the chase, contact damage, death, and the
//! experience gems the dead leave.

use std::collections::HashMap;

use benilla_world::rig_anim::GlobalSeqDrive;
use benilla_world::world_point::WorldPoint;
use bevy::prelude::*;

use super::abilities::Hit;
use super::continent;
use super::data::{self, EnemyDef};
use super::hero::{self, Hero, HERO_RADIUS};
use super::units::{self, flat, yaw_toward, Fx, Marks, UnitSpec, COLOR_HURT};
use super::{Phase, Rng, Run, Selection};
use crate::creature_anim::{move_flags, MovementState};
use crate::net::{FieldChanged, Guid, ObjectStore};

#[derive(Component)]
pub(super) struct Enemy {
    pub def: &'static EnemyDef,
    pub hp: f32,
    pub max_hp: f32,
    pub speed: f32,
    pub damage: f32,
    pub radius: f32,
    pub xp: u32,
    pub boss: bool,
    pub attack_cd: f32,
    pub slow: f32,
    pub slow_t: f32,
    pub root_t: f32,
    pub stun_t: f32,
    pub fear_t: f32,
    pub weaken: f32,
    pub weaken_t: f32,
    pub dots: Vec<Dot>,
    pub dead: bool,
}

pub(super) struct Dot {
    pub dps: f32,
    pub left: f32,
    pub tick: f32,
}

/// A corpse playing its death before it fades.
#[derive(Component)]
pub(super) struct Dying(f32);

#[derive(Resource, Default)]
pub(super) struct Spawner {
    timer: f32,
}

/// An enemy died here, worth this much.
#[derive(Message, Clone, Copy)]
pub(super) struct Loot {
    pub pos: Vec3,
    pub xp: u32,
    pub boss: bool,
}

#[derive(Clone, Copy)]
pub(super) enum GemKind {
    Xp(u32),
    Heal(f32),
}

#[derive(Component)]
pub(super) struct Gem {
    kind: GemKind,
    pulled: bool,
    speed: f32,
}

/// The `GameObjectDisplayInfo` row and scale a pickup wears: a wisp per experience tier, the
/// bigger tiers rarer, and a turkey leg for health.
fn look_for(kind: GemKind) -> (u32, f32) {
    match kind {
        // `TurkeyLeg.mdx`.
        GemKind::Heal(_) => (114, 2.5),
        // `BFD_WispSmallPurple.mdx`.
        GemKind::Xp(xp) if xp >= 20 => (1307, 1.0),
        // `BFD_WispSmallGreen.mdx`.
        GemKind::Xp(xp) if xp >= 5 => (1267, 1.0),
        // `BFD_WispSmall.mdx`.
        GemKind::Xp(_) => (426, 1.0),
    }
}

/// Body radius: the big beasts need more room.
fn radius_for(def: &EnemyDef) -> f32 {
    (if def.big { 1.3 } else { 0.6 }) * def.scale
}

fn spawn_enemy(
    commands: &mut Commands,
    run: &mut Run,
    def: &'static EnemyDef,
    pos: Vec3,
    hero: Vec3,
    boss: bool,
) {
    let scale = data::hp_scale(run.pace());
    let serial = run.guid();
    let e = units::spawn_unit(
        commands,
        &UnitSpec {
            serial,
            display: def.display,
            scale: def.scale,
            level: def.level,
            faction: units::FACTION_MONSTER,
            bytes0: 1 << 8,
            weapons: &[],
            sheath: 0,
        },
        pos,
        yaw_toward(hero - pos),
    );
    let hp = def.hp * scale;
    let chase = if run.route.is_some() {
        ROUTE_CHASE
    } else {
        1.0
    };
    commands.entity(e).insert((
        Enemy {
            def,
            hp,
            max_hp: hp,
            speed: def.speed * chase,
            damage: def.damage * data::damage_scale(run.pace()),
            radius: radius_for(def),
            xp: def.xp,
            boss,
            attack_cd: 0.5,
            slow: 1.0,
            slow_t: 0.0,
            root_t: 0.0,
            stun_t: 0.0,
            fear_t: 0.0,
            weaken: 1.0,
            weaken_t: 0.0,
            dots: Vec::new(),
            dead: false,
        },
        Marks::debuffs(),
    ));
}

fn ground(world: &WorldPoint, mut p: Vec3, fallback: f32) -> Vec3 {
    p.y = world.terrain_height_under(p).unwrap_or(fallback);
    p
}

/// The boss owed now. A battleground's come on the clock; a route's come as each zone is entered.
fn owed_boss(run: &Run, roster: &'static data::Roster, elapsed: f32) -> Option<&'static EnemyDef> {
    if run.route.is_some() {
        return continent::next_boss(run);
    }
    let due = ((elapsed / data::BOSS_EVERY_SECS) as usize).min(roster.bosses.len());
    ((run.bosses as usize) < due).then(|| &roster.bosses[run.bosses as usize])
}

/// Keep the horde topped up on a ring outside the view, with the bosses and the encircling waves
/// on the clock. On a route the horde spawns ahead, toward the next stop.
pub(super) fn spawn_enemies(
    mut commands: Commands,
    time: Res<Time>,
    mut run: ResMut<Run>,
    mut rng: ResMut<Rng>,
    mut spawner: ResMut<Spawner>,
    selection: Res<Selection>,
    hero: Query<&Transform, With<Hero>>,
    enemies: Query<&Enemy>,
    world: WorldPoint,
) {
    let Ok(h) = hero.single() else { return };
    if run.dead_t.is_some() {
        return;
    }
    let hpos = h.translation;
    let elapsed = run.pace();
    let roster = continent::roster_for(&run, selection.map);
    let pool = data::wave_pool(elapsed, roster.enemies.len());

    while let Some(def) = owed_boss(&run, roster, elapsed) {
        let a = match continent::heading(&run, hpos) {
            Some(a) => a,
            None => rng.range(0.0, std::f32::consts::TAU),
        };
        let pos = ground(
            &world,
            hpos + Vec3::new(a.cos(), 0.0, a.sin()) * 30.0,
            hpos.y,
        );
        spawn_enemy(&mut commands, &mut run, def, pos, hpos, true);
        run.bosses += 1;
        run.banner = Some((format!("{} approaches!", def.name), 4.0));
    }

    // Half way between bosses, a closing ring of the moment's weakest.
    let due_rings = ((elapsed + data::BOSS_EVERY_SECS / 2.0) / data::BOSS_EVERY_SECS) as u32;
    if run.rings < due_rings {
        run.rings = due_rings;
        let def = &roster.enemies[pool[0]];
        let n = 28 + (data::progress(elapsed) * 60.0) as usize;
        for i in 0..n {
            let a = i as f32 / n as f32 * std::f32::consts::TAU;
            let pos = ground(
                &world,
                hpos + Vec3::new(a.cos(), 0.0, a.sin()) * 26.0,
                hpos.y,
            );
            spawn_enemy(&mut commands, &mut run, def, pos, hpos, false);
        }
        run.banner = Some(("They're surrounding you!".into(), 3.0));
    }

    spawner.timer -= time.delta_secs();
    if spawner.timer > 0.0 {
        return;
    }
    spawner.timer = 0.12;
    let alive = enemies.iter().filter(|e| !e.dead).count();
    let target = data::target_alive(elapsed);
    if alive >= target {
        return;
    }
    let n = (((target - alive) as f32) / 6.0).ceil().min(4.0) as usize;
    for _ in 0..n {
        let def = &roster.enemies[pool[rng.index(pool.len())]];
        let a = match continent::heading(&run, hpos) {
            Some(ahead) => ahead + rng.range(-1.0, 1.0),
            None => rng.range(0.0, std::f32::consts::TAU),
        };
        let r = rng.range(34.0, 42.0);
        let pos = ground(&world, hpos + Vec3::new(a.cos(), 0.0, a.sin()) * r, hpos.y);
        spawn_enemy(&mut commands, &mut run, def, pos, hpos, false);
    }
}

/// The chase: straight at the hero, pushed apart from each other, stopped by crowd control, and
/// swinging when in reach.
#[allow(clippy::type_complexity)]
pub(super) fn move_enemies(
    time: Res<Time>,
    mut run: ResMut<Run>,
    world: WorldPoint,
    hero: Query<(Entity, &Transform), (With<Hero>, Without<Enemy>)>,
    mut enemies: Query<(Entity, &mut Transform, &mut Enemy, &mut MovementState), Without<Hero>>,
    mut fx: Fx,
    mut hits: MessageWriter<Hit>,
) {
    let dt = time.delta_secs();
    if dt <= 0.0 {
        return;
    }
    let Ok((hero_e, htf)) = hero.single() else {
        return;
    };
    let hpos = htf.translation;
    let hero_down = run.dead_t.is_some();

    // Separation over a coarse grid: only neighbours in the same or adjacent cells push.
    const CELL: f32 = 3.0;
    let bodies: Vec<(Entity, Vec3, f32)> = enemies
        .iter()
        .filter(|(_, _, e, _)| !e.dead)
        .map(|(ent, tf, e, _)| (ent, tf.translation, e.radius))
        .collect();
    let key = |p: Vec3| ((p.x / CELL).floor() as i32, (p.z / CELL).floor() as i32);
    let mut grid: HashMap<(i32, i32), Vec<usize>> = HashMap::new();
    for (i, b) in bodies.iter().enumerate() {
        grid.entry(key(b.1)).or_default().push(i);
    }
    let mut push: HashMap<Entity, Vec3> = HashMap::with_capacity(bodies.len());
    for (i, &(ent, pos, r)) in bodies.iter().enumerate() {
        let (cx, cz) = key(pos);
        let mut acc = Vec3::ZERO;
        for dx in -1..=1 {
            for dz in -1..=1 {
                let Some(cell) = grid.get(&(cx + dx, cz + dz)) else {
                    continue;
                };
                for &j in cell {
                    if j == i {
                        continue;
                    }
                    let (_, other, or) = bodies[j];
                    let d = flat(pos - other);
                    let len = d.length();
                    let min = r + or;
                    if len < min && len > 1e-4 {
                        acc += d / len * (min - len);
                    } else if len <= 1e-4 {
                        acc += Vec3::new((i as f32).sin(), 0.0, (i as f32).cos()) * 0.1;
                    }
                }
            }
        }
        push.insert(ent, acc);
    }

    for (ent, mut tf, mut e, mut mv) in &mut enemies {
        let e = &mut *e;
        if e.dead {
            if mv.speed != 0.0 || mv.flags != 0 {
                mv.speed = 0.0;
                mv.flags = 0;
            }
            continue;
        }
        e.attack_cd -= dt;
        for t in [
            &mut e.slow_t,
            &mut e.root_t,
            &mut e.stun_t,
            &mut e.fear_t,
            &mut e.weaken_t,
        ] {
            *t = (*t - dt).max(0.0);
        }
        if e.slow_t <= 0.0 {
            e.slow = 1.0;
        }
        if e.weaken_t <= 0.0 {
            e.weaken = 1.0;
        }
        let pos = tf.translation;
        let to_hero = flat(hpos - pos);
        let dist = to_hero.length();

        // Strays far behind are brought back in front, as the reference genre does.
        if dist > 75.0 {
            let back = hpos + to_hero.normalize_or_zero() * 38.0;
            tf.translation = ground(&world, back, hpos.y);
            continue;
        }

        // Under Fade the horde has lost the hero: it mills where it stands.
        let lost = run.mobile.vanish_t > 0.0 && !e.boss;
        let held = e.stun_t > 0.0 || e.root_t > 0.0 || lost;
        let contact = dist <= HERO_RADIUS + e.radius + 0.35;
        let mut step = Vec3::ZERO;
        let mut speed = 0.0;
        if !held && !hero_down {
            let dir = if e.fear_t > 0.0 {
                -to_hero.normalize_or_zero()
            } else {
                to_hero.normalize_or_zero()
            };
            if !contact || e.fear_t > 0.0 {
                speed = e.speed * e.slow;
                step = dir * speed;
            }
            if dir != Vec3::ZERO {
                let target = Quat::from_rotation_y(yaw_toward(dir));
                tf.rotation = tf.rotation.slerp(target, (dt * 10.0).min(1.0));
            }
        }
        let shove = push.get(&ent).copied().unwrap_or_default();
        let mut next = pos + step * dt + shove * (dt * 8.0).min(0.5);
        next = ground(&world, next, pos.y);
        tf.translation = next;
        if speed > 0.0 {
            mv.speed = speed;
            mv.flags = move_flags::FORWARD;
        } else if mv.speed != 0.0 || mv.flags != 0 {
            mv.speed = 0.0;
            mv.flags = 0;
        }

        if contact
            && !lost
            && e.stun_t <= 0.0
            && e.fear_t <= 0.0
            && e.attack_cd <= 0.0
            && !hero_down
        {
            e.attack_cd = if e.boss { 1.0 } else { 1.3 };
            let raw = e.damage * e.weaken;
            let taken = hero::hurt_hero(&mut run, raw);
            fx.swing(ent, hero_e, taken);
            if taken > 0.0 {
                fx.number(hero_e, taken, false, COLOR_HURT);
            }
            if run.thorns_t > 0.0 {
                hits.write(Hit::direct(ent, run.thorns * run.stats().might, false));
            }
        }
    }
}

/// Mark timers run down; the state kits reap with them.
pub(super) fn tick_marks(
    time: Res<Time>,
    mut units: Query<(Entity, &Guid, &mut ObjectStore, &mut Marks), With<Enemy>>,
    mut fields: MessageWriter<FieldChanged>,
) {
    let dt = time.delta_secs();
    for (entity, guid, mut store, mut marks) in &mut units {
        marks.tick(dt);
        marks.flush(entity, guid.0, &mut store, &mut fields);
    }
}

/// Corpses fade once their death has played.
pub(super) fn reap_dead(
    mut commands: Commands,
    time: Res<Time>,
    mut dying: Query<(Entity, &mut Dying)>,
) {
    for (e, mut d) in &mut dying {
        d.0 += time.delta_secs();
        if d.0 > 2.5 {
            commands
                .entity(e)
                .remove::<Dying>()
                .insert(benilla_world::model_fade::DespawnFade::default());
        }
    }
}

pub(super) fn mark_dying(commands: &mut Commands, e: Entity) {
    commands.entity(e).insert(Dying(0.0));
}

/// Most gems on the ground at once; past it, a kill's worth joins an existing gem.
const MAX_GEMS: usize = 350;
/// The chance a kill drops a heal gem; a boss always drops one.
const HEAL_DROP_CHANCE: f32 = 0.10;
/// The health a heal gem restores.
const HEAL_GEM: f32 = 30.0;
/// The share of max health a level-up restores.
const LEVEL_HEAL: f32 = 0.10;
/// On a route the hero runs faster than a mount, so the horde runs faster than its base speed to keep pace.
const ROUTE_CHASE: f32 = 1.5;
/// On a route a heal gem lands on the road ahead on this clock, so healing does not wait on kills.
const ROUTE_HEAL_EVERY: f32 = 8.0;
/// The most timed heal gems waiting ahead at once.
const ROUTE_HEAL_CAP: usize = 3;
/// How far ahead of the hero a timed heal gem lands, along the road to the next stop.
const ROUTE_HEAL_RANGE: f32 = 12.0;
/// How far to either side of that road a timed heal gem may land.
const ROUTE_HEAL_SPREAD: f32 = 4.0;
/// Past this distance a timed heal gem is behind the hero: it is cleared so it cannot block new ones.
const ROUTE_HEAL_STALE: f32 = 40.0;

pub(super) fn drop_loot(
    mut commands: Commands,
    mut drops: MessageReader<Loot>,
    mut rng: ResMut<Rng>,
    mut run: ResMut<Run>,
    mut gems: Query<&mut Gem>,
) {
    let mut count = gems.iter().count();
    for d in drops.read() {
        if d.boss || rng.f32() < HEAL_DROP_CHANCE {
            let heal = GemKind::Heal(if d.boss { 1000.0 } else { HEAL_GEM });
            spawn_gem(&mut commands, run.guid(), d.pos + Vec3::X * 0.8, heal);
        }
        if count >= MAX_GEMS && !d.boss {
            if let Some(mut g) = gems.iter_mut().find(|g| matches!(g.kind, GemKind::Xp(_))) {
                if let GemKind::Xp(xp) = &mut g.kind {
                    *xp += d.xp;
                }
                continue;
            }
        }
        spawn_gem(&mut commands, run.guid(), d.pos, GemKind::Xp(d.xp));
        count += 1;
    }
}

fn spawn_gem(commands: &mut Commands, serial: u64, pos: Vec3, kind: GemKind) {
    let (display, scale) = look_for(kind);
    let e = units::spawn_object(commands, serial, display, scale, pos + Vec3::Y * 0.5);
    commands.entity(e).insert(Gem {
        kind,
        pulled: false,
        speed: 6.0,
    });
}

/// On a route, a heal gem lands on the road ahead, where the arrow points, so the hero runs into it.
pub(super) fn spawn_route_heals(
    mut commands: Commands,
    time: Res<Time>,
    mut run: ResMut<Run>,
    mut rng: ResMut<Rng>,
    mut clock: Local<f32>,
    hero: Query<&Transform, With<Hero>>,
    gems: Query<(Entity, &Transform, &Gem)>,
    world: WorldPoint,
) {
    let Ok(h) = hero.single() else { return };
    if run.route.is_none() || run.dead_t.is_some() {
        return;
    }
    let mut waiting = 0;
    for (e, tf, g) in &gems {
        if !matches!(g.kind, GemKind::Heal(_)) {
            continue;
        }
        if flat(tf.translation - h.translation).length() > ROUTE_HEAL_STALE {
            commands.entity(e).despawn();
        } else {
            waiting += 1;
        }
    }
    *clock += time.delta_secs();
    if *clock < ROUTE_HEAL_EVERY {
        return;
    }
    *clock = 0.0;
    if waiting >= ROUTE_HEAL_CAP {
        return;
    }
    let ahead = match continent::heading(&run, h.translation) {
        Some(a) => a,
        None => rng.range(0.0, std::f32::consts::TAU),
    };
    let dir = Vec3::new(ahead.cos(), 0.0, ahead.sin());
    let across = Vec3::new(-dir.z, 0.0, dir.x);
    let side = rng.range(-ROUTE_HEAL_SPREAD, ROUTE_HEAL_SPREAD);
    let pos = ground(
        &world,
        h.translation + dir * ROUTE_HEAL_RANGE + across * side,
        h.translation.y,
    );
    spawn_gem(&mut commands, run.guid(), pos, GemKind::Heal(HEAL_GEM));
}

/// Gems in the magnet's reach fly to the hero; levels bank as they land.
pub(super) fn collect_gems(
    mut commands: Commands,
    time: Res<Time>,
    mut run: ResMut<Run>,
    mut next: ResMut<NextState<Phase>>,
    hero: Query<&Transform, (With<Hero>, Without<Gem>)>,
    mut gems: Query<(Entity, &mut Transform, &mut Gem)>,
) {
    let Ok(h) = hero.single() else { return };
    if run.dead_t.is_some() {
        return;
    }
    let dt = time.delta_secs();
    let stats = run.stats();
    let target = h.translation + Vec3::Y * 1.0;
    for (e, mut tf, mut g) in &mut gems {
        let d = target - tf.translation;
        let dist = d.length();
        if !g.pulled && flat(d).length() <= stats.magnet {
            g.pulled = true;
        }
        if !g.pulled {
            continue;
        }
        if dist < 0.9 {
            match g.kind {
                GemKind::Xp(xp) => run.xp += xp as f32 * data::XP_RATE * stats.growth,
                GemKind::Heal(hp) => run.hp = (run.hp + hp).min(stats.max_hp),
            }
            commands.entity(e).despawn();
            continue;
        }
        g.speed += 30.0 * dt;
        tf.translation += d / dist * (g.speed * dt).min(dist);
    }
    while run.xp >= data::xp_to_next(run.level) {
        run.xp -= data::xp_to_next(run.level);
        run.level += 1;
        run.pending_levels += 1;
        run.hp = (run.hp + stats.max_hp * LEVEL_HEAL).min(stats.max_hp);
        if run.level == super::mobility::LEARN_LEVEL {
            if let Some(m) = super::mobility::for_class(run.class.id) {
                run.banner = Some((format!("You have learned {}! Press F.", m.name), 4.0));
            }
        }
    }
    if run.pending_levels > 0 {
        next.set(Phase::LevelUp);
    }
}

/// Pickups hang still: a wisp's own loop (its circling motes and their trails) stops on its first
/// frame, clip and global sequences alike, as soon as the model attaches.
pub(super) fn still_gems(
    mut players: Query<&mut AnimationPlayer, (With<Gem>, Added<AnimationPlayer>)>,
    mut drives: Query<&mut GlobalSeqDrive, (With<Gem>, Added<GlobalSeqDrive>)>,
) {
    for mut player in &mut players {
        for (_, active) in player.playing_animations_mut() {
            active.set_speed(0.0);
        }
    }
    for mut drive in &mut drives {
        drive.set_paused(true);
    }
}
