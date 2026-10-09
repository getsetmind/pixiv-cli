use pixiv_app::database::{Database, PixivAccount};
use std::path::Path;
pub fn seed(case: &serde_json::Value, home: &Path) {
    if case["seed"].as_bool() != Some(true) {
        return;
    }
    let mut db = Database::open(home.join(".pixiv-cli")).unwrap();
    for id in [2, 1] {
        let mut a = PixivAccount::new(
            id,
            if id == 2 { "合成-user" } else { "" },
            b"synthetic-secret",
        );
        a.pool_last_selected = id == 2;
        if case["mixed_max"].as_bool() == Some(true) && id == 1 {
            a.pool_frozen_until = Some(4102444800);
        }
        if id == 2 {
            a.pool_frozen_until = Some(if case["max_year"].as_bool() == Some(true) {
                i64::MAX
            } else if case["large_year"].as_bool() == Some(true) {
                253402300800
            } else {
                4102444800
            });
            a.premium_status = Some(false);
        }
        db.save_pixiv_credential(&a).unwrap();
    }
    db.set_pixiv_schedulable(&[2], false).unwrap();
}
pub fn assert_states(case: &serde_json::Value, home: &Path) {
    if case["database"].as_bool() != Some(true) {
        return;
    }
    let db = Database::open(home.join(".pixiv-cli")).unwrap();
    let states=db.list_pixiv().unwrap().into_iter().map(|a|serde_json::json!({"id":a.user_id,"revision":a.credential_revision,"schedulable":i64::from(a.schedulable),"frozen":a.pool_frozen_until,"marker":i64::from(a.pool_last_selected)})).collect::<Vec<_>>();
    assert_eq!(
        serde_json::to_value(states).unwrap(),
        case["states"],
        "{}",
        case["name"]
    );
}
