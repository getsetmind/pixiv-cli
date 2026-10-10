#[derive(Clone, Debug, Default)]
pub struct Account {
    pub user_id: i64,
    pub https_proxy_override: Option<String>,
}
