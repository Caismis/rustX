//! The shared Server-side projection of a physical-search failure.
//!
//! Glob and Grep render the Client's stable [`ReadError`] wire codes —
//! recovered by `PhysicalError` downcast, never by parsing message text —
//! onto the same user-meaningful diagnostics the pre-migration
//! Server-local traversal produced, using only the interpreted search
//! root. Traversal details never cross the execution boundary.

use std::path::Path;

use rustx_execution_protocol::ReadError;

/// Renders one physical-search failure as the user-meaningful diagnostic
/// for the interpreted search `root`.
pub(super) fn search_failure(root: &Path, error: &std::io::Error) -> String {
    // The unavailable condition carries its own complete diagnostic —
    // wrapping it in "cannot search" would only obscure it.
    if error.kind() == std::io::ErrorKind::NotConnected {
        return error.to_string();
    }
    let physical = error
        .get_ref()
        .and_then(|inner| inner.downcast_ref::<rustx_execution_server::PhysicalError>())
        .map(|physical| physical.0);
    match physical {
        Some(ReadError::NotFound) => {
            format!("{} is not a directory or a file", root.display())
        }
        Some(ReadError::SearchNeedsDirectory) => {
            "glob searches a directory; the path names a single file".to_owned()
        }
        Some(ReadError::Denied) => {
            format!("cannot search {}: access is denied", root.display())
        }
        Some(ReadError::Conflict) => format!(
            "cannot search {}: files changed while they were being searched",
            root.display()
        ),
        Some(ReadError::SearchBudgetExceeded) => format!(
            "cannot search {}: the search exceeded a bounded budget",
            root.display()
        ),
        _ => format!("cannot search {}: {error}", root.display()),
    }
}
