//! Foundation types peeled out of the `rustx` crate root.
//!
//! Files keep their original module paths (`runtime::types`,
//! `runtime::permission::mode`, `tools::deadline`, ...) so moved items
//! resolve at the same absolute path inside this crate.

pub mod package_io;

pub mod model {
    pub mod finish;
    pub mod inspection;
    pub mod adapter {
        pub mod response_budget;
        pub mod retry;
        pub mod transport;
        pub mod anthropic {
            pub mod config;
            pub mod wire;
        }
        pub mod openai {
            pub mod config;
        }
    }
}

pub mod message {
    pub mod content;
}

pub mod tools {
    pub mod deadline;
    pub mod limits;
    pub mod python;
    pub mod session_files;
    pub mod types;
}

pub mod skills {
    pub mod discovery;
    pub mod frontmatter;
    pub mod package;
}

pub mod capabilities {
    pub mod source;
    pub use source::ToolSourceId;
}

pub mod runtime {
    pub use cancellation::{CancellationCause, CancellationSignal, ExecutionCancellation};
    pub use rustx_identity as identity;

    pub mod cancellation;
    pub mod monotonic;
    pub mod process_supervision;
    pub mod process_wait;
    pub mod types;
    pub mod workflow;
    pub mod permission {
        pub mod mode;
    }
}
