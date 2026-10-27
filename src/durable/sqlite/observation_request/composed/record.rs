//! Closed private metadata records; original JSON source bytes never enter this codec.
//!
//! `MessagePack` structs use positional fields, byte containers stay bytes, and
//! bounded visitors reject hostile declared lengths before reserving a vector.
//! SQLite schema ownership rejects earlier encodings; there is no legacy decoder.
use super::{ConversationStoreError, storage};
use serde::{Deserialize, Serialize, de};
use std::io::Write as _;

pub(super) fn encode<T: Serialize>(
    value: &T,
    maximum: usize,
) -> Result<Vec<u8>, ConversationStoreError> {
    struct Writer {
        bytes: Vec<u8>,
        maximum: usize,
    }
    impl std::io::Write for Writer {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if bytes.len() > self.maximum.saturating_sub(self.bytes.len()) {
                return Err(std::io::ErrorKind::WriteZero.into());
            }
            self.bytes.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut output = Writer {
        bytes: Vec::new(),
        maximum,
    };
    value
        .serialize(
            &mut rmp_serde::Serializer::new(&mut output)
                .with_struct_tuple()
                .with_bytes(rmp_serde::config::BytesMode::ForceAll),
        )
        .map_err(|_| storage("composed metadata exceeds encoding bound"))?;
    output.flush().map_err(|error| storage(error.to_string()))?;
    Ok(output.bytes)
}

pub(super) fn decode<T: de::DeserializeOwned>(bytes: &[u8]) -> Result<T, ConversationStoreError> {
    // All record roots are closed structs encoded as arrays. Reject named maps
    // and previous JSON records rather than accepting a second storage format.
    if !matches!(bytes.first(), Some(0x90..=0x9f | 0xdc | 0xdd)) {
        return Err(storage("unsupported composed metadata encoding"));
    }
    // ReadReader::read_slice uses take(length).read_to_end: allocation follows
    // actual bytes, not an untrusted string/bin length. The input already has its
    // SQL byte bound; Cursor also provides exact trailing-byte rejection.
    let mut decoder = rmp_serde::Deserializer::new(std::io::Cursor::new(bytes));
    decoder.set_max_depth(32);
    let value = T::deserialize(&mut decoder).map_err(|_| storage("composed metadata malformed"))?;
    if decoder.position() != bytes.len() as u64 {
        return Err(storage("composed metadata trailing bytes"));
    }
    Ok(value)
}

pub(super) fn widths<'de, D: de::Deserializer<'de>>(decoder: D) -> Result<Vec<u64>, D::Error> {
    sequence::<_, _, 80>(decoder)
}
pub(super) fn variants<'de, D: de::Deserializer<'de>>(decoder: D) -> Result<Vec<u8>, D::Error> {
    sequence::<_, _, 80>(decoder)
}
pub(super) fn templates<'de, D: de::Deserializer<'de>>(
    decoder: D,
) -> Result<Vec<super::index::Hash>, D::Error> {
    sequence::<_, _, 80>(decoder)
}
fn sequence<'de, D, T, const MAXIMUM: usize>(decoder: D) -> Result<Vec<T>, D::Error>
where
    D: de::Deserializer<'de>,
    T: Deserialize<'de>,
{
    struct Bounded<T, const MAXIMUM: usize>(std::marker::PhantomData<T>);
    impl<'de, T: Deserialize<'de>, const MAXIMUM: usize> de::Visitor<'de> for Bounded<T, MAXIMUM> {
        type Value = Vec<T>;
        fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(formatter, "at most {MAXIMUM} metadata elements")
        }
        fn visit_seq<A: de::SeqAccess<'de>>(self, mut sequence: A) -> Result<Vec<T>, A::Error> {
            let length = sequence.size_hint().unwrap_or(0);
            if length > MAXIMUM {
                return Err(de::Error::custom(
                    "composed metadata element count exceeds bound",
                ));
            }
            let mut values = Vec::with_capacity(length);
            while let Some(value) = sequence.next_element()? {
                if values.len() == MAXIMUM {
                    return Err(de::Error::custom(
                        "composed metadata element count exceeds bound",
                    ));
                }
                values.push(value);
            }
            Ok(values)
        }
    }
    decoder.deserialize_seq(Bounded::<T, MAXIMUM>(std::marker::PhantomData))
}

#[cfg(test)]
#[path = "record_tests.rs"]
mod tests;
