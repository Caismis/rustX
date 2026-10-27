//! Exact quote insertions over the one authoritative native event body.
//! The native serializer retains opaque JSON numbers; only the typed structural
//! serializer decides which native quantities become decimal strings.
use super::{ConversationStoreError, MAX_BYTES, storage};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Overlay {
    native_length: u16,
    source_length: u16,
    native_digest: [u8; 32],
    source_digest: [u8; 32],
    #[serde(deserialize_with = "positions")]
    quotes: Vec<u16>,
}

fn positions<'de, D: serde::Deserializer<'de>>(decoder: D) -> Result<Vec<u16>, D::Error> {
    struct Positions;
    impl<'de> serde::de::Visitor<'de> for Positions {
        type Value = Vec<u16>;
        fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(formatter, "at most {MAX_BYTES} quote positions")
        }
        fn visit_seq<A: serde::de::SeqAccess<'de>>(
            self,
            mut values: A,
        ) -> Result<Self::Value, A::Error> {
            let length = values.size_hint().unwrap_or(0);
            if length > MAX_BYTES {
                return Err(serde::de::Error::custom(
                    "event overlay position count exceeds bound",
                ));
            }
            let mut positions = Vec::with_capacity(length);
            while let Some(position) = values.next_element()? {
                if positions.len() == MAX_BYTES {
                    return Err(serde::de::Error::custom(
                        "event overlay position count exceeds bound",
                    ));
                }
                positions.push(position);
            }
            Ok(positions)
        }
    }
    decoder.deserialize_seq(Positions)
}

pub(super) fn encode(
    native: &[u8],
    source: &[u8],
) -> Result<Option<Vec<u8>>, ConversationStoreError> {
    if native.is_empty()
        || native.len() > MAX_BYTES
        || source.is_empty()
        || source.len() > MAX_BYTES
    {
        return Ok(None);
    }
    let mut quotes = Vec::new();
    let mut original = 0;
    for byte in source {
        if native.get(original) == Some(byte) {
            original += 1;
        } else if *byte == b'"'
            && quotes
                .last()
                .is_none_or(|last| usize::from(*last) < original)
        {
            quotes.push(
                u16::try_from(original)
                    .map_err(|_| storage("event overlay position exceeds bound"))?,
            );
        } else {
            return Ok(None);
        }
    }
    if original != native.len() {
        return Ok(None);
    }
    let overlay = Overlay {
        native_length: u16::try_from(native.len())
            .map_err(|_| storage("native event exceeds overlay bound"))?,
        source_length: u16::try_from(source.len())
            .map_err(|_| storage("event source exceeds overlay bound"))?,
        native_digest: Sha256::digest(native).into(),
        source_digest: Sha256::digest(source).into(),
        quotes,
    };
    // At most MAX_BYTES u16 coordinates: this temporary record is bounded even
    // for an unfavorable shape. Oversized overlays use the ordinary segmented
    // writer; malformed stored overlays never select a second reader.
    let mut output = Vec::new();
    overlay
        .serialize(
            &mut rmp_serde::Serializer::new(&mut output)
                .with_struct_tuple()
                .with_bytes(rmp_serde::config::BytesMode::ForceAll),
        )
        .map_err(|error| storage(error.to_string()))?;
    Ok((output.len() <= MAX_BYTES).then_some(output))
}

pub(super) fn restore(native: &[u8], bytes: &[u8]) -> Result<Vec<u8>, ConversationStoreError> {
    if native.is_empty()
        || native.len() > MAX_BYTES
        || bytes.is_empty()
        || bytes.len() > MAX_BYTES
        || bytes[0] != 0x95
    {
        return Err(storage(
            "event overlay exceeds bound or has an invalid record shape",
        ));
    }
    // Cursor-backed decoding allocates only actual bounded bytes; the explicit
    // sequence visitor rejects hostile declared coordinate counts up front.
    let mut decoder = rmp_serde::Deserializer::new(std::io::Cursor::new(bytes));
    decoder.set_max_depth(8);
    let overlay =
        Overlay::deserialize(&mut decoder).map_err(|_| storage("event overlay malformed"))?;
    if decoder.position() != bytes.len() as u64 {
        return Err(storage("event overlay has trailing bytes"));
    }
    if usize::from(overlay.native_length) != native.len()
        || usize::from(overlay.source_length) > MAX_BYTES
        || usize::from(overlay.source_length) != native.len() + overlay.quotes.len()
        || <[u8; 32]>::from(Sha256::digest(native)) != overlay.native_digest
    {
        return Err(storage("native event overlay authentication failed"));
    }
    let mut source = Vec::with_capacity(usize::from(overlay.source_length));
    let mut previous = None;
    let mut cursor = 0;
    for quote in overlay.quotes {
        let quote = usize::from(quote);
        if quote > native.len() || previous.is_some_and(|last| last >= quote) {
            return Err(storage(
                "event overlay quote positions are not strictly ordered",
            ));
        }
        source.extend_from_slice(&native[cursor..quote]);
        source.push(b'"');
        cursor = quote;
        previous = Some(quote);
    }
    source.extend_from_slice(&native[cursor..]);
    if <[u8; 32]>::from(Sha256::digest(&source)) != overlay.source_digest {
        return Err(storage("event overlay source authentication failed"));
    }
    Ok(source)
}

#[cfg(test)]
mod tests;
