use pixiv_app::lifecycle::Lease;
use serde::Deserialize;
use std::sync::{
    Arc, Barrier,
    atomic::{AtomicUsize, Ordering},
};

#[derive(Deserialize)]
struct Case {
    mode: String,
    value: i32,
    calls: usize,
    errors: Vec<String>,
    same_error: bool,
    panicked: bool,
}

#[test]
fn leases_match_go_values_cached_close_results_concurrent_release_and_panic() {
    let cases: Vec<Case> =
        serde_json::from_str(include_str!("../../../docs/migration/contracts/lease.json")).unwrap();
    assert_eq!(cases.len(), 7);
    for case in cases {
        let calls = Arc::new(AtomicUsize::new(0));
        let called = Arc::clone(&calls);
        let mode = case.mode.clone();
        let closer = Box::new(move || {
            called.fetch_add(1, Ordering::SeqCst);
            match mode.as_str() {
                "failure" | "concurrent_failure" => Err("synthetic release failure".to_owned()),
                "panic" => panic!("synthetic release panic"),
                _ => Ok(()),
            }
        });
        let lease = if case.mode == "nil" {
            None
        } else {
            Some(Arc::new(Lease::new(
                37,
                if case.mode == "no_closer" {
                    None
                } else {
                    Some(closer as Box<dyn FnOnce() -> Result<(), String> + Send>)
                },
            )))
        };
        let panicked = if case.mode == "panic" {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                lease.as_ref().unwrap().close()
            }))
            .is_err()
        } else {
            false
        };
        assert_eq!(panicked, case.panicked, "{}", case.mode);
        let count = case.errors.len();
        let start = Arc::new(Barrier::new(count));
        let results: Vec<_> = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..count)
                .map(|_| {
                    let lease = lease.clone();
                    let start = Arc::clone(&start);
                    scope.spawn(move || {
                        start.wait();
                        lease.as_ref().map_or(Ok(()), |lease| lease.close())
                    })
                })
                .collect();
            handles
                .into_iter()
                .map(|handle| handle.join().unwrap())
                .collect()
        });
        let errors: Vec<_> = results
            .iter()
            .map(|result| {
                result
                    .as_ref()
                    .err()
                    .map(|error| error.to_string())
                    .unwrap_or_default()
            })
            .collect();
        let same_error = results.iter().all(|result| match (&results[0], result) {
            (Ok(()), Ok(())) => true,
            (Err(first), Err(error)) => Arc::ptr_eq(first, error),
            _ => false,
        });
        assert_eq!(errors, case.errors, "{}", case.mode);
        assert_eq!(same_error, case.same_error, "{}", case.mode);
        assert_eq!(calls.load(Ordering::SeqCst), case.calls, "{}", case.mode);
        assert_eq!(
            lease.as_ref().map_or(0, |lease| *lease.value()),
            case.value,
            "{}",
            case.mode
        );
    }
}
