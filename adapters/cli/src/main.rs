#![deny(
    unsafe_code,
    missing_docs,
    dead_code,
    unused_results,
    non_snake_case,
    unreachable_pub
)]

//! Command-line adapter that boots the Maze Defence experience.

use maze_defence_system_bootstrap::Bootstrap;
use maze_defence_world::World;

/// Entry point for the Maze Defence command-line interface.
fn main() {
    let world = World::new();
    let bootstrap = Bootstrap::default();
    println!("{}", bootstrap.welcome_banner(&world));
}
