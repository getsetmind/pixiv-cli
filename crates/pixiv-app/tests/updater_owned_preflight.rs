#![cfg(all(target_os = "linux", target_arch = "x86_64"))]
mod support {
    pub mod updater_owned_process;
}

#[tokio::test]
async fn concrete_preflight_executes_only_reproducibly_built_owned_helper() {
    support::updater_owned_process::replay().await;
}
