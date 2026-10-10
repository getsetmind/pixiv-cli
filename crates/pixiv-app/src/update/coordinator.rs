use super::{
    BuildInfo, CallerContext, Command, CommandRunner, ExternalError, InstallSource,
    ReleaseCheckOptions, ReleaseChecker, ReleaseInstaller, SourceDetector, go_quote,
    installer::{ReleaseInstallerOptions, SignedReleaseInstaller},
    message,
    release::SemanticVersion,
    wrap,
};
use serde::{Deserialize, Serialize};
use std::{error::Error, fmt, sync::Arc};

const GO_INSTALL_PACKAGE: &str = "github.com/FlanChanXwO/pixiv-cli/cmd/pixiv";
const HOMEBREW_STABLE_FORMULA: &str = "FlanChanXwO/tap/pixiv-cli";
const HOMEBREW_BETA_FORMULA: &str = "FlanChanXwO/tap/pixiv-cli-beta";

#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct UpdateRequest {
    pub build_info: BuildInfo,
    pub check: bool,
    pub include_prerelease: bool,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
pub struct UpdateResult {
    pub source: String,
    pub current_version: String,
    pub latest_version: Option<String>,
    pub latest_prerelease: bool,
    pub update_available: bool,
}

#[derive(Debug)]
pub struct UpdateError {
    pub result: UpdateResult,
    pub error: ExternalError,
}
impl fmt::Display for UpdateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.error.fmt(f)
    }
}
impl Error for UpdateError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(self.error.as_ref())
    }
}
impl From<ExternalError> for UpdateError {
    fn from(error: ExternalError) -> Self {
        Self {
            result: UpdateResult::default(),
            error,
        }
    }
}

#[derive(Default)]
pub struct CoordinatorOptions {
    pub source_detector: Option<Arc<dyn SourceDetector>>,
    pub release_checker: Option<Arc<dyn ReleaseChecker>>,
    pub command_runner: Option<Arc<dyn CommandRunner>>,
    pub release_installer: Option<Arc<dyn ReleaseInstaller>>,
}

pub struct Coordinator {
    source_detector: Arc<dyn SourceDetector>,
    release_checker: Arc<dyn ReleaseChecker>,
    command_runner: Option<Arc<dyn CommandRunner>>,
    release_installer: Arc<dyn ReleaseInstaller>,
}
impl Coordinator {
    pub fn new(options: CoordinatorOptions) -> Result<Self, ExternalError> {
        let source_detector = options
            .source_detector
            .ok_or_else(|| message("update source detector is required"))?;
        let release_checker = options
            .release_checker
            .ok_or_else(|| message("update release checker is required"))?;
        Ok(Self {
            source_detector,
            release_checker,
            command_runner: options.command_runner,
            release_installer: options.release_installer.unwrap_or_else(|| {
                Arc::new(SignedReleaseInstaller::new(
                    ReleaseInstallerOptions::default(),
                ))
            }),
        })
    }

    pub async fn execute(
        &self,
        context: CallerContext,
        request: UpdateRequest,
    ) -> Result<UpdateResult, UpdateError> {
        let source = self
            .source_detector
            .detect(&request.build_info)
            .map_err(|error| wrap("detect installation source", error))?;
        if source == InstallSource::Development {
            return Err(message("development builds cannot update themselves").into());
        }
        let current = SemanticVersion::parse(&request.build_info.version).map_err(|error| {
            wrap(
                format!(
                    "parse current build version {}",
                    go_quote(&request.build_info.version)
                ),
                error,
            )
        })?;
        let checked = self
            .release_checker
            .check(
                context.clone(),
                ReleaseCheckOptions {
                    include_prerelease: request.include_prerelease,
                    automatic: false,
                },
            )
            .await
            .map_err(|error| wrap("check available releases", error))?;
        let mut result = UpdateResult {
            source: source.as_str().into(),
            current_version: request.build_info.version,
            ..UpdateResult::default()
        };
        if let Some(selected) = &checked.release {
            let candidate = SemanticVersion::parse(&selected.tag_name).map_err(|error| {
                wrap(
                    format!(
                        "parse selected GitHub release tag {}",
                        go_quote(&selected.tag_name)
                    ),
                    error,
                )
            })?;
            result.latest_version = Some(selected.tag_name.clone());
            result.latest_prerelease = selected.prerelease;
            result.update_available = candidate.compare(&current) > 0;
        }
        if homebrew_channel_switch_requested(&source, request.include_prerelease) {
            result.update_available = true;
        }
        if request.check {
            return Ok(result);
        }
        let homebrew = match &source {
            InstallSource::HomebrewStable if request.include_prerelease => Some(
                self.switch_homebrew_formula(
                    context.clone(),
                    HOMEBREW_STABLE_FORMULA,
                    HOMEBREW_BETA_FORMULA,
                )
                .await,
            ),
            InstallSource::HomebrewStable => Some(
                self.run_homebrew(context.clone(), "upgrade", HOMEBREW_STABLE_FORMULA)
                    .await,
            ),
            InstallSource::HomebrewBeta if request.include_prerelease => Some(
                self.run_homebrew(context.clone(), "upgrade", HOMEBREW_BETA_FORMULA)
                    .await,
            ),
            InstallSource::HomebrewBeta => Some(
                self.switch_homebrew_formula(
                    context.clone(),
                    HOMEBREW_BETA_FORMULA,
                    HOMEBREW_STABLE_FORMULA,
                )
                .await,
            ),
            _ => None,
        };
        if let Some(homebrew) = homebrew {
            return homebrew
                .map(|()| result.clone())
                .map_err(|error| UpdateError { result, error });
        }
        let Some(selected) = checked.release else {
            return Ok(result);
        };
        if !result.update_available {
            return Ok(result);
        }
        match source {
            InstallSource::GoInstall => {
                let runner = self
                    .command_runner
                    .as_ref()
                    .ok_or_else(|| message("go install update command runner is unavailable"))?;
                runner
                    .run(
                        context,
                        Command {
                            name: "go".into(),
                            args: vec![
                                "install".into(),
                                format!("{GO_INSTALL_PACKAGE}@{}", selected.tag_name),
                            ],
                        },
                    )
                    .await
                    .map_err(|error| wrap("run go install update", error))?;
            }
            InstallSource::Release => {
                let tag = selected.tag_name.clone();
                self.release_installer
                    .install(context, selected)
                    .await
                    .map_err(|error| {
                        wrap(format!("install release update {}", go_quote(&tag)), error)
                    })?;
            }
            _ => {}
        }
        Ok(result)
    }

    async fn run_homebrew(
        &self,
        context: CallerContext,
        action: &str,
        formula: &str,
    ) -> Result<(), ExternalError> {
        let runner = self
            .command_runner
            .as_ref()
            .ok_or_else(|| message("Homebrew update command runner is unavailable"))?;
        runner
            .run(
                context,
                Command {
                    name: "brew".into(),
                    args: vec![action.into(), formula.into()],
                },
            )
            .await
            .map_err(|error| wrap(format!("Homebrew {action} {}", go_quote(formula)), error))
    }

    async fn switch_homebrew_formula(
        &self,
        context: CallerContext,
        previous: &str,
        target: &str,
    ) -> Result<(), ExternalError> {
        self.run_homebrew(context.clone(), "uninstall", previous)
            .await
            .map_err(|error| {
                wrap(
                    format!(
                        "remove Homebrew formula {} before switching to {}",
                        go_quote(previous),
                        go_quote(target)
                    ),
                    error,
                )
            })?;
        let Err(install_error) = self.run_homebrew(context.clone(), "install", target).await else {
            return Ok(());
        };
        let rollback = match self.run_homebrew(context, "install", previous).await {
            Ok(()) => format!("rollback reinstall {} succeeded", go_quote(previous)),
            Err(error) => format!("rollback reinstall {} failed: {error}", go_quote(previous)),
        };
        Err(Box::new(HomebrewSwitchError {
            text: format!(
                "install Homebrew formula {} after removing {}: {install_error}; {rollback}",
                go_quote(target),
                go_quote(previous)
            ),
            cause: install_error,
        }))
    }
}

fn homebrew_channel_switch_requested(source: &InstallSource, include_prerelease: bool) -> bool {
    (*source == InstallSource::HomebrewStable && include_prerelease)
        || (*source == InstallSource::HomebrewBeta && !include_prerelease)
}

#[derive(Debug)]
struct HomebrewSwitchError {
    text: String,
    cause: ExternalError,
}
impl fmt::Display for HomebrewSwitchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.text)
    }
}
impl Error for HomebrewSwitchError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(self.cause.as_ref())
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct AutomaticRequest {
    pub build_info: BuildInfo,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct AutomaticNotice {
    pub source: InstallSource,
    pub current_version: String,
    pub latest_version: String,
    pub update_command: String,
}

#[derive(Default)]
pub struct AutomaticCheckerOptions {
    pub source_detector: Option<Arc<dyn SourceDetector>>,
    pub release_checker: Option<Arc<dyn ReleaseChecker>>,
}

pub struct AutomaticChecker {
    source_detector: Arc<dyn SourceDetector>,
    release_checker: Arc<dyn ReleaseChecker>,
}
impl AutomaticChecker {
    pub fn new(options: AutomaticCheckerOptions) -> Result<Self, ExternalError> {
        let source_detector = options
            .source_detector
            .ok_or_else(|| message("automatic update source detector is required"))?;
        let release_checker = options
            .release_checker
            .ok_or_else(|| message("automatic update release checker is required"))?;
        Ok(Self {
            source_detector,
            release_checker,
        })
    }

    pub async fn check(
        &self,
        context: CallerContext,
        request: AutomaticRequest,
    ) -> Result<Option<AutomaticNotice>, ExternalError> {
        if request.build_info.is_development() {
            return Ok(None);
        }
        let current = SemanticVersion::parse(&request.build_info.version).map_err(|error| {
            wrap(
                format!(
                    "parse current build version {}",
                    go_quote(&request.build_info.version)
                ),
                error,
            )
        })?;
        let checked = self
            .release_checker
            .check(
                context,
                ReleaseCheckOptions {
                    automatic: true,
                    include_prerelease: false,
                },
            )
            .await
            .map_err(|error| wrap("check available releases", error))?;
        if checked.throttled {
            return Ok(None);
        }
        let Some(selected) = checked.release else {
            return Ok(None);
        };
        let candidate = SemanticVersion::parse(&selected.tag_name).map_err(|error| {
            wrap(
                format!(
                    "parse selected GitHub release tag {}",
                    go_quote(&selected.tag_name)
                ),
                error,
            )
        })?;
        if candidate.compare(&current) <= 0 {
            return Ok(None);
        }
        let source = self
            .source_detector
            .detect(&request.build_info)
            .map_err(|error| wrap("detect installation source", error))?;
        let update_command = match &source {
            InstallSource::HomebrewStable => format!("brew upgrade {HOMEBREW_STABLE_FORMULA}"),
            InstallSource::HomebrewBeta | InstallSource::Release => "pixiv update".into(),
            InstallSource::GoInstall => {
                format!("go install {GO_INSTALL_PACKAGE}@{}", selected.tag_name)
            }
            InstallSource::Development => return Ok(None),
            InstallSource::Other(source) => {
                return Err(message(format!(
                    "unsupported installation source {} for automatic update",
                    go_quote(source)
                )));
            }
        };
        Ok(Some(AutomaticNotice {
            source,
            current_version: request.build_info.version,
            latest_version: selected.tag_name,
            update_command,
        }))
    }
}
