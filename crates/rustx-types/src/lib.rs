//! Foundation types peeled out of the `rustx` crate root.
//!
//! Files keep their original module paths (`runtime::types`,
//! `runtime::permission::mode`, `tools::deadline`, ...) so moved items
//! resolve at the same absolute path inside this crate.

pub mod message {
    pub mod content;
}

pub mod tools {
    pub mod deadline;
    pub mod session_files;
}

pub mod runtime {
    pub mod monotonic;
    pub mod process_supervision;
    pub mod process_wait;
    pub mod types;
    pub mod permission {
        pub mod mode;
    }
}
