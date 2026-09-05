//! Newtype ids. Plain `u64` counters would let a `SourceId` and a `ClipId`
//! be swapped at a call site and compile anyway; these do not.

use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};

macro_rules! id_type {
    ($name:ident) => {
        #[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
        pub struct $name(u64);

        impl $name {
            /// Process-local monotonic id. Not persisted as "the" id
            /// format across saves in a stable way beyond this session --
            /// offcut-project (the plan's non-destructive project file) is
            /// responsible for any cross-session stability guarantees; this
            /// type only promises uniqueness within one running process.
            pub fn next() -> Self {
                static COUNTER: AtomicU64 = AtomicU64::new(1);
                $name(COUNTER.fetch_add(1, Ordering::Relaxed))
            }

            #[cfg(test)]
            pub fn from_raw_for_test(raw: u64) -> Self {
                $name(raw)
            }
        }
    };
}

id_type!(SourceId);
id_type!(ClipId);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique_and_monotonic() {
        let a = ClipId::next();
        let b = ClipId::next();
        assert_ne!(a, b);
        assert!(b > a);
    }

    #[test]
    fn source_id_and_clip_id_are_distinct_types() {
        // This is a compile-time property, not a runtime one: the fact
        // this module compiles with SourceId and ClipId as separate types
        // (not both raw u64) is the actual test. This function exists so
        // `cargo test` still reports something ran.
        let s = SourceId::next();
        let c = ClipId::next();
        assert_ne!(s.0, 0);
        assert_ne!(c.0, 0);
    }
}
