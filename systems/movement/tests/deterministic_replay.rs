use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
    time::Duration,
};

use maze_defence_core::{
    BugColor, BugId, BugSnapshot, CellCoord, Command, Event, Gold, Health, NavigationFieldView,
    PendingWaveDifficulty, PlayMode, SpeciesTableVersion, TileCoord, TowerKind, WaveDifficulty,
    WaveId,
};
use maze_defence_system_movement::Movement;
use maze_defence_world::{self as world, query, World};

const DEFAULT_STEP_MS: u32 = 250;
const FAST_STEP_MS: u32 = 150;
const SLOW_STEP_MS: u32 = 450;

#[test]
fn deterministic_replay_produces_expected_snapshot() {
    assert_stable_replay(baseline_commands(), 0x4358_d43d_e556_cbf2);
}

#[test]
fn dense_corridor_replay_is_stable() {
    assert_stable_replay(dense_corridor_commands(), 0x6eff_f25b_28b5_2816);
}

#[test]
fn side_hallway_diversion_replay_is_stable() {
    assert_stable_replay(side_hallway_diversion_commands(), 0xd5de_ab2f_0d36_c9f8);
}

#[test]
fn stall_regression_replay_is_stable() {
    assert_stable_replay(stall_regression_commands(), 0x91a5_ddba_d487_5324);
}

#[test]
fn mixed_cadence_replay_is_stable() {
    assert_stable_replay(mixed_cadence_commands(), 0x5bce_9e0b_7737_4231);
}

fn assert_stable_replay(commands: Vec<Command>, expected: u64) {
    let first = replay(commands.clone());
    let second = replay(commands);

    assert_eq!(first, second, "replay diverged between runs");

    let fingerprint = first.fingerprint();
    assert_eq!(
        fingerprint, expected,
        "fingerprint mismatch: {fingerprint:#x}"
    );
}

fn replay(commands: Vec<Command>) -> ReplayOutcome {
    let mut world = World::new();
    let mut movement = Movement::default();
    let mut log = Vec::new();

    for command in commands {
        let mut events = Vec::new();
        world::apply(&mut world, command, &mut events);
        record_events(&events, &mut log);
        process_movement(&mut world, &mut movement, events, &mut log);
    }

    let bugs = query::bug_view(&world)
        .into_vec()
        .into_iter()
        .map(BugState::from)
        .collect();

    let navigation = query::navigation_field(&world);
    let navigation_fingerprint = navigation_fingerprint(&navigation);

    ReplayOutcome {
        bugs,
        events: log,
        navigation_fingerprint,
    }
}

fn process_movement(
    world: &mut World,
    movement: &mut Movement,
    pending_events: Vec<Event>,
    log: &mut Vec<EventRecord>,
) {
    let mut events = pending_events;

    loop {
        if events.is_empty() {
            break;
        }

        let bug_view = query::bug_view(world);
        let occupancy_view = query::occupancy_view(world);
        let mut commands = Vec::new();
        let target_cells = query::target_cells(world);
        let navigation_view = query::navigation_field(world);
        let reservation_ledger = query::reservation_ledger(world);
        movement.handle(
            &events,
            &bug_view,
            occupancy_view,
            navigation_view,
            reservation_ledger,
            &target_cells,
            |cell| query::is_cell_blocked(&*world, cell),
            &mut commands,
        );

        if commands.is_empty() {
            break;
        }

        events.clear();
        for command in commands {
            let mut generated_events = Vec::new();
            world::apply(world, command, &mut generated_events);
            record_events(&generated_events, log);
            events.extend(generated_events);
        }
    }
}

fn record_events(events: &[Event], log: &mut Vec<EventRecord>) {
    log.extend(events.iter().map(EventRecord::from));
}

fn baseline_commands() -> Vec<Command> {
    vec![
        Command::ConfigureTileGrid {
            columns: TileCoord::new(5),
            rows: TileCoord::new(4),
            tile_length: 1.0,
            cells_per_tile: 1,
        },
        Command::SetPlayMode {
            mode: PlayMode::Attack,
        },
        Command::SpawnBug {
            spawner: CellCoord::new(0, 0),
            color: BugColor::from_rgb(0x2f, 0x95, 0x32),
            health: Health::new(3),
            step_ms: DEFAULT_STEP_MS,
        },
        Command::Tick {
            dt: Duration::from_millis(500),
        },
        Command::Tick {
            dt: Duration::from_millis(500),
        },
        Command::Tick {
            dt: Duration::from_secs(1),
        },
        Command::Tick {
            dt: Duration::from_secs(1),
        },
        Command::Tick {
            dt: Duration::from_secs(1),
        },
    ]
}

fn dense_corridor_commands() -> Vec<Command> {
    let mut commands = vec![
        Command::ConfigureTileGrid {
            columns: TileCoord::new(1),
            rows: TileCoord::new(8),
            tile_length: 1.0,
            cells_per_tile: 1,
        },
        Command::SetPlayMode {
            mode: PlayMode::Attack,
        },
    ];

    let palette = [
        (0x2f, 0x70, 0x32),
        (0x2f, 0x78, 0x32),
        (0x2f, 0x80, 0x32),
        (0x2f, 0x88, 0x32),
        (0x2f, 0x90, 0x32),
        (0x2f, 0x98, 0x32),
    ];

    for &(red, green, blue) in &palette {
        commands.push(Command::SpawnBug {
            spawner: CellCoord::new(0, 0),
            color: BugColor::from_rgb(red, green, blue),
            health: Health::new(3),
            step_ms: DEFAULT_STEP_MS,
        });
        commands.push(Command::Tick {
            dt: Duration::from_millis(250),
        });
    }

    for _ in 0..24 {
        commands.push(Command::Tick {
            dt: Duration::from_millis(250),
        });
    }

    commands
}

fn side_hallway_diversion_commands() -> Vec<Command> {
    let mut commands = vec![
        Command::ConfigureTileGrid {
            columns: TileCoord::new(8),
            rows: TileCoord::new(6),
            tile_length: 1.0,
            cells_per_tile: 1,
        },
        Command::SetPlayMode {
            mode: PlayMode::Builder,
        },
        Command::PlaceTower {
            kind: TowerKind::Basic,
            origin: CellCoord::new(2, 2),
        },
        Command::PlaceTower {
            kind: TowerKind::Basic,
            origin: CellCoord::new(6, 3),
        },
        Command::SetPlayMode {
            mode: PlayMode::Attack,
        },
    ];

    let palette = [
        (0x48, 0x70, 0x90),
        (0x48, 0x78, 0x90),
        (0x48, 0x80, 0x90),
        (0x48, 0x88, 0x90),
        (0x48, 0x90, 0x90),
        (0x48, 0x98, 0x90),
    ];

    for &(red, green, blue) in &palette {
        commands.push(Command::SpawnBug {
            spawner: CellCoord::new(4, 0),
            color: BugColor::from_rgb(red, green, blue),
            health: Health::new(5),
            step_ms: DEFAULT_STEP_MS,
        });
        commands.push(Command::Tick {
            dt: Duration::from_millis(250),
        });
    }

    for _ in 0..48 {
        commands.push(Command::Tick {
            dt: Duration::from_millis(250),
        });
    }

    commands
}

fn stall_regression_commands() -> Vec<Command> {
    let mut commands = vec![
        Command::ConfigureTileGrid {
            columns: TileCoord::new(1),
            rows: TileCoord::new(6),
            tile_length: 1.0,
            cells_per_tile: 1,
        },
        Command::SetPlayMode {
            mode: PlayMode::Attack,
        },
        Command::SpawnBug {
            spawner: CellCoord::new(0, 0),
            color: BugColor::from_rgb(0x9a, 0x4c, 0x2f),
            health: Health::new(3),
            step_ms: DEFAULT_STEP_MS,
        },
    ];

    for _ in 0..6 {
        commands.push(Command::Tick {
            dt: Duration::from_millis(250),
        });
    }

    commands.push(Command::SpawnBug {
        spawner: CellCoord::new(0, 0),
        color: BugColor::from_rgb(0x2f, 0x8c, 0xc0),
        health: Health::new(3),
        step_ms: DEFAULT_STEP_MS,
    });

    for _ in 0..18 {
        commands.push(Command::Tick {
            dt: Duration::from_millis(250),
        });
    }

    commands
}

fn mixed_cadence_commands() -> Vec<Command> {
    let mut commands = vec![
        Command::ConfigureTileGrid {
            columns: TileCoord::new(1),
            rows: TileCoord::new(8),
            tile_length: 1.0,
            cells_per_tile: 1,
        },
        Command::SetPlayMode {
            mode: PlayMode::Attack,
        },
        Command::SpawnBug {
            spawner: CellCoord::new(0, 0),
            color: BugColor::from_rgb(0xf2, 0x69, 0x35),
            health: Health::new(4),
            step_ms: FAST_STEP_MS,
        },
        Command::Tick {
            dt: Duration::from_millis(100),
        },
        Command::Tick {
            dt: Duration::from_millis(100),
        },
        Command::Tick {
            dt: Duration::from_millis(100),
        },
        Command::SpawnBug {
            spawner: CellCoord::new(0, 0),
            color: BugColor::from_rgb(0x2f, 0x70, 0xc5),
            health: Health::new(5),
            step_ms: SLOW_STEP_MS,
        },
    ];

    for (index, dt) in [100, 150, 200, 100, 300, 100, 250, 100, 150, 200, 100, 300]
        .into_iter()
        .enumerate()
    {
        if index % 3 == 0 {
            commands.push(Command::Tick {
                dt: Duration::from_millis(50),
            });
        }

        commands.push(Command::Tick {
            dt: Duration::from_millis(dt),
        });
    }

    commands
}

#[test]
fn movement_pauses_in_builder_mode() {
    let mut world = World::new();
    let mut movement = Movement::default();
    let mut events = Vec::new();

    world::apply(
        &mut world,
        Command::SetPlayMode {
            mode: PlayMode::Attack,
        },
        &mut events,
    );
    events.clear();

    world::apply(
        &mut world,
        Command::SpawnBug {
            spawner: CellCoord::new(0, 0),
            color: BugColor::from_rgb(0x2f, 0x95, 0x32),
            health: Health::new(3),
            step_ms: DEFAULT_STEP_MS,
        },
        &mut events,
    );
    events.clear();

    world::apply(
        &mut world,
        Command::Tick {
            dt: Duration::from_millis(500),
        },
        &mut events,
    );

    let bug_view = query::bug_view(&world);
    assert!(
        bug_view.iter().any(|bug| bug.ready_for_step),
        "expected at least one bug ready for a step"
    );
    let occupancy_view = query::occupancy_view(&world);
    let target_cells = query::target_cells(&world);

    let mut commands = Vec::new();
    let navigation_view = query::navigation_field(&world);
    let reservation_ledger = query::reservation_ledger(&world);
    movement.handle(
        &[Event::TimeAdvanced {
            dt: Duration::from_millis(500),
        }],
        &bug_view,
        occupancy_view,
        navigation_view,
        reservation_ledger,
        &target_cells,
        |cell| query::is_cell_blocked(&world, cell),
        &mut commands,
    );
    assert!(
        !commands.is_empty(),
        "expected movement to propose steps while in attack mode"
    );

    commands.clear();
    let navigation_view = query::navigation_field(&world);
    let reservation_ledger = query::reservation_ledger(&world);
    movement.handle(
        &[
            Event::PlayModeChanged {
                mode: PlayMode::Builder,
            },
            Event::TimeAdvanced {
                dt: Duration::from_millis(500),
            },
        ],
        &bug_view,
        occupancy_view,
        navigation_view,
        reservation_ledger,
        &target_cells,
        |cell| query::is_cell_blocked(&world, cell),
        &mut commands,
    );
    assert!(
        commands.is_empty(),
        "movement must not emit commands while builder mode is active"
    );
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct ReplayOutcome {
    bugs: Vec<BugState>,
    events: Vec<EventRecord>,
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
struct BugState {
    id: maze_defence_core::BugId,
    cell: CellCoord,
    step_ms: u32,
    accum_ms: u32,
    ready_for_step: bool,
    color: (u8, u8, u8),
    max_health: Health,
    health: Health,
}

impl From<BugSnapshot> for BugState {
    fn from(snapshot: BugSnapshot) -> Self {
        Self {
            id: snapshot.id,
            cell: snapshot.cell,
            step_ms: snapshot.step_ms,
            accum_ms: snapshot.accum_ms,
            ready_for_step: snapshot.ready_for_step,
            color: (
                snapshot.color.red(),
                snapshot.color.green(),
                snapshot.color.blue(),
            ),
            max_health: snapshot.max_health,
            health: snapshot.health,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum EventRecord {
    TimeAdvanced {
        dt_micros: u128,
    },
    BugAdvanced {
        bug_id: maze_defence_core::BugId,
        from: CellCoord,
        to: CellCoord,
    },
    BugExited {
        bug_id: maze_defence_core::BugId,
        cell: CellCoord,
    },
    PlayModeChanged {
        mode: PlayMode,
    },
    BugSpawned {
        bug_id: maze_defence_core::BugId,
        cell: CellCoord,
        color: (u8, u8, u8),
        health: Health,
    },
    PendingWaveDifficultyChanged {
        pending: PendingWaveDifficulty,
    },
    PressureConfigChanged {
        species_table_version: SpeciesTableVersion,
    },
    WaveStarted {
        wave: WaveId,
        difficulty: WaveDifficulty,
        effective_difficulty: u32,
        reward_multiplier: u32,
        pressure_scalar: u32,
        plan_pressure: u32,
        plan_species_table_version: u32,
        plan_burst_count: u32,
    },
    TowerPlaced {
        tower: maze_defence_core::TowerId,
        kind: TowerKind,
        region: maze_defence_core::CellRect,
    },
    GoldChanged {
        amount: Gold,
    },
    RoundLost {
        bug: BugId,
    },
}

impl From<&Event> for EventRecord {
    fn from(event: &Event) -> Self {
        match event {
            Event::TimeAdvanced { dt } => Self::TimeAdvanced {
                dt_micros: dt.as_micros(),
            },
            Event::BugAdvanced { bug_id, from, to } => Self::BugAdvanced {
                bug_id: *bug_id,
                from: *from,
                to: *to,
            },
            Event::BugExited { bug_id, cell } => Self::BugExited {
                bug_id: *bug_id,
                cell: *cell,
            },
            Event::PlayModeChanged { mode } => Self::PlayModeChanged { mode: *mode },
            Event::BugSpawned {
                bug_id,
                cell,
                color,
                health,
            } => Self::BugSpawned {
                bug_id: *bug_id,
                cell: *cell,
                color: (color.red(), color.green(), color.blue()),
                health: *health,
            },
            Event::PendingWaveDifficultyChanged { pending } => {
                Self::PendingWaveDifficultyChanged { pending: *pending }
            }
            Event::PressureConfigChanged {
                species_table_version,
                ..
            } => Self::PressureConfigChanged {
                species_table_version: *species_table_version,
            },
            Event::WaveStarted {
                wave,
                difficulty,
                effective_difficulty,
                reward_multiplier,
                pressure_scalar,
                plan_pressure,
                plan_species_table_version,
                plan_burst_count,
            } => Self::WaveStarted {
                wave: *wave,
                difficulty: *difficulty,
                effective_difficulty: *effective_difficulty,
                reward_multiplier: *reward_multiplier,
                pressure_scalar: *pressure_scalar,
                plan_pressure: plan_pressure.get(),
                plan_species_table_version: plan_species_table_version.get(),
                plan_burst_count: *plan_burst_count,
            },
            Event::TowerPlaced {
                tower,
                kind,
                region,
            } => Self::TowerPlaced {
                tower: *tower,
                kind: *kind,
                region: *region,
            },
            Event::GoldChanged { amount } => Self::GoldChanged { amount: *amount },
            Event::RoundLost { bug } => Self::RoundLost { bug: *bug },
            Event::TowerRemoved { .. }
            | Event::TowerPlacementRejected { .. }
            | Event::TowerRemovalRejected { .. }
            | Event::ProjectileFired { .. }
            | Event::ProjectileHit { .. }
            | Event::ProjectileExpired { .. }
            | Event::ProjectileRejected { .. }
            | Event::HardWinAchieved { .. }
            | Event::DifficultyLevelChanged { .. }
            | Event::BugDamaged { .. }
            | Event::BugDied { .. } => {
                unreachable!("tower events are not expected in movement replay tests")
            }
        }
    }
}
