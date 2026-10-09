use crate::{Client, Error, Reason, Result, models::NovelContent, transport::Transport};
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct NovelContentRequest {
    pub novel_id: i64,
}
impl<T: Transport> Client<T> {
    pub async fn novel_content(&self, request: NovelContentRequest) -> Result<NovelContent> {
        if request.novel_id <= 0 {
            return Err(Error::new(Reason::InvalidArgument, "NovelContent")
                .with_detail("novel ID must be positive"));
        }
        Err(Error::new(Reason::ContentUnavailable, "NovelContent")
            .with_detail("novel content is unsupported by the v1 App API"))
    }
}
