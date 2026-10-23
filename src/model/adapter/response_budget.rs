//! Bound provider bytes before an SSE/JSON parser or error-body accumulator.
//! A total response budget also bounds a frame with no terminating delimiter.
use futures_util::StreamExt;

pub(crate) const AUXILIARY_RESPONSE_BYTES: usize = 2 * 1024 * 1024;

pub(crate) fn bounded(response: reqwest::Response, limit: usize) -> reqwest::Response {
    let status = response.status();
    let version = response.version();
    let headers = response.headers().clone();
    let stream = response
        .bytes_stream()
        .scan((0usize, false), move |(used, failed), chunk| {
            let next = if *failed {
                None
            } else {
                match chunk {
                    Ok(bytes) if bytes.len() <= limit.saturating_sub(*used) => {
                        *used += bytes.len();
                        Some(Ok(bytes))
                    }
                    Ok(_) => {
                        *failed = true;
                        Some(Err(std::io::Error::new(
                            std::io::ErrorKind::FileTooLarge,
                            "provider response byte budget exceeded",
                        )))
                    }
                    Err(error) => {
                        *failed = true;
                        Some(Err(std::io::Error::other(error)))
                    }
                }
            };
            std::future::ready(next)
        });
    let mut result = http::Response::new(reqwest::Body::wrap_stream(stream));
    *result.status_mut() = status;
    *result.version_mut() = version;
    *result.headers_mut() = headers;
    result.into()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn exact_boundary_and_unterminated_frame_are_bounded_before_parsing() {
        for bytes in [AUXILIARY_RESPONSE_BYTES, AUXILIARY_RESPONSE_BYTES + 1] {
            let response: reqwest::Response =
                http::Response::new(reqwest::Body::from(vec![b'x'; bytes])).into();
            let read = bounded(response, AUXILIARY_RESPONSE_BYTES).bytes().await;
            assert_eq!(read.is_ok(), bytes == AUXILIARY_RESPONSE_BYTES);
            if let Ok(read) = read {
                assert_eq!(read.len(), AUXILIARY_RESPONSE_BYTES);
            }
        }
    }
    #[tokio::test]
    async fn many_small_chunks_share_one_budget_and_release_on_overflow() {
        let chunks =
            futures_util::stream::iter((0..17).map(|_| Ok::<_, std::io::Error>(vec![b'x'; 128])));
        let response: reqwest::Response =
            http::Response::new(reqwest::Body::wrap_stream(chunks)).into();
        assert!(bounded(response, 2048).bytes().await.is_err());
    }
}
