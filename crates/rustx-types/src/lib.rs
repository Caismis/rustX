//! Foundation types peeled out of the `rustx` crate root.
//!
//! Files keep their original module paths (`runtime::types`,
//! `runtime::permission::mode`, `tools::deadline`, ...) so moved items
//! resolve at the same absolute path inside this crate.

pub mod package_io;

pub mod model {
    pub mod finish;
    pub mod inspection;
    pub mod replay;
    pub mod types;
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
            pub mod qwen_xml;
        }
    }
}

pub mod message {
    pub mod content;
    pub mod types;
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
    pub mod availability;
    pub mod error;
    pub mod selected;
    pub mod source;
    pub use source::ToolSourceId;
}

pub mod observation {
    pub mod bounded;
    pub mod preflight;
    pub mod structural_json;
}

pub mod agent {
    pub mod execution {
        #[cfg(any(test, feature = "test-fixtures"))]
        pub mod attribution;
    }
    pub mod state;
}

pub mod app_server {
    pub mod transport {
        pub mod auth;
    }
}

pub mod context {
    pub mod assembly;
    pub mod images;
    pub mod system_prompt;
}

pub mod goal;

pub mod runtime {
    pub use cancellation::{CancellationCause, CancellationSignal, ExecutionCancellation};
    pub use rustx_identity as identity;

    pub mod cancellation;
    pub mod continuation;
    pub mod monotonic;
    pub mod process_supervision;
    pub mod process_wait;
    pub mod types;
    pub mod workflow;
    pub mod permission {
        pub mod mode;
        pub mod resource;
    }
}
