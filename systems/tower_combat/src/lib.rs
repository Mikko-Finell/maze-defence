#![deny(
    unsafe_code,
    missing_docs,
    dead_code,
    unused_results,
    non_snake_case,
    unreachable_pub
)]

//! Pure system that emits projectile firing commands from targeting data.

use maze_defence_core::{
    Command, PlayMode, TowerCooldownSnapshot, TowerCooldownView, TowerId, TowerTarget,
};

/// Tower combat system that queues firing commands for ready towers.
#[derive(Debug, Default)]
pub struct TowerCombat {
    scratch: Vec<Command>,
}

impl TowerCombat {
    /// Creates a new tower combat system with empty scratch buffers.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Emits `Command::FireProjectile` entries for towers ready to fire.
    pub fn handle(
        &mut self,
        play_mode: PlayMode,
        tower_cooldowns: TowerCooldownView,
        tower_targets: &[TowerTarget],
        out: &mut Vec<Command>,
    ) {
        if play_mode != PlayMode::Attack {
            return;
        }

        if tower_targets.is_empty() {
            return;
        }

        let cooldowns = tower_cooldowns.into_vec();
        if cooldowns.is_empty() {
            return;
        }

        self.scratch.clear();

        for target in tower_targets {
            if let Some(snapshot) = find_cooldown(&cooldowns, target.tower) {
                if snapshot.ready_in.is_zero() {
                    self.scratch.push(Command::FireProjectile {
                        tower: target.tower,
                        target: target.bug,
                    });
                }
            }
        }

        if self.scratch.is_empty() {
            return;
        }

        out.reserve(self.scratch.len());
        out.append(&mut self.scratch);
    }
}

fn find_cooldown(
    cooldowns: &[TowerCooldownSnapshot],
    tower: TowerId,
) -> Option<&TowerCooldownSnapshot> {
    cooldowns
        .binary_search_by_key(&tower, |snapshot| snapshot.tower)
        .ok()
        .map(|index| &cooldowns[index])
}

#[cfg(test)]
mod tests {
    use super::*;
    use maze_defence_core::{BugId, CellPoint, TowerKind};
    use std::time::Duration;

    #[test]
    fn builder_mode_is_silent() {
        let mut system = TowerCombat::new();
        let cooldowns = TowerCooldownView::from_snapshots(vec![snapshot(1, Duration::ZERO)]);
        let targets = vec![target(1, 7)];
        let mut out = Vec::new();

        system.handle(PlayMode::Builder, cooldowns, &targets, &mut out);

        assert!(out.is_empty());
    }

    #[test]
    fn firing_respects_cooldown_readiness() {
        let mut system = TowerCombat::new();
        let cooldowns = TowerCooldownView::from_snapshots(vec![
            snapshot(2, Duration::ZERO),
            snapshot(5, Duration::ZERO),
        ]);
        let targets = vec![target(2, 4), target(5, 1)];
        let mut out = Vec::new();

        system.handle(PlayMode::Attack, cooldowns, &targets, &mut out);

        assert_eq!(
            out,
            vec![
                Command::FireProjectile {
                    tower: TowerId::new(2),
                    target: BugId::new(4),
                },
                Command::FireProjectile {
                    tower: TowerId::new(5),
                    target: BugId::new(1),
                },
            ],
        );
    }

    #[test]
    fn non_ready_or_missing_towers_are_skipped() {
        let mut system = TowerCombat::new();
        let cooldowns = TowerCooldownView::from_snapshots(vec![
            snapshot(3, Duration::from_millis(250)),
            snapshot(8, Duration::ZERO),
        ]);
        let targets = vec![target(3, 9), target(8, 2), target(42, 3)];
        let mut out = Vec::new();

        system.handle(PlayMode::Attack, cooldowns, &targets, &mut out);

        assert_eq!(
            out,
            vec![Command::FireProjectile {
                tower: TowerId::new(8),
                target: BugId::new(2),
            }],
        );
    }

    fn snapshot(tower: u32, ready_in: Duration) -> TowerCooldownSnapshot {
        TowerCooldownSnapshot {
            tower: TowerId::new(tower),
            kind: TowerKind::Basic,
            ready_in,
        }
    }

    fn target(tower: u32, bug: u32) -> TowerTarget {
        TowerTarget {
            tower: TowerId::new(tower),
            bug: BugId::new(bug),
            tower_center_cells: CellPoint::new(0.0, 0.0),
            bug_center_cells: CellPoint::new(0.0, 0.0),
        }
    }
}
