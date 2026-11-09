//! Local structural benchmark: sparse assets, no provider or environment setup.
//! Run each case in a fresh process under `/usr/bin/time -v` for CPU/peak RSS.
use std::io::Write;
use std::time::Instant;

fn rchar() -> Option<u64> {
    std::fs::read_to_string("/proc/self/io")
        .ok()?
        .lines()
        .find_map(|line| line.strip_prefix("rchar:")?.trim().parse().ok())
}

fn main() {
    let case = std::env::args().nth(1).unwrap_or_else(|| "small".into());
    let (count, asset_bytes, body_bytes) = match case.as_str() {
        "small" => (1, 1024, 1024),
        "large" => (1, 2 * 1024 * 1024 * 1024_u64, 256 * 1024 * 1024),
        "many" => (32, 16 * 1024 * 1024, 1024),
        _ => panic!("expected small, large or many"),
    };
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    for index in 0..count {
        let name = format!("skill-{index}");
        let package = root.join(&name);
        std::fs::create_dir(&package).unwrap();
        let mut markdown = std::fs::File::create(package.join("SKILL.md")).unwrap();
        writeln!(
            markdown,
            "---\nname: {name}\ndescription: Benchmark definition\n---"
        )
        .unwrap();
        markdown
            .set_len(body_bytes + markdown.metadata().unwrap().len())
            .unwrap();
        std::fs::File::create(package.join("asset.bin"))
            .unwrap()
            .set_len(asset_bytes)
            .unwrap();
    }
    let discovery = rustx::skills::SkillDiscovery::user_root(root);
    let before = rchar();
    let start = Instant::now();
    let first = discovery.discover();
    let discovery_us = start.elapsed().as_micros();
    let start = Instant::now();
    let second = discovery.discover();
    let rescan_us = start.elapsed().as_micros();
    let read_bytes = before.zip(rchar()).map(|(before, after)| after - before);
    assert!(first.diagnostics.is_empty(), "{:?}", first.diagnostics);
    assert_eq!(first, second);
    assert_eq!(first.packages.len(), count);
    println!(
        "{}",
        serde_json::json!({"case":case,"skills":count,"asset_bytes_per_skill":asset_bytes,"body_bytes_per_skill":body_bytes,"discovery_us":discovery_us,"rescan_us":rescan_us,"process_rchar_delta":read_bytes})
    );
}
