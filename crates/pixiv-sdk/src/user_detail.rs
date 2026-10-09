use crate::{Client, Error, Reason, Result, artwork::WireUser, models::*, transport::Transport};
use serde::Deserialize;
use serde_json::Value;
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct UserRequest {
    pub user_id: i64,
}
#[derive(Default, Deserialize)]
struct WireUserProfile {
    webpage: Option<String>,
    gender: Option<String>,
    #[serde(rename = "birth")]
    unused_birth: Option<String>,
    birth_day: Option<String>,
    birth_year: Option<i64>,
    region: Option<String>,
    #[serde(rename = "address_id")]
    unused_address_id: Option<i64>,
    country_code: Option<String>,
    job: Option<String>,
    #[serde(rename = "job_id")]
    unused_job_id: Option<i64>,
    total_follow_users: Option<i64>,
    total_mypixiv_users: Option<i64>,
    total_illusts: Option<i64>,
    total_manga: Option<i64>,
    total_novels: Option<i64>,
    total_illust_bookmarks_public: Option<i64>,
    total_illust_series: Option<i64>,
    total_novel_series: Option<i64>,
    background_image_url: Option<String>,
    twitter_account: Option<String>,
    twitter_url: Option<String>,
    pawoo_url: Option<String>,
    is_premium: Option<bool>,
    is_using_custom_profile_image: Option<bool>,
}
impl WireUserProfile {
    fn map(self) -> UserProfile {
        let _ = self.unused_birth;
        let _ = self.unused_address_id;
        let _ = self.unused_job_id;
        UserProfile {
            webpage: self.webpage.unwrap_or_default(),
            gender: self.gender.unwrap_or_default(),
            birth_day: self.birth_day.unwrap_or_default(),
            birth_year: self.birth_year.unwrap_or_default(),
            region: self.region.unwrap_or_default(),
            country_code: self.country_code.unwrap_or_default(),
            job: self.job.unwrap_or_default(),
            total_follow_users: self.total_follow_users.unwrap_or_default(),
            total_my_pixiv_users: self.total_mypixiv_users.unwrap_or_default(),
            total_illusts: self.total_illusts.unwrap_or_default(),
            total_manga: self.total_manga.unwrap_or_default(),
            total_novels: self.total_novels.unwrap_or_default(),
            total_illust_bookmarks: self.total_illust_bookmarks_public.unwrap_or_default(),
            total_illust_series: self.total_illust_series.unwrap_or_default(),
            total_novel_series: self.total_novel_series.unwrap_or_default(),
            background_image_url: self.background_image_url.unwrap_or_default(),
            twitter_account: self.twitter_account.unwrap_or_default(),
            twitter_url: self.twitter_url.unwrap_or_default(),
            pawoo_url: self.pawoo_url.unwrap_or_default(),
            is_premium: self.is_premium.unwrap_or_default(),
            is_using_custom_profile_image: self.is_using_custom_profile_image.unwrap_or_default(),
        }
    }
}
#[derive(Default, Deserialize)]
struct WireUserWorkspace {
    pc: Option<String>,
    monitor: Option<String>,
    tool: Option<String>,
    scanner: Option<String>,
    tablet: Option<String>,
    mouse: Option<String>,
    printer: Option<String>,
    desktop: Option<String>,
    music: Option<String>,
    desk: Option<String>,
    chair: Option<String>,
    comment: Option<String>,
    workspace_image_url: Option<String>,
}
impl WireUserWorkspace {
    fn map(self) -> UserWorkspace {
        UserWorkspace {
            pc: self.pc.unwrap_or_default(),
            monitor: self.monitor.unwrap_or_default(),
            tool: self.tool.unwrap_or_default(),
            scanner: self.scanner.unwrap_or_default(),
            tablet: self.tablet.unwrap_or_default(),
            mouse: self.mouse.unwrap_or_default(),
            printer: self.printer.unwrap_or_default(),
            desktop: self.desktop.unwrap_or_default(),
            music: self.music.unwrap_or_default(),
            desk: self.desk.unwrap_or_default(),
            chair: self.chair.unwrap_or_default(),
            comment: self.comment.unwrap_or_default(),
            workspace_image_url: self.workspace_image_url.unwrap_or_default(),
        }
    }
}
#[derive(Deserialize)]
struct Envelope {
    user: Option<WireUser>,
    profile: Option<WireUserProfile>,
    profile_publicity: Option<serde_json::Map<String, Value>>,
    workspace: Option<WireUserWorkspace>,
}
fn publicity(value: serde_json::Map<String, Value>) -> Result<UserProfilePublicity> {
    let flag = |key: &str| -> Result<bool> {
        match value.get(key) {
            None => Ok(false),
            Some(Value::Bool(v)) => Ok(*v),
            Some(Value::String(v)) if v == "public" => Ok(true),
            Some(Value::String(v)) if v == "private" => Ok(false),
            _ => Err(Error::new(Reason::MalformedUpstreamResponse, "User")),
        }
    };
    Ok(UserProfilePublicity {
        gender: flag("gender")?,
        region: flag("region")?,
        birth_day: flag("birth_day")?,
        birth_year: flag("birth_year")?,
        job: flag("job")?,
        pawoo: flag("pawoo")?,
    })
}
impl<T: Transport> Client<T> {
    pub async fn user(&self, request: UserRequest) -> Result<UserDetail> {
        if request.user_id <= 0 {
            return Err(
                Error::new(Reason::InvalidArgument, "User").with_detail("user ID must be positive")
            );
        }
        let body = self
            .get(
                "/v1/user/detail",
                vec![("user_id".into(), request.user_id.to_string())],
                "User",
            )
            .await?;
        let malformed = || Error::new(Reason::MalformedUpstreamResponse, "User");
        let envelope: Envelope = serde_json::from_value(body).map_err(|_| malformed())?;
        let user = envelope.user.ok_or_else(malformed)?;
        if user.id.is_none_or(|id| id <= 0) {
            return Err(malformed());
        }
        let detail = UserDetail {
            user: user.map(&self.resource_policy),
            profile: envelope.profile.ok_or_else(malformed)?.map(),
            profile_publicity: publicity(envelope.profile_publicity.ok_or_else(malformed)?)?,
            workspace: envelope.workspace.ok_or_else(malformed)?.map(),
        };
        self.remember_resource(&detail.user.profile_image.resource);
        Ok(detail)
    }
}
