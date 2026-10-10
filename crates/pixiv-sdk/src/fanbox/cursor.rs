use super::{Client, Failure, identity::json_name_eq};
use crate::{
    Reason, Result,
    context::RequestContext,
    cursor::{Cursor, CursorOptions},
};
use serde::{
    Deserialize, Deserializer,
    de::{MapAccess, Visitor},
};
use std::{fmt, sync::Arc};
fn digest(query: &[(&str, &str)]) -> String {
    let query = query
        .iter()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect::<std::collections::BTreeMap<_, _>>();
    crate::continuation::query_digest(&query)
}
fn scoped(op: &str) -> bool {
    matches!(op, "Creators" | "Home" | "Supporting")
}
impl Client {
    async fn verified_user_id(&self, context: Arc<dyn RequestContext>) -> Result<i64> {
        let mut id = self.user_id.lock().await;
        if *id > 0 {
            return Ok(*id);
        }
        let value = self
            .session
            .current_user(context)
            .await
            .map_err(|e| e.classify("CurrentUser"))?;
        if value.user_id <= 0 {
            return Err(Failure::new(
                Reason::MalformedUpstreamResponse,
                "FANBOX identity has no valid user id",
            )
            .classify("CurrentUser"));
        }
        *id = value.user_id;
        Ok(*id)
    }
    pub(super) async fn build_cursor(
        &self,
        context: Arc<dyn RequestContext>,
        op: &str,
        query: &[(&str, &str)],
        url: &str,
    ) -> Result<Cursor> {
        if url.is_empty() {
            return Ok(Cursor::default());
        }
        #[derive(serde::Serialize)]
        struct Payload<'a> {
            u: &'a str,
        }
        let payload = serde_json::to_string(&Payload { u: url })
            .expect("URL serialization")
            .replace('<', "\\u003c")
            .replace('>', "\\u003e")
            .replace('&', "\\u0026")
            .replace('\u{2028}', "\\u2028")
            .replace('\u{2029}', "\\u2029");
        let identity = if scoped(op) {
            self.verified_user_id(context).await?.to_string()
        } else {
            String::new()
        };
        Cursor::new(
            "fanbox",
            op,
            1,
            &digest(query),
            payload.as_bytes(),
            CursorOptions {
                identity,
                ..Default::default()
            },
        )
    }
    pub(super) async fn continuation_url(
        &self,
        context: Arc<dyn RequestContext>,
        op: &str,
        query: &[(&str, &str)],
        cur: &Cursor,
    ) -> Result<String> {
        if cur.is_zero() {
            return Ok(String::new());
        }
        if let Err(error) = cur.validate("fanbox", op, 1, &digest(query)) {
            return Err(Failure::new(Reason::InvalidCursor, error.to_string()).classify(op));
        }
        if scoped(op) {
            let identity = cur.identity().ok_or_else(|| {
                Failure::new(Reason::InvalidCursor, "cursor is not bound to an account")
                    .classify(op)
            })?;
            if self.verified_user_id(context).await?.to_string() != identity {
                return Err(Failure::new(
                    Reason::InvalidCursor,
                    "cursor belongs to a different account",
                )
                .classify(op));
            }
        }
        let payload = cur
            .payload()
            .map_err(|e| Failure::new(Reason::InvalidCursor, e.to_string()).classify(op))?;
        let normalized = crate::codec::normalize_json(&payload).map_err(|_| {
            Failure::new(Reason::InvalidCursor, "cursor payload is malformed").classify(op)
        })?;
        let payload: Payload = serde_json::from_str(&normalized).map_err(|_| {
            Failure::new(Reason::InvalidCursor, "cursor payload is malformed").classify(op)
        })?;
        if payload.0.is_empty() {
            return Err(
                Failure::new(Reason::InvalidCursor, "cursor payload is malformed").classify(op),
            );
        }
        Ok(payload.0)
    }
}
struct Payload(String);
impl<'de> Deserialize<'de> for Payload {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Payload;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("object or null")
            }
            fn visit_unit<E: serde::de::Error>(self) -> std::result::Result<Payload, E> {
                Ok(Payload(String::new()))
            }
            fn visit_map<M: MapAccess<'de>>(
                self,
                mut map: M,
            ) -> std::result::Result<Payload, M::Error> {
                let mut value = String::new();
                while let Some(key) = map.next_key::<String>()? {
                    if json_name_eq(&key, "u") {
                        if let Some(next) = map.next_value::<Option<String>>()? {
                            value = next;
                        }
                    } else {
                        map.next_value::<serde::de::IgnoredAny>()?;
                    }
                }
                Ok(Payload(value))
            }
        }
        d.deserialize_any(V)
    }
}
