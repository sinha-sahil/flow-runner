//! Values as a token stores them: a mirror of temple's [`Value`] that
//! serializes compactly and exactly, with decimals as their digits.

use crate::common::limits::{MAX_DECIMAL_TEXT, MAX_VALUE_DEPTH};
use indexmap::IndexMap;
use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use smol_str::SmolStr;
use std::fmt;
use temple_dsl::Value;

/// A value inside a token. Each variant serializes under a one-letter name,
/// so tagging it costs three bytes of CBOR; [`TAG`] counts on that.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(crate) enum Stored {
    #[serde(rename = "N")]
    Null,
    #[serde(rename = "B")]
    Bool(bool),
    #[serde(rename = "I")]
    Int(i64),
    #[serde(rename = "D")]
    Decimal(String),
    #[serde(rename = "S")]
    Str(Text),
    #[serde(rename = "A")]
    List(Vec<Stored>),
    #[serde(rename = "O")]
    Map(Vec<(Text, Stored)>),
}

/// Text in a token. `SmolStr`'s own deserializer asks for a borrowed `&str`,
/// which the CBOR decoder can only give for text that fits its 4 KB scratch
/// buffer. Asking for "any" gets a `&str` for short text, without
/// allocating, and a `String` for long text, so text of any length loads.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Text(pub SmolStr);

impl std::ops::Deref for Text {
    type Target = str;
    fn deref(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Text {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Serialize for Text {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for Text {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visit;
        impl Visitor<'_> for Visit {
            type Value = Text;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("text")
            }
            fn visit_str<E: de::Error>(self, text: &str) -> Result<Text, E> {
                Ok(Text(SmolStr::new(text)))
            }
            fn visit_string<E: de::Error>(self, text: String) -> Result<Text, E> {
                Ok(Text(SmolStr::from(text)))
            }
        }
        deserializer.deserialize_any(Visit)
    }
}

/// Why a value can't be stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Refused {
    TooDeep,
    TooLarge,
}

/// Convert, refusing values nested deeper than [`MAX_VALUE_DEPTH`].
pub(crate) fn encode(value: &Value) -> Option<Stored> {
    let mut unlimited = usize::MAX;
    encode_within(value, &mut unlimited).ok()
}

/// Convert, refusing values nested deeper than [`MAX_VALUE_DEPTH`] or larger
/// than `room` bytes once stored. `room` shrinks by exactly what each part
/// takes once encoded, so a value that is too large is refused before it is
/// built.
pub(crate) fn encode_within(value: &Value, room: &mut usize) -> Result<Stored, Refused> {
    encode_at(value, 0, room)
}

/// Convert a name and its value, as a token stores a param, within `room`
/// like [`encode_within`].
pub(crate) fn encode_entry(
    name: SmolStr,
    value: &Value,
    room: &mut usize,
) -> Result<(Text, Stored), Refused> {
    encode_entry_at(name, value, 0, room)
}

fn encode_entry_at(
    name: SmolStr,
    value: &Value,
    depth: usize,
    room: &mut usize,
) -> Result<(Text, Stored), Refused> {
    // a two-element array: the name, then the value
    take(room, 1 + text_len(&name))?;
    Ok((Text(name), encode_at(value, depth, room)?))
}

fn take(room: &mut usize, bytes: usize) -> Result<(), Refused> {
    *room = room.checked_sub(bytes).ok_or(Refused::TooLarge)?;
    Ok(())
}

/// Bytes of a CBOR head carrying `n`: a length, or an integer's magnitude.
fn head(n: u64) -> usize {
    match n {
        0..=23 => 1,
        24..=0xff => 2,
        0x100..=0xffff => 3,
        0x1_0000..=0xffff_ffff => 5,
        _ => 9,
    }
}

/// Bytes of a CBOR text string.
fn text_len(text: &str) -> usize {
    head(text.len() as u64) + text.len()
}

/// Bytes of a variant's name: a one-letter text string.
const NAME: usize = 2;

/// Bytes of a variant's tag: a one-entry map keyed by its name.
const TAG: usize = 1 + NAME;

fn encode_at(value: &Value, depth: usize, room: &mut usize) -> Result<Stored, Refused> {
    if depth > MAX_VALUE_DEPTH {
        return Err(Refused::TooDeep);
    }
    // each branch charges exactly what the value will take once encoded
    Ok(match value {
        Value::Null => {
            // a unit variant is its name alone
            take(room, NAME)?;
            Stored::Null
        }
        Value::Bool(flag) => {
            take(room, TAG + 1)?;
            Stored::Bool(*flag)
        }
        Value::Int(number) => {
            // CBOR stores a negative `n` as the magnitude `-1 - n`
            let magnitude = if *number < 0 { -1 - *number } else { *number };
            take(room, TAG + head(magnitude.unsigned_abs()))?;
            Stored::Int(*number)
        }
        Value::Decimal(number) => {
            let digits = number.to_string();
            take(room, TAG + text_len(&digits))?;
            Stored::Decimal(digits)
        }
        Value::Str(text) => {
            take(room, TAG + text_len(text))?;
            Stored::Str(Text(text.clone()))
        }
        Value::Arr(items) => {
            take(room, TAG + head(items.len() as u64))?;
            Stored::List(
                items
                    .iter()
                    .map(|item| encode_at(item, depth + 1, room))
                    .collect::<Result<_, _>>()?,
            )
        }
        Value::Obj(object) => {
            take(room, TAG + head(object.len() as u64))?;
            Stored::Map(
                object
                    .iter()
                    .map(|(key, item)| encode_entry_at(key.clone(), item, depth + 1, room))
                    .collect::<Result<_, _>>()?,
            )
        }
    })
}

/// Bytes of node arguments and params as a token stores them, counted as
/// [`encode_within`] and [`encode_entry`] count them.
pub(crate) fn state_len(args: &[Stored], params: &[(Text, Stored)]) -> usize {
    let mut counter = Counter(0);
    ciborium::ser::into_writer(&args, &mut counter).expect("counting can't fail");
    ciborium::ser::into_writer(&params, &mut counter).expect("counting can't fail");
    // the head of each list isn't charged
    counter.0 - head(args.len() as u64) - head(params.len() as u64)
}

/// A writer that only counts.
struct Counter(usize);

impl std::io::Write for Counter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0 += bytes.len();
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Convert back, refusing malformed decimals and over-deep values.
pub(crate) fn decode(stored: &Stored) -> Option<Value> {
    decode_at(stored, 0)
}

fn decode_at(stored: &Stored, depth: usize) -> Option<Value> {
    if depth > MAX_VALUE_DEPTH {
        return None;
    }
    Some(match stored {
        Stored::Null => Value::Null,
        Stored::Bool(flag) => Value::Bool(*flag),
        Stored::Int(number) => Value::Int(*number),
        Stored::Decimal(digits) if digits.len() <= MAX_DECIMAL_TEXT => {
            let mut value = Value::Decimal(digits.parse().ok()?);
            // `-0.00` parses as `0.00`: keep the sign the digits have
            if let Value::Decimal(number) = &mut value {
                number.set_sign_negative(digits.starts_with('-'));
            }
            value
        }
        Stored::Decimal(_) => return None,
        Stored::Str(text) => Value::Str(text.0.clone()),
        Stored::List(items) => Value::Arr(
            items
                .iter()
                .map(|item| decode_at(item, depth + 1))
                .collect::<Option<_>>()?,
        ),
        Stored::Map(entries) => {
            let mut object = IndexMap::with_capacity(entries.len());
            for (key, item) in entries {
                object.insert(key.0.clone(), decode_at(item, depth + 1)?);
            }
            Value::Obj(object)
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn size_accounting_matches_the_encoding() {
        let text = |chars: usize| Value::Str("é".repeat(chars).as_str().into());
        let values = [
            Value::Null,
            Value::Bool(true),
            Value::Int(0),
            Value::Int(23),
            Value::Int(24),
            Value::Int(-24),
            Value::Int(-25),
            Value::Int(65_536),
            Value::Int(i64::MIN),
            Value::Int(i64::MAX),
            Value::Decimal("12.50".parse().unwrap()),
            text(0),
            text(11),
            text(12),
            text(40_000),
            Value::Arr((0..300).map(Value::Int).collect()),
            Value::obj([
                ("a", Value::Int(1)),
                ("b".repeat(30).as_str(), Value::Arr(vec![Value::Null; 30])),
            ]),
        ];
        let mut room = usize::MAX;
        let mut args = Vec::new();
        for value in &values {
            let before = room;
            let encoded = encode_within(value, &mut room).unwrap();
            let mut bytes = Vec::new();
            ciborium::ser::into_writer(&encoded, &mut bytes).unwrap();
            assert_eq!(before - room, bytes.len(), "{value:?}");
            args.push(encoded);
        }
        let params: Vec<_> = values
            .iter()
            .map(|value| encode_entry(SmolStr::new("name"), value, &mut room).unwrap())
            .collect();
        assert_eq!(usize::MAX - room, state_len(&args, &params));
    }

    #[test]
    fn values_round_trip_exactly() {
        let value = Value::obj([
            ("d", Value::Decimal("129.9900".parse().unwrap())),
            (
                "list",
                Value::Arr(vec![Value::Null, Value::Bool(true), Value::Int(-4)]),
            ),
            ("s", Value::Str("é".into())),
        ]);
        assert_eq!(decode(&encode(&value).unwrap()).unwrap(), value);
    }

    #[test]
    fn too_deep_values_are_refused() {
        let mut deep = Value::Null;
        for _ in 0..200 {
            deep = Value::Arr(vec![deep]);
        }
        assert!(encode(&deep).is_none());
    }
}
