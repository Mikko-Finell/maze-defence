use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
    time::Duration,
};

use maze_defence_core::{BugColor, BugSnapshot, CellCoord, Command, Event, PlayMode, TileCoord};
use maze_defence_system_spawning::{Config, Spawning};
use maze_defence_world::{self as world, query, World};

#[test]
fn emits_multiple_spawn_commands_for_large_dt() {
    let mut world = World::new();
    let mut events = Vec::new();
    world::apply(
        &mut world,
        Command::ConfigureTileGrid {
            columns: TileCoord::new(4),
            rows: TileCoord::new(4),
            tile_length: 1.0,
            cells_per_tile: 1,
        },
        &mut events,
    );

    let spawners = query::bug_spawners(&world);
    assert!(!spawners.is_empty(), "expected at least one spawner");

    let mut spawning = Spawning::new(Config::new(Duration::from_millis(500), 0x1234_5678));
    let mut commands = Vec::new();
    spawning.handle(
        &[Event::TimeAdvanced {
            dt: Duration::from_secs(2),
        }],
        PlayMode::Attack,
        &spawners,
        &mut commands,
    );

    assert_eq!(commands.len(), 4, "expected one spawn per interval");

    let expected_colors = [
        BugColor::from_rgb(0x2f, 0x95, 0x32),
        BugColor::from_rgb(0xc8, 0x2a, 0x36),
        BugColor::from_rgb(0xff, 0xc1, 0x07),
        BugColor::from_rgb(0x58, 0x47, 0xff),
    ];

    for (command, expected_color) in commands.iter().zip(expected_colors.iter().cycle()) {
        match command {
            Command::SpawnBug { color, .. } => assert_eq!(color, expected_color),
            other => panic!("unexpected command emitted: {other:?}"),
        }
    }
}

#[test]
fn builder_mode_resets_accumulator() {
    let spawners = vec![CellCoord::new(0, 0)];
    let mut spawning = Spawning::new(Config::new(Duration::from_secs(1), 0x4d59_5df4_d0f3_3173));

    let mut commands = Vec::new();
    spawning.handle(
        &[Event::TimeAdvanced {
            dt: Duration::from_millis(500),
        }],
        PlayMode::Attack,
        &spawners,
        &mut commands,
    );
    assert!(commands.is_empty(), "no spawn before full interval");

    spawning.handle(
        &[Event::PlayModeChanged {
            mode: PlayMode::Builder,
        }],
        PlayMode::Builder,
        &spawners,
        &mut commands,
    );
    assert!(commands.is_empty(), "builder mode should not spawn");

    spawning.handle(
        &[Event::PlayModeChanged {
            mode: PlayMode::Attack,
        }],
        PlayMode::Attack,
        &spawners,
        &mut commands,
    );
    assert!(commands.is_empty(), "mode changes do not spawn");

    spawning.handle(
        &[Event::TimeAdvanced {
            dt: Duration::from_millis(500),
        }],
        PlayMode::Attack,
        &spawners,
        &mut commands,
    );
    assert!(commands.is_empty(), "accumulator resets while builder");

    spawning.handle(
        &[Event::TimeAdvanced {
            dt: Duration::from_millis(500),
        }],
        PlayMode::Attack,
        &spawners,
        &mut commands,
    );
    assert_eq!(commands.len(), 1, "expected spawn after full interval");
}

#[test]
fn deterministic_replay_produces_identical_sequence() {
    let first = replay(scripted_commands());
    let second = replay(scripted_commands());

    assert_eq!(first, second, "replay diverged between runs");

    let fingerprint = first.fingerprint();
    let expected = 0x557f_3121_0503_0971;
    assert_eq!(
        fingerprint, expected,
        "fingerprint mismatch: {fingerprint:#x}",
    );
}

fn replay(commands: Vec<Command>) -> ReplayOutcome {
    let mut world = World::new();
    let mut spawning = Spawning::new(Config::new(
        Duration::from_millis(750),
        0x4d59_5df4_d0f3_3173,
    ));
    let mut log = Vec::new();

    for command in commands {
        let mut events = Vec::new();
        world::apply(&mut world, command, &mut events);
        process_spawning(&mut world, &mut spawning, events, &mut log);
    }

    let bugs = query::bug_view(&world)
        .into_vec()
        .into_iter()
        .map(BugState::from)
        .collect();

    ReplayOutcome { bugs, spawns: log }
}

fn process_spawning(
    world: &mut World,
    spawning: &mut Spawning,
    pending_events: Vec<Event>,
    log: &mut Vec<SpawnRecord>,
) {
    let mut events = pending_events;

    loop {
        if events.is_empty() {
            break;
        }

        let play_mode = query::play_mode(world);
        let spawners = query::bug_spawners(world);
        let mut commands = Vec::new();
        spawning.handle(&events, play_mode, &spawners, &mut commands);

        if commands.is_empty() {
            break;
        }

        events.clear();

        for command in commands {
            if let Command::SpawnBug { spawner, color } = command {
                log.push(SpawnRecord { spawner, color });
                let mut generated_events = Vec::new();
                world::apply(
                    world,
                    Command::SpawnBug { spawner, color },
                    &mut generated_events,
                );
                events.extend(generated_events);
            }
        }
    }
}

fn scripted_commands() -> Vec<Command> {
    vec![
        Command::ConfigureTileGrid {
            columns: TileCoord::new(6),
            rows: TileCoord::new(6),
            tile_length: 1.0,
            cells_per_tile: 1,
        },
        Command::Tick {
            dt: Duration::from_millis(500),
        },
        Command::Tick {
            dt: Duration::from_millis(500),
        },
        Command::SetPlayMode {
            mode: PlayMode::Builder,
        },
        Command::SetPlayMode {
            mode: PlayMode::Attack,
        },
        Command::Tick {
            dt: Duration::from_secs(1),
        },
        Command::Tick {
            dt: Duration::from_secs(2),
        },
    ]
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct ReplayOutcome {
    bugs: Vec<BugState>,
    spawns: Vec<SpawnRecord>,
}

impl ReplayOutcome {
    fn fingerprint(&self) -> u64 {
        let mut hasher = DefaultHasher::new();
        self.hash(&mut hasher);
        hasher.finish()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct SpawnRecord {
    spawner: CellCoord,
    color: BugColor,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct BugState {
    cell: CellCoord,
    color: BugColor,
}

impl From<BugSnapshot> for BugState {
    fn from(snapshot: BugSnapshot) -> Self {
        Self {
            cell: snapshot.cell,
            color: snapshot.color,
        }
    }
}
