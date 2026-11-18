//! Foundation types peeled out of the `rustx` crate root.
//!
//! Files keep their original module paths (`runtime::types`,
//! `runtime::permission::mode`, `tools::deadline`, ...) so moved items
//! resolve at the same absolute path inside this crate.

pub mod events {
    pub mod interaction;
    pub mod review;
}
pub mod package_io;
pub mod toml_authoring;

pub mod model {
    pub mod finish;
    #[cfg(any(test, feature = "test-fixtures"))]
    pub mod hash_work;
    pub mod images;
    pub mod input;
    pub mod inspection;
    pub mod projected;
    pub mod replay;
    pub mod request_params;
    pub mod transformations;
    pub mod types;
    pub mod uploads;
    pub mod adapter {
        pub mod response_budget;
        pub mod retry;
        pub mod transport;
        pub mod anthropic {
            pub mod config;
            pub mod wire;
        }
        pub mod block_index;
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

    pub mod native {
        pub mod bash {
            pub mod text;
        }
    }
    pub mod output {
        pub mod stdout;
    }
    pub mod schema;

    pub mod mcp {
        #[cfg(feature = "test-fixtures")]
        pub mod test_sync;
    }
}

pub mod skills {
    pub mod discovery;
    pub mod frontmatter;
    pub mod package;

    pub mod identity;
    pub mod source;
}

pub mod capabilities {
    pub mod availability;
    pub mod error;
    pub mod selected;
    pub mod source;
    pub use source::ToolSourceId;
}

pub mod runtime_client {
    pub mod interaction_observation;
}

pub mod observation {
    pub mod bounded;
    pub mod preflight;
    pub mod structural_json;

    pub mod goal;
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

pub mod local_runtime {
    #[cfg(any(test, feature = "test-fixtures"))]
    pub mod static_effects;

    pub mod app_server_policy;
    pub mod session {
        #[cfg(any(test, feature = "test-fixtures"))]
        pub mod create_profile;
        pub mod invalidations;
    }
}

pub mod context {
    pub mod assembly;
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
        pub mod reminder;
        pub mod resource;
    }

    pub mod auxiliary {
        pub mod capacity;
        pub mod observer;
    }
}
pub mod durable {
    pub mod sqlite {
        pub mod content_metadata;
        pub mod event_kind_key;
        pub mod statements;
    }
}
pub mod scheduler {
    pub mod calendar;
}

pub mod workspace_changes {
    pub mod capture;
}
