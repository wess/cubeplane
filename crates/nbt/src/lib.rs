//! # cubeplane-nbt
//!
//! A small, dependency-light implementation of Mojang's Named Binary Tag
//! format. It covers the full tag set (Byte … LongArray) and offers an
//! ergonomic builder so server code can describe registry codecs and chunk
//! heightmaps declaratively:
//!
//! ```
//! use cubeplane_nbt::Nbt;
//!
//! let tag = Nbt::compound()
//!     .put_string("name", "overworld")
//!     .put_int("id", 0)
//!     .put_bool("natural", true);
//! let bytes = tag.to_bytes_named("");
//! assert!(!bytes.is_empty());
//! ```

mod value;

pub use value::{Nbt, NbtError, Value};
