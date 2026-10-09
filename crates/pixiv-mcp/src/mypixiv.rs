use crate::{CallToolResult, IllustFilter, NovelFilter, SearchIllustInput, UserFilter};
use pixiv_app::scheduler::SchedulerError;
use pixiv_sdk::{
    Client,
    cursor::Cursor,
    models::{Artwork, Novel, UserPreview},
    pixiv::{MyPixivArtworksRequest, MyPixivNovelsRequest, MyPixivUsersRequest},
    transport::Transport,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MyPixiv {
    Artworks,
    Novels,
    Users,
}
impl MyPixiv {
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "mypixiv_illusts" => Some(Self::Artworks),
            "mypixiv_novels" => Some(Self::Novels),
            "mypixiv_users" => Some(Self::Users),
            _ => None,
        }
    }
    pub fn operation(self) -> &'static str {
        match self {
            Self::Artworks => "MyPixivArtworks",
            Self::Novels => "MyPixivNovels",
            Self::Users => "MyPixivUsers",
        }
    }
    fn artworks(self) -> bool {
        self == Self::Artworks
    }
}
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct MyPixivInput {
    pub illust_filter: Option<IllustFilter>,
    pub novel_filter: Option<NovelFilter>,
    pub user_filter: Option<UserFilter>,
    pub page: Option<i64>,
    pub limit: Option<i64>,
}
pub fn my_pixiv_tool(kind: MyPixiv) -> Value {
    serde_json::from_str(match kind {
        MyPixiv::Artworks => include_str!("../schemas/mypixiv-illusts.json"),
        MyPixiv::Novels => include_str!("../schemas/mypixiv-novels.json"),
        MyPixiv::Users => include_str!("../schemas/mypixiv-users.json"),
    })
    .expect("my_pixiv schema is valid JSON")
}
pub(crate) fn decode(kind: MyPixiv, arguments: Option<&Value>) -> Result<MyPixivInput, String> {
    let mut arguments = arguments
        .filter(|value| !value.is_null())
        .cloned()
        .unwrap_or_else(|| json!({}));
    if !arguments.is_object() {
        return Err(format!(
            "invalid params: validating \"arguments\": unmarshaling arguments: json: cannot unmarshal {} into Go value of type map[string]interface {{}}",
            match &arguments {
                Value::Number(_) => "number",
                Value::Bool(_) => "bool",
                value => crate::stdio::value_type(value),
            }
        ));
    }
    crate::search::validate_schema_with_bindings(
        &mut arguments,
        &my_pixiv_tool(kind)["inputSchema"],
        "",
        "invalid params: validating \"arguments\": validating root",
        &|path| {
            (
                "In",
                if path.ends_with("/id") {
                    "int64"
                } else {
                    "int"
                },
            )
        },
    )?;
    serde_json::from_value(arguments).map_err(|error| format!("invalid params: {error}"))
}
fn common(input: &MyPixivInput) -> SearchIllustInput {
    SearchIllustInput {
        illust_filter: input.illust_filter.clone(),
        page: input.page,
        limit: input.limit,
        ..Default::default()
    }
}
fn validate(kind: MyPixiv, input: &mut MyPixivInput) -> Result<crate::search::Plan, String> {
    let mut pagination = common(input);
    if !kind.artworks() {
        pagination.illust_filter = None;
    }
    let plan = crate::search::validate(&mut pagination)?;
    if kind == MyPixiv::Novels {
        crate::novel_search::validate_filter(input.novel_filter.as_ref())?;
    }
    if kind == MyPixiv::Users {
        crate::user_search::validate_filter(input.user_filter.as_ref())?;
    }
    Ok(plan)
}
pub async fn my_pixiv<T: Transport>(
    client: &Client<T>,
    kind: MyPixiv,
    mut input: MyPixivInput,
) -> CallToolResult {
    let plan = match validate(kind, &mut input) {
        Ok(plan) => plan,
        Err(error) => return crate::search::failure(error),
    };
    if kind.artworks() {
        crate::search::search_result(
            collect_artworks(client, kind, &input, &plan).await,
            &common(&input),
            &plan,
        )
    } else if kind == MyPixiv::Users {
        crate::user_relationships::result(
            collect_users(client, &input, &plan).await,
            input.limit,
            &plan,
        )
    } else {
        crate::timeline::novel_result(
            collect_novels(client, kind, &input, &plan).await,
            input.limit,
            &plan,
        )
    }
}
pub(crate) async fn saved_my_pixiv<T: Transport + 'static>(
    execution: &pixiv_app::execution::Execution<T>,
    context: &pixiv_app::lifecycle::Context,
    kind: MyPixiv,
    mut input: MyPixivInput,
    proxy: Option<&str>,
) -> CallToolResult {
    let plan = match validate(kind, &mut input) {
        Ok(plan) => plan,
        Err(error) => return crate::search::failure(error),
    };
    let requested = input.clone();
    if kind.artworks() {
        let output = execution
            .read(context, 0, proxy, move |_, client| {
                let input = requested.clone();
                async move { collect_artworks(&client, kind, &input, &plan).await }
            })
            .await;
        crate::search::search_result(output, &common(&input), &plan)
    } else if kind == MyPixiv::Users {
        let output = execution
            .read(context, 0, proxy, move |_, client| {
                let input = requested.clone();
                async move { collect_users(&client, &input, &plan).await }
            })
            .await;
        crate::user_relationships::result(output, input.limit, &plan)
    } else {
        let output = execution
            .read(context, 0, proxy, move |_, client| {
                let input = requested.clone();
                async move { collect_novels(&client, kind, &input, &plan).await }
            })
            .await;
        crate::timeline::novel_result(output, input.limit, &plan)
    }
}
async fn collect_artworks<T: Transport>(
    client: &Client<T>,
    _kind: MyPixiv,
    input: &MyPixivInput,
    plan: &crate::search::Plan,
) -> Result<(Vec<Artwork>, bool, Option<Value>), SchedulerError> {
    let local = pixiv_app::search_filter::normalize_filter(
        "",
        input
            .illust_filter
            .as_ref()
            .map(|filter| filter.r#type.as_str())
            .unwrap_or_default(),
    )
    .map_err(|error| SchedulerError::Message(error.to_string()))?;
    let mut seen = BTreeSet::new();
    let page = pixiv_app::pagination::collect_pages(
        pixiv_app::pagination::Plan {
            skip: plan.skip,
            limit: plan.limit.max(0),
            one_batch: plan.one_batch,
        },
        Cursor::default(),
        |cursor| async move {
            let page = client
                .my_pixiv_artworks(MyPixivArtworksRequest { cursor })
                .await
                .map_err(SchedulerError::from)?;
            Ok((page.items, page.next))
        },
        |item: &Artwork| {
            Ok(input
                .illust_filter
                .as_ref()
                .is_none_or(|filter| crate::search::matches(item, filter, &local))
                && seen.insert(format!("{:?}:{}", item.kind, item.id)))
        },
        None::<fn(Cursor, usize) -> Result<Cursor, SchedulerError>>,
    )
    .await
    .map_err(|error| match error.cause {
        pixiv_app::pagination::Cause::Source(error) => error,
        pixiv_app::pagination::Cause::Message(message) => SchedulerError::Message(message),
    })?;
    Ok((page.items, page.result.has_more, None))
}
async fn collect_novels<T: Transport>(
    client: &Client<T>,
    _kind: MyPixiv,
    input: &MyPixivInput,
    plan: &crate::search::Plan,
) -> Result<(Vec<Novel>, bool), SchedulerError> {
    let mut seen = BTreeSet::new();
    let page = pixiv_app::pagination::collect_pages(
        pixiv_app::pagination::Plan {
            skip: plan.skip,
            limit: plan.limit.max(0),
            one_batch: plan.one_batch,
        },
        Cursor::default(),
        |cursor| async move {
            let page = client
                .my_pixiv_novels(MyPixivNovelsRequest { cursor })
                .await
                .map_err(SchedulerError::from)?;
            Ok((page.items, page.next))
        },
        |item: &Novel| {
            Ok(
                crate::novel_search::matches(item, input.novel_filter.as_ref())
                    && seen.insert(item.id),
            )
        },
        None::<fn(Cursor, usize) -> Result<Cursor, SchedulerError>>,
    )
    .await
    .map_err(|error| match error.cause {
        pixiv_app::pagination::Cause::Source(error) => error,
        pixiv_app::pagination::Cause::Message(message) => SchedulerError::Message(message),
    })?;
    Ok((page.items, page.result.has_more))
}

async fn collect_users<T: Transport>(
    client: &Client<T>,
    input: &MyPixivInput,
    plan: &crate::search::Plan,
) -> Result<(Vec<UserPreview>, bool), SchedulerError> {
    let mut seen = BTreeSet::new();
    let page = pixiv_app::pagination::collect_pages(
        pixiv_app::pagination::Plan {
            skip: plan.skip,
            limit: plan.limit.max(0),
            one_batch: plan.one_batch,
        },
        Cursor::default(),
        |cursor| async move {
            let page = client
                .my_pixiv_users(MyPixivUsersRequest { cursor })
                .await
                .map_err(SchedulerError::from)?;
            Ok((page.items, page.next))
        },
        |item: &UserPreview| {
            Ok(
                crate::user_search::matches(item, input.user_filter.as_ref())
                    && seen.insert(item.user.id),
            )
        },
        None::<fn(Cursor, usize) -> Result<Cursor, SchedulerError>>,
    )
    .await
    .map_err(|error| match error.cause {
        pixiv_app::pagination::Cause::Source(error) => error,
        pixiv_app::pagination::Cause::Message(message) => SchedulerError::Message(message),
    })?;
    Ok((page.items, page.result.has_more))
}
