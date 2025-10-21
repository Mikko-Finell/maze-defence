use maze_defence_core::{
    CellCoord, CellRect, CellRectSize, Command, Event, PlayMode, TowerId, TowerKind,
};
use maze_defence_system_builder::{Builder, BuilderInput, PlacementPreview};

fn basic_preview_at(cell: CellCoord, placeable: bool) -> PlacementPreview {
    PlacementPreview::new(
        TowerKind::Basic,
        cell,
        CellRect::from_origin_and_size(cell, CellRectSize::new(2, 2)),
        placeable,
        None,
    )
}

#[test]
fn confirm_emits_place_command_in_builder_mode() {
    let mut builder = Builder::default();
    let mut commands = Vec::new();

    builder.handle(
        &[Event::PlayModeChanged {
            mode: PlayMode::Builder,
        }],
        Some(basic_preview_at(CellCoord::new(2, 2), true)),
        BuilderInput {
            confirm_action: true,
            ..BuilderInput::default()
        },
        |_| None,
        &mut commands,
    );

    assert_eq!(
        commands,
        vec![Command::PlaceTower {
            kind: TowerKind::Basic,
            origin: CellCoord::new(2, 2),
        }],
        "builder should emit a placement command when confirming a valid preview",
    );
}

#[test]
fn confirm_ignored_when_preview_not_placeable() {
    let mut builder = Builder::default();
    let mut commands = Vec::new();

    builder.handle(
        &[Event::PlayModeChanged {
            mode: PlayMode::Builder,
        }],
        Some(basic_preview_at(CellCoord::new(2, 2), false)),
        BuilderInput {
            confirm_action: true,
            ..BuilderInput::default()
        },
        |_| None,
        &mut commands,
    );

    assert!(
        commands.is_empty(),
        "invalid preview must not emit commands"
    );
}

#[test]
fn confirm_ignored_in_attack_mode() {
    let mut builder = Builder::default();
    let mut commands = Vec::new();

    builder.handle(
        &[],
        Some(basic_preview_at(CellCoord::new(2, 2), true)),
        BuilderInput {
            confirm_action: true,
            ..BuilderInput::default()
        },
        |_| None,
        &mut commands,
    );

    assert!(
        commands.is_empty(),
        "system must not emit commands outside builder mode",
    );
}

#[test]
fn remove_emits_command_when_tower_present() {
    let mut builder = Builder::default();
    let mut commands = Vec::new();
    let hovered_cell = CellCoord::new(2, 2);
    let returned_tower = TowerId::new(7);
    let mut looked_up = None;

    builder.handle(
        &[Event::PlayModeChanged {
            mode: PlayMode::Builder,
        }],
        None,
        BuilderInput {
            remove_action: true,
            cursor_cell: Some(hovered_cell),
            ..BuilderInput::default()
        },
        |cell| {
            looked_up = Some(cell);
            Some(returned_tower)
        },
        &mut commands,
    );

    assert_eq!(looked_up, Some(hovered_cell));
    assert_eq!(
        commands,
        vec![Command::RemoveTower {
            tower: returned_tower,
        }],
        "remove action should target the tower under the cursor",
    );
}

#[test]
fn remove_ignored_when_no_tower_present() {
    let mut builder = Builder::default();
    let mut commands = Vec::new();

    builder.handle(
        &[Event::PlayModeChanged {
            mode: PlayMode::Builder,
        }],
        None,
        BuilderInput {
            remove_action: true,
            cursor_cell: Some(CellCoord::new(1, 1)),
            ..BuilderInput::default()
        },
        |_| None,
        &mut commands,
    );

    assert!(
        commands.is_empty(),
        "no tower under cursor, nothing to remove"
    );
}

#[test]
fn remove_ignored_in_attack_mode() {
    let mut builder = Builder::default();
    let mut commands = Vec::new();

    builder.handle(
        &[],
        None,
        BuilderInput {
            remove_action: true,
            cursor_cell: Some(CellCoord::new(1, 1)),
            ..BuilderInput::default()
        },
        |_| Some(TowerId::new(3)),
        &mut commands,
    );

    assert!(
        commands.is_empty(),
        "system must not emit removal commands in attack mode",
    );
}
