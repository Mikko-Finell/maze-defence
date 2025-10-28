use maze_defence_rendering_macroquad::ControlPanelInputState;

fn run_sequence(sequence: &[bool]) -> Vec<bool> {
    let mut state = ControlPanelInputState::default();
    let mut toggles = Vec::new();
    for &pressed in sequence {
        let mode_toggle = state.take_mode_toggle();
        toggles.push(mode_toggle);
        if pressed {
            state.register_mode_toggle();
        }
    }

    // Flush any trailing latched press so the harness observes the final toggle.
    toggles.push(state.take_mode_toggle());
    toggles
}

fn run_replay_sequence(sequence: &[bool]) -> Vec<bool> {
    let mut state = ControlPanelInputState::default();
    let mut presses = Vec::new();
    for &pressed in sequence {
        let replay = state.take_replay_wave();
        presses.push(replay);
        if pressed {
            state.register_replay_wave();
        }
    }
    presses.push(state.take_replay_wave());
    presses
}

#[test]
fn control_panel_button_toggle_sequence_is_deterministic() {
    let button_sequence = [false, true, false, true, true, false];
    let expected = vec![false, false, true, false, true, true, false];

    let first_run = run_sequence(&button_sequence);
    let second_run = run_sequence(&button_sequence);

    assert_eq!(first_run, expected);
    assert_eq!(first_run, second_run);
}

#[test]
fn replay_button_sequence_is_deterministic() {
    let button_sequence = [true, false, true, false, false, true];
    let expected = vec![false, true, false, true, false, false, true];

    let first_run = run_replay_sequence(&button_sequence);
    let second_run = run_replay_sequence(&button_sequence);

    assert_eq!(first_run, expected);
    assert_eq!(first_run, second_run);
}
