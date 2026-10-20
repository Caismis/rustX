pub fn request_snapshots(
    history: &rustx::runtime::RequestHistory,
) -> Vec<rustx::model::RequestSnapshot> {
    let mut snapshots = Vec::new();
    let mut cursor = None;
    loop {
        let page = history.page(cursor, 32).expect("request snapshot page");
        if page.snapshots.is_empty() {
            break;
        }
        cursor = page.next_sequence;
        snapshots.extend(page.snapshots);
    }
    snapshots
}

