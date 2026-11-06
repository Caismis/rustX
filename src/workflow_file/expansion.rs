//! Allocation-free YAML expansion preflight. `serde_yaml` owns syntax, alias
//! repetition and recursion. Count the expanded JSON footprint before typed
//! aliases can allocate copies. The exact normalized-document policy is 16 MiB.
use serde::de::{DeserializeSeed, IgnoredAny, MapAccess, SeqAccess, Visitor};

// The extra share accommodates defaults and omitted presentation fields.
const EXPANDED_BYTES: usize = 2 * super::MAX_WORKFLOW_DOCUMENT_BYTES;
const ERROR: &str = "Workflow YAML expansion exceeds the 32 MiB decoder bound";

struct Budget {
    remaining: std::cell::Cell<usize>,
    exceeded: std::cell::Cell<bool>,
}
impl Budget {
    fn charge<E: serde::de::Error>(&self, bytes: usize) -> Result<(), E> {
        let Some(remaining) = self.remaining.get().checked_sub(bytes) else {
            self.exceeded.set(true);
            return Err(E::custom(ERROR));
        };
        self.remaining.set(remaining);
        Ok(())
    }
}
impl std::io::Write for &Budget {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.charge::<serde_json::Error>(bytes.len())
            .map_err(std::io::Error::other)?;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
struct Count<'a>(&'a Budget);
impl Count<'_> {
    fn scalar<E: serde::de::Error>(self, value: &impl serde::Serialize) -> Result<(), E> {
        serde_json::to_writer(self.0, value).map_err(E::custom)
    }
}
impl<'de> DeserializeSeed<'de> for Count<'_> {
    type Value = ();
    fn deserialize<D: serde::Deserializer<'de>>(self, deserializer: D) -> Result<(), D::Error> {
        deserializer.deserialize_any(self)
    }
}
impl<'de> Visitor<'de> for Count<'_> {
    type Value = ();
    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("a bounded Workflow YAML expansion")
    }
    fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<(), E> {
        self.scalar(&value)
    }
    fn visit_string<E: serde::de::Error>(self, value: String) -> Result<(), E> {
        self.visit_str(&value)
    }
    fn visit_bool<E: serde::de::Error>(self, value: bool) -> Result<(), E> {
        self.scalar(&value)
    }
    fn visit_i64<E: serde::de::Error>(self, value: i64) -> Result<(), E> {
        self.scalar(&value)
    }
    fn visit_u64<E: serde::de::Error>(self, value: u64) -> Result<(), E> {
        self.scalar(&value)
    }
    fn visit_f64<E: serde::de::Error>(self, value: f64) -> Result<(), E> {
        self.scalar(&value)
    }
    fn visit_unit<E: serde::de::Error>(self) -> Result<(), E> {
        self.scalar(&())
    }
    fn visit_none<E: serde::de::Error>(self) -> Result<(), E> {
        self.visit_unit()
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut sequence: A) -> Result<(), A::Error> {
        self.0.charge(2)?;
        while sequence.next_element_seed(Count(self.0))?.is_some() {
            self.0.charge(1)?;
        }
        Ok(())
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<(), A::Error> {
        self.0.charge(2)?;
        while map.next_key_seed(Count(self.0))?.is_some() {
            self.0.charge(2)?;
            map.next_value_seed(Count(self.0))?;
        }
        Ok(())
    }
    fn visit_enum<A: serde::de::EnumAccess<'de>>(self, data: A) -> Result<(), A::Error> {
        use serde::de::VariantAccess;
        let (_, variant) = data.variant::<IgnoredAny>()?;
        variant.newtype_variant_seed(Count(self.0))
    }
}

pub(super) fn within_bound(bytes: &[u8]) -> bool {
    let budget = Budget {
        remaining: std::cell::Cell::new(EXPANDED_BYTES),
        exceeded: std::cell::Cell::new(false),
    };
    // Syntax/shape/repetition diagnostics remain the typed parser's authority.
    // Only this explicit flag can classify an expansion refusal.
    let _ = Count(&budget).deserialize(serde_yaml::Deserializer::from_slice(bytes));
    !budget.exceeded.get()
}
