use std::time::Duration;

use maze_defence_core::{BugColor, CellCoord, Command, Direction, Event, TileCoord};
use maze_defence_system_movement::Movement;
use maze_defence_world::{self as world, query, World};

#[test]
fn emits_step_commands_toward_target() {
    let mut world = World::new();
    let mut events = Vec::new();
    world::apply(
        &mut world,
        Command::ConfigureTileGrid {
            columns: TileCoord::new(5),
            rows: TileCoord::new(4),
            tile_length: 1.0,
            cells_per_tile: 1,
        },
        &mut events,
    );

    spawn_bug(&mut world, &mut events, CellCoord::new(0, 0));
    spawn_bug(&mut world, &mut events, CellCoord::new(4, 3));

    let mut movement = Movement::default();
    pump_system(&mut world, &mut movement, events);

    let mut tick_events = Vec::new();
    world::apply(
        &mut world,
        Command::Tick {
            dt: Duration::from_millis(250),
        },
        &mut tick_events,
    );

    let bug_view = query::bug_view(&world);
    let occupancy_view = query::occupancy_view(&world);
    let target_cells = query::target_cells(&world);
    let mut commands = Vec::new();
    movement.handle(
        &tick_events,
        &bug_view,
        occupancy_view,
        &target_cells,
        &mut commands,
    );

    let step_commands: Vec<_> = commands
        .iter()
        .filter_map(|command| match command {
            Command::StepBug { bug_id, direction } => Some((bug_id, direction)),
            _ => None,
        })
        .collect();
    assert!(
        !step_commands.is_empty(),
        "expected movement system to emit step commands"
    );

    for (bug_id, direction) in step_commands {
        let bug = bug_view
            .iter()
            .find(|snapshot| &snapshot.id == bug_id)
            .expect("missing bug snapshot");
        let goal = query::goal_for(&world, bug.cell).expect("expected goal for bug");
        let goal_cell = goal.cell();
        let before = bug.cell.manhattan_distance(goal_cell);
        let destination = advance_cell(bug.cell, *direction);
        let after = destination.manhattan_distance(goal_cell);
        assert!(
            after < before,
            "bug {} did not move closer to the target",
            bug.id.get()
        );
    }
}

#[test]
fn step_commands_target_free_cells() {
    let mut world = World::new();
    let mut events = Vec::new();
    world::apply(
        &mut world,
        Command::ConfigureTileGrid {
            columns: TileCoord::new(5),
            rows: TileCoord::new(4),
            tile_length: 1.0,
            cells_per_tile: 1,
        },
        &mut events,
    );

    spawn_bug(&mut world, &mut events, CellCoord::new(0, 0));
    spawn_bug(&mut world, &mut events, CellCoord::new(4, 3));

    let mut movement = Movement::default();
    pump_system(&mut world, &mut movement, events);

    let mut tick_events = Vec::new();
    world::apply(
        &mut world,
        Command::Tick {
            dt: Duration::from_millis(250),
        },
        &mut tick_events,
    );

    let bug_view = query::bug_view(&world);
    let occupancy_view = query::occupancy_view(&world);
    let mut commands = Vec::new();
    let target_cells = query::target_cells(&world);
    movement.handle(
        &tick_events,
        &bug_view,
        occupancy_view,
        &target_cells,
        &mut commands,
    );

    for command in &commands {
        if let Command::StepBug { bug_id, direction } = command {
            let bug = bug_view
                .iter()
                .find(|snapshot| &snapshot.id == bug_id)
                .unwrap();
            let target = advance_cell(bug.cell, *direction);
            assert!(occupancy_view.is_free(target));
        }
    }

    let mut follow_up_events = Vec::new();
    for command in commands {
        world::apply(&mut world, command, &mut follow_up_events);
    }
    pump_system(&mut world, &mut movement, follow_up_events);
}

#[test]
fn replans_after_failed_step() {
    let mut world = World::new();
    let mut events = Vec::new();
    world::apply(
        &mut world,
        Command::ConfigureTileGrid {
            columns: TileCoord::new(3),
            rows: TileCoord::new(3),
            tile_length: 1.0,
            cells_per_tile: 1,
        },
        &mut events,
    );

    spawn_bug(&mut world, &mut events, CellCoord::new(0, 0));
    spawn_bug(&mut world, &mut events, CellCoord::new(2, 0));

    let mut movement = Movement::default();
    pump_system(&mut world, &mut movement, events);

    let mut tick_events = Vec::new();
    world::apply(
        &mut world,
        Command::Tick {
            dt: Duration::from_millis(250),
        },
        &mut tick_events,
    );

    let target_cells = query::target_cells(&world);
    let target_columns: Vec<u32> = target_cells.iter().map(|cell| cell.column()).collect();
    let bug_view = query::bug_view(&world);
    let occupancy_view_initial = query::occupancy_view(&world);
    let (columns, rows) = occupancy_view_initial.dimensions();
    let (bug_id, blocked_direction) = select_blocked_bug(
        &bug_view,
        occupancy_view_initial,
        columns,
        rows,
        &target_columns,
    )
    .expect("expected at least one bug on a boundary");

    let mut bad_step_events = Vec::new();
    world::apply(
        &mut world,
        Command::StepBug {
            bug_id,
            direction: blocked_direction,
        },
        &mut bad_step_events,
    );
    assert!(bad_step_events.is_empty());

    let bug_view_after_failure = query::bug_view(&world);
    let occupancy_view = query::occupancy_view(&world);
    let mut commands = Vec::new();
    movement.handle(
        &tick_events,
        &bug_view_after_failure,
        occupancy_view,
        &target_cells,
        &mut commands,
    );

    let replanned_direction = commands.iter().find_map(|command| match command {
        Command::StepBug {
            bug_id: step_id,
            direction,
        } if step_id == &bug_id => Some(*direction),
        _ => None,
    });

    assert!(
        matches!(replanned_direction, Some(direction) if direction != blocked_direction),
        "expected a new direction different from the blocked move"
    );
}

fn spawn_bug(world: &mut World, events: &mut Vec<Event>, cell: CellCoord) {
    let before_len = events.len();
    world::apply(
        world,
        Command::SpawnBug {
            spawner: cell,
            color: BugColor::from_rgb(0x88, 0x44, 0xbb),
        },
        events,
    );
    assert!(
        events.len() > before_len && matches!(events.last(), Some(Event::BugSpawned { .. })),
        "expected spawn event at {:?}",
        cell
    );
}

fn pump_system(world: &mut World, movement: &mut Movement, mut events: Vec<Event>) {
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
            world::apply(world, command, &mut events);
        }
    }
}

fn advance_cell(cell: CellCoord, direction: Direction) -> CellCoord {
    match direction {
        Direction::North => CellCoord::new(cell.column(), cell.row().saturating_sub(1)),
        Direction::East => CellCoord::new(cell.column() + 1, cell.row()),
        Direction::South => CellCoord::new(cell.column(), cell.row() + 1),
        Direction::West => CellCoord::new(cell.column().saturating_sub(1), cell.row()),
    }
}

fn select_blocked_bug(
    bug_view: &query::BugView,
    occupancy_view: query::OccupancyView<'_>,
    columns: u32,
    rows: u32,
    target_columns: &[u32],
) -> Option<(maze_defence_core::BugId, Direction)> {
    for bug in bug_view.iter() {
        let column = bug.cell.column();
        let row = bug.cell.row();

        if column + 1 >= columns {
            if column > 0 {
                let west = CellCoord::new(column - 1, row);
                if occupancy_view.is_free(west) {
                    return Some((bug.id, Direction::East));
                }
            }
        }

        if column == 0 {
            let east = CellCoord::new(column + 1, row);
            if occupancy_view.is_free(east) {
                return Some((bug.id, Direction::West));
            }
        }

        if row == 0 {
            let south = CellCoord::new(column, row + 1);
            if occupancy_view.is_free(south) {
                return Some((bug.id, Direction::North));
            }
        }

        if row + 1 == rows && !target_columns.contains(&column) {
            if column > 0 {
                let west = CellCoord::new(column - 1, row);
                if occupancy_view.is_free(west) {
                    return Some((bug.id, Direction::South));
                }
            }
            if column + 1 < columns {
                let east = CellCoord::new(column + 1, row);
                if occupancy_view.is_free(east) {
                    return Some((bug.id, Direction::South));
                }
            }
        }
    }

    None
}
