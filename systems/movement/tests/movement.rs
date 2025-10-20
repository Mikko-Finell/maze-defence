use std::time::Duration;

use maze_defence_core::{BugId, CellCoord, Command, Direction, Event, TileCoord};
use maze_defence_system_movement::Movement;
use maze_defence_world::{self as world, query, World};

#[test]
fn assigns_paths_for_all_bugs() {
    let mut world = World::new();
    let mut events = Vec::new();
    world::apply(
        &mut world,
        Command::ConfigureTileGrid {
            columns: TileCoord::new(5),
            rows: TileCoord::new(4),
            tile_length: 1.0,
        },
        &mut events,
    );

    let mut movement = Movement::default();
    pump_system(&mut world, &mut movement, events);

    let bug_view = query::bug_view(&world);
    for bug in bug_view.iter() {
        if bug.cell.row() == 3 {
            continue;
        }
        assert!(bug.next_hop.is_some(), "bug {} missing path", bug.id.get());
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
        },
        &mut events,
    );

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
fn replans_when_world_requests_new_path() {
    let mut world = World::new();
    let mut events = Vec::new();
    world::apply(
        &mut world,
        Command::ConfigureTileGrid {
            columns: TileCoord::new(3),
            rows: TileCoord::new(3),
            tile_length: 1.0,
        },
        &mut events,
    );

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

    let bug_id = BugId::new(0);
    let mut bad_step_events = Vec::new();
    world::apply(
        &mut world,
        Command::StepBug {
            bug_id,
            direction: Direction::East,
        },
        &mut bad_step_events,
    );

    let bug_view_after_failure = query::bug_view(&world);
    let failed_bug = bug_view_after_failure
        .iter()
        .find(|bug| bug.id == bug_id)
        .expect("bug missing after failed step");
    assert!(failed_bug.needs_path);

    tick_events.extend(bad_step_events);
    pump_system(&mut world, &mut movement, tick_events);

    let post_replan_view = query::bug_view(&world);
    let replanned_bug = post_replan_view
        .iter()
        .find(|bug| bug.id == bug_id)
        .expect("bug missing after replanning");
    assert!(replanned_bug.next_hop.is_some());
    assert!(!replanned_bug.needs_path);
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
