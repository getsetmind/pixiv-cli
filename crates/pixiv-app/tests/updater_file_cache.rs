mod support {
    pub mod updater_installer_contract;
}

#[tokio::test]
async fn physical_release_cache_replays_frozen_private_bytes_and_recovery_contract() {
    support::updater_installer_contract::replay_caches().await;
}
