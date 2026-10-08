use pixiv_app::pagination::{Cursor, Plan, collect_pages, traverse_pages};
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
struct Case {
    skip: i64,
    limit: i64,
    one: bool,
    filtered: bool,
    fault: String,
    items: Option<Vec<i64>>,
    fetches: Vec<String>,
    checkpoints: Vec<usize>,
    next: String,
    returned: usize,
    more: bool,
    error: String,
}
#[tokio::test]
async fn traversal_matches_go_windows_checkpoints_cycles_and_partial_failure() {
    let cases: Vec<Case> = serde_json::from_str(include_str!(
        "../../../docs/migration/contracts/traversal.json"
    ))
    .unwrap();
    assert_eq!(cases.len(), 840);
    for case in cases {
        let fetches = RefCell::new(vec![]);
        let checkpoints = RefCell::new(vec![]);
        let fetch = |cursor: Position| {
            fetches.borrow_mut().push(cursor.0.clone());
            let index = cursor.0.parse::<usize>().unwrap_or(0);
            ready(if case.fault == "fetch" && index == 1 {
                Err("fixture fetch failed".to_owned())
            } else if case.fault == "cycle" && index == 1 {
                Ok((vec![], cursor))
            } else {
                Ok((
                    vec![vec![1, 2, 3, 2], vec![4, 5], vec![6, 7]][index].clone(),
                    Position(if index < 2 {
                        (index + 1).to_string()
                    } else {
                        String::new()
                    }),
                ))
            })
        };
        let checkpoint = |cursor: Position, consumed: usize| {
            checkpoints.borrow_mut().push(consumed);
            match case.fault.as_str() {
                "checkpoint" => Err("fixture checkpoint failed".to_owned()),
                "zero" => Ok(Position::default()),
                _ => Ok(Position(format!("{}/{consumed}", cursor.0))),
            }
        };
        let plan = Plan {
            skip: case.skip,
            limit: case.limit,
            one_batch: case.one,
        };
        let mut items = vec![];
        let result = if case.filtered {
            collect_pages(
                plan,
                Position::default(),
                fetch,
                |value: &i64| {
                    if case.fault == "predicate" && *value == 4 {
                        Err("fixture predicate failed".to_owned())
                    } else {
                        Ok(value % 2 == 1)
                    }
                },
                Some(checkpoint),
            )
            .await
            .map(|page| {
                items = page.items;
                assert_eq!(page.next.0, case.next);
                page.result
            })
        } else {
            traverse_pages(
                plan,
                Position::default(),
                fetch,
                |_: &i64| Ok(true),
                None::<fn(Position, usize) -> Result<Position, String>>,
                |batch| {
                    if case.fault == "consume" {
                        return Err("fixture consume failed".to_owned());
                    };
                    items.extend(batch);
                    Ok(())
                },
            )
            .await
            .map(|page| page.result)
        };
        let (progress, error) = match result {
            Ok(result) => (result, String::new()),
            Err(error) => (error.result.clone(), error.to_string()),
        };
        assert_eq!(error, case.error);
        assert_eq!(progress.returned, case.returned);
        assert_eq!(progress.has_more, case.more);
        let visible = if case.filtered && !error.is_empty() {
            None
        } else {
            Some(items)
        };
        assert_eq!(visible, case.items);
        assert_eq!(*fetches.borrow(), case.fetches);
        assert_eq!(*checkpoints.borrow(), case.checkpoints);
    }
}
