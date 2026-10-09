use crate::{Error, Reason, Result};
use serde::{
    Deserializer,
    de::{self, DeserializeSeed, IgnoredAny, MapAccess, Visitor},
};
use serde_json::{Map, Value, value::RawValue};
use std::fmt;

#[derive(Clone, Copy)]
enum Kind {
    String,
    Integer,
    Boolean,
    StringPointer,
    Struct(&'static [Field]),
    RequiredStruct(&'static [Field]),
    RequiredList(&'static [Field]),
    Visibility,
}
struct Field {
    name: &'static str,
    kind: Kind,
}
macro_rules! fields {
    ($($name:literal => $kind:expr),* $(,)?) => { &[$(Field { name: $name, kind: $kind }),*] };
}
const IMAGES: &[Field] = fields!("medium" => Kind::StringPointer);
const USER: &[Field] = fields!(
    "id" => Kind::Integer, "name" => Kind::String, "account" => Kind::String,
    "comment" => Kind::String, "is_followed" => Kind::Boolean,
    "profile_image_urls" => Kind::Struct(IMAGES),
);
const PREVIEW: &[Field] = fields!("user" => Kind::Struct(USER));
const SEARCH: &[Field] = fields!(
    "user_previews" => Kind::RequiredList(PREVIEW),
    "next_url" => Kind::StringPointer,
);
const PROFILE: &[Field] = fields!(
    "webpage" => Kind::StringPointer, "gender" => Kind::String,
    "birth" => Kind::String, "birth_day" => Kind::String,
    "birth_year" => Kind::Integer, "region" => Kind::String,
    "address_id" => Kind::Integer, "country_code" => Kind::String,
    "job" => Kind::String, "job_id" => Kind::Integer,
    "total_follow_users" => Kind::Integer, "total_mypixiv_users" => Kind::Integer,
    "total_illusts" => Kind::Integer, "total_manga" => Kind::Integer,
    "total_novels" => Kind::Integer, "total_illust_bookmarks_public" => Kind::Integer,
    "total_illust_series" => Kind::Integer, "total_novel_series" => Kind::Integer,
    "background_image_url" => Kind::StringPointer, "twitter_account" => Kind::String,
    "twitter_url" => Kind::StringPointer, "pawoo_url" => Kind::StringPointer,
    "is_premium" => Kind::Boolean, "is_using_custom_profile_image" => Kind::Boolean,
);
const PUBLICITY: &[Field] = fields!(
    "gender" => Kind::Visibility, "region" => Kind::Visibility,
    "birth_day" => Kind::Visibility, "birth_year" => Kind::Visibility,
    "job" => Kind::Visibility, "pawoo" => Kind::Visibility,
);
const WORKSPACE: &[Field] = fields!(
    "pc" => Kind::String, "monitor" => Kind::String, "tool" => Kind::String,
    "scanner" => Kind::String, "tablet" => Kind::String, "mouse" => Kind::String,
    "printer" => Kind::String, "desktop" => Kind::String, "music" => Kind::String,
    "desk" => Kind::String, "chair" => Kind::String, "comment" => Kind::String,
    "workspace_image_url" => Kind::StringPointer,
);
const DETAIL: &[Field] = fields!(
    "user" => Kind::RequiredStruct(USER), "profile" => Kind::RequiredStruct(PROFILE),
    "profile_publicity" => Kind::RequiredStruct(PUBLICITY),
    "workspace" => Kind::RequiredStruct(WORKSPACE),
);

fn folded_equal(key: &str, name: &str) -> bool {
    key.chars()
        .map(|letter| match letter {
            '\u{017f}' => 'S',
            '\u{212a}' => 'K',
            ascii if ascii.is_ascii() => ascii.to_ascii_uppercase(),
            other => other,
        })
        .eq(name.chars().map(|letter| letter.to_ascii_uppercase()))
}
fn object(
    raw: &str,
    fields: &'static [Field],
    value: &mut Map<String, Value>,
) -> serde_json::Result<()> {
    let mut deserializer = serde_json::Deserializer::from_str(raw);
    Object { fields, value }.deserialize(&mut deserializer)?;
    deserializer.end()
}
struct Object<'a> {
    fields: &'static [Field],
    value: &'a mut Map<String, Value>,
}
impl<'de> Visitor<'de> for Object<'_> {
    type Value = ();
    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("a JSON object")
    }
    fn visit_unit<E: de::Error>(self) -> std::result::Result<(), E> {
        Ok(())
    }
    fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> std::result::Result<(), M::Error> {
        while let Some(key) = map.next_key::<String>()? {
            let field = self
                .fields
                .iter()
                .find(|field| field.name == key)
                .or_else(|| {
                    self.fields
                        .iter()
                        .find(|field| folded_equal(&key, field.name))
                });
            if let Some(field) = field {
                let raw = map.next_value::<&RawValue>()?;
                apply(self.value, field, raw.get()).map_err(de::Error::custom)?;
            } else {
                map.next_value::<IgnoredAny>()?;
            }
        }
        Ok(())
    }
}
impl<'de> DeserializeSeed<'de> for Object<'_> {
    type Value = ();
    fn deserialize<D: Deserializer<'de>>(
        self,
        deserializer: D,
    ) -> std::result::Result<(), D::Error> {
        deserializer.deserialize_any(self)
    }
}
fn invalid() -> serde_json::Error {
    <serde_json::Error as de::Error>::custom("invalid known field")
}
fn apply(values: &mut Map<String, Value>, field: &Field, raw: &str) -> serde_json::Result<()> {
    let raw = raw.trim();
    let value = match field.kind {
        Kind::String | Kind::Integer | Kind::Boolean if raw == "null" => return Ok(()),
        Kind::String | Kind::StringPointer if raw != "null" => {
            Value::String(serde_json::from_str::<String>(raw)?)
        }
        Kind::StringPointer => Value::Null,
        Kind::Integer => Value::from(raw.parse::<i64>().map_err(|_| invalid())?),
        Kind::Boolean => match raw {
            "true" => Value::Bool(true),
            "false" => Value::Bool(false),
            _ => return Err(invalid()),
        },
        Kind::Struct(fields) => {
            if raw == "null" {
                return Ok(());
            }
            let value = values
                .entry(field.name)
                .or_insert_with(|| Value::Object(Map::new()));
            object(
                raw,
                fields,
                value.as_object_mut().expect("struct fields hold objects"),
            )?;
            return Ok(());
        }
        Kind::RequiredStruct(fields) => {
            if raw == "null" {
                Value::Null
            } else {
                let mut value = Map::new();
                object(raw, fields, &mut value)?;
                Value::Object(value)
            }
        }
        Kind::RequiredList(fields) => {
            if raw == "null" {
                Value::Null
            } else {
                let items: Vec<&RawValue> = serde_json::from_str(raw)?;
                let mut values = Vec::with_capacity(items.len());
                for item in items {
                    if item.get().trim() == "null" {
                        values.push(Value::Null);
                    } else {
                        let mut value = Map::new();
                        object(item.get(), fields, &mut value)?;
                        values.push(Value::Object(value));
                    }
                }
                Value::Array(values)
            }
        }
        Kind::Visibility => match raw {
            "true" => Value::Bool(true),
            "false" => Value::Bool(false),
            _ => match serde_json::from_str::<String>(raw).ok().as_deref() {
                Some("public") => Value::Bool(true),
                Some("private") => Value::Bool(false),
                _ => Value::Null,
            },
        },
        Kind::String => unreachable!("null strings were handled"),
    };
    values.insert(field.name.into(), value);
    Ok(())
}
pub(crate) fn decode(raw: &[u8], operation: &'static str) -> Result<Value> {
    let malformed = || Error::new(Reason::MalformedUpstreamResponse, operation);
    let raw = crate::codec::normalize_json(raw).map_err(|_| malformed())?;
    let schema = if operation == "User" { DETAIL } else { SEARCH };
    let mut value = Map::new();
    object(&raw, schema, &mut value).map_err(|_| malformed())?;
    Ok(Value::Object(value))
}
