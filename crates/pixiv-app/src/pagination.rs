use std::{collections::BTreeSet, fmt, future::Future};

pub trait Cursor: Clone {
    fn is_zero(&self) -> bool;
    fn text(&self) -> &str;
}
impl Cursor for pixiv_sdk::cursor::Cursor {
    fn is_zero(&self) -> bool {
        self.is_zero()
    }
    fn text(&self) -> &str {
        self.as_str()
    }
}
#[derive(Clone, Copy, Debug, Default)]
pub struct Plan {
    pub skip: i64,
    pub limit: i64,
    pub one_batch: bool,
}
#[derive(Clone, Debug, Default)]
pub struct PageResult {
    pub returned: usize,
    pub has_more: bool,
}
pub struct Traversal<C> {
    pub next: C,
    pub result: PageResult,
}
pub struct Collection<T, C> {
    pub items: Vec<T>,
    pub next: C,
    pub result: PageResult,
}
#[derive(Debug)]
pub enum Cause<E> {
    Source(E),
    Message(String),
}
#[derive(Debug)]
pub struct Failure<E> {
    pub cause: Cause<E>,
    pub result: PageResult,
}
impl<E: fmt::Display> fmt::Display for Failure<E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.cause {
            Cause::Source(error) => error.fmt(formatter),
            Cause::Message(message) => formatter.write_str(message),
        }
    }
}

pub async fn traverse_pages<T, C, E, F, U, I, K, S>(
    plan: Plan,
    initial: C,
    mut fetch: F,
    mut include: I,
    mut checkpoint: Option<K>,
    mut consume: S,
) -> Result<Traversal<C>, Failure<E>>
where
    C: Cursor,
    F: FnMut(C) -> U,
    U: Future<Output = Result<(Vec<T>, C), E>>,
    I: FnMut(&T) -> Result<bool, E>,
    K: FnMut(C, usize) -> Result<C, E>,
    S: FnMut(Vec<T>) -> Result<(), E>,
{
    let mut result = PageResult::default();
    for (value, message) in [
        (plan.skip, "page skip must be zero or positive"),
        (plan.limit, "page limit must be zero or positive"),
    ] {
        if value < 0 {
            return Err(Failure {
                cause: Cause::Message(message.into()),
                result,
            });
        }
    }
    let mut skip = plan.skip as usize;
    let limit = plan.limit as usize;
    let mut cursor = initial;
    let mut seen = BTreeSet::new();
    loop {
        if !seen.insert(cursor.text().to_owned()) {
            let scope = if checkpoint.is_some() {
                "stream 0 "
            } else {
                ""
            };
            return Err(Failure {
                cause: Cause::Message(format!(
                    "pagination {scope}cursor repeated: {}",
                    cursor.text()
                )),
                result,
            });
        }
        let (batch, upstream_next) = fetch(cursor.clone()).await.map_err(|error| Failure {
            cause: Cause::Source(error),
            result: result.clone(),
        })?;
        let mut matched = vec![];
        for (index, item) in batch.into_iter().enumerate() {
            if include(&item).map_err(|error| Failure {
                cause: Cause::Source(error),
                result: result.clone(),
            })? {
                matched.push((index + 1, item));
            }
        }
        let removed = skip.min(matched.len());
        skip -= removed;
        matched.drain(..removed);
        let mut next = upstream_next;
        if limit > 0 && matched.len() > limit - result.returned {
            let remaining = limit - result.returned;
            if let Some(checkpoint) = checkpoint.as_mut() {
                next = checkpoint(cursor, matched[remaining - 1].0).map_err(|error| Failure {
                    cause: Cause::Source(error),
                    result: result.clone(),
                })?;
                if next.is_zero() {
                    return Err(Failure {
                        cause: Cause::Message("stream checkpoint cursor must not be zero".into()),
                        result,
                    });
                }
            }
            matched.truncate(remaining);
            result.has_more = true;
        }
        let count = matched.len();
        if count > 0 {
            consume(matched.into_iter().map(|(_, item)| item).collect()).map_err(|error| {
                Failure {
                    cause: Cause::Source(error),
                    result: result.clone(),
                }
            })?;
            result.returned += count;
        }
        if (limit > 0 && result.returned >= limit)
            || (plan.one_batch && skip == 0 && count > 0)
            || next.is_zero()
        {
            result.has_more |= !next.is_zero();
            return Ok(Traversal { next, result });
        }
        cursor = next;
    }
}

pub async fn collect_pages<T, C, E, F, U, I, K>(
    plan: Plan,
    initial: C,
    fetch: F,
    include: I,
    checkpoint: Option<K>,
) -> Result<Collection<T, C>, Failure<E>>
where
    C: Cursor,
    F: FnMut(C) -> U,
    U: Future<Output = Result<(Vec<T>, C), E>>,
    I: FnMut(&T) -> Result<bool, E>,
    K: FnMut(C, usize) -> Result<C, E>,
{
    let mut items = vec![];
    let traversal = traverse_pages(plan, initial, fetch, include, checkpoint, |batch| {
        items.extend(batch);
        Ok(())
    })
    .await
    .map_err(|mut error| {
        error.result = PageResult::default();
        error
    })?;
    Ok(Collection {
        items,
        next: traversal.next,
        result: traversal.result,
    })
}
