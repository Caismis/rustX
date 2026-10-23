//! Classification owns domain acceptance, independently of model settlement.
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Answer {
    label: String,
}

pub(crate) fn accept(text: &str, labels: &[String]) -> Result<String, &'static str> {
    if !text
        .trim_start_matches([' ', '\t', '\r', '\n'])
        .starts_with('{')
    {
        return Err("classification requires a JSON object");
    }
    // Derived struct deserialization rejects duplicate and extra properties;
    // from_str also requires EOF after the one complete object.
    let answer: Answer =
        serde_json::from_str(text).map_err(|_| "classification returned an invalid JSON object")?;
    if !labels.contains(&answer.label) {
        return Err("classification returned a label outside the supplied set");
    }
    Ok(answer.label)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn accepts_only_one_exact_supported_label() {
        let labels = vec!["Yes".into(), "No".into()];
        assert_eq!(accept(r#"{"label":"Yes"}"#, &labels), Ok("Yes".into()));
        for text in [
            r#"{"label":"yes"}"#,
            r#"{"label":"Yes","label":"Yes"}"#,
            r#"{"label":"Yes","confidence":1}"#,
            r#"{"label":"Yes"} trailing"#,
            "```json\n{\"label\":\"Yes\"}\n```",
            "[\"Yes\"]",
            "null",
            "{}",
        ] {
            assert!(accept(text, &labels).is_err(), "{text}");
        }
    }
}
