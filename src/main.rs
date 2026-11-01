//! wa2-native - a native re-implementation of the WHITE ALBUM 2 engine.
//!
//! This file only wires the crate together. Every subsystem lives in
//! its own module, and the re-export block at the bottom keeps the short
//! `crate::name` paths working across the crate.

mod archive;
mod artcache;
mod assets;
mod audio;
mod backlog;
mod cg;
mod choice;
mod cli;
mod cmd;
mod cmdbar;
mod codec;
mod data;
mod font;
mod ft2;
mod gallery;
pub mod game;
mod menu;
mod movie;
mod present;
mod render;
mod rewind;
mod save;
mod saveload;
mod scene;
mod sdl2;
mod session;
mod sha256;
pub mod sound;
mod special;
mod sprite;
mod timeline;
mod title;
mod ui;
mod video;
mod vm;
mod voice;

#[cfg(test)]
mod tests;

// The subsystems are re-exported so the short `crate::name` paths keep
// working across the crate.
pub(crate) use assets::*;
pub(crate) use backlog::*;
pub(crate) use choice::*;
pub(crate) use cmd::*;
pub(crate) use font::*;
pub use game::*;
pub(crate) use gallery::*;
pub(crate) use movie::*;
pub(crate) use present::*;
pub(crate) use rewind::*;
pub(crate) use scene::*;
pub(crate) use session::*;
pub use sound::*;
pub(crate) use special::*;
pub(crate) use sprite::*;
pub(crate) use title::*;
pub(crate) use ui::*;
pub(crate) use voice::*;

fn main() {
    cli::dispatch();
}
