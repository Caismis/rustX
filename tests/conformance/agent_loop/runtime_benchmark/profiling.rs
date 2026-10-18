//! Explicit diagnostic boundaries; the preloaded profiler owns its control socket.
//! Ordinary runs do not connect or change allocator behavior. Flush/dump and
//! handshake costs belong to diagnostics only, never headline timings.
pub(super) fn boundary(phase: &str) {
    let Some(socket) = std::env::var_os("RUSTX_BENCH_PROFILE_SOCKET") else {
        return;
    };
    use std::io::{Read, Write};
    let mut stream = std::os::unix::net::UnixStream::connect(socket).unwrap();
    writeln!(stream, "{phase}").unwrap();
    let mut ack = [0];
    stream.read_exact(&mut ack).unwrap();
    assert_eq!(ack, [b'K'], "profiler boundary failed");
}
