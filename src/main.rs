//! keyfob: API tokens for your shell, your scripts and Claude Code. See `keyfob --help`.

mod commands;
mod decl;
mod guard;
mod store;
mod util;

fn main() {
    std::process::exit(commands::main(std::env::args().skip(1).collect()));
}
