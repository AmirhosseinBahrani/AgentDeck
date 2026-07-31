use serde::{Deserialize, Serialize};
use std::fmt;
use uuid::Uuid;

/// Newtype IDs. With this many ID-carrying tables, bare `String`/`Uuid` arguments
/// get transposed at a call site eventually; the compiler should catch it instead.
macro_rules! id_type {
    ($name:ident) => {
        #[derive(
            Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
        )]
        #[serde(transparent)]
        pub struct $name(pub Uuid);

        impl $name {
            pub fn new() -> Self {
                Self(Uuid::new_v4())
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(f)
            }
        }

        impl From<Uuid> for $name {
            fn from(u: Uuid) -> Self {
                Self(u)
            }
        }
    };
}

id_type!(AgentId);
id_type!(SessionId);
id_type!(TaskId);
id_type!(RunId);

/// Global monotonic event sequence. Ordering is what makes UI gap detection and
/// backfill-from-SQLite possible, so it is a distinct type from any row id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Seq(pub u64);
