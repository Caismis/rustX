//! Retry only an interrupted observation, keeping its descriptor and component.
//! Never wrap a descriptor traversal, candidate transaction, or Git command.
#[cfg(unix)]
pub(super) fn retry_nix<T>(mut operation: impl FnMut() -> nix::Result<T>) -> nix::Result<T> {
    loop {
        match operation() {
            Err(nix::errno::Errno::EINTR) => {}
            result => return result,
        }
    }
}

pub(super) fn retry_io<T>(mut operation: impl FnMut() -> std::io::Result<T>) -> std::io::Result<T> {
    loop {
        match operation() {
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            result => return result,
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn eintr_retries_only_interrupted_observations() {
        for terminal in [
            Ok(7),
            Err(nix::errno::Errno::ENOENT),
            Err(nix::errno::Errno::EACCES),
            Err(nix::errno::Errno::ELOOP),
        ] {
            let mut attempts = 0;
            let result = retry_nix(|| {
                attempts += 1;
                if attempts <= 2 {
                    Err(nix::errno::Errno::EINTR)
                } else {
                    terminal
                }
            });
            assert_eq!(result, terminal);
            assert_eq!(attempts, 3);
        }
        let mut attempts = 0;
        let result: std::io::Result<()> = retry_io(|| {
            attempts += 1;
            Err(if attempts == 1 {
                std::io::ErrorKind::Interrupted
            } else {
                std::io::ErrorKind::PermissionDenied
            }
            .into())
        });
        assert_eq!(
            result.unwrap_err().kind(),
            std::io::ErrorKind::PermissionDenied
        );
        assert_eq!(attempts, 2);
    }
}
