use super::*;

fn plan(protocol: ModelProtocol) -> Plan {
    Plan {
        key: Some(Key {
            protocol,
            origin: None,
            stored_at: None,
        }),
        ..Plan::default()
    }
}

fn reference(raw: &[(String, bool)], protocol: ModelProtocol) -> Result<Vec<String>, ()> {
    let order = (0..raw.len())
        .filter(|&i| !raw[i].1)
        .chain((0..raw.len()).filter(|&i| raw[i].1))
        .collect::<Vec<_>>();
    let labels = crate::model::adapter::correlation::wire_labels(
        &order.iter().map(|&i| raw[i].0.clone()).collect::<Vec<_>>(),
        protocol,
    );
    let mut result = vec![String::new(); raw.len()];
    for (index, label) in order.into_iter().zip(labels) {
        if raw[index].1 && raw[index].0 != label {
            return Err(());
        }
        result[index] = label;
    }
    Ok(result)
}

#[test]
fn every_increment_matches_full_allocator_with_native_priority_and_collisions() {
    for protocol in [
        ModelProtocol::OpenAiChatCompletions,
        ModelProtocol::OpenAiResponses,
        ModelProtocol::AnthropicMessages,
    ] {
        for native_case in [false, true] {
            let mut plan = plan(protocol);
            let mut raw = Vec::new();
            let mut pool = vec![
                "same".to_owned(),
                "other".into(),
                "rustx_occurrence_0".into(),
                "invalid id".into(),
                "🦀".into(),
                "x".repeat(100),
            ];
            for suffix in 0..48 {
                pool.push(format!("rustx_occurrence_0{}", "_".repeat(suffix)));
            }
            for index in 0..256_u32 {
                let id = if index % 13 == 0 && !plan.calls.is_empty() {
                    plan.calls[0].label.to_string()
                } else {
                    pool[(index as usize * 17 + index as usize / 7) % pool.len()].clone()
                };
                let native = native_case && index % 9 == 8;
                raw.push((id.clone(), native));
                let expected = reference(&raw, protocol);
                let actual = plan.push((index, 0), &id, native);
                assert_eq!(actual.is_ok(), expected.is_ok(), "{protocol:?} at {index}");
                if let Ok(expected) = expected {
                    assert_eq!(
                        plan.calls
                            .iter()
                            .map(|call| call.label.to_string())
                            .collect::<Vec<_>>(),
                        expected,
                        "{protocol:?} at {index}"
                    );
                    // End each occurrence before the next repeated raw ID.
                    plan.pending.remove(id.as_str());
                } else {
                    break;
                }
            }
        }
    }
}

#[test]
fn oversized_synthetic_labels_and_new_raw_hash_collisions_match_full_allocator() {
    let protocol = ModelProtocol::OpenAiChatCompletions;
    let mut plan = plan(protocol);
    let mut raw = vec![("repeat".into(), false)];
    plan.push((0, 0), "repeat", false).unwrap();
    for suffix in 0..80 {
        let id = format!("rustx_occurrence_0{}", "_".repeat(suffix));
        let index = u32::try_from(raw.len()).unwrap();
        plan.push((index, 0), &id, false).unwrap();
        raw.push((id, false));
    }
    for id in ["repeat".to_owned(), "invalid id".into()] {
        let index = u32::try_from(raw.len()).unwrap();
        plan.push((index, 0), &id, false).unwrap();
        raw.push((id, false));
    }
    let shadow = plan.calls.back().unwrap().label.to_string();
    let index = u32::try_from(raw.len()).unwrap();
    plan.push((index, 0), &shadow, false).unwrap();
    raw.push((shadow, false));
    assert_eq!(
        plan.calls
            .iter()
            .map(|call| call.label.to_string())
            .collect::<Vec<_>>(),
        reference(&raw, protocol).unwrap()
    );
}
