use std::time::Duration;

use maze_defence_core::{
    AttackPlan, BurstPlan, Command, Event, PlayMode, Pressure, SpawnPatchId, SpeciesId,
    WaveDifficulty,
};
use maze_defence_system_spawning::Spawning;
use maze_defence_world::{self as world, query, World};

#[test]
fn spawning_consumes_cached_plan_and_emits_burst_events() {
    let mut world = World::new();
    let mut events = Vec::new();
    world::apply(
        &mut world,
        Command::ConfigureTileGrid {
            columns: maze_defence_core::TileCoord::new(4),
            rows: maze_defence_core::TileCoord::new(4),
            tile_length: 1.0,
            cells_per_tile: 1,
        },
        &mut events,
    );
    events.clear();

    let wave = query::wave_seed_context(&world).wave();
    let species_table = query::species_table(&world);
    let plan = AttackPlan::new(
        Pressure::new(900),
        species_table.version(),
        vec![BurstPlan::new(
            SpeciesId::new(0),
            SpawnPatchId::new(0),
            std::num::NonZeroU32::new(3).unwrap(),
            std::num::NonZeroU32::new(100).unwrap(),
            0,
        )],
    );

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
        Command::CacheAttackPlan {
            wave,
            difficulty: WaveDifficulty::Normal,
            plan: plan.clone(),
        },
        &mut events,
    );
    events.clear();

    world::apply(
        &mut world,
        Command::StartWave {
            wave,
            difficulty: WaveDifficulty::Normal,
        },
        &mut events,
    );

    let mut spawning = Spawning::new();
    let expected_spawns: u32 = plan.bursts().iter().map(|burst| burst.count().get()).sum();
    let expected_bursts = plan.bursts().len();

    let mut pending_events = events;
    let mut system_events = Vec::new();
    let mut commands = Vec::new();
    let mut burst_events = Vec::new();
    let mut spawned = 0u32;

    for _ in 0..16 {
        let play_mode = query::play_mode(&world);
        let species_table = query::species_table(&world);
        let patch_table = query::patch_table(&world);
        let pressure_config = query::pressure_config(&world);

        spawning.handle(
            &pending_events,
            play_mode,
            species_table,
            patch_table,
            pressure_config,
            |wave_id| query::attack_plan(&world, wave_id).cloned(),
            &mut commands,
            &mut system_events,
        );

        let mut next_events = Vec::new();

        for event in system_events.drain(..) {
            if let Event::BurstDepleted { .. } = &event {
                burst_events.push(event.clone());
            }
            next_events.push(event);
        }

        let mut world_events = Vec::new();
        for command in commands.drain(..) {
            spawned += 1;
            world::apply(&mut world, command, &mut world_events);
        }
        next_events.extend(world_events);

        if spawned >= expected_spawns {
            break;
        }

        let mut tick_events = Vec::new();
        world::apply(
            &mut world,
            Command::Tick {
                dt: Duration::from_millis(100),
            },
            &mut tick_events,
        );
        next_events.extend(tick_events);

        pending_events = next_events;
    }

    assert_eq!(spawned, expected_spawns);
    assert_eq!(burst_events.len(), expected_bursts);

    let Event::BurstDepleted {
        wave: event_wave,
        species,
        patch,
    } = &burst_events[0]
    else {
        panic!("expected burst depleted event");
    };
    assert_eq!(*event_wave, wave);
    assert_eq!(*species, SpeciesId::new(0));
    assert_eq!(*patch, SpawnPatchId::new(0));
}
