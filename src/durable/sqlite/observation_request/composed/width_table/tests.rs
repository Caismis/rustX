use super::*;
use crate::durable::sqlite::observation_request::composed::record;

fn exact(table: &WidthTable) -> Vec<u64> {
    (0..weights::CLASSES)
        .map(|class| table.length(class).unwrap())
        .collect()
}

#[test]
fn constant_bodies_store_one_run_without_losing_any_occurrence_width() {
    let mut table = WidthTable::leaf([513; weights::CLASSES]).unwrap();
    let leaf = exact(&table);
    let mut expected = leaf.clone();
    for _ in 0..13 {
        table = WidthTable::join(&table, &table).unwrap();
        expected = expected.into_iter().map(|length| length * 2 + 1).collect();
        table.validate(table.count).unwrap();
        assert_eq!(exact(&table), expected);
        assert_eq!(table.runs.len(), 1);
        let stored = record::encode(&table, 65536).unwrap();
        assert!(
            stored.len() < 24,
            "one affine body regardless of history size"
        );
        assert!(stored.len() * 8 < record::encode(&expected, 65536).unwrap().len());
        let decoded: WidthTable = record::decode(&stored).unwrap();
        decoded.validate(table.count).unwrap();
        assert_eq!(exact(&decoded), expected);
    }
    assert_eq!(table.count, manifest::MAX_ITEMS);
    assert!(WidthTable::join(&table, &table).is_err());
}

#[test]
fn independently_changing_admission_bodies_join_exactly_for_all_eighty_classes() {
    let mut seed = 17_u64;
    let mut next = || {
        seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
        seed
    };
    for _ in 0..24 {
        let mut tables = Vec::new();
        let mut arrays = Vec::new();
        for _ in 0..32 {
            let bases: Vec<_> = (0..weights::CLASSES)
                .map(|_| 10 + next() % 100_000)
                .collect();
            let reference = bases
                .iter()
                .enumerate()
                .map(|(class, base)| base + weights::delta(class).unwrap())
                .collect::<Vec<_>>();
            let table = WidthTable::leaf(bases).unwrap();
            assert_eq!(exact(&table), reference);
            tables.push(table);
            arrays.push(reference);
        }
        while tables.len() > 1 {
            let right = tables.pop().unwrap();
            let left = tables.pop().unwrap();
            let b = arrays.pop().unwrap();
            let a = arrays.pop().unwrap();
            let expected = a
                .into_iter()
                .zip(b)
                .map(|(a, b)| a + b + 1)
                .collect::<Vec<_>>();
            let joined = WidthTable::join(&left, &right).unwrap();
            assert_eq!(exact(&joined), expected);
            let stored = record::encode(&joined, 65536).unwrap();
            let decoded: WidthTable = record::decode(&stored).unwrap();
            decoded.validate(joined.count).unwrap();
            assert_eq!(exact(&decoded), expected);
            tables.insert(0, decoded);
            arrays.insert(0, expected);
        }
    }
}

#[test]
fn run_validation_checks_interior_rank_maxima_and_rehashed_geometry() {
    for start in 0..weights::CLASSES {
        for end in start..weights::CLASSES {
            let maximum = (start..=end)
                .map(|class| weights::delta(class).unwrap())
                .max()
                .unwrap();
            let mut runs = vec![(0, 1)];
            if start == 0 {
                runs.clear();
            }
            runs.push((u8::try_from(start).unwrap(), SAFE - maximum));
            if end + 1 < weights::CLASSES {
                runs.push((u8::try_from(end + 1).unwrap(), 1));
            }
            let mut table = WidthTable { count: 1, runs };
            table.validate(1).unwrap();
            let at = usize::from(start != 0);
            table.runs[at].1 += 1;
            assert!(table.validate(1).is_err(), "class interval {start}..={end}");
        }
    }
    for runs in [
        vec![],
        vec![(1, 2)],
        vec![(0, 2), (0, 3)],
        vec![(0, 2), (80, 3)],
        vec![(0, 2), (1, 2)],
        vec![(0, 2), (2, 3), (1, 4)],
    ] {
        let original = WidthTable { count: 1, runs };
        let stored = record::encode(&original, 65536).unwrap();
        let decoded: WidthTable = record::decode(&stored).unwrap();
        assert!(decoded.validate(1).is_err());
    }
    let valid = WidthTable::leaf([17; weights::CLASSES]).unwrap();
    assert!(
        valid.validate(2).is_err(),
        "count must match authenticated native node"
    );
    assert!(WidthTable::leaf([17; 79]).is_err());
    assert!(WidthTable::leaf([17; 81]).is_err());
    assert!(valid.length(80).is_err());
}

#[test]
fn obsolete_dense_widths_and_hostile_run_counts_are_not_decoded() {
    assert!(
        record::decode::<WidthTable>(&record::encode(&vec![17_u64; 80], 65536).unwrap()).is_err()
    );
    for bytes in [
        vec![0x92, 0x01, 0xdc, 0, 81],
        vec![0x92, 0x01, 0xdd, 255, 255, 255, 255],
    ] {
        assert!(record::decode::<WidthTable>(&bytes).is_err());
    }
}
