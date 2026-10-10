use super::{DownloadReport, DownloadSaveClient, failure, message};
use crate::{
    lifecycle::Context,
    pagination::{Cause, Plan, traverse_pages},
    scheduler::SchedulerError,
};
use pixiv_sdk::{
    cursor::Cursor,
    models::Artwork,
    pixiv::{UserArtworkBookmarksRequest, UserArtworksRequest},
    reference::Reference,
};
use std::collections::BTreeSet;

enum ListError {
    Client(SchedulerError),
    MissingCapability(&'static str),
}
fn list_error(cause: Cause<ListError>) -> Result<SchedulerError, SchedulerError> {
    match cause {
        Cause::Source(ListError::Client(error)) => Ok(error),
        Cause::Source(ListError::MissingCapability(error)) => Err(message(error)),
        Cause::Message(error) => Ok(message(error)),
    }
}
fn append_artworks(ids: &mut BTreeSet<i64>, items: Vec<Artwork>) -> Result<(), ListError> {
    ids.extend(items.into_iter().map(|item| item.id).filter(|id| *id > 0));
    Ok(())
}
pub(super) async fn collect_user_artworks(
    context: &Context,
    client: &(impl DownloadSaveClient + ?Sized),
    user: &Reference,
    ids: &mut BTreeSet<i64>,
    report: &mut DownloadReport,
) -> Result<(), SchedulerError> {
    for kind in ["illustration", "manga", "ugoira"] {
        if let Some(error) = context.error() {
            return Err(error.into());
        }
        let result = traverse_pages(
            Plan::default(),
            Cursor::default(),
            |cursor| async move {
                let future = client
                    .user_artworks(
                        context.clone(),
                        UserArtworksRequest {
                            user_id: user.id,
                            kind: kind.into(),
                            cursor,
                        },
                    )
                    .ok_or(ListError::MissingCapability(
                        "download client does not support user artwork listing",
                    ))?;
                let page = future.await.map_err(ListError::Client)?;
                Ok((page.items, page.next))
            },
            |_: &Artwork| Ok(true),
            None::<fn(Cursor, usize) -> Result<Cursor, ListError>>,
            |items| append_artworks(ids, items),
        )
        .await;
        if let Err(error) = result {
            let error = list_error(error.cause)?;
            if let Some(error) = context.error() {
                return Err(error.into());
            }
            report.failures.push(failure(
                user.canonical_url().unwrap_or_default(),
                kind,
                error,
            ));
        }
    }
    Ok(())
}
pub(super) async fn collect_user_bookmarks(
    context: &Context,
    client: &(impl DownloadSaveClient + ?Sized),
    user: &Reference,
    ids: &mut BTreeSet<i64>,
) -> Result<(), SchedulerError> {
    traverse_pages(
        Plan::default(),
        Cursor::default(),
        |cursor| async move {
            let future = client
                .user_artwork_bookmarks(
                    context.clone(),
                    UserArtworkBookmarksRequest {
                        user_id: user.id,
                        restrict: "public".into(),
                        tag: String::new(),
                        cursor,
                    },
                )
                .ok_or(ListError::MissingCapability(
                    "download client does not support user artwork bookmark listing",
                ))?;
            let page = future.await.map_err(ListError::Client)?;
            Ok((page.items, page.next))
        },
        |_: &Artwork| Ok(true),
        None::<fn(Cursor, usize) -> Result<Cursor, ListError>>,
        |items| append_artworks(ids, items),
    )
    .await
    .map(|_| ())
    .map_err(|error| match list_error(error.cause) {
        Ok(error) | Err(error) => error,
    })
}
