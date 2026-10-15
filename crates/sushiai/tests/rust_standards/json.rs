use serde::de::{Error, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer};
use std::collections::BTreeSet;
use std::fmt;

struct UniqueKeys;

impl<'de> Deserialize<'de> for UniqueKeys {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(UniqueKeys)
    }
}

impl<'de> Visitor<'de> for UniqueKeys {
    type Value = Self;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("JSON with unique object keys")
    }

    fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Self, M::Error> {
        let mut keys = BTreeSet::new();
        while let Some(key) = map.next_key::<String>()? {
            if !keys.insert(key.clone()) {
                return Err(M::Error::custom(format!("duplicate JSON key: {key}")));
            }
            map.next_value::<UniqueKeys>()?;
        }
        Ok(self)
    }

    fn visit_seq<S: SeqAccess<'de>>(self, mut sequence: S) -> Result<Self, S::Error> {
        while sequence.next_element::<UniqueKeys>()?.is_some() {}
        Ok(self)
    }

    fn visit_bool<E: Error>(self, _: bool) -> Result<Self, E> {
        Ok(self)
    }

    fn visit_i64<E: Error>(self, _: i64) -> Result<Self, E> {
        Ok(self)
    }

    fn visit_u64<E: Error>(self, _: u64) -> Result<Self, E> {
        Ok(self)
    }

    fn visit_f64<E: Error>(self, _: f64) -> Result<Self, E> {
        Ok(self)
    }

    fn visit_str<E: Error>(self, _: &str) -> Result<Self, E> {
        Ok(self)
    }

    fn visit_unit<E: Error>(self) -> Result<Self, E> {
        Ok(self)
    }
}

pub fn validate(text: &str) -> Result<(), serde_json::Error> {
    let mut deserializer = serde_json::Deserializer::from_str(text);
    UniqueKeys::deserialize(&mut deserializer)?;
    deserializer.end()
}
