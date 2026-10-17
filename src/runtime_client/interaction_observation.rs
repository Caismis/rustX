//! Locators into the native immutable pending source; reads never serialize a body here.
pub(crate) fn selection(domain: &str) -> Option<(u64, Vec<String>)> {
    let tail = domain.strip_prefix("pending_interactions.")?;
    let (ordinal, tail) = tail.split_once('.')?;
    let ordinal = ordinal.parse().ok()?;
    let tail = tail.strip_prefix("request.kind.arguments")?;
    let path = if tail.is_empty() {
        Vec::new()
    } else {
        tail.strip_prefix('.')?
            .split('.')
            .map(str::to_owned)
            .collect()
    };
    Some((ordinal, path))
}
