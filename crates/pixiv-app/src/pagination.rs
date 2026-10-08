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
pub struct Stream<F, I, K> {
    pub fetch: F,
    pub include: I,
    pub checkpoint: K,
}
#[derive(Clone, Debug, Default)]
pub struct StreamState<C> {
    pub current: i64,
    pub cursors: Vec<C>,
}
pub struct StreamCollection<T, C> {
    pub items: Vec<T>,
    pub state: StreamState<C>,
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
    C: Cursor + Default,
    F: FnMut(C) -> U,
    U: Future<Output = Result<(Vec<T>, C), E>>,
    I: FnMut(&T) -> Result<bool, E>,
    K: FnMut(C, usize) -> Result<C, E>,
{
    if let Some(checkpoint) = checkpoint {
        let mut streams = [Stream {
            fetch,
            include,
            checkpoint,
        }];
        let page = collect_streams(
            plan,
            &mut streams,
            StreamState {
                current: 0,
                cursors: vec![initial],
            },
        )
        .await?;
        let next = if page.state.current == 0 {
            page.state
                .cursors
                .into_iter()
                .next()
                .expect("single stream cursor exists")
        } else {
            C::default()
        };
        return Ok(Collection {
            items: page.items,
            next,
            result: page.result,
        });
    }
    let mut items = vec![];
    let traversal = traverse_pages(plan, initial, fetch, include, None::<K>, |batch| {
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

pub async fn collect_streams<T, C, E, F, U, I, K>(
    plan: Plan,
    streams: &mut [Stream<F, I, K>],
    initial: StreamState<C>,
) -> Result<StreamCollection<T, C>, Failure<E>>
where
    C: Cursor + Default,
    F: FnMut(C) -> U,
    U: Future<Output = Result<(Vec<T>, C), E>>,
    I: FnMut(&T) -> Result<bool, E>,
    K: FnMut(C, usize) -> Result<C, E>,
{
    for (value, message) in [
        (plan.skip, "page skip must be zero or positive"),
        (plan.limit, "page limit must be zero or positive"),
    ] {
        if value < 0 {
            return Err(stream_message(message));
        }
    }
    let count = streams.len();
    if initial.current < 0 || initial.current > count as i64 {
        return Err(stream_message("stream state current index is out of range"));
    }
    if !initial.cursors.is_empty() && initial.cursors.len() != count {
        return Err(stream_message(
            "stream state cursor count does not match stream count",
        ));
    }
    let mut state = initial;
    if state.cursors.is_empty() {
        state.cursors.resize_with(count, C::default);
    }
    let mut items = vec![];
    let mut result = PageResult::default();
    let mut skip = plan.skip as usize;
    let limit = plan.limit as usize;
    let mut seeking = skip > 0;
    while state.current < count as i64 {
        let index = state.current as usize;
        let stream = &mut streams[index];
        let mut cursor = state.cursors[index].clone();
        let mut seen = BTreeSet::new();
        loop {
            if !seen.insert(cursor.text().to_owned()) {
                return Err(stream_message(format!(
                    "pagination stream {index} cursor repeated: {}",
                    cursor.text()
                )));
            }
            let (batch, next) = (stream.fetch)(cursor.clone())
                .await
                .map_err(stream_source)?;
            let mut matched = vec![];
            for (position, item) in batch.into_iter().enumerate() {
                if (stream.include)(&item).map_err(stream_source)? {
                    matched.push((position + 1, item));
                }
            }
            let removed = skip.min(matched.len());
            skip -= removed;
            matched.drain(..removed);
            if seeking && skip == 0 && !matched.is_empty() {
                seeking = false;
            }
            let returned = !matched.is_empty();
            if limit > 0 && matched.len() > limit - result.returned {
                let remaining = limit - result.returned;
                let checkpoint =
                    (stream.checkpoint)(cursor, matched[remaining - 1].0).map_err(stream_source)?;
                if checkpoint.is_zero() {
                    return Err(stream_message("stream checkpoint cursor must not be zero"));
                }
                matched.truncate(remaining);
                state.cursors[index] = checkpoint;
                result.has_more = true;
            }
            result.returned += matched.len();
            items.extend(matched.into_iter().map(|(_, item)| item));
            if limit > 0 && result.returned >= limit {
                if !result.has_more {
                    state.cursors[index] = next;
                    if state.cursors[index].is_zero() {
                        state.current += 1;
                    }
                    result.has_more = state.current < count as i64;
                }
                return Ok(StreamCollection {
                    items,
                    state,
                    result,
                });
            }
            if plan.one_batch && !seeking && returned {
                state.cursors[index] = next;
                if state.cursors[index].is_zero() {
                    state.current += 1;
                }
                result.has_more = state.current < count as i64;
                return Ok(StreamCollection {
                    items,
                    state,
                    result,
                });
            }
            state.cursors[index] = next.clone();
            if next.is_zero() {
                state.current += 1;
                break;
            }
            cursor = next;
        }
    }
    Ok(StreamCollection {
        items,
        state,
        result,
    })
}

fn stream_message<E>(message: impl Into<String>) -> Failure<E> {
    Failure {
        cause: Cause::Message(message.into()),
        result: PageResult::default(),
    }
}
fn stream_source<E>(error: E) -> Failure<E> {
    Failure {
        cause: Cause::Source(error),
        result: PageResult::default(),
    }
}
