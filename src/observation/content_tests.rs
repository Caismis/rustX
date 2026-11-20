#[cfg(test)]
mod tests {
    #[test]
    fn t19_canonical_json_v1_recursively_orders_keys_preserves_arrays_and_values() {
        let value = serde_json::json!({"z":[{"z":"\n水", "a":2},1],"a":{"z":true,"a":null}});
        assert_eq!(
            {
                let mut bytes = Vec::new();
                rustx_durable::read_layout::canonical::to_writer(&mut bytes, &value).unwrap();
                bytes
            },
            "{\"a\":{\"a\":null,\"z\":true},\"z\":[{\"a\":2,\"z\":\"\\n水\"},1]}".as_bytes()
        );
    }
}
