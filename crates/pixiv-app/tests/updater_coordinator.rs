use pixiv_app::update::{
    BuildInfo, CallerContext, Command, CommandRunner, ExternalError, InstallSource, Release,
    ReleaseAsset, ReleaseCheckOptions, ReleaseCheckResult, ReleaseChecker, ReleaseInstaller,
    SourceDetector, UpdateFuture,
    coordinator::{
        AutomaticChecker, AutomaticCheckerOptions, AutomaticRequest, Coordinator,
        CoordinatorOptions, UpdateRequest,
    },
};
use pixiv_sdk::context::{Context, ContextError, ContextKey};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    fmt,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

fn fixture() -> Value {
    serde_json::from_str(include_str!(
        "../../pixiv-cli/tests/fixtures/updater-coordinator.json"
    ))
    .unwrap()
}

#[derive(Debug)]
struct Sentinel {
    identity: Arc<()>,
    text: &'static str,
}
impl fmt::Display for Sentinel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.text)
    }
}
impl Error for Sentinel {}

struct Ports {
    input: Value,
    context: CallerContext,
    key: ContextKey,
    identities: BTreeMap<&'static str, Arc<()>>,
    observed: Mutex<Value>,
}
impl Ports {
    fn new(input: Value) -> Self {
        let key = ContextKey::new("coordinator witness");
        let context = if input["context"] == "deadline" {
            Context::with_deadline(Instant::now() + Duration::from_secs(86400))
        } else {
            Context::new()
        }
        .with_value(
            key.clone(),
            Arc::new(String::from("owned synthetic context")),
        );
        if input["context"] == "canceled" {
            context.cancel();
        }
        Self {
            input,
            context: Arc::new(context),
            key,
            identities: [
                "detector",
                "checker",
                "command_first",
                "command_install",
                "command_rollback",
                "installer",
            ]
            .into_iter()
            .map(|name| (name, Arc::new(())))
            .collect(),
            observed: Mutex::new(json!({
                "constructed": false, "result": null, "notice": null, "trace": [],
                "detected_build_info": [], "release_check_options": [], "commands": [],
                "installed_releases": [], "contexts": [], "error": null,
            })),
        }
    }

    fn error(&self, name: &str) -> Option<ExternalError> {
        let text = match name {
            "" => return None,
            "context_canceled" => return Some(Box::new(ContextError::Canceled)),
            "context_deadline" => return Some(Box::new(ContextError::DeadlineExceeded)),
            "detector" => "detector failure",
            "checker" => "checker failure",
            "command_first" => "first command failure",
            "command_install" => "target install failure",
            "command_rollback" => "rollback install failure",
            "installer" => "release installer failure",
            other => panic!("unknown sentinel {other}"),
        };
        Some(Box::new(Sentinel {
            identity: self.identities[name].clone(),
            text,
        }))
    }

    fn record_context(&self, out: &mut Value, port: &str, context: &CallerContext) {
        assert_eq!(context.deadline(), self.context.deadline());
        let witness = context
            .value(&self.key)
            .unwrap()
            .downcast::<String>()
            .unwrap();
        assert_eq!(witness.as_str(), "owned synthetic context");
        out["contexts"].as_array_mut().unwrap().push(json!({
            "port": port, "same_identity": Arc::ptr_eq(context, &self.context),
            "has_deadline": context.deadline().is_some(),
            "error": context.error().map(|error| error.to_string()).unwrap_or_default(),
        }));
    }

    fn capture_error(&self, error: &(dyn Error + 'static)) -> Value {
        let mut causes: BTreeMap<&str, bool> =
            self.identities.keys().map(|name| (*name, false)).collect();
        causes.insert("context_canceled", false);
        causes.insert("context_deadline", false);
        let mut current = Some(error);
        while let Some(error) = current {
            if let Some(sentinel) = error.downcast_ref::<Sentinel>() {
                for (name, identity) in &self.identities {
                    if Arc::ptr_eq(identity, &sentinel.identity) {
                        causes.insert(name, true);
                    }
                }
            }
            match error.downcast_ref::<ContextError>() {
                Some(ContextError::Canceled) => {
                    causes.insert("context_canceled", true);
                }
                Some(ContextError::DeadlineExceeded) => {
                    causes.insert("context_deadline", true);
                }
                None => {}
            }
            current = error.source();
        }
        json!({"message": error.to_string(), "is": causes})
    }
}

fn selected_release(input: &Value) -> Option<Release> {
    let candidate = &input["candidate"];
    if candidate.is_null() {
        return None;
    }
    Some(Release {
        tag_name: candidate["TagName"].as_str().unwrap().into(),
        version: candidate["Version"].as_str().unwrap().into(),
        prerelease: candidate["Prerelease"].as_bool().unwrap(),
        assets: candidate["Assets"]
            .as_array()
            .unwrap()
            .iter()
            .map(|asset| ReleaseAsset {
                name: asset["name"].as_str().unwrap().into(),
                download_url: asset["browser_download_url"].as_str().unwrap().into(),
            })
            .collect(),
    })
}
fn wire_release(release: Release) -> Value {
    json!({"TagName":release.tag_name, "Version":release.version,
        "Prerelease":release.prerelease, "Assets":release.assets})
}
impl SourceDetector for Ports {
    fn detect(&self, info: &BuildInfo) -> Result<InstallSource, ExternalError> {
        let mut out = self.observed.lock().unwrap();
        out["trace"].as_array_mut().unwrap().push(json!("detector"));
        out["detected_build_info"]
            .as_array_mut()
            .unwrap()
            .push(json!(info));
        if let Some(error) = self.error(self.input["detector_error"].as_str().unwrap()) {
            return Err(error);
        }
        Ok(serde_json::from_value(self.input["source"].clone()).unwrap())
    }
}
impl ReleaseChecker for Ports {
    fn check(
        &self,
        context: CallerContext,
        options: ReleaseCheckOptions,
    ) -> UpdateFuture<'_, Result<ReleaseCheckResult, ExternalError>> {
        Box::pin(async move {
            let mut out = self.observed.lock().unwrap();
            out["trace"].as_array_mut().unwrap().push(json!("checker"));
            out["release_check_options"]
                .as_array_mut()
                .unwrap()
                .push(json!({
                    "IncludePrerelease":options.include_prerelease, "Automatic":options.automatic,
                }));
            self.record_context(&mut out, "checker", &context);
            if let Some(error) = self.error(self.input["checker_error"].as_str().unwrap()) {
                return Err(error);
            }
            Ok(ReleaseCheckResult {
                release: selected_release(&self.input),
                throttled: self.input["throttled"].as_bool().unwrap(),
            })
        })
    }
}
impl CommandRunner for Ports {
    fn run(
        &self,
        context: CallerContext,
        command: Command,
    ) -> UpdateFuture<'_, Result<(), ExternalError>> {
        Box::pin(async move {
            let mut out = self.observed.lock().unwrap();
            out["trace"]
                .as_array_mut()
                .unwrap()
                .push(json!(format!("command:{}", command.name)));
            self.record_context(&mut out, "command", &context);
            let index = out["commands"].as_array().unwrap().len();
            out["commands"]
                .as_array_mut()
                .unwrap()
                .push(json!({"Name":command.name,"Args":command.args}));
            let name = self.input["command_errors"]
                .get(index)
                .and_then(Value::as_str)
                .unwrap_or("");
            self.error(name).map_or(Ok(()), Err)
        })
    }
}
impl ReleaseInstaller for Ports {
    fn install(
        &self,
        context: CallerContext,
        release: Release,
    ) -> UpdateFuture<'_, Result<(), ExternalError>> {
        Box::pin(async move {
            let mut out = self.observed.lock().unwrap();
            out["trace"]
                .as_array_mut()
                .unwrap()
                .push(json!("installer"));
            self.record_context(&mut out, "installer", &context);
            out["installed_releases"]
                .as_array_mut()
                .unwrap()
                .push(wire_release(release));
            self.error(self.input["installer_error"].as_str().unwrap())
                .map_or(Ok(()), Err)
        })
    }
}

async fn observe(input: &Value) -> Value {
    let ports = Arc::new(Ports::new(input.clone()));
    let detector = if input["missing_detector"] == true {
        None
    } else {
        Some(ports.clone() as Arc<dyn SourceDetector>)
    };
    let checker = if input["missing_checker"] == true {
        None
    } else {
        Some(ports.clone() as Arc<dyn ReleaseChecker>)
    };
    let info = BuildInfo {
        version: input["current_version"].as_str().unwrap().into(),
    };
    let error = match input["operation"].as_str().unwrap() {
        "coordinator" | "coordinator_constructor" => {
            match Coordinator::new(CoordinatorOptions {
                source_detector: detector,
                release_checker: checker,
                command_runner: if input["missing_runner"] == true {
                    None
                } else {
                    Some(ports.clone() as Arc<dyn CommandRunner>)
                },
                release_installer: if input["default_installer"] == true {
                    None
                } else {
                    Some(ports.clone() as Arc<dyn ReleaseInstaller>)
                },
            }) {
                Err(error) => Some(error),
                Ok(coordinator) => {
                    ports.observed.lock().unwrap()["constructed"] = json!(true);
                    if input["operation"] == "coordinator_constructor" {
                        None
                    } else {
                        let (result, error) = match coordinator
                            .execute(
                                ports.context.clone(),
                                UpdateRequest {
                                    build_info: info,
                                    check: input["check"].as_bool().unwrap(),
                                    include_prerelease: input["include_prerelease"]
                                        .as_bool()
                                        .unwrap(),
                                },
                            )
                            .await
                        {
                            Ok(result) => (result, None),
                            Err(error) => {
                                (error.result.clone(), Some(Box::new(error) as ExternalError))
                            }
                        };
                        ports.observed.lock().unwrap()["result"] = json!(result);
                        error
                    }
                }
            }
        }
        "automatic" | "automatic_constructor" => {
            match AutomaticChecker::new(AutomaticCheckerOptions {
                source_detector: detector,
                release_checker: checker,
            }) {
                Err(error) => Some(error),
                Ok(automatic) => {
                    ports.observed.lock().unwrap()["constructed"] = json!(true);
                    if input["operation"] == "automatic_constructor" {
                        None
                    } else {
                        match automatic
                            .check(ports.context.clone(), AutomaticRequest { build_info: info })
                            .await
                        {
                            Ok(notice) => {
                                ports.observed.lock().unwrap()["notice"] = json!(notice);
                                None
                            }
                            Err(error) => Some(error),
                        }
                    }
                }
            }
        }
        other => panic!("representation-only operation must not execute: {other}"),
    };
    if let Some(error) = error {
        ports.observed.lock().unwrap()["error"] = ports.capture_error(error.as_ref());
    }
    ports.observed.lock().unwrap().clone()
}

#[tokio::test]
async fn public_coordinator_and_automatic_policies_match_all_frozen_behavioral_rows() {
    let fixture = fixture();
    let cases = fixture["cases"].as_array().unwrap();
    let mut executed = 0;
    for case in cases
        .iter()
        .filter(|case| case["evidence_scope"] == "behavioral_contract")
    {
        let actual = observe(&case["input"]).await;
        let mut expected = case["outcome"].clone();
        if let Some(error) = expected["error"].as_object_mut() {
            error.remove("unwrap_chain");
        }
        assert_eq!(actual, expected, "{}", case["name"]);
        if case["input"]["operation"] == "automatic" || case["input"]["check"] == true {
            assert_eq!(actual["commands"], json!([]), "{}", case["name"]);
            assert_eq!(actual["installed_releases"], json!([]), "{}", case["name"]);
        }
        executed += 1;
    }
    assert_eq!(executed, 296);
}

#[test]
fn go_nil_and_constructor_representations_remain_explicitly_separate() {
    let fixture = fixture();
    let cases = fixture["cases"].as_array().unwrap();
    let names: BTreeSet<_> = cases
        .iter()
        .filter(|case| case["evidence_scope"] == "go_representation_only")
        .map(|case| case["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        BTreeSet::from([
            "coordinator_constructor/both",
            "coordinator_constructor/detector",
            "coordinator_constructor/checker",
            "coordinator_constructor/none",
            "coordinator_nil",
            "automatic_constructor/both",
            "automatic_constructor/detector",
            "automatic_constructor/checker",
            "automatic_nil",
        ])
    );
    assert_eq!(cases.len(), 305);
}

#[test]
fn go_error_layout_is_metadata_while_full_diagnostic_and_cause_membership_are_contracts() {
    let fixture = fixture();
    assert_eq!(
        fixture["metadata_scope"]["cases[].outcome.error.message"],
        "behavioral_contract: complete returned diagnostic remains captured verbatim"
    );
    assert_eq!(
        fixture["metadata_scope"]["cases[].outcome.error.is"],
        "behavioral_contract: primary and rollback cause preservation remains required independently of language-specific wrapper layout"
    );
    assert_eq!(
        fixture["metadata_scope"]["cases[].outcome.error.unwrap_chain"],
        "go_representation_only: concrete Go wrapping graph and intermediate wrapper messages; not a required Rust error layout"
    );
    let error_rows: Vec<_> = fixture["cases"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|case| !case["outcome"]["error"].is_null())
        .collect();
    assert_eq!(error_rows.len(), 98);
    for case in error_rows {
        assert!(
            !case["outcome"]["error"]["unwrap_chain"]
                .as_array()
                .unwrap()
                .is_empty(),
            "{}",
            case["name"]
        );
        assert_eq!(
            case["outcome"]["error"]["is"].as_object().unwrap().len(),
            8,
            "{}",
            case["name"]
        );
    }
}
