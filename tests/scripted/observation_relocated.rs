//! Tests that stayed in the monolith because their fixtures live in
//! monolith-only modules; assertions are unchanged.

use serde::Serialize;
use serde_json::Value;

#[test]
fn safe_integer_proof_defers_wide_native_values_and_ambiguous_keys() {
    #[derive(Serialize, schemars::JsonSchema)]
    struct Native {
        count: u64,
    }
    #[derive(Serialize, schemars::JsonSchema)]
    struct Constrained {
        #[schemars(range(max = 9_007_199_254_740_991_u64))]
        count: u64,
    }
    #[derive(Serialize, schemars::JsonSchema)]
    struct Signed {
        count: i64,
    }
    let wide = Native { count: u64::MAX };
    assert_eq!(
        crate::observation::preflight::bounded_fit(&wide, 4096),
        Some(true)
    );
    assert_eq!(
        crate::observation::preflight::bounded_safe_integer_fit(&wide, 4096),
        Some(false)
    );
    assert!(crate::observation::bounded::structural_value(&wide).is_some());
    assert!(
        crate::observation::bounded::structural_value(&Constrained { count: u64::MAX }).is_none()
    );
    for count in [i64::MIN, i64::MAX] {
        let value = Signed { count };
        assert_eq!(
            crate::observation::preflight::bounded_safe_integer_fit(&value, 4096),
            Some(false)
        );
        assert!(crate::observation::bounded::structural_value(&value).is_none());
    }
    assert_eq!(
        crate::observation::preflight::bounded_safe_integer_fit(
            &serde_json::json!({"wide": u64::MAX}),
            4096
        ),
        Some(false)
    );
    let ambiguous = std::collections::BTreeMap::from([((1_u8, 2_u8), 3_u8)]);
    assert_eq!(
        crate::observation::preflight::bounded_safe_integer_fit(&ambiguous, 4096),
        None
    );
}

#[test]
fn t11_t14_t20_ordered_schema_properties_preserve_complete_checkpoint_bytes_and_admission() {
    let mut checkpoint = crate::runtime_client::projection::RuntimeClientProjection::new(
        crate::runtime::identity::ConversationId::new("conv_77621431-ad5b-7155-9eca-9f21014c0bc9"),
        Vec::new(),
        serde_json::from_value(serde_json::json!({"configured_tools":[],"revision":1})).unwrap(),
        None,
        64,
    )
    .checkpoint_candidate()
    .unwrap();
    checkpoint.read_cut.through = u64::MAX;
    checkpoint.read_cut.revision = u64::MAX;
    checkpoint
        .read_cut
        .domain_revisions
        .insert("trace".into(), u64::MAX);
    let schema = crate::observation::structural_json::schema_root::<
        crate::runtime_client::checkpoint::RuntimeClientCheckpoint,
    >();
    let expected =
        crate::observation::structural_json::property_encoding(&checkpoint, &schema, true);
    let (_, old_fallbacks) =
        crate::observation::structural_json::PROPERTY_WORK.with(std::cell::Cell::get);
    let actual =
        crate::observation::structural_json::property_encoding(&checkpoint, &schema, false);
    let (ordered, fallbacks) =
        crate::observation::structural_json::PROPERTY_WORK.with(std::cell::Cell::get);
    assert_eq!(actual, expected);
    assert!(ordered > 100 && fallbacks < old_fallbacks);
    let value: Value = serde_json::from_slice(actual.0.as_ref().unwrap()).unwrap();
    assert_eq!(value["read_cut"]["through"], u64::MAX.to_string());
    assert_eq!(value["read_cut"]["revision"], u64::MAX.to_string());
}
