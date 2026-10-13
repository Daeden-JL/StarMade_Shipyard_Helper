//! StarMade design data: blocks, entities, blueprint files and the block catalog.
//!
//! Compiles for both the server and the browser, so the two always agree on what
//! a design contains.

pub mod api;
pub mod block;
pub mod blueprint;
pub mod bytes;
pub mod catalog;
pub mod clone;
pub mod collide;
pub mod dock;
pub mod dockrec;
pub mod edit;
pub mod entity;
pub mod interior;
pub mod links;
pub mod mesh;
pub mod meta;
pub mod metafile;
pub mod model;
pub mod pick;
pub mod pivot;
pub mod rawtag;
pub mod relocate;
pub mod rig;
pub mod shape;
pub mod sim;
pub mod smooth;
mod shape_data;
pub mod smd3;
pub mod tag;
pub mod turret;

pub use block::Block;
pub use blueprint::Blueprint;
pub use entity::{Entity, Pos};

/// Anything that can go wrong reading or writing design data.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("unexpected end of data while reading {0}")]
    Truncated(&'static str),
    #[error("unsupported {what}: {detail}")]
    Unsupported { what: &'static str, detail: String },
    #[error("malformed {what}: {detail}")]
    Malformed { what: &'static str, detail: String },
    #[error("not a blueprint: {0}")]
    NotABlueprint(String),
    #[error("archive error: {0}")]
    Archive(String),
}

pub type Result<T> = std::result::Result<T, Error>;
