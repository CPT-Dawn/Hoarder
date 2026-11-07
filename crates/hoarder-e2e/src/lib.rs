//! End-to-end test harness for Hoarder (PLAN.md §13, primary seam).
//!
//! A [`Session`] is a throwaway headless Sway compositor with the outputs a
//! test asks for. Tests run the `hoarder` binary inside it as a black box
//! ([`Session::command`]) and drive the desktop around it with a helper
//! Wayland client ([`Session::client`]) that paints known patterns, sets and
//! serves Selections, and injects pointer and keyboard input.
//!
//! See `docs/testing.md` for how to run it and what it needs.

pub mod budget;
mod client;
mod frame;
mod image;
pub mod keys;
mod pattern;
mod session;

pub use client::{Client, Input, InputEvent, Reply, Selection, TextField};
pub use frame::{FrameFormat, RawFrame};
pub use image::Image;
pub use pattern::{Pattern, PixelFormat, Rgb8, Rgb10};
pub use session::{BitDepth, Output, OutputSpec, Session, Transform};

/// Anything that can go wrong while driving the harness.
#[derive(Debug)]
pub enum Error {
    /// The session could not be set up (Sway missing, crashed, timed out).
    Setup(String),
    /// A helper command failed.
    Command(String),
    /// A Wayland protocol or connection error in the helper client.
    Wayland(String),
    Io(std::io::Error),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Setup(m) => write!(f, "session setup: {m}"),
            Error::Command(m) => write!(f, "command: {m}"),
            Error::Wayland(m) => write!(f, "wayland: {m}"),
            Error::Io(e) => write!(f, "io: {e}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e)
    }
}

pub type Result<T, E = Error> = std::result::Result<T, E>;
