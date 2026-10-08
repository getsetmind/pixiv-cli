use pixiv_app::pagination::{
    Cursor, Failure, Plan, Stream, StreamCollection, StreamState, collect_streams,
};
use serde::Deserialize;
use std::{cell::RefCell, future::ready};

#[derive(Clone, Default)]
struct Position(String);
impl Cursor for Position {
    fn is_zero(&self) -> bool {
        self.0.is_empty()
    }
    fn text(&self) -> &str {
        &self.0
    }
}
#[derive(Deserialize)]
struct State {
    current: i64,
    cursors: Option<Vec<String>>,
}
#[derive(Deserialize)]
struct Outcome {
    items: Option<Vec<i64>>,
    state: State,
    returned: usize,
    more: bool,
    error: String,
}
#[derive(Deserialize)]
struct Case {
    skip: i64,
    limit: i64,
    one: bool,
    odd: bool,
    initial: State,
    fault: String,
    first: Outcome,
    resume: Option<Outcome>,
    fetches: Vec<String>,
    checkpoints: Vec<String>,
}
fn compare(
    result: Result<StreamCollection<i64, Position>, Failure<String>>,
    expected: &Outcome,
) -> Option<StreamState<Position>> {
    match result {
        Ok(page) => {
            assert_eq!(Some(page.items), expected.items);
            assert!(expected.error.is_empty());
            assert_eq!(page.result.returned, expected.returned);
            assert_eq!(page.result.has_more, expected.more);
            assert_eq!(page.state.current, expected.state.current);
            assert_eq!(
                Some(
                    page.state
                        .cursors
                        .iter()
                        .map(|cursor| cursor.0.clone())
                        .collect::<Vec<_>>()
                ),
                expected.state.cursors
            );
            page.result.has_more.then_some(page.state)
        }
        Err(error) => {
            assert_eq!(error.to_string(), expected.error);
            assert!(expected.items.is_none());
            assert_eq!(error.result.returned, expected.returned);
            assert_eq!(error.result.has_more, expected.more);
            assert_eq!(expected.state.current, 0);
            assert!(expected.state.cursors.is_none());
            None
        }
    }
}
fn decode(cursor: &Position) -> (usize, usize) {
    if cursor.0.is_empty() {
        return (0, 0);
    };
    let parts = cursor.0.split(':').collect::<Vec<_>>();
    (parts[1].parse().unwrap(), parts[2].parse().unwrap())
}
#[tokio::test]
async fn streams_match_go_global_windows_ordered_state_resumption_and_atomic_failures() {
    let cases: Vec<Case> = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/streams.json"
    ))
    .unwrap();
    assert_eq!(cases.len(), 3072);
    for case in cases {
        let fetches = RefCell::new(vec![]);
        let checkpoints = RefCell::new(vec![]);
        let fetch_log = &fetches;
        let checkpoint_log = &checkpoints;
        let fault = &case.fault;
        let odd = case.odd;
        let mut streams = (0..2)
            .map(|index| Stream {
                fetch: move |cursor: Position| {
                    fetch_log.borrow_mut().push(format!("{index}|{}", cursor.0));
                    let (page, offset) = decode(&cursor);
                    ready(if fault == "fetch" && index == 1 {
                        Err("fixture stream fetch failed".to_owned())
                    } else if fault == "cycle" && index == 0 && page == 1 {
                        Ok((vec![], cursor))
                    } else {
                        let pages = if index == 0 {
                            vec![vec![1, 2, 1], vec![], vec![3, 4]]
                        } else {
                            vec![vec![5, 6, 5], vec![7, 8]]
                        };
                        let next = if page + 1 < pages.len() {
                            Position(format!("{index}:{}:0", page + 1))
                        } else {
                            Position::default()
                        };
                        Ok((pages[page][offset..].to_vec(), next))
                    })
                },
                include: move |value: &i64| {
                    if fault == "predicate" && *value == 6 {
                        Err("fixture stream predicate failed".to_owned())
                    } else {
                        Ok(!odd || value % 2 == 1)
                    }
                },
                checkpoint: move |cursor: Position, consumed: usize| {
                    checkpoint_log
                        .borrow_mut()
                        .push(format!("{index}|{}|{consumed}", cursor.0));
                    let (page, offset) = decode(&cursor);
                    match fault.as_str() {
                        "checkpoint" => Err("fixture stream checkpoint failed".to_owned()),
                        "zero" => Ok(Position::default()),
                        _ => Ok(Position(format!("{index}:{page}:{}", offset + consumed))),
                    }
                },
            })
            .collect::<Vec<_>>();
        let initial = StreamState {
            current: case.initial.current,
            cursors: case
                .initial
                .cursors
                .unwrap_or_default()
                .into_iter()
                .map(Position)
                .collect(),
        };
        let state = compare(
            collect_streams(
                Plan {
                    skip: case.skip,
                    limit: case.limit,
                    one_batch: case.one,
                },
                &mut streams,
                initial,
            )
            .await,
            &case.first,
        );
        if let Some(state) = state {
            compare(
                collect_streams(Plan::default(), &mut streams, state).await,
                case.resume.as_ref().unwrap(),
            );
        } else {
            assert!(case.resume.is_none());
        }
        assert_eq!(*fetches.borrow(), case.fetches);
        assert_eq!(*checkpoints.borrow(), case.checkpoints);
    }
}
