//! The continent run: the hero crosses the Eastern Kingdoms zone by zone instead of holding one
//! battleground. Each zone's centre is a stop; reaching one brings that zone's horde, and each zone
//! entered on the way brings its boss (the start and the final zone excepted). The route guides
//! rather than fences: the hero may run anywhere on the continent, and the run is won at the last
//! stop.

use bevy::prelude::*;

use super::data::{self, EnemyDef, MapDef, Roster};
use super::enemies::Enemy;
use super::hero::Hero;
use super::units::{flat, wow_to_bevy};
use super::Run;

/// The routes, by the name of the [`MapDef`] that starts them: zone names in order, each resolved
/// through [`data::MAPS`] for its centre and roster.
pub(super) static ROUTES: &[(&str, &[&str])] = &[(
    "Eastern Kingdoms Run",
    &[
        "Westfall",
        "Elwynn Forest",
        "Duskwood",
        "Redridge Mountains",
        "Dun Morogh",
        "Tirisfal Glades",
        "Naxxramas",
    ],
)];

/// Run speed on a route, over the class's base: 14 yd/s before haste, faster than a mount.
const RUN_SPEED: f32 = 2.0;
/// Within this many yards of a zone's centre, the zone is reached.
const STOP_RADIUS: f32 = 150.0;
/// Horde further behind the hero than this is removed; the spawner keeps the horde ahead.
const CULL_YD: f32 = 150.0;

/// The route a map starts, if it is a continent run.
pub(super) fn route_of(map: &MapDef) -> Option<&'static [&'static str]> {
    ROUTES
        .iter()
        .find(|(name, _)| *name == map.name)
        .map(|(_, zones)| *zones)
}

fn zone_index(zone: &str) -> usize {
    data::MAPS.iter().position(|m| m.name == zone).unwrap_or(0)
}

fn zone_centre(zone: &str) -> Vec3 {
    wow_to_bevy(data::MAPS[zone_index(zone)].center)
}

/// The zone the hero stands in: the last stop reached.
fn current_zone(route: &'static [&'static str], stops: usize) -> &'static str {
    route[stops.clamp(1, route.len()) - 1]
}

/// The roster the horde is drawn from: the zone the hero is crossing on a route, else the picked
/// battleground's.
pub(super) fn roster_for(run: &Run, picked: usize) -> &'static Roster {
    match run.route {
        Some(route) => data::roster(zone_index(current_zone(route, run.stops))),
        None => data::roster(picked),
    }
}

/// The boss a route owes next: one per zone entered, the first of that zone's roster. The start
/// zone and the last bring none.
pub(super) fn next_boss(run: &Run) -> Option<&'static EnemyDef> {
    let route = run.route?;
    let owed = (run.stops - 1).min(route.len() - 2);
    if run.bosses as usize >= owed {
        return None;
    }
    data::roster(zone_index(route[run.bosses as usize + 1]))
        .bosses
        .first()
}

/// The bearing from `at` to the next stop, as `atan2(z, x)` on the ground; `None` off a route or
/// once the route is walked.
pub(super) fn heading(run: &Run, at: Vec3) -> Option<f32> {
    let next = *run.route?.get(run.stops)?;
    let d = flat(zone_centre(next) - at);
    Some(d.z.atan2(d.x))
}

/// Name the route when a run starts.
pub(super) fn begin(run: &mut Run) {
    if let Some(route) = run.route {
        let last = route[route.len() - 1];
        run.banner = Some((format!("Cross the continent to {last}"), 5.0));
    }
}

impl Run {
    /// Where the run stands for its difficulty: the time, or on a route the share of the route
    /// walked, on the same scale as [`data::VICTORY_SECS`].
    pub(super) fn pace(&self) -> f32 {
        match self.route {
            Some(_) => self.route_frac * data::VICTORY_SECS,
            None => self.elapsed,
        }
    }

    /// The run is won: at the last stop of a route, or at the time limit.
    pub(super) fn finished(&self) -> bool {
        match self.route {
            Some(route) => self.stops >= route.len(),
            None => self.elapsed >= data::VICTORY_SECS,
        }
    }

    /// Speed over the class's, before haste: a route is run faster than a mount.
    pub(super) fn run_speed(&self) -> f32 {
        if self.route.is_some() {
            RUN_SPEED
        } else {
            1.0
        }
    }
}

/// The share of the route walked to `at`: the legs behind the last stop reached, plus the way along
/// the leg toward the next.
fn route_frac(points: &[Vec3], reached: usize, at: Vec3) -> f32 {
    let leg = |w: &[Vec3]| w[0].distance(w[1]);
    let total: f32 = points.windows(2).map(leg).sum();
    if total <= 0.0 {
        return 0.0;
    }
    let behind: f32 = points[..reached].windows(2).map(leg).sum();
    let along = match (
        reached.checked_sub(1).and_then(|i| points.get(i)),
        points.get(reached),
    ) {
        (Some(&a), Some(&b)) => {
            let ab = b - a;
            let t = ((at - a).dot(ab) / ab.length_squared().max(f32::EPSILON)).clamp(0.0, 1.0);
            t * ab.length()
        }
        _ => 0.0,
    };
    (behind + along) / total
}

/// Reach the stops in turn, announce each, and keep the share of the route walked.
pub(super) fn track_route(mut run: ResMut<Run>, hero: Query<&Transform, With<Hero>>) {
    let Some(route) = run.route else {
        return;
    };
    if run.dead_t.is_some() || run.finished() {
        return;
    }
    let Ok(h) = hero.single() else {
        return;
    };
    let at = flat(h.translation);
    while !run.finished() {
        let zone = route[run.stops];
        if flat(zone_centre(zone) - at).length() > STOP_RADIUS {
            break;
        }
        run.stops += 1;
        if !run.finished() {
            let next = route[run.stops];
            run.banner = Some((format!("Entering {zone}. Next: {next}"), 4.0));
        }
    }
    if run.finished() {
        run.route_frac = 1.0;
        return;
    }
    let points: Vec<Vec3> = route.iter().map(|&z| flat(zone_centre(z))).collect();
    run.route_frac = run.route_frac.max(route_frac(&points, run.stops, at));
}

/// Horde left far behind is removed. Bosses stay, to be fought on the way.
pub(super) fn cull_behind(
    mut commands: Commands,
    run: Res<Run>,
    hero: Query<&Transform, With<Hero>>,
    enemies: Query<(Entity, &Transform, &Enemy)>,
) {
    if run.route.is_none() || run.dead_t.is_some() {
        return;
    }
    let Ok(h) = hero.single() else {
        return;
    };
    let at = flat(h.translation);
    for (e, tf, enemy) in &enemies {
        if !enemy.boss && flat(tf.translation - at).length() > CULL_YD {
            commands.entity(e).despawn();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes_resolve_to_zones_with_hordes_bosses_and_leash() {
        for &(name, zones) in ROUTES {
            let start = data::MAPS
                .iter()
                .find(|m| m.name == name)
                .expect("the route's map");
            assert!(zones.len() >= 3, "{name} needs stops between its ends");
            assert_eq!(start.center, data::MAPS[zone_index(zones[0])].center);
            for zone in zones {
                assert!(
                    data::MAPS.iter().any(|m| m.name == *zone),
                    "{zone} is not a map"
                );
                let roster = data::roster(zone_index(zone));
                assert!(roster.enemies.len() >= 3, "{zone} needs a horde");
                assert!(!roster.bosses.is_empty(), "{zone} needs a boss");
                let from_start = flat(zone_centre(zone) - zone_centre(zones[0])).length();
                assert!(from_start < start.leash, "{zone} lies beyond the leash");
            }
        }
    }

    #[test]
    fn route_frac_counts_legs_behind_and_along() {
        let p = [
            Vec3::ZERO,
            Vec3::new(100.0, 0.0, 0.0),
            Vec3::new(100.0, 0.0, 100.0),
        ];
        assert!((route_frac(&p, 1, Vec3::new(50.0, 0.0, 0.0)) - 0.25).abs() < 1e-4);
        assert!((route_frac(&p, 2, Vec3::new(100.0, 0.0, 50.0)) - 0.75).abs() < 1e-4);
    }
}
