use std::time::Duration;

use maze_defence_core::{
    BugColor, BugId, BugView, CellCoord, Command, Direction, Event, Health, OccupancyView,
    PlayMode, TileCoord, TowerKind,
};
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
    world::apply(
        &mut world,
        Command::SpawnBug {
            spawner: CellCoord::new(0, 0),
            color: BugColor::from_rgb(0x2f, 0x95, 0x32),
            health: Health::new(3),
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
    let target_cells = query::target_cells(&world);
    let navigation_view = query::navigation_field(&world);
    let reservation_ledger = query::reservation_ledger(&world);
    let mut commands = Vec::new();
    movement.handle(
        &tick_events,
        &bug_view,
        occupancy_view,
        navigation_view,
        reservation_ledger,
        &target_cells,
        |cell| query::is_cell_blocked(&world, cell),
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
fn bugs_progress_despite_distant_blockers() {
    let mut world = World::new();
    let mut events = Vec::new();
    world::apply(
        &mut world,
        Command::ConfigureTileGrid {
            columns: TileCoord::new(1),
            rows: TileCoord::new(1),
            tile_length: 1.0,
            cells_per_tile: 2,
        },
        &mut events,
    );

    let mut movement = Movement::default();
    pump_system(&mut world, &mut movement, events);

    apply_and_pump(
        &mut world,
        &mut movement,
        Command::SpawnBug {
            spawner: CellCoord::new(0, 0),
            color: BugColor::from_rgb(0x2f, 0x95, 0x32),
            health: Health::new(3),
        },
    );

    let step = Duration::from_millis(250);
    drive_tick_and_collect(&mut world, &mut movement, step);
    drive_tick_and_collect(&mut world, &mut movement, step);

    apply_and_pump(
        &mut world,
        &mut movement,
        Command::SpawnBug {
            spawner: CellCoord::new(0, 0),
            color: BugColor::from_rgb(0x2f, 0x95, 0x32),
            health: Health::new(3),
        },
    );

    let mut tick_events = Vec::new();
    world::apply(&mut world, Command::Tick { dt: step }, &mut tick_events);

    let bug_view = query::bug_view(&world);
    let front_bug = bug_view
        .iter()
        .find(|bug| bug.id == BugId::new(0))
        .expect("front bug missing");
    let trailing_bug = bug_view
        .iter()
        .find(|bug| bug.id == BugId::new(1))
        .expect("trailing bug missing");
    assert!(
        front_bug.cell.row() >= trailing_bug.cell.row() + 2,
        "expected at least one empty cell between bugs"
    );

    let occupancy_view = query::occupancy_view(&world);
    let targets = query::target_cells(&world);
    let navigation_view = query::navigation_field(&world);
    let reservation_ledger = query::reservation_ledger(&world);
    let mut commands = Vec::new();
    movement.handle(
        &tick_events,
        &bug_view,
        occupancy_view,
        navigation_view,
        reservation_ledger,
        &targets,
        |cell| query::is_cell_blocked(&world, cell),
        &mut commands,
    );

    let trailing_step = commands.iter().find(|command| {
        matches!(
            command,
            Command::StepBug {
                bug_id,
                direction: Direction::South,
            } if *bug_id == BugId::new(1)
        )
    });

    assert!(
        trailing_step.is_some(),
        "expected bug behind the blocker to advance toward the exit"
    );
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
    world::apply(
        &mut world,
        Command::SpawnBug {
            spawner: CellCoord::new(0, 0),
            color: BugColor::from_rgb(0x2f, 0x95, 0x32),
            health: Health::new(3),
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
    let navigation_view = query::navigation_field(&world);
    let reservation_ledger = query::reservation_ledger(&world);
    movement.handle(
        &tick_events,
        &bug_view,
        occupancy_view,
        navigation_view,
        reservation_ledger,
        &target_cells,
        |cell| query::is_cell_blocked(&world, cell),
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
fn emits_step_commands_in_bug_id_order() {
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
    world::apply(
        &mut world,
        Command::SpawnBug {
            spawner: CellCoord::new(0, 0),
            color: BugColor::from_rgb(0x2f, 0x95, 0x32),
            health: Health::new(3),
        },
        &mut events,
    );
    world::apply(
        &mut world,
        Command::SpawnBug {
            spawner: CellCoord::new(4, 0),
            color: BugColor::from_rgb(0x2f, 0x95, 0x32),
            health: Health::new(3),
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
    let target_cells = query::target_cells(&world);
    let navigation_view = query::navigation_field(&world);
    let reservation_ledger = query::reservation_ledger(&world);
    let mut commands = Vec::new();
    movement.handle(
        &tick_events,
        &bug_view,
        occupancy_view,
        navigation_view,
        reservation_ledger,
        &target_cells,
        |cell| query::is_cell_blocked(&world, cell),
        &mut commands,
    );

    let mut emitted: Vec<BugId> = commands
        .iter()
        .filter_map(|command| match command {
            Command::StepBug { bug_id, .. } => Some(*bug_id),
            _ => None,
        })
        .collect();

    emitted.dedup();
    let mut sorted = emitted.clone();
    sorted.sort_by_key(|bug_id| bug_id.get());
    assert_eq!(emitted, sorted, "step commands must be ordered by BugId");
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
    world::apply(
        &mut world,
        Command::SpawnBug {
            spawner: CellCoord::new(0, 0),
            color: BugColor::from_rgb(0x2f, 0x95, 0x32),
            health: Health::new(3),
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

    let target_cells = query::target_cells(&world);
    let bug_view = query::bug_view(&world);
    let occupancy_view_initial = query::occupancy_view(&world);
    let (columns, rows) = occupancy_view_initial.dimensions();
    let (bug_id, blocked_direction) =
        select_blocked_bug(&bug_view, occupancy_view_initial, columns, rows)
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
    let navigation_view = query::navigation_field(&world);
    let reservation_ledger = query::reservation_ledger(&world);
    movement.handle(
        &tick_events,
        &bug_view_after_failure,
        occupancy_view,
        navigation_view,
        reservation_ledger,
        &target_cells,
        |cell| query::is_cell_blocked(&world, cell),
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

#[test]
fn bugs_respect_tower_blockers() {
    let mut world = World::new();
    let mut events = Vec::new();
    world::apply(
        &mut world,
        Command::ConfigureTileGrid {
            columns: TileCoord::new(3),
            rows: TileCoord::new(3),
            tile_length: 1.0,
            cells_per_tile: 2,
        },
        &mut events,
    );

    let mut movement = Movement::default();
    pump_system(&mut world, &mut movement, events);

    let mut builder_events = Vec::new();
    world::apply(
        &mut world,
        Command::SetPlayMode {
            mode: PlayMode::Builder,
        },
        &mut builder_events,
    );
    pump_system(&mut world, &mut movement, builder_events);

    let target_cells = query::target_cells(&world);
    let target_cell = target_cells
        .first()
        .copied()
        .expect("expected at least one target cell");
    let spawn = CellCoord::new(target_cell.column(), 0);
    let blocked_cell = CellCoord::new(target_cell.column(), 1);
    let tower_origin = CellCoord::new(target_cell.column(), 1);

    let mut tower_events = Vec::new();
    world::apply(
        &mut world,
        Command::PlaceTower {
            kind: TowerKind::Basic,
            origin: tower_origin,
        },
        &mut tower_events,
    );
    assert!(
        tower_events
            .iter()
            .any(|event| matches!(event, Event::TowerPlaced { .. })),
        "expected tower placement to succeed"
    );

    let mut attack_events = Vec::new();
    world::apply(
        &mut world,
        Command::SetPlayMode {
            mode: PlayMode::Attack,
        },
        &mut attack_events,
    );
    pump_system(&mut world, &mut movement, attack_events);

    let mut spawn_events = Vec::new();
    world::apply(
        &mut world,
        Command::SpawnBug {
            spawner: spawn,
            color: BugColor::from_rgb(0x2f, 0x95, 0x32),
            health: Health::new(3),
        },
        &mut spawn_events,
    );
    assert!(
        spawn_events
            .iter()
            .any(|event| matches!(event, Event::BugSpawned { .. })),
        "bug spawn request must succeed"
    );

    let mut tick_events = Vec::new();
    world::apply(
        &mut world,
        Command::Tick {
            dt: Duration::from_millis(250),
        },
        &mut tick_events,
    );

    let mut frame_events = spawn_events;
    frame_events.extend(tick_events);

    assert!(
        query::is_cell_blocked(&world, blocked_cell),
        "tower footprint must be treated as blocked"
    );

    let bug_view = query::bug_view(&world);
    let occupancy_view = query::occupancy_view(&world);
    let target_cells = query::target_cells(&world);
    let navigation_view = query::navigation_field(&world);
    let reservation_ledger = query::reservation_ledger(&world);
    let mut commands = Vec::new();
    movement.handle(
        &frame_events,
        &bug_view,
        occupancy_view,
        navigation_view,
        reservation_ledger,
        &target_cells,
        |cell| query::is_cell_blocked(&world, cell),
        &mut commands,
    );

    for command in &commands {
        if let Command::StepBug { bug_id, direction } = command {
            let bug = bug_view
                .iter()
                .find(|snapshot| &snapshot.id == bug_id)
                .expect("bug snapshot present");
            let destination = advance_cell(bug.cell, *direction);
            assert_ne!(
                destination, blocked_cell,
                "movement should not direct bugs into tower cells"
            );
            assert!(
                !query::is_cell_blocked(&world, destination),
                "movement must avoid blocked cells"
            );
        }
    }
}

#[test]
fn blocked_bugs_do_not_accumulate_extra_step_time() {
    fn drive_tick(
        world_state: &mut World,
        movement: &mut Movement,
        dt: Duration,
        bug_id: BugId,
        apply_steps: bool,
    ) -> usize {
        let mut tick_events = Vec::new();
        world::apply(world_state, Command::Tick { dt }, &mut tick_events);
        let mut pending_events = tick_events;
        let mut iteration = 0;
        let mut step_commands = 0;

        loop {
            if pending_events.is_empty() {
                break;
            }

            let bug_view = query::bug_view(world_state);
            let occupancy_view = query::occupancy_view(world_state);
            let targets = query::target_cells(world_state);
            let navigation_view = query::navigation_field(world_state);
            let reservation_ledger = query::reservation_ledger(world_state);
            let mut commands = Vec::new();
            movement.handle(
                &pending_events,
                &bug_view,
                occupancy_view,
                navigation_view,
                reservation_ledger,
                &targets,
                |cell| query::is_cell_blocked(world_state, cell),
                &mut commands,
            );

            if iteration == 0 {
                step_commands = commands
                    .iter()
                    .filter(|command| {
                        matches!(
                            command,
                            Command::StepBug { bug_id: id, .. } if *id == bug_id
                        )
                    })
                    .count();
            }

            if commands.is_empty() {
                break;
            }

            pending_events.clear();
            for command in commands {
                let should_skip = !apply_steps
                    && matches!(
                        command,
                        Command::StepBug { bug_id: id, .. } if id == bug_id
                    );
                if should_skip {
                    continue;
                }
                world::apply(world_state, command, &mut pending_events);
            }

            iteration += 1;
        }

        step_commands
    }

    let mut world = World::new();
    let mut movement = Movement::default();
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
    world::apply(
        &mut world,
        Command::SpawnBug {
            spawner: CellCoord::new(0, 0),
            color: BugColor::from_rgb(0x2f, 0x95, 0x32),
            health: Health::new(3),
        },
        &mut events,
    );

    pump_system(&mut world, &mut movement, events);

    let bug_id = query::bug_view(&world)
        .into_vec()
        .into_iter()
        .next()
        .map(|snapshot| snapshot.id)
        .expect("bug must be present");

    let step_quantum = Duration::from_millis(250);

    for _ in 0..3 {
        let _ = drive_tick(&mut world, &mut movement, step_quantum, bug_id, false);
        let bug_snapshot = query::bug_view(&world)
            .into_vec()
            .into_iter()
            .find(|bug| bug.id == bug_id)
            .expect("bug should remain while blocked");
        assert_eq!(
            bug_snapshot.accumulated, step_quantum,
            "blocked bug must saturate the accumulator",
        );
        assert!(
            bug_snapshot.ready_for_step,
            "blocked bug should stay ready to advance",
        );
    }

    let step_commands_after_unblock =
        drive_tick(&mut world, &mut movement, step_quantum, bug_id, true);
    assert_eq!(
        step_commands_after_unblock, 1,
        "bug should advance exactly once after unblocking",
    );

    let bug_snapshot = query::bug_view(&world)
        .into_vec()
        .into_iter()
        .find(|bug| bug.id == bug_id)
        .expect("bug should remain after advancing");
    assert!(
        bug_snapshot.accumulated < step_quantum,
        "bug must not retain more than one quantum",
    );
    assert!(
        !bug_snapshot.ready_for_step,
        "bug should wait for a new quantum before the next step",
    );
}

#[test]
fn bug_reaches_exit_when_central_tower_blocks_direct_route() {
    let mut world = World::new();
    let mut movement = Movement::default();

    apply_and_pump(
        &mut world,
        &mut movement,
        Command::ConfigureTileGrid {
            columns: TileCoord::new(6),
            rows: TileCoord::new(6),
            tile_length: 1.0,
            cells_per_tile: 1,
        },
    );

    apply_and_pump(
        &mut world,
        &mut movement,
        Command::SetPlayMode {
            mode: PlayMode::Builder,
        },
    );

    apply_and_pump(
        &mut world,
        &mut movement,
        Command::PlaceTower {
            kind: TowerKind::Basic,
            origin: CellCoord::new(2, 2),
        },
    );

    apply_and_pump(
        &mut world,
        &mut movement,
        Command::SetPlayMode {
            mode: PlayMode::Attack,
        },
    );

    apply_and_pump(
        &mut world,
        &mut movement,
        Command::SpawnBug {
            spawner: CellCoord::new(3, 0),
            color: BugColor::from_rgb(0x2f, 0x95, 0x32),
            health: Health::new(1000),
        },
    );

    let mut event_log = Vec::new();
    let mut exited = false;

    for _ in 0..200 {
        let tick_events =
            drive_tick_and_collect(&mut world, &mut movement, Duration::from_millis(250));
        if tick_events.is_empty() {
            continue;
        }
        if tick_events
            .iter()
            .any(|event| matches!(event, Event::BugExited { .. }))
        {
            exited = true;
        }
        event_log.extend(tick_events);
        if exited {
            break;
        }
    }

    assert!(exited, "expected bug to reach the exit despite the detour");
    assert!(
        event_log.iter().any(|event| {
            matches!(
                event,
                Event::BugAdvanced { from, to, .. }
                    if from.column() != to.column()
            )
        }),
        "expected bug to move sideways while navigating around the tower",
    );
}

#[test]
fn bug_reaches_exit_when_two_towers_force_wide_detour() {
    let mut world = World::new();
    let mut movement = Movement::default();

    apply_and_pump(
        &mut world,
        &mut movement,
        Command::ConfigureTileGrid {
            columns: TileCoord::new(8),
            rows: TileCoord::new(6),
            tile_length: 1.0,
            cells_per_tile: 1,
        },
    );

    apply_and_pump(
        &mut world,
        &mut movement,
        Command::SetPlayMode {
            mode: PlayMode::Builder,
        },
    );

    apply_and_pump(
        &mut world,
        &mut movement,
        Command::PlaceTower {
            kind: TowerKind::Basic,
            origin: CellCoord::new(2, 2),
        },
    );

    apply_and_pump(
        &mut world,
        &mut movement,
        Command::PlaceTower {
            kind: TowerKind::Basic,
            origin: CellCoord::new(6, 3),
        },
    );

    apply_and_pump(
        &mut world,
        &mut movement,
        Command::SetPlayMode {
            mode: PlayMode::Attack,
        },
    );

    apply_and_pump(
        &mut world,
        &mut movement,
        Command::SpawnBug {
            spawner: CellCoord::new(4, 0),
            color: BugColor::from_rgb(0x2f, 0x95, 0x32),
            health: Health::new(1000),
        },
    );

    let mut event_log = Vec::new();
    let mut exited = false;

    for _ in 0..240 {
        let tick_events =
            drive_tick_and_collect(&mut world, &mut movement, Duration::from_millis(250));
        if tick_events.is_empty() {
            continue;
        }
        if tick_events
            .iter()
            .any(|event| matches!(event, Event::BugExited { .. }))
        {
            exited = true;
        }
        event_log.extend(tick_events);
        if exited {
            break;
        }
    }

    assert!(
        exited,
        "expected bug to reach the exit after navigating both towers",
    );
    assert!(
        event_log.iter().any(|event| {
            matches!(
                event,
                Event::BugAdvanced { from, to, .. }
                    if from.column() != to.column()
            )
        }),
        "expected bug to travel sideways while searching for a route",
    );
}

#[test]
fn corner_spawn_bug_reaches_exit_through_chicane() {
    let mut world = World::new();
    let mut movement = Movement::default();

    apply_and_pump(
        &mut world,
        &mut movement,
        Command::ConfigureTileGrid {
            columns: TileCoord::new(8),
            rows: TileCoord::new(6),
            tile_length: 1.0,
            cells_per_tile: 1,
        },
    );

    apply_and_pump(
        &mut world,
        &mut movement,
        Command::SetPlayMode {
            mode: PlayMode::Builder,
        },
    );

    apply_and_pump(
        &mut world,
        &mut movement,
        Command::PlaceTower {
            kind: TowerKind::Basic,
            origin: CellCoord::new(0, 2),
        },
    );

    apply_and_pump(
        &mut world,
        &mut movement,
        Command::PlaceTower {
            kind: TowerKind::Basic,
            origin: CellCoord::new(4, 2),
        },
    );

    apply_and_pump(
        &mut world,
        &mut movement,
        Command::SetPlayMode {
            mode: PlayMode::Attack,
        },
    );

    apply_and_pump(
        &mut world,
        &mut movement,
        Command::SpawnBug {
            spawner: CellCoord::new(1, 0),
            color: BugColor::from_rgb(0x2f, 0x95, 0x32),
            health: Health::new(1000),
        },
    );

    let mut event_log = Vec::new();
    let mut exited = false;

    for _ in 0..240 {
        let tick_events =
            drive_tick_and_collect(&mut world, &mut movement, Duration::from_millis(250));
        if tick_events.is_empty() {
            continue;
        }
        if tick_events
            .iter()
            .any(|event| matches!(event, Event::BugExited { .. }))
        {
            exited = true;
        }
        event_log.extend(tick_events);
        if exited {
            break;
        }
    }

    assert!(
        exited,
        "expected bug to reach the exit after navigating the chicane",
    );
    assert!(
        event_log.iter().any(|event| {
            matches!(
                event,
                Event::BugAdvanced { from, to, .. }
                    if from.column() != to.column()
            )
        }),
        "expected bug to weave sideways before leaving the maze",
    );
}

fn apply_and_pump(world: &mut World, movement: &mut Movement, command: Command) {
    let mut events = Vec::new();
    world::apply(world, command, &mut events);
    pump_system(world, movement, events);
}

fn drive_tick_and_collect(world: &mut World, movement: &mut Movement, dt: Duration) -> Vec<Event> {
    let mut pending = Vec::new();
    world::apply(world, Command::Tick { dt }, &mut pending);
    let mut emitted = pending.clone();

    loop {
        if pending.is_empty() {
            break;
        }

        let bug_view = query::bug_view(world);
        let occupancy_view = query::occupancy_view(world);
        let targets = query::target_cells(world);
        let navigation_view = query::navigation_field(world);
        let reservation_ledger = query::reservation_ledger(world);
        let mut commands = Vec::new();
        movement.handle(
            &pending,
            &bug_view,
            occupancy_view,
            navigation_view,
            reservation_ledger,
            &targets,
            |cell| query::is_cell_blocked(&*world, cell),
            &mut commands,
        );

        if commands.is_empty() {
            break;
        }

        pending.clear();
        for command in commands {
            let mut generated = Vec::new();
            world::apply(world, command, &mut generated);
            if !generated.is_empty() {
                emitted.extend(generated.iter().cloned());
                pending.extend(generated);
            }
        }
    }

    emitted
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
    bug_view: &BugView,
    occupancy_view: OccupancyView<'_>,
    columns: u32,
    rows: u32,
) -> Option<(maze_defence_core::BugId, Direction)> {
    for bug in bug_view.iter() {
        let column = bug.cell.column();
        let row = bug.cell.row();

        if column + 1 >= columns && column > 0 {
            let west = CellCoord::new(column - 1, row);
            if occupancy_view.is_free(west) {
                return Some((bug.id, Direction::East));
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

        if row.saturating_add(1) >= rows {
            return Some((bug.id, Direction::South));
        }
    }

    None
}
