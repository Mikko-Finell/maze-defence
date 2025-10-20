use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
    time::Duration,
};

use maze_defence_core::{CellCoord, Command, Event, TileCoord};
use maze_defence_system_movement::Movement;
use maze_defence_world::{self as world, query, World};

#[test]
fn deterministic_replay_produces_expected_snapshot() {
    let first = replay(scripted_commands());
    let second = replay(scripted_commands());

    assert_eq!(first, second, "replay diverged between runs");

    let fingerprint = first.fingerprint();
    let expected = 0xc9d3_068b_fa07_df8b;
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

    ReplayOutcome { bugs, events: log }
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
        movement.handle(
            &events,
            &bug_view,
            occupancy_view,
            &target_cells,
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

fn scripted_commands() -> Vec<Command> {
    vec![
        Command::ConfigureTileGrid {
            columns: TileCoord::new(5),
            rows: TileCoord::new(4),
            cells_per_tile: 4,
            tile_length: 1.0,
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

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct ReplayOutcome {
    bugs: Vec<BugState>,
    events: Vec<EventRecord>,
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
    ready_for_step: bool,
    accumulated_micros: u128,
    color: (u8, u8, u8),
}

impl From<query::BugSnapshot> for BugState {
    fn from(snapshot: query::BugSnapshot) -> Self {
        Self {
            id: snapshot.id,
            cell: snapshot.cell,
            ready_for_step: snapshot.ready_for_step,
            accumulated_micros: snapshot.accumulated.as_micros(),
            color: (
                snapshot.color.red(),
                snapshot.color.green(),
                snapshot.color.blue(),
            ),
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
        }
    }
}
