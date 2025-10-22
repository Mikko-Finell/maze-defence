use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
};

use maze_defence_core::{
    BugColor, BugId, CellCoord, CellPoint, CellRect, Command, Event, Health, NavigationFieldView,
    PlayMode, TileCoord, TowerId, TowerKind, TowerTarget,
};
use maze_defence_system_tower_targeting::TowerTargeting;
use maze_defence_world::{self as world, query, World};

#[test]
fn deterministic_replay_handles_equidistant_bugs_and_builder_mode() {
    let script = scripted_commands();
    let script_len = script.len();
    let first = replay(script.clone());
    let second = replay(script);

    assert_eq!(first, second, "replay diverged between runs");
    assert_eq!(first.assignments.len(), script_len);

    let fingerprint = first.fingerprint();
    let expected = 0x947c_9912_6280_80d1;
    assert_eq!(
        fingerprint, expected,
        "fingerprint mismatch: {fingerprint:#x}"
    );

    let spawn_ids: Vec<_> = first
        .events
        .iter()
        .filter_map(|event| match event {
            EventRecord::BugSpawned { bug, .. } => Some(*bug),
            _ => None,
        })
        .collect();
    assert_eq!(spawn_ids.len(), 2, "expected exactly two spawn events");
    let expected_bug = spawn_ids
        .iter()
        .copied()
        .min()
        .expect("spawn_ids contains entries");

    assert!(
        script_len > 6,
        "scripted commands must populate all snapshots"
    );

    let after_first_spawn = &first.assignments[4];
    assert_eq!(after_first_spawn.targets.len(), 1);
    assert_eq!(after_first_spawn.targets[0].bug, expected_bug);

    let after_second_spawn = &first.assignments[5];
    assert_eq!(after_second_spawn.targets.len(), 1);
    assert_eq!(after_second_spawn.targets[0].bug, expected_bug);

    let builder_snapshot = &first.assignments[6];
    assert!(
        builder_snapshot.targets.is_empty(),
        "builder mode must clear targets"
    );
}

fn replay(commands: Vec<Command>) -> ReplayOutcome {
    let mut world = World::new();
    let mut targeting = TowerTargeting::new();
    let mut current_targets = Vec::new();
    let mut assignments = Vec::new();
    let mut events = Vec::new();

    for command in commands {
        let mut generated = Vec::new();
        world::apply(&mut world, command, &mut generated);
        events.extend(generated.into_iter().map(EventRecord::from));

        let play_mode = query::play_mode(&world);
        let towers = query::towers(&world);
        let bugs = query::bug_view(&world);
        let cells_per_tile = query::cells_per_tile(&world);

        targeting.handle(
            play_mode,
            &towers,
            &bugs,
            cells_per_tile,
            &mut current_targets,
        );

        assignments.push(TargetSnapshot::from(&current_targets));
    }

    let navigation = query::navigation_field(&world);
    let navigation_fingerprint = navigation_fingerprint(&navigation);

    ReplayOutcome {
        events,
        assignments,
        navigation_fingerprint,
    }
}

fn scripted_commands() -> Vec<Command> {
    let configure = Command::ConfigureTileGrid {
        columns: TileCoord::new(6),
        rows: TileCoord::new(6),
        tile_length: 1.0,
        cells_per_tile: 2,
    };
    let enter_builder = Command::SetPlayMode {
        mode: PlayMode::Builder,
    };
    let place_tower = Command::PlaceTower {
        kind: TowerKind::Basic,
        origin: CellCoord::new(1, 1),
    };

    let (first_spawner, second_spawner) = select_equidistant_spawners(
        configure.clone(),
        enter_builder.clone(),
        place_tower.clone(),
    );

    let enter_attack = Command::SetPlayMode {
        mode: PlayMode::Attack,
    };
    let spawn_first = Command::SpawnBug {
        spawner: first_spawner,
        color: BugColor::from_rgb(0xff, 0, 0),
        health: Health::new(3),
    };
    let spawn_second = Command::SpawnBug {
        spawner: second_spawner,
        color: BugColor::from_rgb(0, 0xff, 0),
        health: Health::new(3),
    };
    let exit_to_builder = Command::SetPlayMode {
        mode: PlayMode::Builder,
    };

    vec![
        configure,
        enter_builder,
        place_tower,
        enter_attack,
        spawn_first,
        spawn_second,
        exit_to_builder,
    ]
}

fn select_equidistant_spawners(
    configure: Command,
    enter_builder: Command,
    place_tower: Command,
) -> (CellCoord, CellCoord) {
    let mut world = World::new();
    let mut events = Vec::new();

    world::apply(&mut world, configure, &mut events);
    events.clear();
    world::apply(&mut world, enter_builder, &mut events);
    events.clear();
    world::apply(&mut world, place_tower, &mut events);

    let tower_snapshot = query::towers(&world)
        .iter()
        .next()
        .copied()
        .expect("tower placement succeeded");
    let region = tower_snapshot.region;
    let tower_center = (
        region.origin().column() + region.size().width() / 2,
        region.origin().row() + region.size().height() / 2,
    );

    let spawners = query::bug_spawners(&world);
    let mut pair = None;
    for (index, first) in spawners.iter().enumerate() {
        let first_distance = squared_distance_to_center(*first, tower_center);
        for second in spawners.iter().skip(index + 1) {
            if squared_distance_to_center(*second, tower_center) == first_distance {
                pair = Some((*first, *second));
                break;
            }
        }
        if pair.is_some() {
            break;
        }
    }

    pair.expect("expected at least one pair of equidistant spawners")
}

fn squared_distance_to_center(cell: CellCoord, center: (u32, u32)) -> u64 {
    let dx = cell.column().abs_diff(center.0);
    let dy = cell.row().abs_diff(center.1);
    u64::from(dx).saturating_mul(u64::from(dx)) + u64::from(dy).saturating_mul(u64::from(dy))
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct ReplayOutcome {
    events: Vec<EventRecord>,
    assignments: Vec<TargetSnapshot>,
    navigation_fingerprint: u64,
}

fn navigation_fingerprint(view: &NavigationFieldView<'_>) -> u64 {
    let mut hasher = DefaultHasher::new();
    view.width().hash(&mut hasher);
    view.height().hash(&mut hasher);
    view.cells().hash(&mut hasher);
    hasher.finish()
}

impl ReplayOutcome {
    fn fingerprint(&self) -> u64 {
        let mut hasher = DefaultHasher::new();
        self.hash(&mut hasher);
        hasher.finish()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct TargetSnapshot {
    targets: Vec<TargetRecord>,
}

impl TargetSnapshot {
    fn from(targets: &[TowerTarget]) -> Self {
        Self {
            targets: targets.iter().map(TargetRecord::from).collect(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct TargetRecord {
    tower: TowerId,
    bug: BugId,
    tower_center: HalfCellCoord,
    bug_center: HalfCellCoord,
}

impl From<&TowerTarget> for TargetRecord {
    fn from(target: &TowerTarget) -> Self {
        Self {
            tower: target.tower,
            bug: target.bug,
            tower_center: HalfCellCoord::from(target.tower_center_cells),
            bug_center: HalfCellCoord::from(target.bug_center_cells),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct HalfCellCoord {
    column_twice: i32,
    row_twice: i32,
}

impl From<CellPoint> for HalfCellCoord {
    fn from(point: CellPoint) -> Self {
        Self {
            column_twice: to_half_cell(point.column()),
            row_twice: to_half_cell(point.row()),
        }
    }
}

fn to_half_cell(value: f32) -> i32 {
    (value * 2.0).round() as i32
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum EventRecord {
    PlayModeChanged {
        mode: PlayMode,
    },
    TowerPlaced {
        tower: TowerId,
        kind: TowerKind,
        region: CellRect,
    },
    BugSpawned {
        bug: BugId,
        cell: CellCoord,
        color: BugColor,
        health: Health,
    },
}

impl From<Event> for EventRecord {
    fn from(event: Event) -> Self {
        match event {
            Event::PlayModeChanged { mode } => Self::PlayModeChanged { mode },
            Event::TowerPlaced {
                tower,
                kind,
                region,
            } => Self::TowerPlaced {
                tower,
                kind,
                region,
            },
            Event::BugSpawned {
                bug_id,
                cell,
                color,
                health,
            } => Self::BugSpawned {
                bug: bug_id,
                cell,
                color,
                health,
            },
            other => panic!("unexpected event during targeting replay: {other:?}"),
        }
    }
}
