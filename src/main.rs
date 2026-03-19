mod config;
mod core;
mod engine;
mod network;
mod observe;
mod pet;
mod social;
mod system;
mod ui;
#[cfg(feature = "web")]
mod web;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    system::entrypoint::run()
}
