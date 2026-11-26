//! Minimal token-byte helpers shared by selected-input accounting.

use crate::model::input::ModelInputMessage;

/// One immutable leaf's exact model-visible JSON. Called only when the native
/// node aggregate is first computed; unsupported Tool projections remain errors.
pub(crate) fn model_visible_item_bytes(message: &ModelInputMessage) -> Option<u64> {
    let visible = message.view().model_visible().ok()?;
    Some(encoded_bytes(&visible))
}

/// Price semantic role/content directly into a counting sink. Message IDs,
/// source digests and projection provenance never enter model token accounting.
fn encoded_bytes(value: &(impl serde::Serialize + ?Sized)) -> u64 {
    struct Count(u64);
    impl std::io::Write for Count {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0 = self.0.saturating_add(bytes.len() as u64);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut count = Count(0);
    serde_json::to_writer(&mut count, value).expect("semantic model content serializes");
    #[cfg(any(test, feature = "test-fixtures"))]
    ACCOUNTED_BYTES.with(|total| total.set(total.get().saturating_add(count.0)));
    #[cfg(any(test, feature = "test-fixtures"))]
    crate::agent::execution::attribution::record_context(
        crate::agent::execution::attribution::ContextWork {
            token_bytes: count.0,
            ..Default::default()
        },
    );
    count.0
}

#[cfg(any(test, feature = "test-fixtures"))]
thread_local! {
    static ACCOUNTED_BYTES: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

#[cfg(any(test, feature = "test-fixtures"))]
/// Bytes accounted through `encoded_bytes` since the last take.
pub fn take_accounted_bytes() -> u64 {
    ACCOUNTED_BYTES.with(|total| total.replace(0))
}
